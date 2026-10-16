//! In-window settings menu (egui), drawn on top of the visuals.
//!
//! egui is an *immediate-mode* UI: every frame we describe the whole menu
//! from scratch (`ui.checkbox(&mut value, ...)` both draws the checkbox and
//! applies a click to `value`). There's no widget tree to keep in sync with
//! the settings — the settings *are* the state.
//!
//! The menu edits the live `Config` directly; the app notices the change and
//! applies it exactly like a config-file reload. Actions that aren't config
//! values (jump to a scene, save, quit...) are returned as `Action`s.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Margin, RichText, Shadow, Stroke, Vec2};
use winit::event::WindowEvent;
use winit::window::Window;

use crate::config::{Config, MAX_BRIGHTNESS_RATE_CEILING};
use crate::gpu::Gpu;

/// The ☰ button stays visible this long after the mouse last moved.
const BUTTON_VISIBLE_FOR: Duration = Duration::from_secs(3);
const PANEL_WIDTH: f32 = 300.0;

/// Things the menu asks the app to do (besides editing the config).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    JumpScene(usize),
    SetFullscreen(bool),
    SetHud(bool),
    Save,
    Quit,
}

/// Read-only facts the menu displays.
pub struct View<'a> {
    pub scenes: &'a [&'static str],
    pub current_scene: usize,
    pub fullscreen: bool,
    pub hud: bool,
    pub audio_status: &'a str,
    pub fps: f32,
    pub config_path: Option<&'a Path>,
}

pub struct Menu {
    ctx: egui::Context,
    input: egui_winit::State,
    painter: egui_wgpu::Renderer,
    open: bool,
    button_until: Instant,
    /// Result of the last save, shown for a few seconds.
    notice: Option<(Instant, String)>,
    painted_last_frame: bool,
    /// Render scale while its slider is being dragged (applied on release:
    /// every change reallocates the render targets).
    pending_scale: Option<f32>,
}

impl Menu {
    pub fn new(window: &Window, gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        style(&ctx);
        let input = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        // The swapchain is a non-sRGB format: egui then writes its (sRGB)
        // colours as-is, which is what we want.
        let painter = egui_wgpu::Renderer::new(&gpu.device, gpu.config.format, Default::default());
        Self {
            ctx,
            input,
            painter,
            open: false,
            button_until: Instant::now(),
            notice: None,
            painted_last_frame: true,
            pending_scale: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if open {
            self.button_until = Instant::now() + BUTTON_VISIBLE_FOR;
        }
    }

    /// Feeds a window event to egui. Returns true if egui used it (e.g. a
    /// click on the menu), so the app should ignore it.
    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CursorMoved { .. }) {
            self.button_until = Instant::now() + BUTTON_VISIBLE_FOR;
        }
        let consumed = self.input.on_window_event(window, event).consumed;
        consumed && (self.open || self.ctx.is_pointer_over_egui())
    }

    /// True while the pointer is over the menu or its button (keep the
    /// cursor visible).
    pub fn wants_pointer(&self) -> bool {
        self.open || self.ctx.is_pointer_over_egui()
    }

    pub fn show_notice(&mut self, text: String) {
        self.notice = Some((Instant::now(), text));
    }

    /// Runs the UI for this frame and draws it on top of `target`.
    pub fn frame(
        &mut self,
        gpu: &Gpu,
        window: &Window,
        target: &wgpu::TextureView,
        view: &View,
        cfg: &mut Config,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let raw = self.input.take_egui_input(window);
        let show_button = self.open || Instant::now() < self.button_until;
        if !show_button && !self.painted_last_frame {
            // Nothing to show: skip egui entirely (the input taken above is
            // just discarded). Saves ~4% CPU while you're simply watching.
            return actions;
        }
        let notice = self
            .notice
            .as_ref()
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(4))
            .map(|(_, s)| s.clone());
        let mut open = self.open;
        let pending_scale = &mut self.pending_scale;
        let output = self.ctx.run_pass(raw, |ctx| {
            if show_button && menu_button(ctx, open) {
                open = !open;
            }
            if open {
                panel(
                    ctx,
                    view,
                    cfg,
                    pending_scale,
                    notice.as_deref(),
                    &mut actions,
                );
            }
        });
        self.open = open;
        self.input
            .handle_platform_output(window, output.platform_output);

        // Tessellate into triangles and draw them over the finished frame.
        let ppp = output.pixels_per_point;
        let jobs = self.ctx.tessellate(output.shapes, ppp);
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.painter
                    .update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        // One more pass after hiding, so the last frame clears the button.
        self.painted_last_frame = show_button;
        if !jobs.is_empty() {
            let (w, h) = gpu.size();
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [w, h],
                pixels_per_point: ppp,
            };
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let mut cmds =
                self.painter
                    .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &jobs, &screen);
            {
                let pass = crate::gpu::color_pass(&mut encoder, "menu", target, None);
                // egui-wgpu wants a 'static pass; forget_lifetime() trades
                // the borrow check for a runtime check that the encoder
                // isn't used while the pass is alive.
                self.painter
                    .render(&mut pass.forget_lifetime(), &jobs, &screen);
            }
            cmds.push(encoder.finish());
            gpu.queue.submit(cmds);
        }
        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }
        actions
    }
}

