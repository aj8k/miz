//! Window, input and the frame loop (winit `ApplicationHandler`).
//!
//! Settings have a single source of truth: `Shared::cfg`, the config as
//! currently in effect. Keys (C, L, F), the menu and config-file reloads all
//! change it and go through `State::apply_settings`; "Save as default"
//! writes it back to disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::window::{Fullscreen, Window, WindowId};

use crate::audio::{AudioEngine, AudioStatus, SourceKind};
use crate::capture;
use crate::cli::Options;
use crate::config::{self, Config};
use crate::gpu::Gpu;
use crate::gpu::shaders::{self, ShaderLib};
use crate::menu::{self, Menu};
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
        shared: Shared {
            engine,
            cfg,
            config_path,
            source_override: opts.source,
        },
        opts,
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
    shared: Shared,
    opts: Options,
    /// Created on `resumed` (winit only allows creating windows from there).
    state: Option<State>,
    /// A startup error, reported after the event loop exits.
    error: Option<anyhow::Error>,
}

/// State that lives outside the window: audio, and the settings in effect.
struct Shared {
    engine: AudioEngine,
    cfg: Config,
    config_path: Option<PathBuf>,
    /// `--source` on the command line wins over the config file (until the
    /// source is changed in the menu).
    source_override: Option<SourceKind>,
}

