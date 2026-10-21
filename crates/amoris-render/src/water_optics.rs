//! Scene inputs and layer partition for water. Allocated at scene resolution only when needed.
use crate::post::{DEPTH, HDR, SceneFormat, Targets, tex};

pub struct Optics {
    pub layer_pipeline: wgpu::RenderPipeline,
    pub layer_layout: wgpu::BindGroupLayout,
    pub depth_layout: wgpu::BindGroupLayout,
    pub depth_pipelines: [wgpu::RenderPipeline; 5],
    pub sampler: wgpu::Sampler,
    pub input: Inputs,
    resolve: Option<(u32, wgpu::ComputePipeline)>,
    source_depth: Option<wgpu::TextureView>,
}
pub struct Inputs {
    pub mask: wgpu::TextureView,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub layers: [wgpu::BindGroup; 2],
    pub size: (u32, u32),
    resolve_group: Option<wgpu::BindGroup>,
}
fn buffer_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Inputs {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, size: (u32, u32)) -> Self {
        let (w, h) = size;
        let mask_tex = tex(
            device,
            "water surface mask",
            w,
            h,
            DEPTH,
            1,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let mask = mask_tex.create_view(&Default::default());
        let color = tex(
            device,
            "water background",
            w,
            h,
            HDR,
            1,
            1,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = tex(
            device,
            "water opaque depth",
            w,
            h,
            wgpu::TextureFormat::R32Float,
            1,
            1,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let base = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water transparent base"),
                contents: &[0; 16],
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let layers = std::array::from_fn(|i| {
            let mode = [i as u32 + 1, 0, 0, 0];
            let params = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("water layer"),
                    contents: bytemuck::cast_slice(&mode),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water transparent layer"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: base.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: params.as_entire_binding(),
                    },
                ],
            })
        });
        Self {
            color_view: color.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            color,
            mask,
            layers,
            size,
            resolve_group: None,
        }
    }
}
impl Optics {
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
    ) -> Self {
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water layer partition"),
            entries: &[
                buffer_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                buffer_entry(5),
            ],
        });
        let layer_pipeline = Self::layer_pipeline(device, module, groups, &layer_layout, format);
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water depth geometry"),
            entries: &[
                buffer_entry(1),
                crate::water_ledge::header_entry(wgpu::ShaderStages::VERTEX_FRAGMENT),
                crate::water_ledge::mask_entry(wgpu::ShaderStages::VERTEX_FRAGMENT),
                crate::water_mesh::geometry_entry(),
                crate::water_surface::texture_entry(7),
                crate::water_surface::texture_entry(8),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water depth"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(&depth_layout),
            ],
            immediate_size: 0,
        });
        let depth_pipelines = [
            "vs_shallow",
            "vs_shallow_blocky",
            "vs_shallow_surround",
            "vs_shallow_stitch",
            "vs_shallow_ledge",
        ]
        .map(|entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("water surface depth"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &if entry == "vs_shallow_ledge" {
                        vec![Some(crate::water_ledge::vertices())]
                    } else {
                        vec![]
                    },
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_water_depth"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Greater),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        let input = Inputs::new(device, &layer_layout, (1, 1));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water scene sampling"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layer_pipeline,
            layer_layout,
            depth_layout,
            depth_pipelines,
            input,
            sampler,
            resolve: None,
            source_depth: None,
        }
    }
    fn layer_pipeline(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        layer: &wgpu::BindGroupLayout,
        format: SceneFormat,
    ) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water transparent"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(layer),
            ],
            immediate_size: 0,
        });
        crate::transparent::pipeline_entry(device, module, &layout, format, "fs_transparent_water")
    }
    pub fn reformat(
        &mut self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
    ) {
        self.layer_pipeline =
            Self::layer_pipeline(device, module, groups, &self.layer_layout, format);
        self.source_depth = None;
    }
    pub fn prepare(&mut self, device: &wgpu::Device, targets: &Targets) -> bool {
        if self.input.size == targets.scene && self.source_depth.as_ref() == Some(&targets.depth) {
            return false;
        }
        if self
            .resolve
            .as_ref()
            .is_none_or(|(n, _)| *n != targets.format.samples)
        {
            let module =
                crate::shaders::depth_module(device, "water_resolve", targets.format.samples);
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("water resolve opaque depth"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            });
            self.resolve = Some((targets.format.samples, pipeline));
        }
        self.input = Inputs::new(device, &self.layer_layout, targets.scene);
        self.input.resolve_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water depth resolve"),
            layout: &self.resolve.as_ref().unwrap().1.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.input.depth_view),
                },
            ],
        }));
        self.source_depth = Some(targets.depth.clone());
        true
    }
    pub fn background(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.color.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.input.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: targets.scene.0,
                height: targets.scene.1,
                depth_or_array_layers: 1,
            },
        );
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water scene depth"),
            timestamp_writes: profiler.compute_scope("water scene depth"),
        });
        pass.set_pipeline(&self.resolve.as_ref().unwrap().1);
        pass.set_bind_group(0, self.input.resolve_group.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups(targets.scene.0.div_ceil(8), targets.scene.1.div_ceil(8), 1);
    }
}
