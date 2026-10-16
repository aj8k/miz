//! Window, input and the frame loop (winit `ApplicationHandler`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::window::{Fullscreen, Window, WindowId};

use crate::audio::{AudioEngine, AudioStatus};
use crate::capture;
use crate::cli::Options;
use crate::config::{self, Config};
use crate::gpu::Gpu;
use crate::gpu::shaders::{self, ShaderLib};
use crate::render::Renderer;
use crate::snapshot::Snapshot;
use crate::watch::{Change, Watcher};

const CURSOR_HIDE_AFTER: Duration = Duration::from_secs(3);
pub fn run(
    engine: AudioEngine,
    opts: Options,
    cfg: Config,
    config_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    let mut app = App {
        engine,
        opts,
        cfg,
        config_path,
        state: None,
        error: None,
    };
    event_loop.run_app(&mut app).context("event loop failed")?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct App {
    engine: AudioEngine,
    opts: Options,
    cfg: Config,
    config_path: Option<PathBuf>,
    /// Created on `resumed` (winit only allows creating windows from there).
    state: Option<State>,
    /// A startup error, reported after the event loop exits.
    error: Option<anyhow::Error>,
}

/// The parts of `App` a reload needs (split borrow from `State`).
struct AppReload<'a> {
    engine: &'a mut AudioEngine,
    cfg: &'a mut Config,
    config_path: Option<&'a std::path::Path>,
    source_override: Option<crate::audio::SourceKind>,
}

/// `--capture` progress.
struct Capture {
    path: PathBuf,
    at: Instant,
    total: u32,
    done: u32,
}

struct State {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    snap: Snapshot,
    start: Instant,
    last_frame: Instant,
    show_hud: bool,
    last_mouse_move: Instant,
    cursor_hidden: bool,
    fps_window_start: Instant,
    fps_frames: u32,
    /// `--capture`: what's left to save.
    capture: Option<Capture>,
    /// Set when the app should quit (capture finished or failed).
    finished: Option<anyhow::Result<()>>,
    /// See `frame_interval`.
    target_fps: f64,
    frame_interval: Duration,
    /// False until the compositor tells us the monitor's refresh rate
    /// (on Wayland that's a moment after the window appears).
    refresh_known: bool,
    /// Shader directory in dev mode (None = shaders built into the binary).
    shader_dir: Option<PathBuf>,
    watcher: Option<Watcher>,
    /// Ideal time of the next frame on a fixed timeline (not "now +
    /// interval", which would drift by however late each wakeup was).
    next_frame: Instant,
    redraw_pending: bool,
}

