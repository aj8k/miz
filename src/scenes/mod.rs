//! Scenes: each one renders the music into an HDR texture.

pub mod bass_pulse;

/// What a scene gets to work with each frame.
/// (Grows in later phases: feedback scenes need the device and size.)
pub struct SceneCtx<'a> {
    /// Group 0 of every pipeline: globals, bands, waveform (common.wgsl).
    pub audio_group: &'a wgpu::BindGroup,
}

pub trait Scene {
    fn name(&self) -> &'static str;

    /// Draws one frame into `target` (format `gpu::HDR_FORMAT`).
    fn render(
        &mut self,
        ctx: &SceneCtx,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    );
}
