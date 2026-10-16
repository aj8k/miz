//! GPU side of the photosensitivity limiter (see shaders/luma.wgsl).

use crate::gpu::shaders::{Shader, ShaderLib};

/// 16x16 threads per workgroup, each sampling every 4th pixel.
const PIXELS_PER_GROUP: u32 = 16 * 4;

pub struct Limiter {
    sum: wgpu::Buffer,
    state: wgpu::Buffer,
    compute_layout: wgpu::BindGroupLayout,
    compute_group: wgpu::BindGroup,
    reduce: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    /// Read-only view of the state for the present pass and HUD.
    pub read_layout: wgpu::BindGroupLayout,
    pub read_group: wgpu::BindGroup,
    size: (u32, u32),
}

impl Limiter {
    pub fn new(
        device: &wgpu::Device,
        shaders: &ShaderLib,
        audio_layout: &wgpu::BindGroupLayout,
        scene: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Self {
        let storage = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // Zero-initialized: `initialized == 0` makes the first frame adopt
        // the current brightness instead of fading in from black.
        let sum = storage("limiter sum", 4);
        let state = storage("limiter state", 16);

        let buffer_entry = |binding, read_only, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("limiter compute"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
                buffer_entry(2, false, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let read_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("limiter read"),
            entries: &[buffer_entry(0, true, wgpu::ShaderStages::FRAGMENT)],
        });
        let read_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("limiter read"),
            layout: &read_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: state.as_entire_binding(),
            }],
        });

        let module = shaders.module(device, Shader::Luma);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("limiter"),
            bind_group_layouts: &[Some(audio_layout), Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let reduce = compute("reduce");
        let update = compute("update");
        let compute_group = Self::bind(device, &compute_layout, scene, &sum, &state);
        Self {
            sum,
            state,
            compute_layout,
            compute_group,
            reduce,
            update,
            read_layout,
            read_group,
            size,
        }
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene: &wgpu::TextureView,
        sum: &wgpu::Buffer,
        state: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("limiter compute"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: state.as_entire_binding(),
                },
            ],
        })
    }

    /// The scene target was reallocated: measure the new one. The limiter
    /// state is kept, so a resize can't cause a jump either.
    pub fn rebind(&mut self, device: &wgpu::Device, scene: &wgpu::TextureView, size: (u32, u32)) {
        self.compute_group =
            Self::bind(device, &self.compute_layout, scene, &self.sum, &self.state);
        self.size = size;
    }

    /// Measures the scene and updates the exposure for this frame. Must run
    /// after the scene is rendered and before the present pass.
    pub fn run(&self, encoder: &mut wgpu::CommandEncoder, audio_group: &wgpu::BindGroup) {
        let groups = |n: u32| n.div_ceil(PIXELS_PER_GROUP);
        // Two passes: the update must see every workgroup's atomic add.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("limiter reduce"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.reduce);
            pass.set_bind_group(0, audio_group, &[]);
            pass.set_bind_group(1, &self.compute_group, &[]);
            pass.dispatch_workgroups(groups(self.size.0), groups(self.size.1), 1);
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("limiter update"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.update);
        pass.set_bind_group(0, audio_group, &[]);
        pass.set_bind_group(1, &self.compute_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
