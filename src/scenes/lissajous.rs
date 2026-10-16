//! Stereo oscilloscope with phosphor persistence.
//!
//! One persistence buffer: each frame it is (1) multiplied in place by a
//! decay factor using blending, (2) the new trace is added on top, (3) a
//! display pass adds glow and graticule into the scene target.

use super::{Scene, SceneCtx, SceneInit};
use crate::gpu::shaders::Shader;
use crate::gpu::{self, ADDITIVE, HDR_FORMAT, HdrTarget, Sampling};

/// Must match SEGMENTS in lissajous_line.wgsl: 6 vertices per segment.
const SEGMENTS: u32 = 1023;
/// Phosphor time constant, seconds.
const PERSISTENCE_S: f32 = 0.35;

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

pub struct Lissajous {
    phosphor: HdrTarget,
    group: wgpu::BindGroup,
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
        let module = init.shaders.module(device, Shader::LissajousLine);
        let line = gpu::pipeline(
            device,
            "lissajous_line",
            &module,
            "vs_main",
            &[init.audio_layout],
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
        let clear = self.needs_clear.then_some(wgpu::Color::BLACK);
        self.needs_clear = false;
        {
            let mut pass = gpu::color_pass(encoder, "phosphor", &self.phosphor.view, clear);
            pass.set_bind_group(0, ctx.audio_group, &[]);
            let k = f64::from((-ctx.dt / PERSISTENCE_S).exp());
            pass.set_blend_constant(wgpu::Color {
                r: k,
                g: k,
                b: k,
                a: 1.0,
            });
            pass.set_pipeline(&self.decay);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.line);
            pass.draw(0..SEGMENTS * 6, 0..1);
        }
        let mut pass = gpu::color_pass(encoder, "scope display", target, None);
        pass.set_pipeline(&self.display);
        pass.set_bind_group(0, ctx.audio_group, &[]);
        pass.set_bind_group(1, &self.group, &[]);
        pass.draw(0..3, 0..1);
    }
}
