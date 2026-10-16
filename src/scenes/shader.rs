//! Scenes that are a single fullscreen fragment shader (bass pulse, radial
//! spectrum, nebula): all the work happens per pixel in WGSL.

use super::{Scene, SceneCtx, SceneInit};
use crate::gpu::{self, HDR_FORMAT, shaders};

pub struct ShaderScene {
    name: &'static str,
    pipeline: wgpu::RenderPipeline,
}

impl ShaderScene {
    /// `source` must define `fs_main` (the vertex stage is the shared
    /// fullscreen triangle from common.wgsl).
    pub fn new(init: &SceneInit, name: &'static str, source: &str) -> Self {
        let module = shaders::compile(init.device, name, source);
        let pipeline = gpu::fullscreen_pipeline(
            init.device,
            name,
            &module,
            &[init.audio_layout],
            HDR_FORMAT,
            None,
        );
        Self { name, pipeline }
    }
}

impl Scene for ShaderScene {
    fn name(&self) -> &'static str {
        self.name
    }

    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let mut pass = gpu::color_pass(encoder, self.name, target, Some(wgpu::Color::BLACK));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, ctx.audio_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
