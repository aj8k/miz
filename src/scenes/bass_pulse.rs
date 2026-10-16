//! The simplest scene: one fullscreen fragment shader (shaders/bass_pulse.wgsl).

use super::{Scene, SceneCtx};
use crate::gpu::{self, HDR_FORMAT, shaders};

pub struct BassPulse {
    pipeline: wgpu::RenderPipeline,
}

impl BassPulse {
    pub fn new(device: &wgpu::Device, audio_layout: &wgpu::BindGroupLayout) -> Self {
        let module = shaders::compile(device, "bass_pulse", shaders::BASS_PULSE);
        let pipeline = gpu::fullscreen_pipeline(
            device,
            "bass_pulse",
            &module,
            &[audio_layout],
            HDR_FORMAT,
            None,
        );
        Self { pipeline }
    }
}

impl Scene for BassPulse {
    fn name(&self) -> &'static str {
        "bass pulse"
    }

    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let mut pass = gpu::color_pass(encoder, "bass_pulse", target, Some(wgpu::Color::BLACK));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, ctx.audio_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
