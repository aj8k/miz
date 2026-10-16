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
use winit::window::{Fullscreen, Window, WindowId};

use crate::audio::{AudioEngine, AudioStatus};
use crate::capture;
use crate::cli::Options;
use crate::gpu::Gpu;
use crate::render::Renderer;
use crate::snapshot::Snapshot;

const CURSOR_HIDE_AFTER: Duration = Duration::from_secs(3);
/// Frame-rate target. We render at refresh / N for the integer N that gets
/// closest to this (144 Hz -> 72 fps, 120 -> 60, 60 -> 60): every frame
/// then stays on screen for the same number of refreshes, so motion is
/// even, while the GPU/CPU do a fraction of the work of full refresh rate.
const TARGET_FPS: f64 = 60.0;

pub fn run(engine: AudioEngine, opts: Options) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    let mut app = App {
        engine,
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
    engine: AudioEngine,
    opts: Options,
    /// Created on `resumed` (winit only allows creating windows from there).
    state: Option<State>,
    /// A startup error, reported after the event loop exits.
    error: Option<anyhow::Error>,
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
    /// `--capture`: where and when to save a frame.
    capture: Option<(PathBuf, Instant)>,
    frame_interval: Duration,
    /// Ideal time of the next frame on a fixed timeline (not "now +
    /// interval", which would drift by however late each wakeup was).
    next_frame: Instant,
    redraw_pending: bool,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, opts: &Options) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("miz")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );
        let gpu = Gpu::new(event_loop, Arc::clone(&window))?;
        let renderer = Renderer::new(&gpu, &opts.scene)?;
        let now = Instant::now();
        let interval = frame_interval(&window);
        log::info!("frame interval {:.2} ms", interval.as_secs_f64() * 1e3);
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
            capture: opts.capture.clone().map(|p| {
                (
                    p,
                    now + Duration::from_secs_f32(opts.capture_after.max(0.0)),
                )
            }),
            frame_interval: interval,
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
            self.window.pre_present_notify();
            self.gpu.queue.present(frame);
            self.fps_frames += 1;
        }

        self.redraw_pending = false;
        self.next_frame += self.frame_interval;
        if self.next_frame < now {
            // Fell behind (stall, suspend): restart the timeline instead of
            // rendering a burst of catch-up frames.
            self.next_frame = now + self.frame_interval;
        }

        let elapsed = self.fps_window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            // Cheap, and catches the window moving to another monitor.
            self.frame_interval = frame_interval(&self.window);
            let fps = self.fps_frames as f32 / elapsed.as_secs_f32();
            self.update_title(fps, &engine.status());
            self.fps_frames = 0;
            self.fps_window_start = Instant::now();
        }
    }

    fn update_title(&self, fps: f32, status: &AudioStatus) {
        let lock = if self.renderer.scenes_locked() {
            " [locked]"
        } else {
            ""
        };
        let mut title = format!("miz — {}{lock} — {fps:.0} fps", self.renderer.scene_name());
        if self.show_hud {
            title += &format!(" — gain {:.0} dB", self.snap.gain_db);
        }
        if let AudioStatus::Retrying { reason, .. } = status {
            title += &format!(" — audio: {reason}");
        }
        log::debug!("{title}");
        self.window.set_title(&title);
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
        match State::new(event_loop, &self.opts) {
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
                if let Some((path, at)) = &state.capture
                    && Instant::now() >= *at
                {
                    let time = state.start.elapsed().as_secs_f64();
                    let result = capture::save_frame(
                        &state.gpu,
                        &mut state.renderer,
                        &state.snap,
                        time,
                        state.show_hud,
                        path,
                    );
                    match result {
                        Ok(()) => eprintln!("saved {}", path.display()),
                        Err(e) => self.error = Some(e),
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

/// Frame interval for `TARGET_FPS` on the window's current monitor.
fn frame_interval(window: &Window) -> Duration {
    let refresh = window
        .current_monitor()
        .and_then(|m| m.refresh_rate_millihertz())
        .map_or(TARGET_FPS, |mhz| f64::from(mhz) / 1000.0);
    let divisor = (refresh / TARGET_FPS).round().max(1.0);
    Duration::from_secs_f64(divisor / refresh)
}
