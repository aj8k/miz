//! Stereo oscilloscope with phosphor persistence. Its colour follows the
//! music's intensity and rhythm: calm, quiet music draws in nearly one soft
//! tint; loud, busy, fast music in saturated rings of colour from the core
//! to the rim that cycle faster, pulsing with the beats.
//!
//! One persistence buffer: each frame it is (1) multiplied in place by a
//! decay factor using blending, (2) the new trace is added on top, (3) a
//! display pass adds glow and graticule into the scene target.

use bytemuck::{Pod, Zeroable};

use super::{Scene, SceneCtx, SceneInit};
use crate::analysis::smooth::one_pole_alpha;
use crate::gpu::shaders::Shader;
use crate::gpu::{self, ADDITIVE, HDR_FORMAT, HdrTarget, Sampling};
use crate::render::params::Globals;

/// Must match SEGMENTS in lissajous_line.wgsl: 6 vertices per segment.
const SEGMENTS: u32 = 1023;
/// Phosphor time constants, seconds: energetic / calm.
const PERSISTENCE_S: (f32, f32) = (0.22, 0.55);

/// Blend state that computes `dst * constant`: the fragment's own colour
/// is multiplied by zero, the existing pixel by the blend constant.
const MULTIPLY_BY_CONSTANT: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::Constant,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::REPLACE,
};

/// The beam's colour state. Must match `Colour` in lissajous_line.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ColourUniform {
    /// How intense the music is, 0 = calm and sparse .. 1 = loud, busy, fast.
    intensity: f32,
    /// Position on the colour wheel; advances faster when intense.
    hue: f32,
    _pad: [f32; 2],
}

pub struct Lissajous {
    phosphor: HdrTarget,
    group: wgpu::BindGroup,
    colour: Colour,
    colour_buffer: wgpu::Buffer,
    colour_group: wgpu::BindGroup,
    needs_clear: bool,
    decay: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    display: wgpu::RenderPipeline,
}

impl Lissajous {
    pub fn new(init: &SceneInit) -> Self {
        let device = init.device;
        let module = init.shaders.module(device, Shader::LissajousDecay);
        let decay = gpu::fullscreen_pipeline(
            device,
            "lissajous_decay",
            &module,
            &[init.audio_layout],
            HDR_FORMAT,
            Some(MULTIPLY_BY_CONSTANT),
        );
        let colour_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scope colour"),
            size: size_of::<ColourUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colour_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scope colour"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let colour_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scope colour"),
            layout: &colour_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: colour_buffer.as_entire_binding(),
            }],
        });
        let module = init.shaders.module(device, Shader::LissajousLine);
        let line = gpu::pipeline(
            device,
            "lissajous_line",
            &module,
            "vs_main",
            &[init.audio_layout, &colour_layout],
            HDR_FORMAT,
            Some(ADDITIVE),
        );
        let module = init.shaders.module(device, Shader::LissajousDisplay);
        let display = gpu::fullscreen_pipeline(
            device,
            "lissajous_display",
            &module,
            &[init.audio_layout, &init.sampling.layout],
            HDR_FORMAT,
            None,
        );
        let phosphor = HdrTarget::new(device, "phosphor", init.size);
        let group = init.sampling.bind(device, &phosphor.view);
        Self {
            phosphor,
            group,
            colour: Colour::new(),
            colour_buffer,
            colour_group,
            needs_clear: false,
            decay,
            line,
            display,
        }
    }
}