/// Adwaita-ish dark styling: rounded, soft, a blue accent.
fn style(ctx: &egui::Context) {
    let accent = Color32::from_rgb(0x35, 0x84, 0xe4);
    let mut visuals = egui::Visuals::dark();
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.selection.bg_fill = accent;
    visuals.hyperlink_color = accent;
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.spacing.slider_width = 150.0;
    });
}

/// The ☰ button in the top-right corner. Returns true when clicked.
fn menu_button(ctx: &egui::Context, open: bool) -> bool {
    egui::Area::new(egui::Id::unique("menu-button"))
        .anchor(Align2::RIGHT_TOP, [-12.0, 12.0])
        .show(ctx, |ui| {
            let size = Vec2::splat(36.0);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let hovered = response.hovered();
            let bg = if open || hovered {
                Color32::from_rgba_unmultiplied(60, 60, 64, 230)
            } else {
                Color32::from_rgba_unmultiplied(30, 30, 32, 170)
            };
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(10), bg);
            // Three bars ("hamburger"), drawn rather than taken from a font.
            let stroke = Stroke::new(2.0, Color32::from_gray(230));
            for dy in [-6.0, 0.0, 6.0] {
                let y = rect.center().y + dy;
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + 10.0, y),
                        egui::pos2(rect.right() - 10.0, y),
                    ],
                    stroke,
                );
            }
            response.on_hover_text("Menu (M or right-click)").clicked()
        })
        .inner
}

fn panel(
    ctx: &egui::Context,
    view: &View,
    cfg: &mut Config,
    pending_scale: &mut Option<f32>,
    notice: Option<&str>,
    actions: &mut Vec<Action>,
) {
    let frame = egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(28, 28, 30, 240))
        .stroke(Stroke::new(1.0, Color32::from_gray(60)))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(14))
        .shadow(Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(140),
        });
    // Window height minus the button row above and the pinned footer below.
    let max_height = (ctx.content_rect().height() - 150.0).max(120.0);
    egui::Area::new(egui::Id::unique("menu-panel"))
        .anchor(Align2::RIGHT_TOP, [-12.0, 56.0])
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(PANEL_WIDTH);
                // Use the full window height before scrolling kicks in.
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .min_scrolled_height(max_height)
                    .show(ui, |ui| contents(ui, view, cfg, pending_scale, actions));
                footer(ui, view, notice, actions);
            });
        });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(Color32::from_gray(200)));
}

