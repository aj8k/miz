//! Frame graph: scene -> HDR target -> present (tonemap, sRGB, dither) -> HUD.

pub mod params;

use crate::gpu::{self, Gpu, HDR_FORMAT, shaders};
use crate::scenes::{Scene, SceneCtx, bass_pulse::BassPulse};
use crate::snapshot::{MAX_BANDS, Snapshot, WAVE_LEN};
use params::{FrameParams, Globals};

/// Audio data on the GPU: bind group 0 for every pipeline.
///
/// There are two copies of the globals: scenes render at the internal
/// resolution, the present/HUD passes at window resolution. (Writing one
/// buffer twice wouldn't work: all queue writes land before the submitted
/// commands execute, so every pass would see the last value.)
struct AudioBuffers {
    globals: wgpu::Buffer,
    window_globals: wgpu::Buffer,
    bands: wgpu::Buffer,
    wave: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    window_group: wgpu::BindGroup,
}

impl AudioBuffers {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size: usize, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = buffer("globals", size_of::<Globals>(), wgpu::BufferUsages::UNIFORM);
        let window_globals = buffer(
            "window globals",
            size_of::<Globals>(),
            wgpu::BufferUsages::UNIFORM,
        );
        let bands = buffer("bands", MAX_BANDS * 4, wgpu::BufferUsages::STORAGE);
        let wave = buffer("wave", WAVE_LEN * 8, wgpu::BufferUsages::STORAGE);

        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("audio"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage),
                entry(2, storage),
            ],
        });
        let make_group = |globals: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("audio"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: bands.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wave.as_entire_binding(),
                    },
                ],
            })
        };
        let group = make_group(&globals);
        let window_group = make_group(&window_globals);
        Self {
            globals,
            window_globals,
            bands,
            wave,
            layout,
            group,
            window_group,
        }
    }
}

/// An offscreen texture we render into and later sample from.
struct Target {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Target {
    fn new(device: &wgpu::Device, label: &str, size: (u32, u32)) -> Self {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            view: tex.create_view(&Default::default()),
            size,
        }
    }
}

pub struct Renderer {
    audio: AudioBuffers,
    params: FrameParams,
    scene: Box<dyn Scene>,
    scene_target: Target,
    /// Internal resolution relative to the window (0.5..1.0).
    render_scale: f32,

    sampler: wgpu::Sampler,
    present_layout: wgpu::BindGroupLayout,
    present_group: wgpu::BindGroup,
    present_pipeline: wgpu::RenderPipeline,
    hud_pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let audio = AudioBuffers::new(device);
        let render_scale = 1.0;
        let scene_target = Target::new(device, "scene", scaled(gpu.size(), render_scale));

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let present_group = texture_group(device, &present_layout, &scene_target.view, &sampler);

        let module = shaders::compile(device, "present", shaders::PRESENT);
        let present_pipeline = gpu::fullscreen_pipeline(
            device,
            "present",
            &module,
            &[&audio.layout, &present_layout],
            gpu.config.format,
            None,
        );
        let module = shaders::compile(device, "hud", shaders::HUD);
        let hud_pipeline = gpu::fullscreen_pipeline(
            device,
            "hud",
            &module,
            &[&audio.layout],
            gpu.config.format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        let scene = Box::new(BassPulse::new(device, &audio.layout));

        Self {
            audio,
            params: FrameParams::new(),
            scene,
            scene_target,
            render_scale,
            sampler,
            present_layout,
            present_group,
            present_pipeline,
            hud_pipeline,
        }
    }

    pub fn scene_name(&self) -> &'static str {
        self.scene.name()
    }

    /// Reallocates size-dependent targets after the window was resized.
    pub fn resize(&mut self, gpu: &Gpu) {
        let size = scaled(gpu.size(), self.render_scale);
        if size == self.scene_target.size {
            return;
        }
        self.scene_target = Target::new(&gpu.device, "scene", size);
        self.present_group = texture_group(
            &gpu.device,
            &self.present_layout,
            &self.scene_target.view,
            &self.sampler,
        );
    }

    pub fn render(
        &mut self,
        gpu: &Gpu,
        output: &wgpu::TextureView,
        snap: &Snapshot,
        time: f64,
        dt: f32,
        show_hud: bool,
    ) {
        let queue = &gpu.queue;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());

        // 1. Upload this frame's audio data (scenes see the scene-target size).
        self.params.update(snap, time, dt, self.scene_target.size);
        queue.write_buffer(
            &self.audio.globals,
            0,
            bytemuck::bytes_of(&self.params.globals),
        );
        queue.write_buffer(
            &self.audio.bands,
            0,
            bytemuck::cast_slice(&self.params.bands),
        );
        queue.write_buffer(&self.audio.wave, 0, bytemuck::cast_slice(&snap.wave));

        // 2. Scene into the HDR target.
        let ctx = SceneCtx {
            audio_group: &self.audio.group,
        };
        self.scene
            .render(&ctx, &mut encoder, &self.scene_target.view);

        // 3. Present + HUD into the swapchain image, at window resolution.
        let mut window_globals = self.params.globals;
        let (w, h) = gpu.size();
        window_globals.resolution = [w as f32, h as f32];
        queue.write_buffer(
            &self.audio.window_globals,
            0,
            bytemuck::bytes_of(&window_globals),
        );

        {
            let mut pass =
                gpu::color_pass(&mut encoder, "present", output, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&self.present_pipeline);
            pass.set_bind_group(0, &self.audio.window_group, &[]);
            pass.set_bind_group(1, &self.present_group, &[]);
            pass.draw(0..3, 0..1);
            if show_hud {
                pass.set_pipeline(&self.hud_pipeline);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }
}

fn scaled((w, h): (u32, u32), scale: f32) -> (u32, u32) {
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

fn texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