impl Scene for Lissajous {
    fn name(&self) -> &'static str {
        "stereo scope"
    }

    fn activate(&mut self) {
        self.needs_clear = true;
    }

    fn resize(&mut self, device: &wgpu::Device, sampling: &Sampling, size: (u32, u32)) {
        self.phosphor = HdrTarget::new(device, "phosphor", size);
        self.group = sampling.bind(device, &self.phosphor.view);
    }

    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        self.colour.update(ctx.globals, ctx.dt);
        ctx.queue.write_buffer(
            &self.colour_buffer,
            0,
            bytemuck::bytes_of(&self.colour.uniform()),
        );

        let clear = self.needs_clear.then_some(wgpu::Color::BLACK);
        self.needs_clear = false;
        {
            let mut pass = gpu::color_pass(encoder, "phosphor", &self.phosphor.view, clear);
            pass.set_bind_group(0, ctx.audio_group, &[]);
            let (fast, slow) = PERSISTENCE_S;
            let persistence = fast + (slow - fast) * ctx.calm;
            let k = f64::from((-ctx.dt / persistence).exp());
            pass.set_blend_constant(wgpu::Color {
                r: k,
                g: k,
                b: k,
                a: 1.0,
            });
            pass.set_pipeline(&self.decay);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.line);
            pass.set_bind_group(1, &self.colour_group, &[]);
            pass.draw(0..SEGMENTS * 6, 0..1);
        }
        let mut pass = gpu::color_pass(encoder, "scope display", target, None);
        pass.set_pipeline(&self.display);
        pass.set_bind_group(0, ctx.audio_group, &[]);
        pass.set_bind_group(1, &self.group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Colour wheel speed, turns per second: calm / most intense.
const HUE_SPEED: (f32, f32) = (0.006, 0.08);
/// Onsets per second that count as fully fast-paced.
const FAST_PACE: f32 = 6.0;
/// The track's usual level is averaged over this long (seconds); colour
/// follows how far the music is above or below it right now.
const BASELINE_S: f32 = 20.0;
/// How strongly moments above / below the usual level change the colour,
/// and how much the usual level itself counts.
const CONTRAST: f32 = 4.0;
const OVERALL: f32 = 0.6;

/// Tracks how intense the music is (loudness, pace, how much of the
/// spectrum is sounding), both right now and as usual for this track, and
/// turns the colour wheel accordingly.
struct Colour {
    intensity: f32,
    hue: f32,
    /// Onsets per second, averaged over a few seconds.
    pace: f32,
    last_beat_count: Option<u32>,
    /// Current level (fast) and the track's usual level (slow), 0..1.
    level: f32,
    baseline: f32,
    /// Seconds of music heard so far (the baseline settles faster at first).
    heard_s: f32,
}

impl Colour {
    fn new() -> Self {
        Self {
            intensity: 0.0,
            hue: 0.6,
            pace: 0.0,
            last_beat_count: None,
            level: 0.0,
            baseline: 0.45,
            heard_s: 0.0,
        }
    }

    fn uniform(&self) -> ColourUniform {
        ColourUniform {
            intensity: self.intensity,
            hue: self.hue,
            _pad: [0.0; 2],
        }
    }

    fn update(&mut self, g: &Globals, dt: f32) {
        // Pace: each onset adds 1/tau, the sum decays with tau.
        let new = self
            .last_beat_count
            .map_or(0, |last| g.beat_count.wrapping_sub(last));
        self.last_beat_count = Some(g.beat_count);
        let tau = 3.0;
        self.pace = self.pace * (-dt / tau).exp() + new.min(8) as f32 / tau;

        // Loudness is taken per frame, so colour follows the dynamics
        // within a bar, not just from one section to the next.
        let now = level(g.loudness, self.pace, g.complexity);
        self.level += (now - self.level) * one_pole_alpha(dt, 0.12);
        if g.idle < 0.5 {
            // A running mean at first, then a BASELINE_S-long memory.
            self.heard_s += dt;
            let tau = BASELINE_S.min(self.heard_s);
            self.baseline += (self.level - self.baseline) * one_pole_alpha(dt, tau);
        }
        self.intensity = intensity(self.level, self.baseline) * (1.0 - g.idle);
        let (slow, fast) = HUE_SPEED;
        self.hue = (self.hue + dt * (slow + (fast - slow) * self.intensity)).fract();
    }
}

/// How much is going on: 0 = quiet, slow and sparse .. 1 = loud, fast and
/// busy. Loudness weighs most, the rhythm's pace next (single beats add
/// their own pulse in the shader).
fn level(loudness: f32, pace: f32, complexity: f32) -> f32 {
    let pace = (pace / FAST_PACE).min(1.0);
    let busy = ((complexity - 0.12) / 0.5).clamp(0.0, 1.0);
    (0.6 * loudness + 0.25 * pace + 0.15 * busy).clamp(0.0, 1.0)
}

/// Colour intensity 0..1: mid-way for this track's usual level, higher
/// when the music is above it, lower below; a generally loud track sits
/// higher than a generally quiet one.
fn intensity(level: f32, baseline: f32) -> f32 {
    (0.55 + CONTRAST * (level - baseline) + OVERALL * (baseline - 0.45)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_grows_with_loudness_pace_and_busyness() {
        let calm = level(0.1, 0.5, 0.1);
        let loud = level(0.8, 0.5, 0.1);
        let loud_fast = level(0.8, 6.0, 0.1);
        let all = level(0.8, 6.0, 0.6);
        assert!(calm < 0.1, "{calm}");
        assert!(calm < loud && loud < loud_fast && loud_fast < all);
        assert!(all > 0.85 && all <= 1.0, "{all}");
    }

    #[test]
    fn colour_follows_moments_above_and_below_the_usual_level() {
        // Same track (same baseline): louder moments are more colourful.
        let base = 0.5;
        assert!(intensity(0.65, base) > intensity(0.5, base) + 0.3);
        assert!(intensity(0.35, base) < intensity(0.5, base) - 0.3);
        // At its usual level, a loud track is more colourful than a quiet one.
        assert!(intensity(0.7, 0.7) > intensity(0.2, 0.2) + 0.25);
    }

    #[test]
    fn colours_turn_faster_when_intense() {
        let turn = |loudness: f32| {
            let mut c = Colour::new();
            let g = Globals {
                loudness,
                ..Globals::default()
            };
            let start = c.hue;
            for _ in 0..500 {
                c.update(&g, 0.01);
            }
            (c.hue - start).rem_euclid(1.0)
        };
        // A steadily loud track vs a steadily quiet one (each at its own
        // usual level, so only the overall term differs).
        assert!(turn(1.0) > 1.5 * turn(0.0));
    }
}