impl Shared {
    fn source(&self) -> SourceKind {
        self.source_override
            .or_else(|| self.cfg.source_kind())
            .unwrap_or(SourceKind::PipeWire)
    }
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
    menu: Menu,
    snap: Snapshot,
    start: Instant,
    last_frame: Instant,
    show_hud: bool,
    last_mouse_move: Instant,
    cursor_hidden: bool,
    fps_window_start: Instant,
    fps_frames: u32,
    fps: f32,
    /// `--capture`: what's left to save.
    capture: Option<Capture>,
    /// Set when the app should quit (capture finished/failed, menu Quit).
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
    fn new(event_loop: &ActiveEventLoop, opts: &Options, shared: &Shared) -> anyhow::Result<Self> {
        let cfg = &shared.cfg;
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
        let mut menu = Menu::new(&window, &gpu);
        menu.set_open(opts.menu);
        let watcher = match Watcher::new(shared.config_path.as_deref(), shader_dir.as_deref()) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("file watching unavailable, no hot reload: {e}");
                None
            }
        };
        if cfg.visual.fullscreen {
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }

        let now = Instant::now();
        let target_fps = f64::from(cfg.visual.target_fps);
        let (interval, refresh_known) = frame_interval(&window, target_fps);
        Ok(Self {
            window,
            gpu,
            renderer,
            menu,
            snap: Snapshot::default(),
            start: now,
            last_frame: now,
            show_hud: opts.hud,
            last_mouse_move: now,
            cursor_hidden: false,
            fps_window_start: now,
            fps_frames: 0,
            fps: 0.0,
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

    fn redraw(&mut self, shared: &mut Shared) {
        let now = Instant::now();
        // Clamp: after a stall (suspend, dragging the window) don't jump.
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // Never blocks: if the audio thread is publishing right now, we
        // just reuse the previous snapshot for this frame.
        shared.engine.snapshot().read_into(&mut self.snap);

        if let Some(frame) = self.gpu.acquire() {
            let view = frame.texture.create_view(&Default::default());
            let time = (now - self.start).as_secs_f64();
            self.renderer
                .render(&self.gpu, &view, &self.snap, time, dt, self.show_hud);
            self.run_menu(&view, shared);
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
            self.fps = self.fps_frames as f32 / elapsed.as_secs_f32();
            self.update_title(&shared.engine.status());
            self.fps_frames = 0;
            self.fps_window_start = Instant::now();
        }
    }

    /// Draws the menu over the frame and carries out what it asks for.
    fn run_menu(&mut self, target: &wgpu::TextureView, shared: &mut Shared) {
        let scenes = self.renderer.scenes().names();
        let status = shared.engine.status().to_string();
        let view = menu::View {
            scenes: &scenes,
            current_scene: self.renderer.scenes().current(),
            fullscreen: self.window.fullscreen().is_some(),
            hud: self.show_hud,
            audio_status: &status,
            fps: self.fps,
            config_path: shared.config_path.as_deref(),
        };
        let before = shared.cfg.clone();
        let mut edited = shared.cfg.clone();
        let actions = self
            .menu
            .frame(&self.gpu, &self.window, target, &view, &mut edited);
        if edited != before {
            if edited.audio.source != before.audio.source {
                // An explicit choice in the menu beats --source.
                shared.source_override = None;
            }
            self.apply_settings(edited, shared);
        }
        for action in actions {
            let time = self.start.elapsed().as_secs_f64();
            match action {
                menu::Action::JumpScene(i) => self.renderer.scenes().jump(i, time),
                menu::Action::SetHud(on) => self.show_hud = on,
                menu::Action::SetFullscreen(on) => {
                    let mut cfg = shared.cfg.clone();
                    cfg.visual.fullscreen = on;
                    self.apply_settings(cfg, shared);
                }
                menu::Action::Save => self.save(shared),
                menu::Action::Quit => self.finished = Some(Ok(())),
            }
        }
    }

    /// Makes `cfg` the settings in effect: renderer, audio, frame rate,
    /// fullscreen. Used by keys, the menu and config-file reloads.
    fn apply_settings(&mut self, cfg: Config, shared: &mut Shared) {
        self.renderer.apply_config(&self.gpu, &cfg);
        self.target_fps = f64::from(cfg.visual.target_fps);
        self.update_frame_interval();
        if cfg.visual.fullscreen != self.window.fullscreen().is_some() {
            let mode = cfg
                .visual
                .fullscreen
                .then_some(Fullscreen::Borderless(None));
            self.window.set_fullscreen(mode);
            self.last_mouse_move = Instant::now();
            self.show_cursor();
        }
        if cfg.visual.gpu != shared.cfg.visual.gpu {
            eprintln!("miz: visual.gpu changes take effect after a restart");
        }
        shared.cfg = cfg;
        shared
            .engine
            .reconfigure(shared.source(), shared.cfg.analyzer());
    }

    /// Writes the settings in effect (plus the current scene as the start
    /// scene) to the config file.
    fn save(&mut self, shared: &mut Shared) {
        let Some(path) = shared.config_path.clone() else {
            return;
        };
        shared.cfg.visual.scene = self.renderer.scene_key().to_string();
        let result = config::save(&path, &shared.cfg);
        let notice = match result {
            Ok(()) => format!("Saved to {}", tilde(&path)),
            Err(e) => format!("Couldn't save: {e:#}"),
        };
        eprintln!("miz: {notice}");
        self.menu.show_notice(notice);
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

    fn update_title(&self, status: &AudioStatus) {
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
            "miz — {}{lock} — {mode} — {:.0} fps",
            self.renderer.scene_name(),
            self.fps
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
    fn reload(&mut self, changes: &[Change], shared: &mut Shared) {
        for change in changes {
            match change {
                Change::Config => {
                    let Some(path) = shared.config_path.clone() else {
                        continue;
                    };
                    match config::load(&path) {
                        // Our own "Save" also lands here; it's a no-op then.
                        Ok(cfg) if cfg == shared.cfg => {}
                        Ok(cfg) => {
                            self.apply_settings(cfg, shared);
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

    fn show_cursor(&mut self) {
        if self.cursor_hidden {
            self.window.set_cursor_visible(true);
            self.cursor_hidden = false;
        }
    }

    fn update_cursor(&mut self) {
        let fullscreen = self.window.fullscreen().is_some();
        let idle = self.last_mouse_move.elapsed() > CURSOR_HIDE_AFTER;
        if fullscreen && idle && !self.cursor_hidden && !self.menu.wants_pointer() {
            self.window.set_cursor_visible(false);
            self.cursor_hidden = true;
        }
    }

    fn on_key(&mut self, code: KeyCode, shared: &mut Shared) {
        let time = self.start.elapsed().as_secs_f64();
        let mut cfg = shared.cfg.clone();
        match code {
            KeyCode::KeyQ => self.finished = Some(Ok(())),
            // Esc closes the menu first; with the menu closed it quits.
            KeyCode::Escape if self.menu.is_open() => self.menu.set_open(false),
            KeyCode::Escape => self.finished = Some(Ok(())),
            KeyCode::KeyM => self.menu.set_open(!self.menu.is_open()),
            KeyCode::KeyH => self.show_hud = !self.show_hud,
            KeyCode::KeyN => self.renderer.scenes().next(time),
            KeyCode::KeyP => self.renderer.scenes().prev(time),
            KeyCode::Digit1 => self.renderer.scenes().jump(0, time),
            KeyCode::Digit2 => self.renderer.scenes().jump(1, time),
            KeyCode::Digit3 => self.renderer.scenes().jump(2, time),
            KeyCode::Digit4 => self.renderer.scenes().jump(3, time),
            KeyCode::KeyF => cfg.visual.fullscreen = !cfg.visual.fullscreen,
            KeyCode::KeyL => cfg.visual.lock = !cfg.visual.lock,
            KeyCode::KeyC => {
                cfg.visual.calm = !cfg.visual.calm;
                eprintln!(
                    "miz: {} mode",
                    if cfg.visual.calm { "calm" } else { "energetic" }
                );
            }
            _ => {}
        }
        if cfg != shared.cfg {
            self.apply_settings(cfg, shared);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match State::new(event_loop, &self.opts, &self.shared) {
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
        // The menu sees every event first; what it uses isn't ours.
        let menu_consumed = state.menu.on_event(&state.window, &event);
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Physical pixels: also fires when the scale factor changes.
            WindowEvent::Resized(size) => {
                state.gpu.resize(size);
                state.renderer.resize(&state.gpu);
            }
            WindowEvent::RedrawRequested => {
                state.redraw(&mut self.shared);
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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } if !menu_consumed => match button {
                MouseButton::Right => state.menu.set_open(!state.menu.is_open()),
                // A click on the picture closes the menu.
                MouseButton::Left => state.menu.set_open(false),
                _ => {}
            },
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } if !menu_consumed => {
                // Physical keys: shortcuts stay where they are regardless of
                // the active keyboard layout (e.g. Ukrainian).
                state.on_key(code, &mut self.shared);
                if let Some(result) = state.finished.take() {
                    if let Err(e) = result {
                        self.error = Some(e);
                    }
                    event_loop.exit();
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
            state.reload(&changes, &mut self.shared);
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

/// `/home/me/.config/x` -> `~/.config/x`, for messages.
fn tilde(path: &Path) -> String {
    match std::env::var_os("HOME") {
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}
