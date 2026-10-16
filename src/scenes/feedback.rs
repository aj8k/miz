//! Feedback warp: the previous frame, warped, plus the fresh waveform.
//!
//! Two HDR textures take turns ("ping-pong"): each frame reads the one
//! written last frame and writes the other. A texture can't be read and
//! written in the same pass, hence two.
//!
//!   frame N:   warp(buf[A]) -> buf[B];  wave += buf[B];  display(buf[B]) -> target
//!   frame N+1: warp(buf[B]) -> buf[A];  ...

use super::{Scene, SceneCtx, SceneInit};
use crate::gpu::shaders::Shader;
use crate::gpu::{self, ADDITIVE, HDR_FORMAT, HdrTarget, Sampling};

/// Must match SEGMENTS in feedback_wave.wgsl: 6 vertices per segment.
const WAVE_SEGMENTS: u32 = 512;

pub struct Feedback {
    buffers: [HdrTarget; 2],
    /// `groups[i]` samples `buffers[i]`.
    groups: [wgpu::BindGroup; 2],
    /// Index of the buffer holding the most recent frame.
    latest: usize,
    /// Clear the history before the next frame (see `Scene::activate`).
    needs_clear: bool,
    warp: wgpu::RenderPipeline,
    wave: wgpu::RenderPipeline,
    display: wgpu::RenderPipeline,
}

impl Feedback {
    pub fn new(init: &SceneInit) -> Self {
        let device = init.device;
        let layouts = [init.audio_layout, &init.sampling.layout];

        let module = init.shaders.module(device, Shader::FeedbackWarp);
        let warp =
            gpu::fullscreen_pipeline(device, "feedback_warp", &module, &layouts, HDR_FORMAT, None);
        let module = init.shaders.module(device, Shader::FeedbackWave);
        let wave = gpu::pipeline(
            device,
            "feedback_wave",
            &module,
            "vs_main",
            &[init.audio_layout],
            HDR_FORMAT,
            Some(ADDITIVE),
        );
        let module = init.shaders.module(device, Shader::FeedbackDisplay);
        let display = gpu::fullscreen_pipeline(
            device,
            "feedback_display",
            &module,
            &layouts,
            HDR_FORMAT,
            None,
        );

        let (buffers, groups) = Self::make_buffers(device, init.sampling, init.size);
        Self {
            buffers,
            groups,
            latest: 0,
            needs_clear: false,
            warp,
            wave,
            display,
        }
    }

    fn make_buffers(
        device: &wgpu::Device,
        sampling: &Sampling,
        size: (u32, u32),
    ) -> ([HdrTarget; 2], [wgpu::BindGroup; 2]) {
        let buffers = [
            HdrTarget::new(device, "feedback A", size),
            HdrTarget::new(device, "feedback B", size),
        ];
        let groups = [
            sampling.bind(device, &buffers[0].view),
            sampling.bind(device, &buffers[1].view),
        ];
        (buffers, groups)
    }
}

impl Scene for Feedback {
    fn name(&self) -> &'static str {
        "feedback warp"
    }

    fn activate(&mut self) {
        self.needs_clear = true;
    }

    fn resize(&mut self, device: &wgpu::Device, sampling: &Sampling, size: (u32, u32)) {
        // Starting from black after a resize is fine: the image rebuilds
        // itself within a second or two.
        (self.buffers, self.groups) = Self::make_buffers(device, sampling, size);
        self.latest = 0;
    }

    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let prev = self.latest;
        let next = 1 - prev;
        if self.needs_clear {
            // An empty pass whose only job is its Clear load op.
            gpu::color_pass(
                encoder,
                "feedback clear",
                &self.buffers[prev].view,
                Some(wgpu::Color::BLACK),
            );
            self.needs_clear = false;
        }

        // 1+2. Warp the previous frame into the other buffer, then add the
        // fresh waveform on top (same pass: same target, different pipeline).
        {
            let mut pass = gpu::color_pass(encoder, "feedback", &self.buffers[next].view, None);
            pass.set_bind_group(0, ctx.audio_group, &[]);
            pass.set_pipeline(&self.warp);
            pass.set_bind_group(1, &self.groups[prev], &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.wave);
            pass.draw(0..WAVE_SEGMENTS * 6, 0..1);
        }

        // 3. Show it.
        {
            let mut pass = gpu::color_pass(encoder, "feedback display", target, None);
            pass.set_pipeline(&self.display);
            pass.set_bind_group(0, ctx.audio_group, &[]);
            pass.set_bind_group(1, &self.groups[next], &[]);
            pass.draw(0..3, 0..1);
        }

        self.latest = next;
    }
}
