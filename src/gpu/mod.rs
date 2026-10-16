//! GPU context: instance, surface, device and swapchain configuration.

pub mod shaders;

use std::sync::Arc;

use anyhow::{Context, anyhow};
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// Format of all offscreen render targets. 16-bit float per channel: slow
/// fades (multiply by 0.995 per frame) keep changing instead of getting
/// stuck on an 8-bit step, and values above 1.0 survive until tonemapping.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub struct Gpu {
    instance: wgpu::Instance,
    window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    pub fn new(event_loop: &ActiveEventLoop, window: Arc<Window>) -> anyhow::Result<Self> {
        // The display handle is required by the GL backend on Wayland.
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            ))
            .with_env(), // honour WGPU_BACKEND etc. for experiments
        );
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("creating the window surface")?;
        // LowPower = the integrated GPU on a hybrid laptop: plenty for these
        // shaders, and the discrete GPU can stay asleep (battery, fans).
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter (is a Vulkan driver installed?)")?;
        let info = adapter.get_info();
        log::info!(
            "GPU: {} ({:?}, {})",
            info.name,
            info.backend,
            info.driver_info
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("miz device"),
            ..Default::default()
        }))
        .context("creating the GPU device")?;
        // The default handler panics on any validation error; log instead so
        // a bad frame doesn't kill the app.
        device.on_uncaptured_error(Arc::new(|e| log::error!("wgpu: {e}")));

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("the surface is not supported by this adapter"))?;
        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB 8-bit format: the present shader encodes sRGB
        // itself so it can dither after encoding (see present.wgsl).
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .unwrap_or(config.format);
        if config.format.is_srgb() {
            log::warn!("no linear swapchain format; colours will be slightly off");
        }
        // Fifo = classic vsync: never tears, never renders frames nobody sees.
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        Ok(Self {
            instance,
            window,
            surface,
            device,
            queue,
            config,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Resizes the swapchain. Sizes are *physical* pixels, so HiDPI scaling
    /// changes arrive here as ordinary resizes.
    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Gets the next swapchain image, handling the recoverable cases.
    /// `None` means "skip this frame".
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as C;
        match self.surface.get_current_texture() {
            C::Success(frame) => Some(frame),
            C::Suboptimal(frame) => {
                // Usable now; reconfigure for the next frame.
                self.surface.configure(&self.device, &self.config);
                Some(frame)
            }
            C::Timeout | C::Occluded => None,
            C::Outdated => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            C::Lost => {
                log::warn!("surface lost; recreating");
                match self.instance.create_surface(Arc::clone(&self.window)) {
                    Ok(s) => {
                        self.surface = s;
                        self.surface.configure(&self.device, &self.config);
                    }
                    Err(e) => log::error!("could not recreate surface: {e}"),
                }
                None
            }
            other => {
                log::warn!("could not acquire frame: {other:?}");
                None
            }
        }
    }
}

/// Creates a pipeline that draws the fullscreen triangle from common.wgsl
/// with `fs_main` from `module`.
pub fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().copied().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A render pass with one color attachment.
pub fn color_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}
