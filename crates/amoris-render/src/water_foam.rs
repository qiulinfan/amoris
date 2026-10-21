//! Persistent GPU surface foam. History is presentation-only, reset with a region or timeline.
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    count: [u32; 4],
    cell: f32,
    dt: f32,
    clear: u32,
    padding: u32,
}

pub struct WaterFoam {
    params: wgpu::Buffer,
    impacts: wgpu::Buffer,
    state: [wgpu::Buffer; 2],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    groups: [wgpu::BindGroup; 2],
    pipeline: wgpu::ComputePipeline,
    count: [u32; 4],
    cell: f32,
    current: usize,
}
impl WaterFoam {
    pub fn new(
        device: &wgpu::Device,
        columns: &wgpu::Buffer,
        size: [u32; 2],
        cell: f32,
        tiled: bool,
    ) -> Self {
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water foam params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let impacts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water spray foam impulses"),
            size: u64::from(size[0]) * u64::from(size[1]) * 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let state = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("water foam history"),
                size: u64::from(size[0]) * u64::from(size[1]) * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        // A texture keeps the fragment pipeline within WebGPU's eight storage-buffer bindings,
        // and preserves pixel sampling independently of the surface mesh's level of detail.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water foam density"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water foam"),
            entries: &std::array::from_fn::<_, 6, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: if binding == 4 {
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    }
                } else {
                    wgpu::BindingType::Buffer {
                        ty: if binding == 0 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: binding != 3,
                            }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    }
                },
                count: None,
            }),
        });
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water foam"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: columns.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: state[i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: state[1 - i].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: impacts.as_entire_binding(),
                    },
                ],
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water foam"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water foam"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_foam.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water foam"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        Self {
            params,
            impacts,
            state,
            texture,
            view,
            groups,
            pipeline,
            count: [size[0], size[1], u32::from(tiled), 0],
            cell,
            current: 0,
        }
    }
    pub fn impacts(&self) -> &wgpu::Buffer {
        &self.impacts
    }
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.state[self.current]
    }
    /// Encode at most once per submission. A rewind clears the old history on the GPU.
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        dt: f32,
        clear: bool,
        timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if dt <= 0.0 && !clear {
            return;
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                count: self.count,
                cell: self.cell,
                dt: dt.max(0.0),
                clear: u32::from(clear),
                padding: 0,
            }),
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water foam"),
            timestamp_writes: timestamps,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.groups[self.current], &[]);
        pass.dispatch_workgroups(self.count[0].div_ceil(8), self.count[1].div_ceil(8), 1);
        self.current = 1 - self.current;
    }
}
