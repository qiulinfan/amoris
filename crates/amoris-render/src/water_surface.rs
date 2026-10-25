//! Full-resolution material attributes, refreshed only when presentation columns change.
use crate::shaders;
pub(crate) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
pub(crate) struct Surface {
    pub normal_depth: wgpu::TextureView,
    pub flow_height: wgpu::TextureView,
    pub enabled: bool,
    size: [u32; 2],
    group: Option<wgpu::BindGroup>,
    field: Option<wgpu::BindGroup>,
    dirty: bool,
    #[cfg(test)]
    textures: [wgpu::Texture; 2],
}
pub(crate) struct Pipelines {
    local: wgpu::BindGroupLayout,
    field: wgpu::BindGroupLayout,
    fill: wgpu::ComputePipeline,
}
impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let local = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water attributes outputs"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba32Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            }),
        });
        let field = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water attributes field"),
            entries: &[1, 2]
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 1 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage { read_only: true }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .into_iter()
                .chain([
                    crate::water_ledge::header_entry(wgpu::ShaderStages::COMPUTE),
                    crate::water_ledge::mask_entry(wgpu::ShaderStages::COMPUTE),
                ])
                .collect::<Vec<_>>(),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water attributes"),
            bind_group_layouts: &[Some(&local), None, None, Some(&field)],
            immediate_size: 0,
        });
        let module = shaders::module(device, "water_surface");
        let fill = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water attributes"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("attributes"),
            compilation_options: shaders::compute_options(),
            cache: None,
        });
        Self { local, field, fill }
    }
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surface: &mut Surface,
        (uniform, columns, ledge, mask): (
            &wgpu::Buffer,
            &wgpu::Buffer,
            &wgpu::Buffer,
            &wgpu::TextureView,
        ),
        changed: bool,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        surface.dirty |= changed;
        if !surface.enabled || !surface.dirty {
            return;
        }
        if surface.group.is_none() {
            surface.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water attributes outputs"),
                layout: &self.local,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&surface.normal_depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&surface.flow_height),
                    },
                ],
            }));
            surface.field = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water attributes field"),
                layout: &self.field,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: columns.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: ledge.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: wgpu::BindingResource::TextureView(mask),
                    },
                ],
            }));
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water attributes"),
            timestamp_writes: profiler.compute_scope("water attributes"),
        });
        pass.set_pipeline(&self.fill);
        pass.set_bind_group(0, surface.group.as_ref().unwrap(), &[]);
        pass.set_bind_group(3, surface.field.as_ref().unwrap(), &[]);
        pass.dispatch_workgroups(surface.size[0].div_ceil(8), surface.size[1].div_ceil(8), 1);
        surface.dirty = false;
    }
}
impl Surface {
    pub fn new(device: &wgpu::Device, size: [u32; 2], enabled: bool) -> Self {
        let dimensions = if enabled { size } else { [1, 1] };
        let texture = |name| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d {
                    width: dimensions[0],
                    height: dimensions[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let textures = [
            texture("water normal and depth"),
            texture("water flow and surface"),
        ];
        Self {
            normal_depth: textures[0].create_view(&Default::default()),
            flow_height: textures[1].create_view(&Default::default()),
            enabled,
            size,
            group: None,
            field: None,
            dirty: true,
            #[cfg(test)]
            textures,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    #[test]
    fn attributes_match_fine_vertices_and_refresh_after_changed_columns() {
        let gpu = crate::Gpu::headless(crate::BackendChoice::from_env())
            .expect("attribute check requires a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("attribute GPU validation: {e}")
        }));
        let pipelines = Pipelines::new(&gpu.device);
        let mut profiler =
            crate::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, false, &gpu.info);
        for (nx, nz, tiled) in [(32u32, 32u32, true), (37, 19, false)] {
            let mut columns = (0..nx * nz)
                .map(|n| {
                    let depth = if n % 5 == 0 {
                        0.0
                    } else if n % 11 == 0 {
                        0.005
                    } else {
                        0.25 + (n % 7) as f32 * 0.03
                    };
                    [
                        n as f32 * 0.0001 + depth,
                        depth,
                        (n % nx) as f32 * 0.02,
                        (n / nx) as f32 * -0.01,
                    ]
                })
                .collect::<Vec<_>>();
            let field = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("attribute input"),
                size: u64::from(nx * nz) * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut params = [0u32; 32];
            params[2] = 0.25f32.to_bits();
            params[4..8].copy_from_slice(&[nx, nz, u32::from(tiled), 1]);
            let uniform = wgpu::util::DeviceExt::create_buffer_init(
                &gpu.device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("attribute uniforms"),
                    contents: bytemuck::cast_slice(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );
            let classifier = crate::water_ledge::Pipelines::new(&gpu.device);
            let mut ledges = crate::water_ledge::Ledges::new(
                &gpu.device,
                &classifier,
                &uniform,
                &field,
                [nx, nz],
            );
            let mut surface = Surface::new(&gpu.device, [nx, nz], true);
            for pass in 0..2 {
                if pass == 1 {
                    columns[7] = [3.25, 0.7, 4.0, -2.0];
                }
                let packed = if tiled {
                    (0..nx / 16 * nz / 16)
                        .flat_map(|tile| {
                            (0..256).map(move |n| {
                                ((tile % (nx / 16)) * 16 + n % 16)
                                    + nx * ((tile / (nx / 16)) * 16 + n / 16)
                            })
                        })
                        .map(|n| columns[n as usize])
                        .collect::<Vec<_>>()
                } else {
                    columns.clone()
                };
                gpu.queue
                    .write_buffer(&field, 0, bytemuck::cast_slice(&packed));
                let row = (nx * 16).div_ceil(256) * 256;
                let stages: [wgpu::Buffer; 2] = std::array::from_fn(|_| {
                    gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("attribute readback"),
                        size: u64::from(row * nz),
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                });
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                classifier.encode(&mut encoder, &mut ledges, true, true, &mut profiler);
                pipelines.encode(
                    &gpu.device,
                    &mut encoder,
                    &mut surface,
                    (&uniform, &field, &ledges.args, &ledges.mask),
                    true,
                    &mut profiler,
                );
                for (texture, stage) in surface.textures.iter().zip(&stages) {
                    encoder.copy_texture_to_buffer(
                        texture.as_image_copy(),
                        wgpu::TexelCopyBufferInfo {
                            buffer: stage,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(row),
                                rows_per_image: Some(nz),
                            },
                        },
                        wgpu::Extent3d {
                            width: nx,
                            height: nz,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                gpu.queue.submit([encoder.finish()]);
                let mut values = [
                    vec![0.0f32; (row * nz / 4) as usize],
                    vec![0.0f32; (row * nz / 4) as usize],
                ];
                for (stage, out) in stages.iter().zip(&mut values) {
                    crate::readback::read_blocking(&gpu.device, stage, out).unwrap();
                }
                let get = |i: i32, k: i32| {
                    columns[(i.clamp(0, nx as i32 - 1) as u32
                        + nx * k.clamp(0, nz as i32 - 1) as u32)
                        as usize]
                };
                let vertex = |i: i32, k: i32| {
                    let c = get(i, k);
                    if c[1] > 0.01 {
                        return c;
                    }
                    let (mut sum, mut count) = (0.0f32, 0u32);
                    for z in -1..=1 {
                        for x in -1..=1 {
                            let p = get(i + x, k + z);
                            if p[1] > 0.01 {
                                sum += p[0];
                                count += 1;
                            }
                        }
                    }
                    [
                        if count > 0 {
                            sum / count as f32
                        } else {
                            c[0] - 0.05
                        },
                        0.0,
                        0.0,
                        0.0,
                    ]
                };
                for k in 0..nz as i32 {
                    for i in 0..nx as i32 {
                        let c = vertex(i, k);
                        // Pass 1 raises the bed at the single known spike (7,0). Its four
                        // valid adjacent faces are cuts; use the opposite one-sided slope.
                        let cut = |a: [i32; 2], b: [i32; 2]| {
                            pass == 1
                                && (a == [7, 0] || b == [7, 0])
                                && a[0] >= 0
                                && a[1] >= 0
                                && b[0] >= 0
                                && b[1] >= 0
                                && a[0] < nx as i32
                                && b[0] < nx as i32
                                && a[1] < nz as i32
                                && b[1] < nz as i32
                        };
                        let slope = |di, dk| {
                            let lo = cut([i, k], [i - di, k - dk]);
                            let hi = cut([i, k], [i + di, k + dk]);
                            let a = if lo {
                                get(i, k)[0]
                            } else {
                                vertex(i - di, k - dk)[0]
                            };
                            let b = if hi {
                                get(i, k)[0]
                            } else {
                                vertex(i + di, k + dk)[0]
                            };
                            if lo && hi {
                                0.0
                            } else {
                                (b - a) * if lo || hi { 2.0 } else { 1.0 }
                            }
                        };
                        let expected = [
                            [-slope(1, 0), 0.5, -slope(0, 1), c[1]],
                            [c[2], c[3], (c[2] * c[2] + c[3] * c[3]).sqrt(), c[0]],
                        ];
                        let at = (k as u32 * row / 4 + i as u32 * 4) as usize;
                        for image in 0..2 {
                            for channel in 0..4 {
                                let a = values[image][at + channel];
                                let b = expected[image][channel];
                                assert!(
                                    (a - b).abs() <= 8.0 * f32::EPSILON * b.abs().max(1.0),
                                    "{nx}/{nz}/{tiled}/{pass}/{i}/{k}/{image}/{channel}: {a}/{b}"
                                );
                            }
                        }
                    }
                }
                assert!(!surface.dirty);
            }
        }
    }
}
