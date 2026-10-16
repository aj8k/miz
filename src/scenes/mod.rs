//! Scenes: each one renders the music into an HDR texture.

pub mod feedback;
pub mod lissajous;
pub mod manager;
pub mod shader;

use anyhow::bail;

use crate::gpu::Sampling;
use crate::gpu::shaders::{Shader, ShaderLib};
use shader::ShaderScene;

/// Everything a scene needs to build its GPU resources.
pub struct SceneInit<'a> {
    pub device: &'a wgpu::Device,
    pub shaders: &'a ShaderLib,
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
    /// Frame delta in seconds (also in the globals, for CPU-side use).
    pub dt: f32,
}

pub trait Scene {
    fn name(&self) -> &'static str;

    /// The scene is about to become visible again after a while. Scenes
    /// with history (feedback, persistence) clear it here, so stale
    /// frames from minutes ago don't flash up.
    fn activate(&mut self) {}

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

/// The scenes in rotation, in order (keys 1-4).
pub const ROTATION: &[&str] = &["feedback", "lissajous", "radial", "nebula"];
/// Accepted by `--scene` but not part of the rotation.
pub const EXTRA: &[&str] = &["bass-pulse"];

pub fn create(name: &str, init: &SceneInit) -> anyhow::Result<Box<dyn Scene>> {
    Ok(match name {
        "feedback" => Box::new(feedback::Feedback::new(init)),
        "lissajous" => Box::new(lissajous::Lissajous::new(init)),
        "radial" => Box::new(ShaderScene::new(init, "radial spectrum", Shader::Radial)),
        "nebula" => Box::new(ShaderScene::new(init, "nebula", Shader::Nebula)),
        "bass-pulse" => Box::new(ShaderScene::new(init, "bass pulse", Shader::BassPulse)),
        other => bail!(
            "unknown scene '{other}' (available: {}, {})",
            ROTATION.join(", "),
            EXTRA.join(", ")
        ),
    })
}
