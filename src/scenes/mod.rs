//! Scenes: each one renders the music into an HDR texture.

pub mod bass_pulse;
pub mod feedback;

use anyhow::bail;

use crate::gpu::Sampling;

/// Everything a scene needs to build its GPU resources.
pub struct SceneInit<'a> {
    pub device: &'a wgpu::Device,
    /// Layout of group 0 (globals, bands, waveform) shared by all pipelines.
    pub audio_layout: &'a wgpu::BindGroupLayout,
    /// Layout + sampler for reading back a texture (group 1).
    pub sampling: &'a Sampling,
    /// Size of the scene's render target, pixels.
    pub size: (u32, u32),
}

/// What a scene gets each frame.
pub struct SceneCtx<'a> {
    /// Group 0 of every pipeline: globals, bands, waveform (common.wgsl).
    pub audio_group: &'a wgpu::BindGroup,
}

pub trait Scene {
    fn name(&self) -> &'static str;

    /// The render target changed size: reallocate size-dependent buffers.
    fn resize(&mut self, _device: &wgpu::Device, _sampling: &Sampling, _size: (u32, u32)) {}

    /// Draws one frame into `target` (format `gpu::HDR_FORMAT`).
    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    );
}

/// Scene names accepted by `--scene`, in display order.
pub const NAMES: &[&str] = &["feedback", "bass-pulse"];

pub fn create(name: &str, init: &SceneInit) -> anyhow::Result<Box<dyn Scene>> {
    Ok(match name {
        "feedback" => Box::new(feedback::Feedback::new(init)),
        "bass-pulse" => Box::new(bass_pulse::BassPulse::new(init)),
        other => bail!("unknown scene '{other}' (available: {})", NAMES.join(", ")),
    })
}
