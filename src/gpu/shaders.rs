//! Shader sources. WGSL has no `#include`, so `common.wgsl` is prepended
//! to every shader before compiling.

const COMMON: &str = include_str!("../../shaders/common.wgsl");

pub const BASS_PULSE: &str = include_str!("../../shaders/bass_pulse.wgsl");
pub const PRESENT: &str = include_str!("../../shaders/present.wgsl");
pub const HUD: &str = include_str!("../../shaders/hud.wgsl");

pub fn compile(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let full = format!("{COMMON}\n{source}");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(full.into()),
    })
}
