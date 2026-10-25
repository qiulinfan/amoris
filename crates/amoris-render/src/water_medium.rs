//! GPU camera-side medium transport. The field stays GPU-resident, including classification.
//! Scratch color is separate from its source; an indirect dispatch of zero does no image work.
use crate::{
    post::{HDR, Targets, tex},
    profiler::GpuProfiler,
    shaders,
};
use std::collections::BTreeMap;

struct Region {
    uniform: wgpu::Buffer,
    columns: wgpu::Buffer,
    ledge: wgpu::Buffer,
    mask: wgpu::TextureView,
    args: wgpu::Buffer,
    fields: wgpu::BindGroup,
    classify: wgpu::BindGroup,
    integrate: wgpu::BindGroup,
}
struct Images {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    view: wgpu::Buffer,
    sh: wgpu::Buffer,
    _scratch: wgpu::Texture,
    scratch_view: wgpu::TextureView,
    owner: wgpu::Buffer,
    params: wgpu::Buffer,
    copy: wgpu::BindGroup,
    tile_rows: u32,
    tiles: u32,
    stride: u32,
    regions: BTreeMap<u64, Region>,
}
/// What the waterline and the sun's shafts read besides the field (water.md 58): the sun's shadow
/// cascades, the caustics map and its placement, and the ripples.
pub(crate) struct MediumLight<'a> {
    pub shadows: &'a wgpu::TextureView,
    pub shadow_sampler: &'a wgpu::Sampler,
    pub caustics: &'a wgpu::TextureView,
    pub caustics_params: &'a wgpu::Buffer,
    pub ripples: &'a wgpu::TextureView,
    pub ripple_sampler: &'a wgpu::Sampler,
}
pub(crate) struct WaterMedium {
    field_layout: wgpu::BindGroupLayout,
    light_layout: wgpu::BindGroupLayout,
    classify_layout: wgpu::BindGroupLayout,
    integrate_layout: Option<wgpu::BindGroupLayout>,
    copy_layout: wgpu::BindGroupLayout,
    classify: wgpu::ComputePipeline,
    integrate: Option<(u32, wgpu::ComputePipeline)>,
    copy: wgpu::ComputePipeline,
    images: Option<Images>,
    stamp: u32,
}
fn buffer(binding: u32, ty: wgpu::BufferBindingType, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}
fn texture(
    binding: u32,
    ty: wgpu::TextureSampleType,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: ty,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled,
        },
        count: None,
    }
}
fn output(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: HDR,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}
fn layout(
    device: &wgpu::Device,
    name: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(name),
        entries,
    })
}
fn pipeline(
    device: &wgpu::Device,
    name: &str,
    module: &wgpu::ShaderModule,
    entry: &str,
    layout0: &wgpu::BindGroupLayout,
    light: Option<&wgpu::BindGroupLayout>,
    fields: Option<&wgpu::BindGroupLayout>,
) -> wgpu::ComputePipeline {
    let groups = if let Some(fields) = fields {
        vec![Some(layout0), light, None, Some(fields)]
    } else {
        vec![Some(layout0)]
    };
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(name),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(name),
        layout: Some(&layout),
        module,
        entry_point: Some(entry),
        compilation_options: shaders::compute_options(),
        cache: None,
    })
}
fn entry(binding: u32, resource: wgpu::BindingResource<'_>) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource }
}
fn tile_binding(buffer: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: 0,
        size: std::num::NonZeroU64::new(16),
    })
}
impl WaterMedium {
    pub fn new(device: &wgpu::Device) -> Self {
        let uniform = wgpu::BufferBindingType::Uniform;
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let write = wgpu::BufferBindingType::Storage { read_only: false };
        let field_layout = layout(
            device,
            "water medium field",
            &[
                buffer(1, uniform, false),
                buffer(2, read, false),
                crate::water_ledge::header_entry(wgpu::ShaderStages::COMPUTE),
                crate::water_ledge::mask_entry(wgpu::ShaderStages::COMPUTE),
            ],
        );
        let classify_layout = layout(
            device,
            "water medium classification",
            &[
                buffer(0, uniform, false),
                buffer(6, write, false),
                buffer(7, uniform, true),
            ],
        );
        let compute = wgpu::ShaderStages::COMPUTE;
        let sampled = |binding: u32, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding: u32, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Sampler(ty),
            count: None,
        };
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let light_layout = layout(
            device,
            "water medium light",
            &[
                sampled(
                    0,
                    wgpu::TextureSampleType::Depth,
                    wgpu::TextureViewDimension::D2Array,
                ),
                sampler(1, wgpu::SamplerBindingType::Comparison),
                sampled(12, filterable, wgpu::TextureViewDimension::D2),
                sampler(13, wgpu::SamplerBindingType::Filtering),
                sampled(15, filterable, wgpu::TextureViewDimension::D2),
                buffer(16, uniform, false),
            ],
        );
        let module = shaders::depth_module(device, "water_medium", 1);
        let classify = pipeline(
            device,
            "water medium classify",
            &module,
            "classify",
            &classify_layout,
            None,
            Some(&field_layout),
        );
        let copy_layout = layout(
            device,
            "water medium copy",
            &[
                texture(
                    0,
                    wgpu::TextureSampleType::Float { filterable: false },
                    false,
                ),
                output(1),
                buffer(2, uniform, true),
            ],
        );
        let module = shaders::module(device, "water_medium_copy");
        let copy = pipeline(
            device,
            "water medium copy",
            &module,
            "main",
            &copy_layout,
            None,
            None,
        );
        Self {
            field_layout,
            light_layout,
            classify_layout,
            integrate_layout: None,
            copy_layout,
            classify,
            integrate: None,
            copy,
            images: None,
            stamp: 0,
        }
    }
    pub fn clear(&mut self) {
        self.images = None;
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        targets: &Targets,
        view: &wgpu::Buffer,
        sh: &wgpu::Buffer,
    ) {
        let samples = targets.format.samples;
        if self.integrate.as_ref().is_none_or(|(n, _)| *n != samples) {
            let uniform = wgpu::BufferBindingType::Uniform;
            let group = layout(
                device,
                "water medium integrate",
                &[
                    buffer(0, uniform, false),
                    buffer(1, uniform, false),
                    texture(
                        2,
                        wgpu::TextureSampleType::Float { filterable: false },
                        false,
                    ),
                    texture(3, wgpu::TextureSampleType::Depth, samples > 1),
                    output(4),
                    buffer(
                        5,
                        wgpu::BufferBindingType::Storage { read_only: false },
                        false,
                    ),
                    buffer(
                        8,
                        wgpu::BufferBindingType::Storage { read_only: true },
                        false,
                    ),
                    buffer(7, uniform, true),
                ],
            );
            let module = shaders::depth_module(device, "water_medium", samples);
            self.integrate = Some((
                samples,
                pipeline(
                    device,
                    "water medium integrate",
                    &module,
                    "integrate",
                    &group,
                    Some(&self.light_layout),
                    Some(&self.field_layout),
                ),
            ));
            self.integrate_layout = Some(group);
            self.images = None;
        }
        if self.images.as_ref().is_some_and(|i| {
            i.color == targets.color.view
                && i.depth == targets.depth
                && i.view == *view
                && i.sh == *sh
        }) {
            return;
        }
        let (w, h) = targets.scene;
        // An owner stamp selects the first overlapping region for each pixel. Tile it to stay
        // within the baseline storage-binding limit, including unusually large render targets.
        let rows = (device.limits().max_storage_buffer_binding_size / (u64::from(w) * 4))
            .max(1)
            .min(u64::from(h)) as u32;
        let tiles = h.div_ceil(rows);
        let stride = device.limits().min_uniform_buffer_offset_alignment.max(16);
        let owner = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water medium owners"),
            size: u64::from(w) * u64::from(rows) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water medium tiles"),
            size: u64::from(tiles) * u64::from(stride),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scratch = tex(
            device,
            "water medium scratch",
            w,
            h,
            HDR,
            1,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        );
        let scratch_view = scratch.create_view(&Default::default());
        let copy = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water medium copy"),
            layout: &self.copy_layout,
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(&scratch_view)),
                entry(1, wgpu::BindingResource::TextureView(&targets.color.view)),
                entry(2, tile_binding(&params)),
            ],
        });
        self.images = Some(Images {
            color: targets.color.view.clone(),
            depth: targets.depth.clone(),
            view: view.clone(),
            sh: sh.clone(),
            _scratch: scratch,
            scratch_view,
            owner,
            params,
            copy,
            tile_rows: rows,
            tiles,
            stride,
            regions: BTreeMap::new(),
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        targets: &Targets,
        view: &wgpu::Buffer,
        sh: &wgpu::Buffer,
        light: &MediumLight<'_>,
        fields: &[(
            u64,
            &wgpu::Buffer,
            &wgpu::Buffer,
            &wgpu::Buffer,
            &wgpu::TextureView,
        )],
    ) {
        if fields.is_empty() {
            return;
        }
        self.prepare(device, targets, view, sh);
        let lighting = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water medium light"),
            layout: &self.light_layout,
            entries: &[
                entry(0, wgpu::BindingResource::TextureView(light.shadows)),
                entry(1, wgpu::BindingResource::Sampler(light.shadow_sampler)),
                entry(12, wgpu::BindingResource::TextureView(light.ripples)),
                entry(13, wgpu::BindingResource::Sampler(light.ripple_sampler)),
                entry(15, wgpu::BindingResource::TextureView(light.caustics)),
                entry(16, light.caustics_params.as_entire_binding()),
            ],
        });
        let images = self.images.as_mut().expect("medium targets prepared");
        images
            .regions
            .retain(|id, _| fields.iter().any(|(key, _, _, _, _)| key == id));
        for &(id, uniform, columns, ledge, mask) in fields {
            if images.regions.get(&id).is_some_and(|r| {
                r.uniform == *uniform
                    && r.columns == *columns
                    && r.ledge == *ledge
                    && r.mask == *mask
            }) {
                continue;
            }
            let args = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("water medium indirect"),
                size: 32,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
                mapped_at_creation: false,
            });
            let fields = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water medium field"),
                layout: &self.field_layout,
                entries: &[
                    entry(1, uniform.as_entire_binding()),
                    entry(2, columns.as_entire_binding()),
                    entry(10, ledge.as_entire_binding()),
                    entry(11, wgpu::BindingResource::TextureView(mask)),
                ],
            });
            let classify = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water medium classify"),
                layout: &self.classify_layout,
                entries: &[
                    entry(0, view.as_entire_binding()),
                    entry(6, args.as_entire_binding()),
                    entry(7, tile_binding(&images.params)),
                ],
            });
            let integrate = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water medium integrate"),
                layout: self.integrate_layout.as_ref().unwrap(),
                entries: &[
                    entry(0, view.as_entire_binding()),
                    entry(1, sh.as_entire_binding()),
                    entry(2, wgpu::BindingResource::TextureView(&images.color)),
                    entry(3, wgpu::BindingResource::TextureView(&images.depth)),
                    entry(4, wgpu::BindingResource::TextureView(&images.scratch_view)),
                    entry(5, images.owner.as_entire_binding()),
                    entry(8, args.as_entire_binding()),
                    entry(7, tile_binding(&images.params)),
                ],
            });
            images.regions.insert(
                id,
                Region {
                    uniform: uniform.clone(),
                    columns: columns.clone(),
                    ledge: ledge.clone(),
                    mask: mask.clone(),
                    args,
                    fields,
                    classify,
                    integrate,
                },
            );
        }
        let mut parameters = vec![0u8; (images.tiles * images.stride) as usize];
        for tile in 0..images.tiles {
            self.stamp = self.stamp.wrapping_add(1);
            if self.stamp == 0 {
                encoder.clear_buffer(&images.owner, 0, None);
                self.stamp = 1;
            }
            let y = tile * images.tile_rows;
            let rows = images.tile_rows.min(targets.scene.1 - y);
            let offset = (tile * images.stride) as usize;
            parameters[offset..offset + 16]
                .copy_from_slice(bytemuck::cast_slice(&[y, rows, self.stamp, 0]));
        }
        queue.write_buffer(&images.params, 0, &parameters);
        for tile in 0..images.tiles {
            let offset = tile * images.stride;
            for &(id, _, _, _, _) in fields {
                let region = &images.regions[&id];
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("water medium classify"),
                        timestamp_writes: profiler.compute_scope("water medium classify"),
                    });
                    pass.set_pipeline(&self.classify);
                    pass.set_bind_group(0, &region.classify, &[offset]);
                    pass.set_bind_group(3, &region.fields, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("water medium integrate"),
                        timestamp_writes: profiler.compute_scope("water medium integrate"),
                    });
                    pass.set_pipeline(&self.integrate.as_ref().unwrap().1);
                    pass.set_bind_group(0, &region.integrate, &[offset]);
                    pass.set_bind_group(1, &lighting, &[]);
                    pass.set_bind_group(3, &region.fields, &[]);
                    pass.dispatch_workgroups_indirect(&region.args, 0);
                }
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("water medium copy"),
                        timestamp_writes: profiler.compute_scope("water medium copy"),
                    });
                    pass.set_pipeline(&self.copy);
                    pass.set_bind_group(0, &images.copy, &[offset]);
                    pass.dispatch_workgroups_indirect(&region.args, 0);
                }
            }
        }
    }
}
