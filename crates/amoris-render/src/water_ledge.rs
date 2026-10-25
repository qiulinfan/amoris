//! GPU-only bed-aware cap topology. The same helper supplies raster and medium partitions.
use crate::{profiler::GpuProfiler, shaders};
pub const STRIDE: u64 = 4;
pub fn header_entry(visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 10,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
pub fn vertices() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: STRIDE,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 0,
            shader_location: 0,
        }],
    }
}
pub fn mask_entry(visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 11,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Uint,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
pub struct Ledges {
    pub mask: wgpu::TextureView,
    pub indices: wgpu::Buffer,
    pub args: wgpu::Buffer,
    group: wgpu::BindGroup,
    field: wgpu::BindGroup,
    size: [u32; 2],
    last_enabled: bool,
}
pub struct Pipelines {
    local: wgpu::BindGroupLayout,
    field: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}
impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let local = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water ledge outputs"),
            entries: &[1, 2]
                .map(|i| wgpu::BindGroupLayoutEntry {
                    binding: i,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .into_iter()
                .chain([wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                }])
                .collect::<Vec<_>>(),
        });
        let field = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water ledge field"),
            entries: &[1, 2].map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if i == 1 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water ledge classify"),
            bind_group_layouts: &[Some(&local), None, None, Some(&field)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water ledge classify"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}",
                    include_str!("../shaders/common.wgsl"),
                    include_str!("../shaders/water_field.wgsl"),
                    include_str!("../shaders/water_ledge_classify.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water ledge classify"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("classify"),
            compilation_options: shaders::compute_options(),
            cache: None,
        });
        Self {
            local,
            field,
            pipeline,
        }
    }
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        ledges: &mut Ledges,
        changed: bool,
        enabled: bool,
        profiler: &mut GpuProfiler,
    ) {
        let refresh = changed || ledges.last_enabled != enabled;
        ledges.last_enabled = enabled;
        if !refresh {
            return;
        }
        encoder.clear_buffer(&ledges.args, 0, None);
        if !enabled {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water ledge classify"),
            timestamp_writes: profiler.compute_scope("water ledge classify"),
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &ledges.group, &[]);
        pass.set_bind_group(3, &ledges.field, &[]);
        pass.dispatch_workgroups(
            ((ledges.size[0] - 1) * (ledges.size[1] - 1)).div_ceil(64),
            1,
            1,
        );
    }
}
impl Ledges {
    pub fn new(
        device: &wgpu::Device,
        p: &Pipelines,
        uniform: &wgpu::Buffer,
        columns: &wgpu::Buffer,
        size: [u32; 2],
    ) -> Self {
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water active ledge quads"),
            size: (u64::from(size[0] - 1) * u64::from(size[1] - 1) * STRIDE).max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water ledge draw and shared classification"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water ledge mask"),
            size: wgpu::Extent3d {
                width: size[0] - 1,
                height: size[1] - 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mask = texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water ledge classify"),
            layout: &p.local,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: indices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: args.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
            ],
        });
        let field = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water ledge field"),
            layout: &p.field,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: columns.as_entire_binding(),
                },
            ],
        });
        Self {
            mask,
            indices,
            args,
            group,
            field,
            size,
            last_enabled: false,
        }
    }
}

