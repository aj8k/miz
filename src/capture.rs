//! `--capture FILE.ppm`: render one frame offscreen and save it (dev tool,
//! e.g. for checking a shader change without looking at the screen).
//! PPM is a trivial uncompressed format; most image viewers open it.

use std::io::Write;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, anyhow};

use crate::gpu::Gpu;
use crate::render::Renderer;
use crate::snapshot::Snapshot;

pub fn save_frame(
    gpu: &Gpu,
    renderer: &mut Renderer,
    snap: &Snapshot,
    time: f64,
    show_hud: bool,
    path: &Path,
) -> anyhow::Result<()> {
    let (w, h) = gpu.size();
    let format = gpu.config.format;
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    // dt = 0: re-render the current state without advancing animations.
    renderer.render(
        gpu,
        &tex.create_view(&Default::default()),
        snap,
        time,
        0.0,
        show_hud,
    );

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
        tex.as_image_copy(),
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