fn contents(
    ui: &mut egui::Ui,
    view: &View,
    cfg: &mut Config,
    pending_scale: &mut Option<f32>,
    actions: &mut Vec<Action>,
) {
    section(ui, "Scene");
    for (i, name) in view.scenes.iter().enumerate() {
        let key_hint = if i < 4 {
            format!("{}  ", i + 1)
        } else {
            "    ".into()
        };
        let label = format!("{key_hint}{}", capitalize(name));
        if ui
            .selectable_label(i == view.current_scene, label)
            .clicked()
        {
            actions.push(Action::JumpScene(i));
        }
    }
    ui.checkbox(&mut cfg.visual.lock, "Stay on this scene  (L)");
    ui.add_enabled_ui(!cfg.visual.lock, |ui| {
        ui.add(
            egui::Slider::new(&mut cfg.visual.rotate_minutes, 0.0..=30.0)
                .step_by(0.5)
                .text("auto-rotate")
                .custom_formatter(|v, _| {
                    if v == 0.0 {
                        "off".into()
                    } else {
                        format!("{v} min")
                    }
                }),
        );
    });
    ui.add(
        egui::Slider::new(&mut cfg.visual.crossfade_seconds, 1.0..=15.0)
            .step_by(0.5)
            .suffix(" s")
            .text("crossfade"),
    );

    ui.separator();
    section(ui, "Mood");
    ui.checkbox(&mut cfg.visual.calm, "Calm mode  (C)")
        .on_hover_text("Slower, softer and dimmer. Off = energetic (still flash-safe).");

    ui.separator();
    section(ui, "Display");
    let mut fullscreen = view.fullscreen;
    if ui.checkbox(&mut fullscreen, "Fullscreen  (F)").changed() {
        actions.push(Action::SetFullscreen(fullscreen));
    }
    let mut hud = view.hud;
    if ui.checkbox(&mut hud, "Debug HUD  (H)").changed() {
        actions.push(Action::SetHud(hud));
    }
    let mut scale = pending_scale.unwrap_or(cfg.visual.render_scale);
    let response = ui
        .add(
            egui::Slider::new(&mut scale, 0.25..=1.0)
                .step_by(0.05)
                .text("render scale"),
        )
        .on_hover_text("Internal resolution. Lower = lighter on the GPU.");
    if response.dragged() {
        *pending_scale = Some(scale);
    } else {
        if response.changed() || response.drag_stopped() {
            cfg.visual.render_scale = scale;
        }
        *pending_scale = None;
    }
    ui.horizontal(|ui| {
        ui.label("frame rate");
        ui.radio_value(&mut cfg.visual.target_fps, 30.0, "30");
        ui.radio_value(&mut cfg.visual.target_fps, 60.0, "60");
    });

    ui.separator();
    section(ui, "Audio");
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut cfg.audio.source,
            "pipewire".to_string(),
            "What's playing",
        );
        ui.radio_value(&mut cfg.audio.source, "test".to_string(), "Test signal");
    });
    ui.add(
        egui::Slider::new(&mut cfg.audio.range_db, 16.0..=60.0)
            .step_by(1.0)
            .suffix(" dB")
            .text("dynamic range"),
    )
    .on_hover_text("Smaller = punchier reactions, larger = subtler.");
    ui.small(RichText::new(view.audio_status).weak());

    ui.separator();
    section(ui, "Flash safety");
    let ceiling = MAX_BRIGHTNESS_RATE_CEILING;
    ui.add(
        egui::Slider::new(&mut cfg.safety.max_brightness_rate, 0.02..=ceiling)
            .step_by(0.01)
            .text("energetic"),
    )
    .on_hover_text(
        "Max change of the average brightness per second (fraction of white). \
         Can be made stricter, never looser.",
    );
    ui.add(
        egui::Slider::new(&mut cfg.safety.calm_max_brightness_rate, 0.02..=ceiling)
            .step_by(0.01)
            .text("calm"),
    );
}

/// Always visible under the scrolling part: save / quit and status.
fn footer(ui: &mut egui::Ui, view: &View, notice: Option<&str>, actions: &mut Vec<Action>) {
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_save = view.config_path.is_some();
        if ui
            .add_enabled(can_save, egui::Button::new("Save as default"))
            .on_hover_text("Write these settings (and the current scene) to config.toml")
            .clicked()
        {
            actions.push(Action::Save);
        }
        if ui.button("Quit  (Q)").clicked() {
            actions.push(Action::Quit);
        }
    });
    if let Some(text) = notice {
        ui.small(text);
    }
    ui.add_space(2.0);
    ui.small(
        RichText::new(format!(
            "{:.0} fps · M / right-click / Esc closes",
            view.fps
        ))
        .weak(),
    );
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}