/// Explicit diagnostics only; ordinary frames do not map the header or active IDs.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Stats {
    pub active_quads: u32,
    /// Includes collapsed unused side/film triangles; replacement caps are eight triangles per quad.
    pub submitted_triangles: u32,
    pub capacity_quads: u32,
}
pub struct Snapshot {
    stage: wgpu::Buffer,
    capacity: u32,
}
impl Snapshot {
    fn decode(&self, w: [u32; 4]) -> Stats {
        Stats {
            active_quads: w[1],
            submitted_triangles: w[0] / 3 * w[1],
            capacity_quads: self.capacity,
        }
    }
    pub async fn read(self) -> Result<Stats, String> {
        let mut w = [0u32; 4];
        crate::readback::read(&self.stage, &mut w).await?;
        Ok(self.decode(w))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_blocking(self, device: &wgpu::Device) -> Result<Stats, String> {
        let mut w = [0u32; 4];
        crate::readback::read_blocking(device, &self.stage, &mut w)?;
        Ok(self.decode(w))
    }
}
impl Ledges {
    pub fn copy_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Snapshot {
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water ledge diagnostic header"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(&self.args, 0, &stage, 0, 16);
        queue.submit([e.finish()]);
        Snapshot {
            stage,
            capacity: (self.size[0] - 1) * (self.size[1] - 1),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{BackendChoice, Gpu};
    use wgpu::util::DeviceExt;
    fn input<T: bytemuck::Pod>(g: &Gpu, values: &[T], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        g.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ledge independent fixture"),
                contents: bytemuck::cast_slice(values),
                usage,
            })
    }
    fn output(g: &Gpu, bytes: u64) -> wgpu::Buffer {
        g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ledge probe output"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }
    fn layout(g: &Gpu, entries: &[(u32, wgpu::BufferBindingType)]) -> wgpu::BindGroupLayout {
        g.device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ledge probe bindings"),
                entries: &entries
                    .iter()
                    .map(|(binding, ty)| wgpu::BindGroupLayoutEntry {
                        binding: *binding,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: *ty,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    })
                    .collect::<Vec<_>>(),
            })
    }
    fn bind(
        g: &Gpu,
        layout: &wgpu::BindGroupLayout,
        entries: &[(u32, &wgpu::Buffer)],
    ) -> wgpu::BindGroup {
        g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ledge probe bindings"),
            layout,
            entries: &entries
                .iter()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        })
    }
    fn compute(
        g: &Gpu,
        module: &wgpu::ShaderModule,
        entry: &str,
        local: &wgpu::BindGroupLayout,
        field: Option<&wgpu::BindGroupLayout>,
    ) -> wgpu::ComputePipeline {
        let groups = if let Some(field) = field {
            vec![Some(local), None, None, Some(field)]
        } else {
            vec![Some(local)]
        };
        let layout = g
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ledge probe"),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
        g.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
    }
    fn dispatch(
        g: &Gpu,
        p: &wgpu::ComputePipeline,
        local: &wgpu::BindGroup,
        field: Option<&wgpu::BindGroup>,
        count: u32,
        out: &wgpu::Buffer,
    ) -> Vec<[f32; 4]> {
        let stage = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ledge probe readback"),
            size: out.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = g.device.create_command_encoder(&Default::default());
        {
            let mut pass = e.begin_compute_pass(&Default::default());
            pass.set_pipeline(p);
            pass.set_bind_group(0, local, &[]);
            if let Some(field) = field {
                pass.set_bind_group(3, field, &[]);
            }
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        e.copy_buffer_to_buffer(out, 0, &stage, 0, out.size());
        g.queue.submit([e.finish()]);
        let mut values = vec![[0.0; 4]; (out.size() / 16) as usize];
        crate::readback::read_blocking(&g.device, &stage, &mut values).unwrap();
        values
    }
    fn export(name: &str, text: &str) {
        if let Some(path) = std::env::var_os("WATER_LEDGE_FIXTURE") {
            let path = std::path::PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(path)
            };
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(name), text).unwrap();
        }
    }
    #[test]
    fn actual_four_way_caps_walls_and_medium_exclude_the_rock_side_air_wedge() {
        let g = Gpu::headless(BackendChoice::from_env()).expect("ledge reference needs GPU");
        g.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("ledge reference: {e}")));
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let write = wgpu::BufferBindingType::Storage { read_only: false };
        let uniform = wgpu::BufferBindingType::Uniform;
        let entries = [(1, uniform), (2, read), (10, uniform)].map(|(binding, ty)| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }
        });
        let field_layout = g
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ledge probe field"),
                entries: &entries
                    .into_iter()
                    .chain([mask_entry(wgpu::ShaderStages::COMPUTE)])
                    .collect::<Vec<_>>(),
            });
        let vertex_layout = layout(&g, &[(0, uniform), (7, read), (8, write)]);
        let medium_layout = layout(&g, &[(8, read), (9, read), (10, write)]);
        export("forward.wgsl", &shaders::source("forward"));
        export(
            "classify.wgsl",
            &format!(
                "{}\n{}\n{}",
                include_str!("../shaders/common.wgsl"),
                include_str!("../shaders/water_field.wgsl"),
                include_str!("../shaders/water_ledge_classify.wgsl")
            ),
        );
        let forward = shaders::module(&g.device, "forward");
        let raster = compute(
            &g,
            &forward,
            "probe_water_ledges",
            &vertex_layout,
            Some(&field_layout),
        );
        let source = format!(
            "{}\n{}",
            shaders::depth_source("water_medium", 1),
            include_str!("../shaders/water_ledge_probe.wgsl")
        );
        export("medium.wgsl", &source);
        let medium = g.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("actual ledge medium reference"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let query = compute(
            &g,
            &medium,
            "probe_ledge_medium",
            &medium_layout,
            Some(&field_layout),
        );
        let classifier = Pipelines::new(&g.device);
        let mut view_words = [0.0f32; 256];
        for matrix in 0..4 {
            for i in 0..4 {
                view_words[matrix * 16 + i * 5] = 1.0;
            }
        }
        let view = input(&g, &view_words, wgpu::BufferUsages::UNIFORM);
        for axis in 0..2 {
            for reverse in [false, true] {
                let columns = (0..4)
                    .map(|n| {
                        let coordinate = if axis == 0 { n % 2 } else { n / 2 };
                        let high = (coordinate == 0) != reverse;
                        let mut c = [
                            if high { 6.4f32 } else { 0.6 },
                            if high { 0.4 } else { 0.6 },
                            0.0,
                            0.0,
                        ];
                        c[axis + 2] = if reverse { -1.0 } else { 1.0 };
                        c
                    })
                    .collect::<Vec<_>>();
                export(
                    &format!("field-{axis}-{reverse}.json"),
                    &serde_json::to_string(&columns).unwrap(),
                );
                let original_columns = columns.clone();
                let columns = input(
                    &g,
                    &columns,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                );
                let mut words = [0u32; 32];
                words[2] = 1.0f32.to_bits();
                words[4] = 2;
                words[5] = 2;
                words[7] = 1;
                let params = input(&g, &words, wgpu::BufferUsages::UNIFORM);
                let mut ledges = Ledges::new(&g.device, &classifier, &params, &columns, [2, 2]);
                let mut profiler = GpuProfiler::new(&g.device, &g.queue, false, &g.info);
                let mut e = g.device.create_command_encoder(&Default::default());
                classifier.encode(&mut e, &mut ledges, true, true, &mut profiler);
                g.queue.submit([e.finish()]);
                let field = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ledge probe field"),
                    layout: &field_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: columns.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: ledges.args.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 11,
                            resource: wgpu::BindingResource::TextureView(&ledges.mask),
                        },
                    ],
                });
                let indices = input(
                    &g,
                    &(0..72u32).collect::<Vec<_>>(),
                    wgpu::BufferUsages::STORAGE,
                );
                let out = output(&g, 72 * 32);
                let local = bind(&g, &vertex_layout, &[(0, &view), (7, &indices), (8, &out)]);
                let vertices = dispatch(&g, &raster, &local, Some(&field), 72, &out);
                let mut wall = 0;
                let mut side = 0;
                for pair in vertices.as_chunks::<2>().0 {
                    assert!(pair.iter().flatten().all(|v| v.is_finite()));
                    let kind = pair[1][3];
                    if kind == 2.0 {
                        wall += 1;
                        assert!((pair[0][axis * 2] - 1.0).abs() < 1e-6);
                        assert!(pair[0][1] >= 0.59 && pair[0][1] <= 6.41);
                        assert!(pair[1][axis * 2] * (if reverse { -1.0 } else { 1.0 }) > 0.99);
                    }
                    if kind == 4.0 {
                        side += 1;
                        assert!(pair[0][1] >= 5.99 && pair[0][1] <= 6.41);
                    }
                    if kind == 3.0 {
                        let high = (pair[0][axis * 2] < 1.0) != reverse;
                        let expected = if high { 6.4 } else { 0.6 };
                        if (pair[0][axis * 2] - 1.0).abs() > 1e-6 {
                            assert!((pair[0][1] - expected).abs() < 2e-6);
                        }
                    }
                }
                assert_eq!(wall, 12);
                assert_eq!(side, 12);
                for scale in [0.0, -1.0] {
                    let mut stopped = original_columns.clone();
                    for c in &mut stopped {
                        c[axis + 2] *= scale;
                    }
                    g.queue
                        .write_buffer(&columns, 0, bytemuck::cast_slice(&stopped));
                    let stopped = dispatch(&g, &raster, &local, Some(&field), 72, &out);
                    assert_eq!(
                        stopped
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .filter(|p| p[1][3] == 4.0)
                            .count(),
                        12,
                        "still water lost its actual sidewall"
                    );
                    assert!(
                        stopped
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .filter(|p| p[1][3] == 2.0)
                            .all(|p| p[0][3] == 0.0),
                        "stationary/uphill water drew a falling film"
                    );
                }
                g.queue
                    .write_buffer(&columns, 0, bytemuck::cast_slice(&original_columns));
                let high = if reverse { 1.25 } else { 0.75 };
                let low = if reverse { 0.75 } else { 1.25 };
                let air = if reverse { 0.99 } else { 1.01 };
                let mut records = Vec::<[f32; 4]>::new();
                for (coordinate, y) in
                    [(air, 3.0), (high, 6.2), (low, 0.3), (high, 3.0), (low, 0.7)]
                {
                    let mut p = [1.0, y, 1.0, 0.0];
                    p[axis * 2] = coordinate;
                    records.extend([p, [0.0; 4]]);
                }
                for y in [3.0, 6.2, 0.3] {
                    let mut p = [1.0, y, 1.0, 1.0];
                    p[axis * 2] = 0.5;
                    let mut d = [0.0; 4];
                    d[axis * 2] = 1.0;
                    records.extend([p, d]);
                }
                let mut p = [1.0, 7.0, 1.0, 8.0];
                p[axis * 2] = low;
                records.extend([p, [0.0, -1.0, 0.0, 0.0]]);
                let mut p = [1.0, 0.0, 1.0, 8.0];
                p[axis * 2] = high;
                records.extend([p, [0.0, 1.0, 0.0, 0.0]]);
                export(
                    &format!("rays-{axis}-{reverse}.json"),
                    &serde_json::to_string(&records).unwrap(),
                );
                let records = input(&g, &records, wgpu::BufferUsages::STORAGE);
                let bounds = input(
                    &g,
                    &[
                        0,
                        0,
                        0,
                        0,
                        0.0f32.to_bits(),
                        6.0f32.to_bits(),
                        0.6f32.to_bits(),
                        6.4f32.to_bits(),
                    ],
                    wgpu::BufferUsages::STORAGE,
                );
                let out = output(&g, 10 * 16);
                let local = bind(
                    &g,
                    &medium_layout,
                    &[(8, &bounds), (9, &records), (10, &out)],
                );
                let values = dispatch(&g, &query, &local, Some(&field), 10, &out);
                for (i, want) in [0.0, 1.0, 1.0, 0.0, 0.0].iter().enumerate() {
                    assert_eq!(
                        values[i][0], *want,
                        "inside axis={axis} reverse={reverse} point={i}"
                    );
                }
                for (i, want) in [0.0, 0.5, 0.5, 0.6, 0.4].iter().enumerate() {
                    assert!(
                        (values[i + 5][1] - want).abs() < 3e-5,
                        "path axis={axis} reverse={reverse} ray={i}: {}",
                        values[i + 5][1]
                    );
                }
                g.queue
                    .write_buffer(&ledges.args, 0, bytemuck::cast_slice(&[72u32, 0, 0, 0]));
                let legacy = dispatch(&g, &query, &local, Some(&field), 10, &out);
                assert_eq!(
                    legacy[0][0], 1.0,
                    "air-wedge control must detect the old wrong volume"
                );
                assert!(legacy[5][1] > 0.05);
            }
        }
    }
    #[test]
    fn a_resolved_steep_ramp_and_unreceived_columns_do_not_create_false_ledges() {
        let g = Gpu::headless(BackendChoice::from_env()).expect("ledge ramp needs GPU");
        g.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("ledge ramp: {e}")));
        let pipeline = Pipelines::new(&g.device);
        let mut params = [0u32; 32];
        params[2] = 1.0f32.to_bits();
        params[4] = 4;
        params[5] = 4;
        params[7] = 1;
        let uniform = input(&g, &params, wgpu::BufferUsages::UNIFORM);
        for axis in 0..2 {
            let columns = (0..16)
                .map(|n| {
                    let at = if axis == 0 { n % 4 } else { n / 4 };
                    [0.4 + at as f32 * 2.0, 0.4, 1.0, 0.0]
                })
                .collect::<Vec<_>>();
            let columns = input(&g, &columns, wgpu::BufferUsages::STORAGE);
            let mut ledges = Ledges::new(&g.device, &pipeline, &uniform, &columns, [4, 4]);
            let mut profiler = GpuProfiler::new(&g.device, &g.queue, false, &g.info);
            let mut e = g.device.create_command_encoder(&Default::default());
            pipeline.encode(&mut e, &mut ledges, true, true, &mut profiler);
            g.queue.submit([e.finish()]);
            let mut w = [0u32; 4];
            let stage = g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ramp counts"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut e = g.device.create_command_encoder(&Default::default());
            e.copy_buffer_to_buffer(&ledges.args, 0, &stage, 0, 16);
            g.queue.submit([e.finish()]);
            crate::readback::read_blocking(&g.device, &stage, &mut w).unwrap();
            assert_eq!(w[1], 0, "continuous ramp became a staircase");
        }
        let columns = input(
            &g,
            &[
                [6.4f32, 0.4, 1.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                [6.4, 0.4, 1.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
            ],
            wgpu::BufferUsages::STORAGE,
        );
        params[4] = 2;
        params[5] = 2;
        let uniform = input(&g, &params, wgpu::BufferUsages::UNIFORM);
        let mut ledges = Ledges::new(&g.device, &pipeline, &uniform, &columns, [2, 2]);
        let mut profiler = GpuProfiler::new(&g.device, &g.queue, false, &g.info);
        let mut e = g.device.create_command_encoder(&Default::default());
        pipeline.encode(&mut e, &mut ledges, true, true, &mut profiler);
        let stage = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("missing tile counts"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        e.copy_buffer_to_buffer(&ledges.args, 0, &stage, 0, 16);
        g.queue.submit([e.finish()]);
        let mut w = [0u32; 4];
        crate::readback::read_blocking(&g.device, &stage, &mut w).unwrap();
        assert_eq!(w[1], 0);
    }
    #[test]
    fn film_factors_match_independent_internal_bounce_sums() {
        let g = Gpu::headless(BackendChoice::from_env()).expect("film reference needs GPU");
        g.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("film reference: {e}")));
        let local_layout = layout(
            &g,
            &[
                (0, wgpu::BufferBindingType::Uniform),
                (7, wgpu::BufferBindingType::Storage { read_only: true }),
                (8, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        );
        let module = shaders::module(&g.device, "forward");
        let pipeline = compute(&g, &module, "probe_water_films", &local_layout, None);
        let mut cases = Vec::<[f32; 5]>::new();
        for cosine in [0.0, 0.001, 0.05, 0.25, 0.5, 0.9, 1.0] {
            for thickness in [0.0, 0.003, 0.03, 0.3] {
                cases.push([cosine, thickness, 0.0, 0.5, 3.0]);
            }
        }
        let words = cases
            .iter()
            .flatten()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>();
        let params = input(&g, &words, wgpu::BufferUsages::STORAGE);
        let view = input(&g, &[0u32; 256], wgpu::BufferUsages::UNIFORM);
        let out = output(&g, cases.len() as u64 * 32);
        let local = bind(&g, &local_layout, &[(0, &view), (7, &params), (8, &out)]);
        let result = dispatch(&g, &pipeline, &local, None, cases.len() as u32, &out);
        let mut expected = Vec::new();
        for (i, c) in cases.iter().enumerate() {
            let mut reference = [[0.0; 3]; 2];
            let cosine = f64::from(c[0]);
            let ct = (1.0 - 0.75 * 0.75 * (1.0 - cosine * cosine)).sqrt();
            let rs = (0.75 * cosine - ct) / (0.75 * cosine + ct);
            let rp = (cosine - 0.75 * ct) / (cosine + 0.75 * ct);
            for channel in 0..3 {
                let a = (-f64::from(c[2 + channel]) * f64::from(c[1]) / ct).exp();
                let (mut r, mut t) = (0.0, 0.0);
                for f in [rs * rs, rp * rp] {
                    r += f * 0.5;
                    let mut forward = (1.0 - f) * a;
                    for _ in 0..20000 {
                        t += forward * (1.0 - f) * 0.5;
                        let backward = forward * f * a;
                        r += backward * (1.0 - f) * 0.5;
                        forward = backward * f * a;
                        if forward < 1e-14 {
                            break;
                        }
                    }
                }
                assert!(
                    (f64::from(result[i * 2][channel]) - r).abs() < 6e-6,
                    "film reflection {i}/{channel}"
                );
                assert!(
                    (f64::from(result[i * 2 + 1][channel]) - t).abs() < 6e-6,
                    "film transmission {i}/{channel}"
                );
                reference[0][channel] = r;
                reference[1][channel] = t;
                assert!(r + t <= 1.0 + 1e-12);
            }
            expected.push(reference);
        }
        export(
            "film.json",
            &serde_json::to_string(&serde_json::json!({"cases":cases,"expected":expected}))
                .unwrap(),
        );
    }
}
