//! Current-GPU-field surface patches. Classification retains its original decisions;
//! patch-major indices are emitted from templates only when patch selections change.
//! One baseline indexed indirect draw is shared by the surface mask and optical color pass.
//! Excluded slots share column zero as a collapsed primitive; absent tiles use finite dry data.
use crate::shaders;

pub(crate) struct Pipelines {
    local: wgpu::BindGroupLayout,
    field: wgpu::BindGroupLayout,
    build: wgpu::ComputePipeline,
    #[cfg(test)]
    reference_build: wgpu::ComputePipeline,
    compact: crate::water_compact::Pipelines,
    templates: wgpu::Buffer,
    settings: wgpu::Buffer,
    held: Option<[u32; 8]>,
    generation: u32,
}
pub(crate) struct Mesh {
    pub spec: [u32; 4],
    pub indices: wgpu::Buffer,
    pub args: wgpu::Buffer,
    pub geometry: wgpu::Buffer,
    compact: crate::water_compact::Compact,
    local: wgpu::BindGroup,
    field: wgpu::BindGroup,
    tiles: [u32; 2],
}
/// Diagnostic counts from the most recently submitted water surface. Reading is explicit;
/// normal rendering never maps these buffers or waits for a CPU visibility decision.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct MeshStats {
    pub input_triangles: u32,
    pub active_triangles: u32,
    pub submitted_triangles: u32,
    pub patches: u32,
    pub visible: u32,
    pub dry: u32,
    pub outside: u32,
    pub rewritten_patches: u32,
    pub levels: [u32; 4],
    pub max_error_pixels: f32,
    pub transition_patches: u32,
    pub accuracy_resets: u32,
    pub capacity_triangles: u32,
    /// Whether this frame rebuilt the dense index list; classification changes remain separate.
    pub repacked_indices: bool,
    /// Explicit additional buffers for cached counts, chunk offsets, grid and indirect dispatch.
    pub compaction_buffer_bytes: u64,
}
/// Explicit native geometry readback for accuracy checks, never used by normal rendering.
pub struct MeshIndices {
    pub spec: [u32; 4],
    pub indices: Vec<u32>,
    pub geometry: Vec<[u32; 4]>,
}
impl MeshIndices {
    /// Decode the actual GPU-selected current-height stencil for native geometry verification.
    pub fn stencil(&self, vertex: u32) -> ([f64; 2], [u32; 6], [f64; 6]) {
        let [nx, _, _, _] = self.spec;
        if vertex & 0x80000000 == 0 {
            return (
                [(vertex % nx) as f64, (vertex / nx) as f64],
                [vertex, 0, 0, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            );
        }
        let header = self.geometry[0];
        let value = vertex & 0x7fffffff;
        let tile = value / header[0];
        let id = value % header[0];
        let tiles = (nx - 2) / 16 + 1;
        let base = [tile % tiles * 16, tile / tiles * 16];
        let at = 1 + id as usize * 4;
        let a = self.geometry[at];
        let b = self.geometry[at + 1];
        let c = self.geometry[at + 2];
        let d = self.geometry[at + 3];
        let state = self.geometry[(header[1] + tile * 2) as usize];
        let t = f32::from_bits(state[2]) as f64;
        let position = [
            base[0] as f64 + f32::from_bits(a[0]) as f64,
            base[1] as f64 + f32::from_bits(a[1]) as f64,
        ];
        let local = [f32::from_bits(a[0]), f32::from_bits(a[1])];
        if local.iter().any(|&v| v == 0.0 || v == 16.0) {
            let vertex = position[0] as u32 + nx * position[1] as u32;
            return (
                position,
                [vertex, 0, 0, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            );
        }
        let ids = [a[2], a[3], b[0], c[0], c[1], c[2]]
            .map(|v| base[0] + v % 17 + nx * (base[1] + v / 17));
        let weights = [b[1], b[2], b[3], c[3], d[0], d[1]].map(|v| f32::from_bits(v) as f64);
        let weights = std::array::from_fn(|n| weights[n] * if n < 3 { 1.0 - t } else { t });
        (position, ids, weights)
    }
}

// All unit boundary edges are retained at every level, independently of neighbor levels.
// Header pairs contain offset/count for levels 0..3; level zero uses the original grid.
fn templates() -> Vec<u32> {
    crate::water_topology::shared().indices.clone()
}

fn buffer(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
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
}
pub(crate) fn geometry_entry() -> wgpu::BindGroupLayoutEntry {
    let mut e = buffer(9, wgpu::BufferBindingType::Storage { read_only: true });
    e.visibility = wgpu::ShaderStages::VERTEX;
    e
}
fn entry(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: b.as_entire_binding(),
    }
}
#[cfg(test)]
fn reference_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/water_field.wgsl"),
        include_str!("../tests/fixtures/water_mesh_stable_reference.wgsl"),
    )
}
impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let uniform = wgpu::BufferBindingType::Uniform;
        let local = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water mesh outputs"),
            entries: &[
                buffer(0, uniform),
                buffer(1, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer(3, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer(4, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(5, uniform),
                buffer(6, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let field = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water mesh current field"),
            entries: &[
                buffer(1, uniform),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: true }),
                crate::water_ledge::header_entry(wgpu::ShaderStages::COMPUTE),
                crate::water_ledge::mask_entry(wgpu::ShaderStages::COMPUTE),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water mesh"),
            bind_group_layouts: &[Some(&local), None, None, Some(&field)],
            immediate_size: 0,
        });
        let module = shaders::module(device, "water_mesh");
        let build = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water mesh"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("build"),
            compilation_options: shaders::compute_options(),
            cache: None,
        });
        #[cfg(test)]
        let reference_build = {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("water mesh stable-index test reference"),
                source: wgpu::ShaderSource::Wgsl(reference_source().into()),
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("water mesh stable-index test reference"),
                layout: Some(&layout),
                module: &module,
                entry_point: Some("build"),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let templates = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water LOD templates"),
                contents: bytemuck::cast_slice(&templates()),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let settings = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water LOD settings"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            local,
            field,
            build,
            #[cfg(test)]
            reference_build,
            compact: crate::water_compact::Pipelines::new(device),
            templates,
            settings,
            held: None,
            generation: 1,
        }
    }
    pub fn configure(&mut self, queue: &wgpu::Queue, lod: crate::LodSettings, dt: f32) {
        let pixels = if lod.pixels.is_finite() {
            lod.pixels.max(0.0)
        } else {
            0.0
        };
        let hysteresis = if lod.hysteresis.is_finite() {
            lod.hysteresis.clamp(0.0, 0.95)
        } else {
            0.0
        };
        let base = [
            pixels.to_bits(),
            hysteresis.to_bits(),
            u32::from(lod.mode == crate::LodMode::On),
        ];
        if self.held.is_some_and(|old| old[..3] != base) {
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        let words = [
            base[0],
            base[1],
            base[2],
            self.generation,
            dt.clamp(0.0, 0.1).to_bits(),
            0.15f32.to_bits(),
            0,
            0,
        ];
        if self.held != Some(words) {
            queue.write_buffer(&self.settings, 0, bytemuck::cast_slice(&words));
            self.held = Some(words);
        }
    }
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        mesh: &Mesh,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        encoder.clear_buffer(&mesh.args, 0, None);
        if mesh.tiles.contains(&0) {
            self.compact.encode(encoder, &mesh.compact, profiler);
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water mesh"),
            timestamp_writes: profiler.compute_scope("water mesh"),
        });
        pass.set_pipeline(&self.build);
        pass.set_bind_group(0, &mesh.local, &[]);
        pass.set_bind_group(3, &mesh.field, &[]);
        pass.dispatch_workgroups(mesh.tiles[0], mesh.tiles[1], 1);
        drop(pass);
        self.compact.encode(encoder, &mesh.compact, profiler);
    }
}
#[cfg(test)]
impl Pipelines {
    fn encode_reference(&self, encoder: &mut wgpu::CommandEncoder, mesh: &Mesh) {
        encoder.clear_buffer(&mesh.args, 0, None);
        if mesh.tiles.contains(&0) {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&self.reference_build);
        pass.set_bind_group(0, &mesh.local, &[]);
        pass.set_bind_group(3, &mesh.field, &[]);
        pass.dispatch_workgroups(mesh.tiles[0], mesh.tiles[1], 1);
    }
}
impl Mesh {
    pub fn new(
        device: &wgpu::Device,
        pipelines: &Pipelines,
        view: &wgpu::Buffer,
        uniform: &wgpu::Buffer,
        columns: &wgpu::Buffer,
        (ledge, ledge_mask): (&wgpu::Buffer, &wgpu::TextureView),
        spec: [u32; 4],
    ) -> Self {
        let [nx, nz, blocky, per] = spec;
        let (units, vertices) = if blocky != 0 {
            ([nx / per, nz / per], 18)
        } else {
            ([nx - 1, nz - 1], 6)
        };
        let topology = crate::water_topology::shared();
        let tiles = units.map(|n| n.div_ceil(16));
        let extra = if blocky == 0 {
            u64::from(topology.extra) * u64::from(tiles[0]) * u64::from(tiles[1])
        } else {
            0
        };
        let mut records = topology.vertices.clone();
        records.resize(records.len() + (tiles[0] * tiles[1] * 2) as usize, [0; 4]);
        let geometry_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        #[cfg(test)]
        let geometry_usage = geometry_usage | wgpu::BufferUsages::COPY_DST;
        let geometry = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water morph geometry and state"),
                contents: bytemuck::cast_slice(&records),
                usage: geometry_usage,
            },
        );
        assert!(
            u64::from(topology.vertex_count) * u64::from(tiles[0]) * u64::from(tiles[1])
                < 0x80000000
        );
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water masked indices"),
            size: ((u64::from(units[0]) * u64::from(units[1]) * vertices + extra) * 4).max(4),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDEX
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let args = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water mesh indirect and counts"),
            size: 96,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water previous patch masks"),
            size: (u64::from(tiles[0]) * u64::from(tiles[1]) * 4).max(4),
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let local = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water mesh outputs"),
            layout: &pipelines.local,
            entries: &[
                entry(0, view),
                entry(1, &indices),
                entry(2, &args),
                entry(3, &mask),
                entry(4, &pipelines.templates),
                entry(5, &pipelines.settings),
                entry(6, &geometry),
            ],
        });
        let field = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water mesh current field"),
            layout: &pipelines.field,
            entries: &[
                entry(1, uniform),
                entry(2, columns),
                entry(10, ledge),
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::TextureView(ledge_mask),
                },
            ],
        });
        let compact = crate::water_compact::Compact::new(
            device,
            &pipelines.compact,
            [&indices, &args, &mask, &pipelines.templates, &geometry],
            spec,
        );
        Self {
            spec,
            indices,
            args,
            geometry,
            compact,
            local,
            field,
            tiles,
        }
    }
    pub fn draw_indices(&self) -> &wgpu::Buffer {
        &self.indices
    }
    pub fn draw_args(&self) -> &wgpu::Buffer {
        &self.args
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_indices(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<MeshIndices, String> {
        let stats = self.stats(device, queue)?;
        let bytes = self.draw_indices().size();
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water geometry diagnostic"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(self.draw_indices(), 0, &stage, 0, bytes);
        let geometry_stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water morph diagnostic"),
            size: self.geometry.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&self.geometry, 0, &geometry_stage, 0, self.geometry.size());
        queue.submit([encoder.finish()]);
        let mut geometry = vec![[0u32; 4]; (self.geometry.size() / 16) as usize];
        crate::readback::read_blocking(device, &geometry_stage, &mut geometry)?;
        let mut indices = vec![0u32; (bytes / 4) as usize];
        crate::readback::read_blocking(device, &stage, &mut indices)?;
        indices.truncate(stats.submitted_triangles as usize * 3);
        Ok(MeshIndices {
            spec: self.spec,
            indices,
            geometry,
        })
    }
    pub fn copy_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> StatsReadback {
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water mesh diagnostics"),
            size: 96,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(self.draw_args(), 0, &stage, 0, 96);
        queue.submit([encoder.finish()]);
        StatsReadback {
            stage,
            spec: self.spec,
            capacity: (self.indices.size() / 12) as u32,
            compaction_bytes: self.compact.bytes(),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<MeshStats, String> {
        let r = self.copy_stats(device, queue);
        let mut words = [0u32; 24];
        crate::readback::read_blocking(device, &r.stage, &mut words)?;
        Ok(r.decode(words))
    }
}

pub(crate) struct StatsReadback {
    stage: wgpu::Buffer,
    spec: [u32; 4],
    capacity: u32,
    compaction_bytes: u64,
}
impl StatsReadback {
    fn decode(&self, words: [u32; 24]) -> MeshStats {
        let [nx, nz, blocky, per] = self.spec;
        MeshStats {
            input_triangles: if blocky != 0 {
                (nx / per) * (nz / per) * 6
            } else {
                (nx - 1) * (nz - 1) * 2
            },
            active_triangles: words[9] / 3,
            submitted_triangles: words[0] / 3,
            patches: words[5],
            visible: words[6],
            dry: words[7],
            outside: words[8],
            rewritten_patches: words[10],
            levels: words[11..15].try_into().unwrap(),
            max_error_pixels: f32::from_bits(words[15]),
            transition_patches: words[16],
            accuracy_resets: words[17],
            capacity_triangles: self.capacity,
            compaction_buffer_bytes: self.compaction_bytes,
            repacked_indices: words[18] != 0,
        }
    }
    pub async fn read(self) -> Result<MeshStats, String> {
        let mut words = [0u32; 24];
        crate::readback::read(&self.stage, &mut words).await?;
        Ok(self.decode(words))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    #[test]
    fn lod_templates_cover_the_patch_and_keep_every_shared_boundary_edge() {
        let t = super::templates();
        for level in 1..4 {
            let indices = &t[t[level * 2] as usize..(t[level * 2] + t[level * 2 + 1]) as usize];
            let mut edges = BTreeMap::new();
            let mut area = 0i32;
            for tri in indices.as_chunks::<3>().0 {
                let p = tri
                    .iter()
                    .map(|&v| [(v % 17) as i32, (v / 17) as i32])
                    .collect::<Vec<_>>();
                let twice = (p[1][1] - p[0][1]) * (p[2][0] - p[0][0])
                    - (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]);
                assert!(twice > 0);
                area += twice;
                for i in 0..3 {
                    let a = tri[i].min(tri[(i + 1) % 3]);
                    let b = tri[i].max(tri[(i + 1) % 3]);
                    *edges.entry((a, b)).or_insert(0) += 1;
                }
            }
            assert_eq!(area, 512);
            let mut boundary = 0;
            for ((a, b), count) in edges {
                let a = [a % 17, a / 17];
                let b = [b % 17, b / 17];
                let outer = (a[0] == b[0] && (a[0] == 0 || a[0] == 16))
                    || (a[1] == b[1] && (a[1] == 0 || a[1] == 16));
                if outer {
                    assert_eq!(count, 1);
                    assert_eq!(a[0].abs_diff(b[0]) + a[1].abs_diff(b[1]), 1);
                    boundary += 1;
                } else {
                    assert_eq!(count, 2);
                }
            }
            assert_eq!(boundary, 64);
            assert_eq!(indices.len(), 1536 >> level);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod compaction_tests {
    use super::*;
    use glam::Vec3;
    use wgpu::util::DeviceExt;

    #[test]
    fn stable_index_reference_shader_validates() {
        let source = reference_source();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        // Production classifies with exactly the original prefix, before any stable-slot writes.
        let original = include_str!("../tests/fixtures/water_mesh_stable_reference.wgsl");
        let prefix = original
            .split("    workgroupBarrier();if(patch_update==0u){return;}")
            .next()
            .unwrap();
        assert_eq!(
            include_str!("../shaders/water_mesh.wgsl"),
            format!("{prefix}}}\n")
        );
    }
    #[test]
    fn selected_templates_never_emit_the_collapsed_zero_triangle() {
        // Report the real source-bound replay identity without opening a GPU adapter.
        println!(
            "resident numerical kernel {}",
            crate::water_resident::kernel_key()
        );
        let g = crate::water_topology::shared();
        for level in 0..10 {
            let offset = g.indices[level * 2] as usize;
            let count = g.indices[level * 2 + 1] as usize;
            assert!(count > 0 && count.is_multiple_of(3));
            for &tri in g.indices[offset..offset + count].as_chunks::<3>().0 {
                let emitted = if level >= 4 {
                    tri.map(|v| 0x80000000 | v)
                } else {
                    tri
                };
                assert_ne!(emitted, [0, 0, 0], "template {level}");
            }
            assert!(count <= 1536 + g.extra as usize);
        }
    }
    struct Pair {
        view: wgpu::Buffer,
        columns: wgpu::Buffer,
        ledge: wgpu::Buffer,
        ledge_mask: wgpu::Texture,
        dense: Mesh,
        reference: Mesh,
        spec: [u32; 4],
    }
    impl Pair {
        fn new(gpu: &crate::Gpu, p: &Pipelines, spec: [u32; 4]) -> Self {
            let device = &gpu.device;
            let view = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh oracle view"),
                size: 752,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut shallow = [0u32; 32];
            shallow[2] = 0.25f32.to_bits();
            shallow[4..8].copy_from_slice(&[spec[0], spec[1], 0, spec[3]]);
            shallow[19] = (spec[2] as f32).to_bits();
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh oracle field"),
                contents: bytemuck::cast_slice(&shallow),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let columns = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh oracle columns"),
                size: u64::from(spec[0]) * u64::from(spec[1]) * 16,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let ledge = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh oracle no ledges"),
                contents: bytemuck::cast_slice(&[0u32; 4]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let ledge_mask = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh oracle no ledges"),
                size: wgpu::Extent3d {
                    width: spec[0] - 1,
                    height: spec[1] - 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mask = ledge_mask.create_view(&Default::default());
            let dense = Mesh::new(device, p, &view, &uniform, &columns, (&ledge, &mask), spec);
            let reference = Mesh::new(device, p, &view, &uniform, &columns, (&ledge, &mask), spec);
            dense.compact.force_axis(&gpu.queue, 7);
            Self {
                view,
                columns,
                ledge,
                ledge_mask,
                dense,
                reference,
                spec,
            }
        }
        fn camera(&self, queue: &wgpu::Queue, kind: &str) {
            let [nx, nz, _, _] = self.spec;
            let x = nx as f32 * 0.25;
            let z = nz as f32 * 0.25;
            let mut camera = match kind {
                "outside" => crate::CameraState::look_at(
                    Vec3::new(10000.0, 20.0, 10000.0),
                    Vec3::new(10000.0, 0.0, 9999.0),
                ),
                "under" => crate::CameraState::look_at(
                    Vec3::new(x * 0.5, 0.5, z * 0.5),
                    Vec3::new(x * 0.5, 0.5, z * 0.1),
                ),
                "partial" => {
                    crate::CameraState::look_at(Vec3::new(-3.0, 6.0, 5.0), Vec3::new(5.0, 1.0, 3.0))
                }
                _ => crate::CameraState::look_at(
                    Vec3::new(x * 0.5, z + 12.0, z * 1.8 + 12.0),
                    Vec3::new(x * 0.5, 1.0, z * 0.5),
                ),
            };
            if kind == "ortho" {
                camera.ortho_height = Some(z * 2.0 + 16.0);
            }
            let projection = camera.proj(16.0 / 9.0);
            let view = camera.view();
            let vp = projection * view;
            // Exact common.wgsl View layout; unused lighting/cascade data stays zero.
            let mut words = [0f32; 188];
            words[..16].copy_from_slice(&vp.to_cols_array());
            words[16..32].copy_from_slice(&view.to_cols_array());
            words[32..48].copy_from_slice(&projection.to_cols_array());
            words[48..64].copy_from_slice(&vp.inverse().to_cols_array());
            words[64..68].copy_from_slice(&[
                camera.position.x,
                camera.position.y,
                camera.position.z,
                0.0,
            ]);
            words[68..72].copy_from_slice(&[640.0, 360.0, 1.0 / 640.0, 1.0 / 360.0]);
            words[184..188].copy_from_slice(&[1.0, 1.0, 0.0, 0.0]);
            queue.write_buffer(&self.view, 0, bytemuck::cast_slice(&words));
        }
        fn field(&self, queue: &wgpu::Queue, kind: &str, height: f32) {
            let [nx, nz, _, _] = self.spec;
            let values = (0..nx * nz)
                .map(|n| {
                    let (x, z) = (n % nx, n / nx);
                    let wet = match kind {
                        "dry" => false,
                        "sparse" => (x / 16 + z / 16).is_multiple_of(3),
                        _ => true,
                    };
                    [
                        height + x as f32 * 0.0001 + z as f32 * 0.0002,
                        if wet { 1.0 } else { 0.0 },
                        0.0,
                        0.0,
                    ]
                })
                .collect::<Vec<_>>();
            queue.write_buffer(&self.columns, 0, bytemuck::cast_slice(&values));
        }
        fn ledges(&self, queue: &wgpu::Queue, enabled: bool) {
            let width = self.spec[0] - 1;
            let height = self.spec[1] - 1;
            let cuts = vec![u32::from(enabled); (width * height) as usize];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.ledge_mask,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&cuts),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            queue.write_buffer(
                &self.ledge,
                0,
                bytemuck::cast_slice(&[0u32, u32::from(enabled), 0, 0]),
            );
        }
        fn state(&self, queue: &wgpu::Queue, p: &Pipelines, a: u32, b: u32, t: f32) {
            let mut states =
                vec![[0u32; 4]; (self.dense.tiles[0] * self.dense.tiles[1] * 2) as usize];
            for rows in states.as_chunks_mut::<2>().0 {
                rows[0] = [a, b, t.to_bits(), if a == b { 1 } else { 2 }];
                rows[1] = [p.generation, 0, 0, 0];
            }
            if states.is_empty() {
                return;
            }
            let offset = crate::water_topology::shared().vertices.len() as u64 * 16;
            for m in [&self.dense, &self.reference] {
                queue.write_buffer(&m.geometry, offset, bytemuck::cast_slice(&states));
            }
        }
        fn compare(
            &self,
            gpu: &crate::Gpu,
            p: &Pipelines,
            profiler: &mut crate::profiler::GpuProfiler,
            label: &str,
        ) -> MeshStats {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            p.encode(&mut encoder, &self.dense, profiler);
            p.encode_reference(&mut encoder, &self.reference);
            self.compare_outputs(gpu, encoder, label)
        }
        // Water::encode leaves both mesh passes idle while culling is disabled. Flush field/view
        // writes and compare both retained lists, then compare a fresh classification on re-enable.
        fn compare_while_culling_disabled(&self, gpu: &crate::Gpu, label: &str) -> MeshStats {
            self.compare_outputs(
                gpu,
                gpu.device.create_command_encoder(&Default::default()),
                label,
            )
        }
        fn compare_outputs(
            &self,
            gpu: &crate::Gpu,
            mut encoder: wgpu::CommandEncoder,
            label: &str,
        ) -> MeshStats {
            let stage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh oracle exact counters"),
                size: 192,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(&self.dense.args, 0, &stage, 0, 96);
            encoder.copy_buffer_to_buffer(&self.reference.args, 0, &stage, 96, 96);
            gpu.queue.submit([encoder.finish()]);
            let mut stats = [0u32; 48];
            crate::readback::read_blocking(&gpu.device, &stage, &mut stats).unwrap();
            assert_eq!(
                &stats[1..18],
                &stats[25..42],
                "classification changed: {label}, {:?}",
                self.spec
            );
            let dense = self.dense.read_indices(&gpu.device, &gpu.queue).unwrap();
            let reference = self
                .reference
                .read_indices(&gpu.device, &gpu.queue)
                .unwrap();
            let [nx, nz, blocky, per] = self.spec;
            let units = if blocky != 0 {
                [nx / per, nz / per]
            } else {
                [nx - 1, nz - 1]
            };
            let vertices = if blocky != 0 { 18u64 } else { 6 };
            let full = u64::from(units[0]) * u64::from(units[1]) * vertices;
            let extra = u64::from(reference.geometry[0][2]);
            let tiles = units.map(|n| n.div_ceil(16));
            let mut by_patch = vec![Vec::<[u32; 3]>::new(); (tiles[0] * tiles[1]) as usize];
            let mut old_directed = Vec::new();
            for (n, &tri) in reference.indices.as_chunks::<3>().0.iter().enumerate() {
                if tri == [0, 0, 0] {
                    continue;
                }
                let slot = n as u64 * 3;
                // Independent provenance: assign the actual old GPU triple by its stable slot,
                // never by the new template metadata, count formula or reconstructed vertices.
                let patch_id = if slot < full {
                    let cell = slot / vertices;
                    (cell % u64::from(units[0])) / 16
                        + u64::from(tiles[0]) * (cell / u64::from(units[0]) / 16)
                } else {
                    assert!(extra > 0);
                    (slot - full) / extra
                };
                by_patch[patch_id as usize].push(tri);
                old_directed.push(tri);
            }
            let expected = by_patch.into_iter().flatten().flatten().collect::<Vec<_>>();
            assert_eq!(dense.indices.len() % 3, 0);
            assert_eq!(
                dense.indices.len(),
                expected.len(),
                "triangle count: {label}"
            );
            let mismatch = dense
                .indices
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b);
            assert!(
                mismatch.is_none(),
                "patch-major order: {label}, {:?}; first {:?}",
                self.spec,
                mismatch.map(|n| (n, dense.indices[n], expected[n]))
            );
            let mut actual_directed = dense.indices.as_chunks::<3>().0.to_vec();
            assert!(actual_directed.iter().all(|&t| t != [0, 0, 0]));
            old_directed.sort_unstable();
            actual_directed.sort_unstable();
            assert_eq!(
                old_directed, actual_directed,
                "directed triangle multiplicities: {label}"
            );
            assert_eq!(
                dense.indices.len(),
                stats[9] as usize,
                "classification active count: {label}"
            );
            assert_eq!(
                dense.geometry, reference.geometry,
                "morph state changed: {label}"
            );
            self.dense.stats(&gpu.device, &gpu.queue).unwrap()
        }
    }
    fn settings(
        p: &mut Pipelines,
        queue: &wgpu::Queue,
        mode: crate::LodMode,
        pixels: f32,
        dt: f32,
    ) {
        p.configure(
            queue,
            crate::LodSettings {
                mode,
                pixels,
                ..Default::default()
            },
            dt,
        );
    }
    fn assert_reuse(stats: MeshStats) {
        assert!(!stats.repacked_indices, "unexpected repack: {stats:?}");
        assert_eq!(stats.rewritten_patches, 0);
    }
    fn assert_repack(stats: MeshStats) {
        assert!(
            stats.repacked_indices && stats.rewritten_patches > 0,
            "insensitive repack: {stats:?}"
        );
    }
    #[test]
    fn gpu_patch_major_compaction_matches_directed_reference_and_deterministic_patch_order() {
        let gpu = crate::Gpu::headless(crate::BackendChoice::from_env())
            .expect("water metadata compaction requires GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("water metadata compaction validation: {e}")
        }));
        gpu.device.set_device_lost_callback(|r, m| {
            if r != wgpu::DeviceLostReason::Destroyed {
                panic!("water metadata compaction lost {r:?}: {m}")
            }
        });
        let mut p = Pipelines::new(&gpu.device);
        p.configure(
            &gpu.queue,
            crate::LodSettings {
                mode: crate::LodMode::On,
                pixels: 1000000.0,
                ..Default::default()
            },
            0.0,
        );
        let mut profiler =
            crate::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, false, &gpu.info);
        for spec in [
            [129, 129, 0, 1],
            [37, 19, 0, 1],
            [128, 128, 1, 4],
            [8, 8, 1, 16],
        ] {
            settings(&mut p, &gpu.queue, crate::LodMode::On, 1000000.0, 0.0);
            let pair = Pair::new(&gpu, &p, spec);
            pair.camera(&gpu.queue, "full");
            pair.field(&gpu.queue, "full", 1.0);
            pair.state(&gpu.queue, &p, 0, 0, 0.0);
            assert!(
                pair.compare(&gpu, &p, &mut profiler, "initial")
                    .repacked_indices
            );
            assert!(
                !pair
                    .compare(&gpu, &p, &mut profiler, "unchanged")
                    .repacked_indices
            );
            let first = pair
                .dense
                .read_indices(&gpu.device, &gpu.queue)
                .unwrap()
                .indices;
            for axis in [gpu.device.limits().max_compute_workgroups_per_dimension, 7] {
                pair.dense.compact.force_axis(&gpu.queue, axis);
                pair.dense.compact.invalidate_cache(&gpu.queue);
                let stats = pair.compare(&gpu, &p, &mut profiler, "forced identical-key rebuild");
                assert!(stats.repacked_indices);
                assert_eq!(stats.rewritten_patches, 0);
                let rebuilt = pair
                    .dense
                    .read_indices(&gpu.device, &gpu.queue)
                    .unwrap()
                    .indices;
                assert_eq!(first, rebuilt, "rebuild depends on dispatch axis {axis}");
                assert_reuse(pair.compare(
                    &gpu,
                    &p,
                    &mut profiler,
                    "forced rebuild followed by reuse",
                ));
            }
            if spec[2] == 0 {
                for level in 0..4 {
                    pair.state(&gpu.queue, &p, level, level, 0.0);
                    let stats = pair.compare(&gpu, &p, &mut profiler, "static LOD");
                    if spec[0] == 129 {
                        assert!(stats.levels[level as usize] > 0, "missing level {level}");
                    }
                }
                for (a, b) in crate::water_topology::pairs() {
                    pair.state(&gpu.queue, &p, a as u32, b as u32, 0.25);
                    let stats = pair.compare(&gpu, &p, &mut profiler, "bridge tail");
                    assert!(
                        stats.transition_patches > 0,
                        "bridge test insensitive: {spec:?}"
                    );
                    pair.state(&gpu.queue, &p, a as u32, b as u32, 0.75);
                    assert!(
                        !pair
                            .compare(&gpu, &p, &mut profiler, "new morph weight same key")
                            .repacked_indices
                    );
                }
            }
            pair.field(&gpu.queue, "full", 1.1);
            assert!(
                !pair
                    .compare(&gpu, &p, &mut profiler, "current height same key")
                    .repacked_indices
            );
            pair.camera(&gpu.queue, "partial");
            pair.compare(&gpu, &p, &mut profiler, "partial camera");
            pair.field(&gpu.queue, "sparse", 1.1);
            pair.compare(&gpu, &p, &mut profiler, "sparse field");
            pair.field(&gpu.queue, "dry", 1.1);
            assert_eq!(
                pair.compare(&gpu, &p, &mut profiler, "dry")
                    .submitted_triangles,
                0
            );
            assert!(
                !pair
                    .compare(&gpu, &p, &mut profiler, "dry reuse")
                    .repacked_indices
            );
            pair.camera(&gpu.queue, "full");
            pair.field(&gpu.queue, "full", 1.0);
            pair.compare(&gpu, &p, &mut profiler, "full restore");
            pair.camera(&gpu.queue, "outside");
            assert_eq!(
                pair.compare(&gpu, &p, &mut profiler, "outside")
                    .submitted_triangles,
                0
            );
            assert!(
                !pair
                    .compare(&gpu, &p, &mut profiler, "outside reuse")
                    .repacked_indices
            );
            if spec == [129, 129, 0, 1] {
                pair.camera(&gpu.queue, "full");
                let stats = pair.compare(&gpu, &p, &mut profiler, "configuration baseline LOD");
                assert!(stats.levels[3] > 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "configuration baseline reuse"));

                settings(&mut p, &gpu.queue, crate::LodMode::Off, 1000000.0, 0.0);
                let stats = pair.compare(&gpu, &p, &mut profiler, "LOD disabled");
                assert_eq!(stats.levels[1..].iter().sum::<u32>(), 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "LOD disabled reuse"));
                settings(&mut p, &gpu.queue, crate::LodMode::On, 1000000.0, 0.0);
                assert_repack(pair.compare(&gpu, &p, &mut profiler, "LOD re-enabled"));
                let generation = p.generation;
                settings(&mut p, &gpu.queue, crate::LodMode::On, 500000.0, 0.0);
                assert_ne!(p.generation, generation);
                assert_reuse(pair.compare(
                    &gpu,
                    &p,
                    &mut profiler,
                    "new generation identical selections",
                ));
                settings(&mut p, &gpu.queue, crate::LodMode::On, 0.000000001, 0.0);
                let stats = pair.compare(&gpu, &p, &mut profiler, "new pixel bound forces detail");
                assert_eq!(stats.levels[1..].iter().sum::<u32>(), 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "new pixel bound reuse"));
                settings(&mut p, &gpu.queue, crate::LodMode::On, 1000000.0, 0.0);
                assert_repack(pair.compare(&gpu, &p, &mut profiler, "pixel bound restores LOD"));

                pair.ledges(&gpu.queue, true);
                let stats = pair.compare(&gpu, &p, &mut profiler, "ledge mask forces full detail");
                assert_eq!(stats.levels[1..].iter().sum::<u32>(), 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "ledge full-detail reuse"));
                pair.ledges(&gpu.queue, false);
                settings(&mut p, &gpu.queue, crate::LodMode::On, 1000000.0, 0.1);
                let stats = pair.compare(&gpu, &p, &mut profiler, "ledge removed LOD transition");
                assert!(stats.transition_patches > 0);
                assert_repack(stats);
                let stats = pair.compare(&gpu, &p, &mut profiler, "ledge removed LOD endpoint");
                assert!(stats.levels[3] > 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "ledge removed LOD reuse"));

                // These two fields/views reach the GPU while both classification passes are idle.
                pair.field(&gpu.queue, "dry", 0.5);
                pair.camera(&gpu.queue, "outside");
                assert_reuse(pair.compare_while_culling_disabled(&gpu, "culling off dry field"));
                pair.field(&gpu.queue, "full", 1.0);
                pair.camera(&gpu.queue, "partial");
                assert_reuse(
                    pair.compare_while_culling_disabled(&gpu, "culling off changed view and field"),
                );
                assert_repack(pair.compare(
                    &gpu,
                    &p,
                    &mut profiler,
                    "culling on reclassifies current inputs",
                ));
                pair.field(&gpu.queue, "dry", 1.0);
                pair.compare_while_culling_disabled(&gpu, "culling off dry after partial");
                assert_repack(pair.compare(
                    &gpu,
                    &p,
                    &mut profiler,
                    "culling on consumes current dry field",
                ));
                pair.camera(&gpu.queue, "full");
                pair.field(&gpu.queue, "full", 1.0);
                pair.compare_while_culling_disabled(&gpu, "culling off wet restore");
                assert_repack(pair.compare(&gpu, &p, &mut profiler, "culling on wet restore"));
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "culling on unchanged reuse"));

                pair.camera(&gpu.queue, "under");
                let stats = pair.compare(&gpu, &p, &mut profiler, "camera enters medium");
                assert!(stats.visible > 0);
                assert_eq!(stats.levels[1..].iter().sum::<u32>(), 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "underwater same-view reuse"));
                pair.camera(&gpu.queue, "ortho");
                let stats = pair.compare(&gpu, &p, &mut profiler, "orthographic full-detail view");
                assert!(stats.visible > 0);
                assert_eq!(stats.levels[1..].iter().sum::<u32>(), 0);
                assert_repack(stats);
                assert_reuse(pair.compare(&gpu, &p, &mut profiler, "orthographic same-view reuse"));
            }
        }
    }
}
