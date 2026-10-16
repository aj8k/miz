//! `--capture FILE.ppm`: save the frame(s) shown on screen (dev tool, e.g.
//! for checking a shader change without looking at the screen).
//! PPM is a trivial uncompressed format; most image viewers open it.

use std::io::Write;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, anyhow};

use crate::gpu::Gpu;

/// Copies `texture` (the swapchain image about to be presented, so the
/// saved frame is exactly what's on screen) to the CPU and writes a PPM.
pub fn save_texture(gpu: &Gpu, texture: &wgpu::Texture, path: &Path) -> anyhow::Result<()> {
    let (w, h) = (texture.width(), texture.height());
    let format = texture.format();
    // Buffer rows must be a multiple of 256 bytes for texture->buffer copies.
    let padded =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture readback"),
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);

    let (tx, rx) = mpsc::channel();
    buf.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| anyhow!("waiting for the GPU: {e}"))?;
    rx.recv()
        .context("readback callback never ran")?
        .map_err(|e| anyhow!("mapping readback buffer: {e}"))?;

    let data = buf.get_mapped_range(..).map_err(|e| anyhow!("{e}"))?;
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut out = Vec::with_capacity((w * h * 3) as usize + 32);
    write!(out, "P6\n{w} {h}\n255\n")?;
    for row in data.chunks_exact(padded as usize) {
        for px in row[..(w * 4) as usize].as_chunks::<4>().0 {
            if bgra {
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            } else {
                out.extend_from_slice(&px[..3]);
            }
        }
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
