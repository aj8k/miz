//! Frame graph: scene -> HDR target -> present (tonemap, sRGB, dither) -> HUD.

mod limiter;
pub mod params;

use crate::config::Config;
use crate::gpu::shaders::{Shader, ShaderLib};
use crate::gpu::{self, Gpu, HdrTarget, Sampling};
use crate::scenes::manager::SceneManager;
use crate::scenes::{SceneCtx, SceneInit};
use crate::snapshot::{MAX_BANDS, Snapshot, WAVE_LEN};
use limiter::Limiter;
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
            visibility: wgpu::ShaderStages::VERTEX
                | wgpu::ShaderStages::FRAGMENT
                | wgpu::ShaderStages::COMPUTE,
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

/// Everything built from shaders. Rebuilt as a unit on shader hot reload,
/// so a failed reload leaves the previous set fully intact.
struct Pipelines {
    scenes: SceneManager,
    limiter: Limiter,
    present: wgpu::RenderPipeline,
    hud: wgpu::RenderPipeline,
}

impl Pipelines {
    fn build(
        gpu: &Gpu,
        shaders: &ShaderLib,
        audio: &AudioBuffers,
        sampling: &Sampling,
        scene_target: &HdrTarget,
        scene_name: &str,
    ) -> anyhow::Result<Self> {
        let device = &gpu.device;
        let limiter = Limiter::new(
            device,
            shaders,
            &audio.layout,
            &scene_target.view,
            scene_target.size,
        );
        let module = shaders.module(device, Shader::Present);
        let present = gpu::fullscreen_pipeline(
            device,
            "present",
            &module,
            &[&audio.layout, &sampling.layout, &limiter.read_layout],
            gpu.config.format,
            None,
        );
        let module = shaders.module(device, Shader::Hud);
        let hud = gpu::fullscreen_pipeline(
            device,
            "hud",
            &module,
            &[&audio.layout, &limiter.read_layout],
            gpu.config.format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let init = SceneInit {
            device,
            shaders,
            audio_layout: &audio.layout,
            sampling,
            size: scene_target.size,
        };
        let scenes = SceneManager::new(&init, scene_name)?;
        Ok(Self {
            scenes,
            limiter,
            present,
            hud,
        })
    }
}

pub struct Renderer {
    audio: AudioBuffers,
    params: FrameParams,
    sampling: Sampling,
    scene_target: HdrTarget,
    /// Internal resolution relative to the window (0.25..1.0).
    render_scale: f32,
    present_group: wgpu::BindGroup,
    p: Pipelines,
}

impl Renderer {
    pub fn new(
        gpu: &Gpu,
        shaders: &ShaderLib,
        cfg: &Config,
        scene_name: &str,
    ) -> anyhow::Result<Self> {
        let device = &gpu.device;
        let audio = AudioBuffers::new(device);
        let render_scale = cfg.visual.render_scale;
        let scene_target = HdrTarget::new(device, "scene", scaled(gpu.size(), render_scale));
        let sampling = Sampling::new(device);
        let present_group = sampling.bind(device, &scene_target.view);
        let p = Pipelines::build(gpu, shaders, &audio, &sampling, &scene_target, scene_name)?;
        let mut renderer = Self {
            audio,
            params: FrameParams::new(),
            sampling,
            scene_target,
            render_scale,
            present_group,
            p,
        };
        renderer.apply_config(gpu, cfg);
        Ok(renderer)
    }

    /// Rebuilds all pipelines from `shaders`, keeping the current scene,
    /// lock and timing. GPU-side validation errors are caught with an error
    /// scope; on any error the old pipelines stay in place.
    pub fn rebuild(&mut self, gpu: &Gpu, shaders: &ShaderLib) -> Result<(), String> {
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let built = Pipelines::build(
            gpu,
            shaders,
            &self.audio,
            &self.sampling,
            &self.scene_target,
            self.p.scenes.current_key(),
        );
        let gpu_error = pollster::block_on(scope.pop());
        match (built, gpu_error) {
            (Ok(mut p), None) => {
                p.scenes.adopt_state(&self.p.scenes);
                self.p = p;
                Ok(())
            }
            (Err(e), _) => Err(format!("{e:#}")),
            (_, Some(e)) => Err(e.to_string()),
        }
    }

    /// Applies the live-reloadable parts of the config.
    pub fn apply_config(&mut self, gpu: &Gpu, cfg: &Config) {
        self.params.set_calm(cfg.visual.calm);
        self.params.set_safety(
            cfg.safety.max_brightness_rate,
            cfg.safety.calm_max_brightness_rate,
        );
        self.p.scenes.set_timing(
            f64::from(cfg.visual.rotate_minutes) * 60.0,
            f64::from(cfg.visual.crossfade_seconds),
        );
        if (cfg.visual.render_scale - self.render_scale).abs() > f32::EPSILON {
            self.render_scale = cfg.visual.render_scale;
            self.resize(gpu);
        }
    }

    pub fn toggle_calm(&mut self) -> bool {
        self.params.toggle_calm()
    }

    pub fn calm(&self) -> bool {
        self.params.calm()
    }

    pub fn scenes(&mut self) -> &mut SceneManager {
        &mut self.p.scenes
    }

    pub fn scene_name(&self) -> &'static str {
        self.p.scenes.name()
    }

    pub fn scenes_locked(&self) -> bool {
        self.p.scenes.locked()
    }

    /// Reallocates size-dependent targets after the window was resized.
    pub fn resize(&mut self, gpu: &Gpu) {
        let size = scaled(gpu.size(), self.render_scale);
        if size == self.scene_target.size {
            return;
        }
        self.scene_target = HdrTarget::new(&gpu.device, "scene", size);
        self.present_group = self.sampling.bind(&gpu.device, &self.scene_target.view);
        self.p.scenes.resize(&gpu.device, &self.sampling, size);
        self.p
            .limiter
            .rebind(&gpu.device, &self.scene_target.view, size);
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
        let transition = self.p.scenes.update(time);
        self.params.update(snap, time, dt, self.scene_target.size);
        self.params.globals.transition = transition;
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
            dt,
        };
        self.p
            .scenes
            .render(&ctx, &mut encoder, &self.scene_target.view);

        // 3. Safety: measure the scene, update the exposure (GPU only).
        self.p.limiter.run(&mut encoder, &self.audio.group);

        // 4. Present + HUD into the swapchain image, at window resolution.
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
            pass.set_pipeline(&self.p.present);
            pass.set_bind_group(0, &self.audio.window_group, &[]);
            pass.set_bind_group(1, &self.present_group, &[]);
            pass.set_bind_group(2, &self.p.limiter.read_group, &[]);
            pass.draw(0..3, 0..1);
            if show_hud {
                pass.set_pipeline(&self.p.hud);
                pass.set_bind_group(1, &self.p.limiter.read_group, &[]);
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