impl State {
    fn new(
        event_loop: &ActiveEventLoop,
        opts: &Options,
        cfg: &Config,
        config_path: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("miz")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0));
        // Wayland app_id / X11 WM_CLASS "miz": lets the desktop match the
        // window to miz.desktop (dock icon, grouping).
        let attrs = WindowAttributesExtWayland::with_name(attrs, "miz", "miz");
        let attrs = WindowAttributesExtX11::with_name(attrs, "miz", "miz");
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );
        let power = match cfg.visual.gpu.as_str() {
            "high-performance" => wgpu::PowerPreference::HighPerformance,
            _ => wgpu::PowerPreference::LowPower,
        };
        let gpu = Gpu::new(event_loop, Arc::clone(&window), power)?;

        // Dev mode: shaders from disk (config dir, else the source tree).
        let shader_dir = match (opts.dev, cfg.shader_dir()) {
            (_, Some(dir)) => Some(dir),
            (true, None) => Some(shaders::source_tree_dir()),
            (false, None) => None,
        };
        let shaders = ShaderLib::load(shader_dir.as_deref())
            .map_err(|e| anyhow::anyhow!("shader errors:\n{e}"))?;
        if let Some(dir) = &shader_dir {
            eprintln!(
                "miz: dev mode, shaders from {} (reloaded on save)",
                dir.display()
            );
        }
        let scene = opts.scene.as_deref().unwrap_or(&cfg.visual.scene);
        let renderer = Renderer::new(&gpu, &shaders, cfg, scene)?;
        let watcher = match Watcher::new(config_path, shader_dir.as_deref()) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("file watching unavailable, no hot reload: {e}");
                None
            }
        };

        let now = Instant::now();
        let target_fps = f64::from(cfg.visual.target_fps);
        let (interval, refresh_known) = frame_interval(&window, target_fps);
        Ok(Self {
            window,
            gpu,
            renderer,
            snap: Snapshot::default(),
            start: now,
            last_frame: now,
            show_hud: opts.hud,
            last_mouse_move: now,
            cursor_hidden: false,
            fps_window_start: now,
            fps_frames: 0,
            capture: opts.capture.clone().map(|path| Capture {
                path,
                at: now + Duration::from_secs_f32(opts.capture_after.max(0.0)),
                total: opts.capture_frames.max(1),
                done: 0,
            }),
            finished: None,
            target_fps,
            frame_interval: interval,
            refresh_known,
            shader_dir,
            watcher,
            next_frame: now,
            redraw_pending: true,
        })
    }

    fn redraw(&mut self, engine: &AudioEngine) {
        let now = Instant::now();
        // Clamp: after a stall (suspend, dragging the window) don't jump.
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Never blocks: if the audio thread is publishing right now, we
        // just reuse the previous snapshot for this frame.
        engine.snapshot().read_into(&mut self.snap);

        if let Some(frame) = self.gpu.acquire() {
            let view = frame.texture.create_view(&Default::default());
            let time = (now - self.start).as_secs_f64();
            self.renderer
                .render(&self.gpu, &view, &self.snap, time, dt, self.show_hud);
            self.capture_if_due(&frame.texture, now);
            self.window.pre_present_notify();
            self.gpu.queue.present(frame);
            self.fps_frames += 1;
        }

        if !self.refresh_known {
            self.update_frame_interval();
        }
        // Long silence: the idle animation is slow, so half the frame rate
        // looks the same and saves power while nothing is playing.
        let interval = if self.snap.idle > 0.98 {
            self.frame_interval * 2
        } else {
            self.frame_interval
        };
        self.redraw_pending = false;
        self.next_frame += interval;
        if self.next_frame < now {
            // Fell behind (stall, suspend): restart the timeline instead of
            // rendering a burst of catch-up frames.
            self.next_frame = now + interval;
        }

        let elapsed = self.fps_window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            // Cheap, and catches the window moving to another monitor.
            self.update_frame_interval();
            let fps = self.fps_frames as f32 / elapsed.as_secs_f32();
            self.update_title(fps, &engine.status());
            self.fps_frames = 0;
            self.fps_window_start = Instant::now();
        }
    }

    fn update_frame_interval(&mut self) {
        let (interval, known) = frame_interval(&self.window, self.target_fps);
        if known && !self.refresh_known {
            log::info!("frame interval {:.2} ms", interval.as_secs_f64() * 1e3);
        }
        self.frame_interval = interval;
        self.refresh_known = known;
    }

    fn capture_if_due(&mut self, texture: &wgpu::Texture, now: Instant) {
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        if now < cap.at {
            return;
        }
        let path = if cap.total == 1 {
            cap.path.clone()
        } else {
            let stem = cap.path.with_extension("");
            PathBuf::from(format!("{}_{:03}.ppm", stem.display(), cap.done))
        };
        if let Err(e) = capture::save_texture(&self.gpu, texture, &path) {
            self.finished = Some(Err(e));
            return;
        }
        eprintln!("saved {}", path.display());
        cap.done += 1;
        if cap.done == cap.total {
            self.finished = Some(Ok(()));
        }
    }

    fn update_title(&self, fps: f32, status: &AudioStatus) {
        let lock = if self.renderer.scenes_locked() {
            " [locked]"
        } else {
            ""
        };
        let mode = if self.renderer.calm() {
            "calm"
        } else {
            "energetic"
        };
        let mut title = format!(
            "miz — {}{lock} — {mode} — {fps:.0} fps",
            self.renderer.scene_name()
        );
        if self.show_hud {
            title += &format!(" — gain {:.0} dB", self.snap.gain_db);
        }
        if let AudioStatus::Retrying { reason, .. } = status {
            title += &format!(" — audio: {reason}");
        }
        log::debug!("{title}");
        self.window.set_title(&title);
    }

    /// Applies settled config / shader file changes (hot reload).
    fn reload(&mut self, changes: &[Change], app: &mut AppReload) {
        for change in changes {
            match change {
                Change::Config => {
                    let Some(path) = app.config_path else {
                        continue;
                    };
                    match config::load(path) {
                        Ok(cfg) => {
                            self.renderer.apply_config(&self.gpu, &cfg);
                            self.target_fps = f64::from(cfg.visual.target_fps);
                            self.update_frame_interval();
                            // A --source flag keeps priority over the file.
                            let source = app
                                .source_override
                                .or_else(|| cfg.source_kind())
                                .unwrap_or(crate::audio::SourceKind::PipeWire);
                            app.engine.reconfigure(source, cfg.analyzer());
                            if cfg.visual.gpu != app.cfg.visual.gpu {
                                eprintln!("miz: visual.gpu changes take effect after a restart");
                            }
                            *app.cfg = cfg;
                            eprintln!("miz: config reloaded");
                        }
                        Err(e) => eprintln!("miz: {e:#}\nmiz: keeping the previous config"),
                    }
                }
                Change::Shaders => {
                    let result = ShaderLib::load(self.shader_dir.as_deref())
                        .and_then(|lib| self.renderer.rebuild(&self.gpu, &lib));
                    match result {
                        Ok(()) => eprintln!("miz: shaders reloaded"),
                        Err(e) => {
                            eprintln!("miz: shader errors, keeping the previous shaders:\n{e}")
                        }
                    }
                }
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        let fullscreen = match self.window.fullscreen() {
            Some(_) => None,
            // Borderless on the current monitor: no video mode switch.
            None => Some(Fullscreen::Borderless(None)),
        };
        self.window.set_fullscreen(fullscreen);
        self.last_mouse_move = Instant::now();
        self.show_cursor();
    }

    fn show_cursor(&mut self) {
        if self.cursor_hidden {
            self.window.set_cursor_visible(true);
            self.cursor_hidden = false;
        }
    }

    fn update_cursor(&mut self) {
        let fullscreen = self.window.fullscreen().is_some();
        if fullscreen && !self.cursor_hidden && self.last_mouse_move.elapsed() > CURSOR_HIDE_AFTER {
            self.window.set_cursor_visible(false);
            self.cursor_hidden = true;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(
            event_loop,
            &self.opts,
            &self.cfg,
            self.config_path.as_deref(),
        ) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Physical pixels: also fires when the scale factor changes.
            WindowEvent::Resized(size) => {
                state.gpu.resize(size);
                state.renderer.resize(&state.gpu);
            }
            WindowEvent::RedrawRequested => {
                state.redraw(&self.engine);
                if let Some(result) = state.finished.take() {
                    if let Err(e) = result {
                        self.error = Some(e);
                    }
                    event_loop.exit();
                }
            }
            WindowEvent::CursorMoved { .. } => {
                state.last_mouse_move = Instant::now();
                state.show_cursor();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => {
                // Physical keys: shortcuts stay where they are regardless of
                // the active keyboard layout (e.g. Ukrainian).
                let time = state.start.elapsed().as_secs_f64();
                let scenes = state.renderer.scenes();
                match code {
                    KeyCode::KeyQ | KeyCode::Escape => event_loop.exit(),
                    KeyCode::KeyF => state.toggle_fullscreen(),
                    KeyCode::KeyH => state.show_hud = !state.show_hud,
                    KeyCode::KeyN => scenes.next(time),
                    KeyCode::KeyP => scenes.prev(time),
                    KeyCode::KeyL => scenes.toggle_lock(),
                    KeyCode::KeyC => {
                        let calm = state.renderer.toggle_calm();
                        eprintln!("miz: {} mode", if calm { "calm" } else { "energetic" });
                    }
                    KeyCode::Digit1 => scenes.jump(0, time),
                    KeyCode::Digit2 => scenes.jump(1, time),
                    KeyCode::Digit3 => scenes.jump(2, time),
                    KeyCode::Digit4 => scenes.jump(3, time),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Runs when the event queue is drained: schedule the next frame and
    /// sleep until then (instead of spinning at the display refresh rate).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if let Some(changes) = state.watcher.as_mut().map(Watcher::poll)
            && !changes.is_empty()
        {
            let mut app = AppReload {
                engine: &mut self.engine,
                cfg: &mut self.cfg,
                config_path: self.config_path.as_deref(),
                source_override: self.opts.source,
            };
            state.reload(&changes, &mut app);
        }
        state.update_cursor();
        if state.redraw_pending {
            // Sleep until the redraw event arrives (on Wayland it waits for
            // the compositor's frame callback).
            event_loop.set_control_flow(ControlFlow::Wait);
        } else if Instant::now() >= state.next_frame {
            state.redraw_pending = true;
            state.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(state.next_frame));
        }
    }
}

/// Frame interval on the window's current monitor. We render at
/// refresh / N for the integer N that gets closest to `target_fps` (144 Hz
/// -> 72 fps, 120 -> 60, 60 -> 60): every frame then stays on screen for
/// the same number of refreshes, so motion is even, while the GPU/CPU do a
/// fraction of the work of running at the full refresh rate.
///
/// Returns the interval and whether the refresh rate was actually known.
fn frame_interval(window: &Window, target_fps: f64) -> (Duration, bool) {
    let refresh = window
        .current_monitor()
        .and_then(|m| m.refresh_rate_millihertz())
        .map(|mhz| f64::from(mhz) / 1000.0);
    let hz = refresh.unwrap_or(target_fps);
    let divisor = (hz / target_fps).round().max(1.0);
    (Duration::from_secs_f64(divisor / hz), refresh.is_some())
}
