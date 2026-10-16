//! Owns all scenes; switches between them with crossfades, rotates
//! automatically every few minutes unless locked.

use super::{ROTATION, Scene, SceneCtx, SceneInit};
use crate::gpu::shaders::Shader;
use crate::gpu::{self, HDR_FORMAT, HdrTarget, Sampling};

/// A crossfade in progress: from `from` to the manager's `current`.
struct Fade {
    from: usize,
    start: f64,
}

pub struct SceneManager {
    scenes: Vec<Box<dyn Scene>>,
    /// Registry name of each scene (to rebuild the same set on reload).
    keys: Vec<String>,
    current: usize,
    fade: Option<Fade>,
    locked: bool,
    /// Time (seconds since start) of the last switch, for auto-rotation.
    last_switch: f64,
    rotate_every_s: f64,
    fade_s: f64,
    /// During a fade both scenes render into these, then get blended.
    slots: [HdrTarget; 2],
    slot_groups: [wgpu::BindGroup; 2],
    crossfade: wgpu::RenderPipeline,
}

impl SceneManager {
    /// Builds every scene up front (they're cheap) and starts on `start`.
    pub fn new(init: &SceneInit, start: &str) -> anyhow::Result<Self> {
        let mut scenes = Vec::new();
        let mut keys: Vec<String> = ROTATION.iter().map(|k| k.to_string()).collect();
        for name in ROTATION {
            scenes.push(super::create(name, init)?);
        }
        let current = match ROTATION.iter().position(|n| *n == start) {
            Some(i) => i,
            // An extra scene (or a typo, which `create` reports) is appended.
            None => {
                scenes.push(super::create(start, init)?);
                keys.push(start.to_string());
                scenes.len() - 1
            }
        };

        let module = init.shaders.module(init.device, Shader::Crossfade);
        let crossfade = gpu::fullscreen_pipeline(
            init.device,
            "crossfade",
            &module,
            &[
                init.audio_layout,
                &init.sampling.layout,
                &init.sampling.layout,
            ],
            HDR_FORMAT,
            None,
        );
        let (slots, slot_groups) = make_slots(init.device, init.sampling, init.size);
        Ok(Self {
            scenes,
            keys,
            current,
            fade: None,
            locked: false,
            last_switch: 0.0,
            rotate_every_s: 5.0 * 60.0,
            fade_s: 4.0,
            slots,
            slot_groups,
            crossfade,
        })
    }

    pub fn name(&self) -> &'static str {
        self.scenes[self.current].name()
    }

    /// Registry name of the current scene (e.g. "nebula").
    pub fn current_key(&self) -> &str {
        &self.keys[self.current]
    }

    /// Rotation interval (0 = never) and crossfade length, in seconds.
    pub fn set_timing(&mut self, rotate_every_s: f64, fade_s: f64) {
        self.rotate_every_s = rotate_every_s;
        self.fade_s = fade_s.max(0.1);
    }

    /// Takes over lock and timing from the manager this one replaces (shader
    /// reload). It was built starting on the same scene.
    pub fn adopt_state(&mut self, old: &SceneManager) {
        self.locked = old.locked;
        self.last_switch = old.last_switch;
        self.rotate_every_s = old.rotate_every_s;
        self.fade_s = old.fade_s;
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn toggle_lock(&mut self) {
        self.locked = !self.locked;
    }

    pub fn next(&mut self, time: f64) {
        self.switch_to((self.current + 1) % self.scenes.len(), time);
    }

    pub fn prev(&mut self, time: f64) {
        let n = self.scenes.len();
        self.switch_to((self.current + n - 1) % n, time);
    }

    /// Jump to scene `index` (0-based; out-of-range is ignored).
    pub fn jump(&mut self, index: usize, time: f64) {
        if index < self.scenes.len() {
            self.switch_to(index, time);
        }
    }

    fn switch_to(&mut self, index: usize, time: f64) {
        // Switching mid-fade: fade out of whichever scene dominates now.
        let from = match &self.fade {
            Some(f) if self.progress(time) < 0.5 => f.from,
            _ => self.current,
        };
        self.last_switch = time;
        if index == from {
            self.current = index;
            self.fade = None;
            return;
        }
        self.scenes[index].activate();
        self.current = index;
        self.fade = Some(Fade { from, start: time });
    }

    /// Eased crossfade progress, 0..1.
    fn progress(&self, time: f64) -> f32 {
        let Some(f) = &self.fade else { return 1.0 };
        let x = ((time - f.start) / self.fade_s).clamp(0.0, 1.0) as f32;
        x * x * (3.0 - 2.0 * x) // smoothstep: no sudden start or stop
    }

    /// Advances fades and auto-rotation; returns the crossfade progress.
    pub fn update(&mut self, time: f64) -> f32 {
        if let Some(f) = &self.fade
            && time - f.start >= self.fade_s
        {
            self.fade = None;
        }
        let rotate = !self.locked && self.rotate_every_s > 0.0 && self.fade.is_none();
        if rotate && time - self.last_switch >= self.rotate_every_s {
            self.next(time);
        }
        self.progress(time)
    }

    pub fn resize(&mut self, device: &wgpu::Device, sampling: &Sampling, size: (u32, u32)) {
        for s in &mut self.scenes {
            s.resize(device, sampling, size);
        }
        (self.slots, self.slot_groups) = make_slots(device, sampling, size);
    }

    pub fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let Some(fade) = &self.fade else {
            self.scenes[self.current].render(ctx, encoder, target);
            return;
        };
        // Both scenes keep animating during the fade.
        let from = fade.from;
        self.scenes[from].render(ctx, encoder, &self.slots[0].view);
        self.scenes[self.current].render(ctx, encoder, &self.slots[1].view);
        let mut pass = gpu::color_pass(encoder, "crossfade", target, None);
        pass.set_pipeline(&self.crossfade);
        pass.set_bind_group(0, ctx.audio_group, &[]);
        pass.set_bind_group(1, &self.slot_groups[0], &[]);
        pass.set_bind_group(2, &self.slot_groups[1], &[]);
        pass.draw(0..3, 0..1);
    }
}

fn make_slots(
    device: &wgpu::Device,
    sampling: &Sampling,
    size: (u32, u32),
) -> ([HdrTarget; 2], [wgpu::BindGroup; 2]) {
    let slots = [
        HdrTarget::new(device, "fade from", size),
        HdrTarget::new(device, "fade to", size),
    ];
    let groups = [
        sampling.bind(device, &slots[0].view),
        sampling.bind(device, &slots[1].view),
    ];
    (slots, groups)
}
