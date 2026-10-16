//! Shader sources. WGSL has no `#include`, so `common.wgsl` is prepended
//! to every shader before compiling.

const COMMON: &str = include_str!("../../shaders/common.wgsl");

pub const BASS_PULSE: &str = include_str!("../../shaders/bass_pulse.wgsl");
pub const PRESENT: &str = include_str!("../../shaders/present.wgsl");
pub const HUD: &str = include_str!("../../shaders/hud.wgsl");
pub const FEEDBACK_WARP: &str = include_str!("../../shaders/feedback_warp.wgsl");
pub const FEEDBACK_WAVE: &str = include_str!("../../shaders/feedback_wave.wgsl");
pub const FEEDBACK_DISPLAY: &str = include_str!("../../shaders/feedback_display.wgsl");
pub const LISSAJOUS_DECAY: &str = include_str!("../../shaders/lissajous_decay.wgsl");
pub const LISSAJOUS_LINE: &str = include_str!("../../shaders/lissajous_line.wgsl");
pub const LISSAJOUS_DISPLAY: &str = include_str!("../../shaders/lissajous_display.wgsl");
pub const RADIAL: &str = include_str!("../../shaders/radial.wgsl");
pub const NEBULA: &str = include_str!("../../shaders/nebula.wgsl");
pub const CROSSFADE: &str = include_str!("../../shaders/crossfade.wgsl");

pub fn compile(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let full = format!("{COMMON}\n{source}");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(full.into()),
    })
}
