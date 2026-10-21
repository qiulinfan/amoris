//! Water regions (docs/spec/water.md 9): the columns of each `WaterRegion` the feed carries, drawn
//! in a dedicated optical stage by forward.wgsl's `vs_shallow` (Smooth: a vertex at each column's centre,
//! two triangles between four centres from an index buffer) or `vs_shallow_blocky` (Blocky: flat
//! over each block, with vertical sides where neighbouring blocks differ), both shaded by
//! `fs_shallow`. GPU patches cull dry/out-of-view geometry using current columns; the retained
//! triangles use bounded LOD away from inferred bed/surface jumps. At those jumps GPU-built
//! quarter caps and bed-limited sidewalls replace the crossing triangles; directed outflow
//! adds a thin visual film below the upstream bed, without changing authoritative volume. The renderer keeps a
//! copy of nonresident columns, tile by tile, and uploads only the tiles a frame changed.
//! GPU visual replicas supply resident fields; camera-side medium transport reads GPU columns.
//! Nothing here simulates authoritative water. Surface foam is a separate GPU history transported by the feed's
//! velocity, advanced only when simulated time advances and cleared on reset or rewind. Dilute
//! ballistic spray reads the same current GPU columns and contributes local foam impulses; it is
//! also disposable presentation history and never changes authoritative water.
//!
//! The shallow-water spike's host-made field (`ShallowWaterView`, its examples) is drawn the same
//! way as one more region, its columns in rows instead of tiles.

use std::collections::BTreeMap;

use amoris_assets::frame::{ResidentWaterUpdate, WaterLookView, WaterUpdate};
use amoris_assets::water_field::{CheckedCheckpoint, FieldRecipe};
use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};

/// Columns along a tile's edge.
const TILE: u32 = 16;
/// A column wetter than this is wet (the shader's `SHALLOW_WET`).
const WET: f32 = 0.01;
/// The width of the strip that joins a region's edge to the water around it, metres: its inner rim
/// follows each edge column's surface, its outer rim the surrounding water's level (water.md 9).
const STITCH_M: f32 = 4.0;

fn culling_from_env() -> bool {
    match std::env::var("AMORIS_WATER_CULL") {
        Ok(s) => match s.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            _ => {
                log::warn!("AMORIS_WATER_CULL: {s:?} is not on or off; using on");
                true
            }
        },
        Err(_) => true,
    }
}

/// One frame's water from the host (spike, docs/spikes/shallow-water.md): the grid's corner (world
/// x, z), its column size, its column counts, and per column (x fastest) the surface (world y), the
/// depth and the velocity (m/s, x and z).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShallowWaterView {
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub nx: u32,
    pub nz: u32,
    pub columns: Vec<[f32; 4]>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    origin: [f32; 2],
    cell: f32,
    time: f32,
    /// Columns along x and z, 1 when the columns are in tiles (else rows), columns to a block.
    count: [u32; 4],
    /// Absorption and scattering per metre (rgb); w unused.
    absorb: [f32; 4],
    scatter: [f32; 4],
    /// The surrounding water: its level (world y), how far it reaches and the width of the
    /// stitch strip joining it to the region's edge, metres, and 1 for a Blocky region.
    surround: [f32; 4],
    /// Extra isotropic scattering color and density per metre.
    fog: [f32; 4],
    /// Refraction, screen-space reflection, subgrid detail, unused.
    optics: [f32; 4],
}

/// A region as the renderer holds it.
struct Region {
    origin: [f32; 2],
    cell_m: f32,
    size: [u32; 2],
    look: WaterLookView,
    /// The columns, tile by tile (or in rows for the spike's field).
    columns: Vec<[f32; 4]>,
    tiled: bool,
    /// Tiles to upload (by index), or all of them.
    dirty: Vec<u32>,
    all_dirty: bool,
    gpu: Option<RegionGpu>,
    /// Whether the water around it is drawn this frame.
    surround_on: bool,
    resident: Option<ResidentRegion>,
}

struct ResidentRegion {
    checkpoint: std::sync::Arc<CheckedCheckpoint>,
    checkpoint_digest: [u8; 32],
    recipe: FieldRecipe,
    edge_level: Option<f32>,
    cache: Option<crate::water_replica::WaterReplica>,
}

struct RegionGpu {
    uniform: wgpu::Buffer,
    columns: wgpu::Buffer,
    group: wgpu::BindGroup,
    depth_group: wgpu::BindGroup,
    foam: crate::water_foam::WaterFoam,
    surface: crate::water_surface::Surface,
    columns_changed: bool,
    last_time: Option<f64>,
    foam_dt: f32,
    clear_foam: bool,
    spray: Option<crate::water_spray::Pool>,
    spray_dt: f32,
    clear_spray: bool,
    /// The column counts the buffers were made for.
    size: [u32; 2],
    mesh: Option<crate::water_mesh::Mesh>,
    ledges: crate::water_ledge::Ledges,
}

impl Region {
    fn column(&self, i: u32, k: u32) -> [f32; 4] {
        let at = if self.tiled {
            let tx = self.size[0] / TILE;
            ((i / TILE) + tx * (k / TILE)) * TILE * TILE + (i % TILE) + TILE * (k % TILE)
        } else {
            i + self.size[0] * k
        };
        self.columns.get(at as usize).copied().unwrap_or([0.0; 4])
    }

    /// The level of the water drawn around the region: the mean surface of its wet edge columns.
    fn edge_level(&self) -> Option<f32> {
        if let Some(resident) = &self.resident {
            return resident.edge_level;
        }
        let [nx, nz] = self.size;
        let (mut sum, mut n) = (0.0f64, 0u32);
        let mut take = |c: [f32; 4]| {
            if c[1] > WET {
                sum += f64::from(c[0]);
                n += 1;
            }
        };
        for i in 0..nx {
            take(self.column(i, 0));
            take(self.column(i, nz - 1));
        }
        for k in 1..nz.saturating_sub(1) {
            take(self.column(0, k));
            take(self.column(nx - 1, k));
        }
        (n > 0).then(|| (sum / f64::from(n)) as f32)
    }

    /// The column under world `(x, z)`.
    fn column_at(&self, x: f32, z: f32) -> Option<[f32; 4]> {
        if self.resident.is_some() {
            return None;
        }
        let (fx, fz) = (
            (x - self.origin[0]) / self.cell_m,
            (z - self.origin[1]) / self.cell_m,
        );
        if !(fx >= 0.0 && fz >= 0.0) {
            return None;
        }
        let (i, k) = (fx as u32, fz as u32);
        (i < self.size[0] && k < self.size[1]).then(|| self.column(i, k))
    }
}

fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
    vertex: &str,
) -> wgpu::RenderPipeline {
    let premultiplied = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(vertex),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vertex),
            compilation_options: Default::default(),
            buffers: &if vertex == "vs_shallow_ledge" {
                vec![Some(crate::water_ledge::vertices())]
            } else {
                vec![]
            },
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_shallow"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &format.constants(),
                ..Default::default()
            },
            targets: &format.targets(Some(premultiplied), false),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: format.multisample(),
        multiview_mask: None,
        cache: None,
    })
}

/// The water regions and their drawing.
pub struct Water {
    cull_on: bool,
    dummy_geometry: wgpu::Buffer,
    lod: crate::LodSettings,
    surface: Option<crate::water_surface::Pipelines>,
    mesh: Option<crate::water_mesh::Pipelines>,
    optics: Option<crate::water_optics::Optics>,
    medium: Option<crate::water_medium::WaterMedium>,
    frame_layouts: [wgpu::BindGroupLayout; 3],
    foam_on: bool,
    spray_on: bool,
    spray: Option<crate::water_spray::Pipelines>,
    format: SceneFormat,
    effects: [f32; 4],
    smooth: wgpu::RenderPipeline,
    ledge_surface: wgpu::RenderPipeline,
    ledge_pipelines: Option<crate::water_ledge::Pipelines>,
    blocky: wgpu::RenderPipeline,
    surround: wgpu::RenderPipeline,
    stitch: wgpu::RenderPipeline,
    layout: wgpu::PipelineLayout,
    group_layout: wgpu::BindGroupLayout,
    regions: BTreeMap<u64, Region>,
    /// Two triangles between each four neighbouring columns, per column counts.
    indices: BTreeMap<[u32; 2], wgpu::Buffer>,
}

/// The spike's host-made field's key among the regions.
const SPIKE: u64 = u64::MAX;

impl Water {
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
    ) -> Water {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: if binding == 2 {
                wgpu::ShaderStages::VERTEX
            } else {
                wgpu::ShaderStages::VERTEX_FRAGMENT
            },
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                crate::water_ledge::header_entry(wgpu::ShaderStages::VERTEX_FRAGMENT),
                crate::water_ledge::mask_entry(wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                crate::water_surface::texture_entry(7),
                crate::water_surface::texture_entry(8),
                crate::water_mesh::geometry_entry(),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(&group_layout),
            ],
            immediate_size: 0,
        });
        Water {
            cull_on: culling_from_env(),
            dummy_geometry: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("unused water geometry"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            lod: crate::LodSettings::default(),
            surface: None,
            mesh: None,
            optics: None,
            medium: None,
            frame_layouts: groups.map(Clone::clone),
            foam_on: true,
            spray_on: true,
            spray: None,
            format,
            effects: [1.0, 1.0, 1.0, 0.0],
            smooth: pipeline(device, module, &layout, format, "vs_shallow"),
            ledge_surface: pipeline(device, module, &layout, format, "vs_shallow_ledge"),
            ledge_pipelines: None,
            blocky: pipeline(device, module, &layout, format, "vs_shallow_blocky"),
            surround: pipeline(device, module, &layout, format, "vs_shallow_surround"),
            stitch: pipeline(device, module, &layout, format, "vs_shallow_stitch"),
            layout,
            group_layout,
            regions: BTreeMap::new(),
            indices: BTreeMap::new(),
        }
    }

    pub fn set_format(
        &mut self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        format: SceneFormat,
    ) {
        if let Some(optics) = &mut self.optics {
            optics.reformat(
                device,
                module,
                [
                    &self.frame_layouts[0],
                    &self.frame_layouts[1],
                    &self.frame_layouts[2],
                ],
                format,
            );
        }
        self.format = format;
        if let Some(spray) = &mut self.spray {
            spray.reformat(device, format);
        }
        self.smooth = pipeline(device, module, &self.layout, format, "vs_shallow");
        self.ledge_surface = pipeline(device, module, &self.layout, format, "vs_shallow_ledge");
        self.blocky = pipeline(device, module, &self.layout, format, "vs_shallow_blocky");
        self.surround = pipeline(device, module, &self.layout, format, "vs_shallow_surround");
        self.stitch = pipeline(device, module, &self.layout, format, "vs_shallow_stitch");
    }

    pub(crate) fn owned_rectangles(&self) -> Vec<[f32; 4]> {
        self.regions
            .values()
            .map(|r| {
                [
                    r.origin[0],
                    r.origin[1],
                    r.origin[0] + r.size[0] as f32 * r.cell_m,
                    r.origin[1] + r.size[1] as f32 * r.cell_m,
                ]
            })
            .collect()
    }

    /// Forgets every region the feed sent (a reset frame); the spike's field stays.
    pub fn clear(&mut self) {
        if let Some(medium) = &mut self.medium {
            medium.clear();
        }
        self.regions.retain(|&id, _| id == SPIKE);
        for r in self.regions.values_mut() {
            if let Some(g) = &mut r.gpu {
                g.clear_foam = true;
                g.clear_spray = true;
                g.spray = None;
                g.last_time = None;
            }
        }
    }

    pub fn set_spray(&mut self, on: bool) {
        if self.spray_on == on {
            return;
        }
        self.spray_on = on;
        for r in self.regions.values_mut() {
            if let Some(g) = &mut r.gpu {
                g.spray = None;
                g.clear_spray = true;
            }
        }
    }
    pub fn set_foam(&mut self, on: bool) {
        if self.foam_on == on {
            return;
        }
        self.foam_on = on;
        for r in self.regions.values_mut() {
            if let Some(g) = &mut r.gpu {
                g.clear_foam = true;
            }
        }
    }

    pub fn set_optics(&mut self, refraction: bool, reflection: bool, detail: bool) {
        self.effects = [refraction, reflection, detail, false].map(|on| f32::from(u8::from(on)));
    }

    pub fn set_culling(&mut self, on: bool) {
        self.cull_on = on;
    }
    pub fn set_lod(&mut self, lod: crate::LodSettings) {
        self.lod = lod;
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn probe_vertices(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        module: &wgpu::ShaderModule,
        id: u64,
        (view, indices): (&wgpu::Buffer, &[u32]),
    ) -> Result<Vec<[f32; 4]>, String> {
        let gpu = self
            .regions
            .get(&id)
            .and_then(|r| r.gpu.as_ref())
            .ok_or("missing probe region")?;
        let mesh = gpu.mesh.as_ref().ok_or("missing probe geometry")?;
        if indices.is_empty() || indices.iter().any(|v| v & 0x80000000 == 0) {
            return Err("probe needs nonempty morph vertex indices".into());
        }
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water vertex accuracy probe"),
            layout: None,
            module,
            entry_point: Some("probe_water_vertices"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        let input = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water probe indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let bytes = indices.len() as u64 * 32;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water probe positions"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water probe readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water vertex probe"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let group3 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water vertex field"),
            layout: &pipeline.get_bind_group_layout(3),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: gpu.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: gpu.columns.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: mesh.geometry.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group0, &[]);
            pass.set_bind_group(3, &group3, &[]);
            pass.dispatch_workgroups((indices.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &stage, 0, bytes);
        queue.submit([encoder.finish()]);
        let mut data = vec![[0.0; 4]; indices.len() * 2];
        crate::readback::read_blocking(device, &stage, &mut data)?;
        Ok(data)
    }
    pub fn mesh_readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> impl std::future::Future<Output = Result<Vec<(u64, crate::water_mesh::MeshStats)>, String>>
    + use<> {
        let staged = if self.cull_on {
            self.regions
                .iter()
                .filter_map(|(&id, r)| {
                    r.gpu
                        .as_ref()?
                        .mesh
                        .as_ref()
                        .map(|m| (id, m.copy_stats(device, queue)))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        async move {
            let mut out = Vec::new();
            for (id, r) in staged {
                out.push((id, r.read().await?));
            }
            Ok(out)
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn mesh_indices(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<(u64, crate::water_mesh::MeshIndices)>, String> {
        if !self.cull_on {
            return Ok(Vec::new());
        }
        self.regions
            .iter()
            .filter_map(|(&id, r)| r.gpu.as_ref()?.mesh.as_ref().map(|m| (id, m)))
            .map(|(id, m)| m.read_indices(device, queue).map(|data| (id, data)))
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn mesh_stats(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<(u64, crate::water_mesh::MeshStats)>, String> {
        if !self.cull_on {
            return Ok(Vec::new());
        }
        self.regions
            .iter()
            .filter_map(|(&id, r)| r.gpu.as_ref()?.mesh.as_ref().map(|m| (id, m)))
            .map(|(id, m)| m.stats(device, queue).map(|s| (id, s)))
            .collect()
    }

    /// Applies the feed's water: regions changed, then regions gone.
    pub fn apply(&mut self, updates: &[WaterUpdate], removed: &[u64]) {
        for u in updates {
            let tiles = (u.size[0] / TILE) * (u.size[1] / TILE);
            let r = self.regions.entry(u.id).or_insert_with(|| Region {
                origin: u.origin,
                cell_m: u.cell_m,
                size: u.size,
                look: u.look.clone().unwrap_or_default(),
                columns: Vec::new(),
                tiled: true,
                dirty: Vec::new(),
                all_dirty: true,
                gpu: None,
                surround_on: false,
                resident: None,
            });
            if r.resident.take().is_some() {
                r.gpu = None;
                r.tiled = true;
            }
            if r.size != u.size || r.columns.len() != (tiles * TILE * TILE) as usize {
                r.size = u.size;
                // Absent tiles are dry finite columns. A collapsed masked primitive may share
                // column zero; an f32::MIN placeholder can overflow its clip-space transform.
                r.columns = vec![[0.0, -1.0, 0.0, 0.0]; (tiles * TILE * TILE) as usize];
                r.all_dirty = true;
            }
            if (r.origin, r.cell_m) != (u.origin, u.cell_m) {
                r.gpu = None;
            }
            (r.origin, r.cell_m) = (u.origin, u.cell_m);
            if let Some(l) = &u.look {
                r.look = l.clone();
            }
            let tx = u.size[0] / TILE;
            for t in &u.tiles {
                let n = t.key[0] + tx * t.key[1];
                if n >= tiles || t.columns.len() != (TILE * TILE) as usize {
                    continue;
                }
                let at = (n * TILE * TILE) as usize;
                r.columns[at..at + t.columns.len()].copy_from_slice(&t.columns);
                if !r.all_dirty {
                    r.dirty.push(n);
                }
            }
        }
        for id in removed {
            self.regions.remove(id);
        }
    }

    /// Apply a published bounded program. Only checkpoint transitions transfer a full field.
    pub fn apply_resident(&mut self, updates: &[ResidentWaterUpdate]) -> Result<(), String> {
        let mut prepared = Vec::with_capacity(updates.len());
        let mut ids = std::collections::BTreeSet::new();
        for u in updates {
            if !ids.insert(u.id) {
                return Err("resident update repeats a region".into());
            }
            let previous = self.regions.get(&u.id).and_then(|r| r.resident.as_ref());
            let checkpoint = u
                .checkpoint
                .as_ref()
                .or_else(|| previous.map(|r| &r.checkpoint))
                .ok_or("resident visual update has no seed")?;
            let recipe = u.recipe.apply(previous.map(|r| &r.recipe))?;
            crate::water_resident::validate_recipe_kernel(&recipe)?;
            let seed_digest = if u.checkpoint.is_some() {
                recipe.validate_checked(checkpoint)?;
                checkpoint.digest()
            } else {
                let old = previous.ok_or("resident visual patch has no cached region")?;
                if recipe.checkpoint() != old.recipe.checkpoint() {
                    return Err("resident visual checkpoint transition is missing its seed".into());
                }
                for step in &u.recipe.steps {
                    step.validate(checkpoint.columns.len())?;
                }
                if recipe.steps().is_empty() && recipe.digest != old.checkpoint_digest {
                    return Err("resident replacement checkpoint digest differs".into());
                }
                old.checkpoint_digest
            };
            prepared.push((u, recipe, checkpoint.size, seed_digest));
        }
        // Commit only after every patch has a matching base and validates. Failed batches keep
        // all CPU recipes and GPU caches at their prior boundary.
        for (u, recipe, size, seed_digest) in prepared {
            if !self
                .regions
                .get(&u.id)
                .is_some_and(|r| r.resident.is_some())
            {
                self.regions.insert(
                    u.id,
                    Region {
                        origin: u.origin,
                        cell_m: u.cell_m,
                        size,
                        look: u.look.clone(),
                        columns: vec![],
                        tiled: false,
                        dirty: vec![],
                        all_dirty: false,
                        gpu: None,
                        surround_on: false,
                        resident: Some(ResidentRegion {
                            checkpoint: u.checkpoint.as_ref().expect("prepared seed").clone(),
                            checkpoint_digest: seed_digest,
                            recipe,
                            edge_level: u.edge_level,
                            cache: None,
                        }),
                    },
                );
                continue;
            }
            let r = self
                .regions
                .get_mut(&u.id)
                .expect("prepared resident region");
            let resident = r.resident.as_mut().expect("resident region selected");
            if let Some(checkpoint) = &u.checkpoint {
                if r.size != size {
                    resident.cache = None;
                    r.gpu = None;
                }
                resident.checkpoint.clone_from(checkpoint);
                resident.checkpoint_digest = seed_digest;
            }
            if r.cell_m != u.cell_m {
                r.gpu = None;
            }
            r.size = size;
            r.origin = u.origin;
            r.cell_m = u.cell_m;
            r.look = u.look.clone();
            resident.recipe = recipe;
            resident.edge_level = u.edge_level;
        }
        Ok(())
    }

    /// The spike's host-made field from the next frame on; `None` draws none.
    pub fn set_spike(&mut self, water: Option<&ShallowWaterView>) {
        let Some(w) = water.filter(|w| w.nx > 1 && w.nz > 1) else {
            self.regions.remove(&SPIKE);
            return;
        };
        let r = self.regions.entry(SPIKE).or_insert_with(|| Region {
            origin: w.origin,
            cell_m: w.cell_m,
            size: [w.nx, w.nz],
            look: WaterLookView {
                fog: [0.05, 0.16, 0.45, 0.12],
                absorb: [1.98, 0.38, 0.27],
                scatter: [0.02; 3],
                blocky: false,
                per: 1,
                surround_m: 0.0,
            },
            columns: Vec::new(),
            tiled: false,
            dirty: Vec::new(),
            all_dirty: true,
            gpu: None,
            surround_on: false,
            resident: None,
        });
        if (r.origin, r.cell_m) != (w.origin, w.cell_m) {
            r.gpu = None;
        }
        (r.origin, r.cell_m, r.size) = (w.origin, w.cell_m, [w.nx, w.nz]);
        r.columns.clone_from(&w.columns);
        r.all_dirty = true;
    }

    /// The fog of the region whose surface is over `eye`, if any (water.md 9).
    pub fn fog_at(&self, eye: [f32; 3]) -> Option<[f32; 4]> {
        self.regions.values().find_map(|r| {
            let c = r.column_at(eye[0], eye[2])?;
            (c[1] > WET && eye[1] < c[0]).then_some(r.look.fog)
        })
    }

    /// Uploads what changed and the frame's uniforms at drawn time `t` (seconds).
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        t: f64,
        targets: &crate::post::Targets,
        module: &wgpu::ShaderModule,
        sea_present: bool,
    ) {
        if self.regions.is_empty() {
            return;
        }
        let optics = self.optics.get_or_insert_with(|| {
            crate::water_optics::Optics::new(
                device,
                module,
                [
                    &self.frame_layouts[0],
                    &self.frame_layouts[1],
                    &self.frame_layouts[2],
                ],
                targets.format,
            )
        });
        let changed = optics.prepare(device, targets);
        let ledge_pipelines = self
            .ledge_pipelines
            .get_or_insert_with(|| crate::water_ledge::Pipelines::new(device));
        for r in self.regions.values_mut() {
            let [nx, nz] = r.size;
            if nx < 2 || nz < 2 || (r.resident.is_none() && r.columns.is_empty()) {
                continue;
            }
            if let Some(resident) = &mut r.resident {
                if resident.cache.is_none() {
                    resident.cache = Some(
                        crate::water_replica::WaterReplica::new(device, queue, r.size)
                            .expect("resident visual GPU creation"),
                    );
                }
                resident
                    .cache
                    .as_mut()
                    .expect("resident visual cache")
                    .synchronize(device, queue, &resident.checkpoint, &resident.recipe)
                    .expect("published resident visual program");
            }
            let bytes = u64::from(nx) * u64::from(nz) * 16;
            if r.gpu
                .as_ref()
                .is_none_or(|g| g.size != r.size || g.surface.enabled == r.look.blocky)
            {
                if let Some(resident) = &mut r.resident {
                    resident
                        .cache
                        .as_mut()
                        .expect("resident cache prepared")
                        .invalidate_presentation();
                }
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water"),
                    size: std::mem::size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let columns = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water columns"),
                    size: bytes.max(16),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let foam =
                    crate::water_foam::WaterFoam::new(device, &columns, r.size, r.cell_m, r.tiled);
                let surface = crate::water_surface::Surface::new(device, r.size, !r.look.blocky);
                let ledges = crate::water_ledge::Ledges::new(
                    device,
                    ledge_pipelines,
                    &uniform,
                    &columns,
                    r.size,
                );
                let group = optical_group(
                    device,
                    &self.group_layout,
                    (&uniform, &columns),
                    foam.view(),
                    &surface,
                    (&self.dummy_geometry, &ledges.args, &ledges.mask),
                    optics,
                );
                let depth_group = depth_group(
                    device,
                    &optics.depth_layout,
                    &uniform,
                    &columns,
                    &surface,
                    (&self.dummy_geometry, &ledges.args, &ledges.mask),
                );
                r.gpu = Some(RegionGpu {
                    mesh: None,
                    ledges,
                    surface,
                    columns_changed: true,
                    uniform,
                    columns,
                    group,
                    depth_group,
                    foam,
                    last_time: None,
                    foam_dt: 0.0,
                    clear_foam: false,
                    spray: None,
                    spray_dt: 0.0,
                    clear_spray: true,
                    size: r.size,
                });
                r.all_dirty = true;
            }
            let edge_level = r.edge_level();
            let Some(g) = &mut r.gpu else {
                continue;
            };
            if changed {
                g.group = optical_group(
                    device,
                    &self.group_layout,
                    (&g.uniform, &g.columns),
                    g.foam.view(),
                    &g.surface,
                    (
                        g.mesh
                            .as_ref()
                            .map(|m| &m.geometry)
                            .unwrap_or(&self.dummy_geometry),
                        &g.ledges.args,
                        &g.ledges.mask,
                    ),
                    optics,
                );
            }
            // Adjacent ticks can compute the same drawn boundary one f64 ulp apart (61/60 -
            // 1/60 < 1). Keep that boundary monotonic; it is not a rewind of the simulation.
            let t = g
                .last_time
                .filter(|&last| {
                    t < last && last - t <= last.abs().max(t.abs()).max(1.0) * f64::EPSILON * 4.0
                })
                .unwrap_or(t);
            g.clear_foam |= g.last_time.is_some_and(|last| t < last);
            g.clear_spray |= g.last_time.is_some_and(|last| t < last);
            g.spray_dt = g.last_time.map_or(0.0, |last| (t - last).max(0.0) as f32);
            g.foam_dt = if self.foam_on { g.spray_dt } else { 0.0 };
            g.last_time = Some(t);
            g.columns_changed |= r.resident.is_none() && (r.all_dirty || !r.dirty.is_empty());
            if r.all_dirty && r.resident.is_none() {
                queue.write_buffer(&g.columns, 0, bytemuck::cast_slice(&r.columns));
            } else {
                r.dirty.sort_unstable();
                r.dirty.dedup();
                for &n in &r.dirty {
                    let at = (n * TILE * TILE) as usize;
                    let tile = &r.columns[at..at + (TILE * TILE) as usize];
                    queue.write_buffer(&g.columns, at as u64 * 16, bytemuck::cast_slice(tile));
                }
            }
            r.dirty.clear();
            r.all_dirty = false;
            let (a, s) = (r.look.absorb, r.look.scatter);
            let surround = match edge_level {
                Some(level) if r.look.surround_m > 0.0 && !sea_present => [
                    level,
                    r.look.surround_m,
                    STITCH_M.min(r.look.surround_m),
                    if r.look.blocky { 1.0 } else { 0.0 },
                ],
                _ => [0.0, 0.0, 0.0, f32::from(u8::from(r.look.blocky))],
            };
            let u = Uniform {
                origin: r.origin,
                cell: r.cell_m,
                time: t as f32,
                count: [nx, nz, u32::from(r.tiled), r.look.per.max(1)],
                absorb: [a[0], a[1], a[2], 0.0],
                scatter: [s[0], s[1], s[2], 0.0],
                surround,
                fog: r.look.fog,
                optics: self.effects,
            };
            r.surround_on = surround[1] > 0.0;
            queue.write_buffer(&g.uniform, 0, bytemuck::bytes_of(&u));
            if !self.cull_on && !r.look.blocky && !self.indices.contains_key(&r.size) {
                let mut idx = Vec::with_capacity(((nx - 1) * (nz - 1) * 6) as usize);
                for k in 0..nz - 1 {
                    for i in 0..nx - 1 {
                        let a = i + nx * k;
                        let (b, c, d) = (a + nx, a + nx + 1, a + 1);
                        idx.extend_from_slice(&[a, b, c, a, c, d]);
                    }
                }
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water indices"),
                    size: (idx.len() * 4) as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&idx));
                self.indices.insert(r.size, buffer);
            }
        }
    }

    /// Advance presentation histories before any render pass samples them.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: &mut crate::profiler::GpuProfiler,
        view: &wgpu::Buffer,
        dt: f32,
    ) {
        if self.spray_on && self.regions.values().any(|r| r.gpu.is_some()) {
            let spray = self
                .spray
                .get_or_insert_with(|| crate::water_spray::Pipelines::new(device, self.format));
            encoder.clear_buffer(&spray.budget, 0, None);
        }
        for r in self.regions.values_mut() {
            let Some(g) = &mut r.gpu else { continue };
            if let Some(resident) = &mut r.resident {
                g.columns_changed |= resident
                    .cache
                    .as_mut()
                    .expect("resident visual cache prepared")
                    .encode(device, queue, encoder, &g.columns, profiler);
            }
            self.ledge_pipelines
                .as_ref()
                .expect("ledge pipeline prepared")
                .encode(
                    encoder,
                    &mut g.ledges,
                    g.columns_changed,
                    !r.look.blocky,
                    profiler,
                );
            if g.surface.enabled {
                self.surface
                    .get_or_insert_with(|| crate::water_surface::Pipelines::new(device))
                    .encode(
                        device,
                        encoder,
                        &mut g.surface,
                        (&g.uniform, &g.columns, &g.ledges.args, &g.ledges.mask),
                        g.columns_changed,
                        profiler,
                    );
            }
            if self.spray_on || g.clear_spray {
                encoder.clear_buffer(g.foam.impacts(), 0, None);
            }
            if self.spray_on {
                let pipes = self.spray.as_ref().expect("spray pipelines prepared");
                let pool = g.spray.get_or_insert_with(|| {
                    crate::water_spray::Pool::new(device, pipes, view, &g.columns, g.foam.impacts())
                });
                pool.encode(
                    queue,
                    encoder,
                    pipes,
                    crate::water_spray::Advance {
                        field: crate::water_spray::FieldSpec {
                            origin: r.origin,
                            size: r.size,
                            cell: r.cell_m,
                            tiled: r.tiled,
                            absorb: r.look.absorb,
                        },
                        dt: g.spray_dt,
                        changed: g.columns_changed,
                        clear: g.clear_spray,
                    },
                    profiler,
                );
            }
            g.clear_spray = false;
            g.columns_changed = false;
            if g.foam_dt > 0.0 || g.clear_foam {
                let timestamps = profiler.compute_scope("water foam");
                g.foam
                    .encode(queue, encoder, g.foam_dt, g.clear_foam, timestamps);
                g.foam_dt = 0.0;
                g.clear_foam = false;
            }
            if self.cull_on {
                let pipelines = self
                    .mesh
                    .get_or_insert_with(|| crate::water_mesh::Pipelines::new(device));
                pipelines.configure(queue, self.lod, dt);
                let spec = [
                    g.size[0],
                    g.size[1],
                    u32::from(r.look.blocky),
                    r.look.per.max(1),
                ];
                if g.mesh.as_ref().is_none_or(|m| m.spec != spec) {
                    g.mesh = Some(crate::water_mesh::Mesh::new(
                        device,
                        pipelines,
                        view,
                        &g.uniform,
                        &g.columns,
                        (&g.ledges.args, &g.ledges.mask),
                        spec,
                    ));
                    let optics = self.optics.as_ref().expect("water optics ready");
                    let geometry = &g.mesh.as_ref().unwrap().geometry;
                    g.group = optical_group(
                        device,
                        &self.group_layout,
                        (&g.uniform, &g.columns),
                        g.foam.view(),
                        &g.surface,
                        (geometry, &g.ledges.args, &g.ledges.mask),
                        optics,
                    );
                    g.depth_group = depth_group(
                        device,
                        &optics.depth_layout,
                        &g.uniform,
                        &g.columns,
                        &g.surface,
                        (geometry, &g.ledges.args, &g.ledges.mask),
                    );
                }
                pipelines.encode(
                    encoder,
                    g.mesh.as_ref().expect("water mesh prepared"),
                    profiler,
                );
            }
        }
    }

    /// Draws every region, groups 0 to 2 bound as for the lit forward pipelines.
    pub fn depth_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        groups: [&wgpu::BindGroup; 3],
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        let optics = self
            .optics
            .as_ref()
            .expect("water optics prepared before drawing");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("water surface mask"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &optics.input.mask,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: profiler.render_scope("water surface mask"),
            ..Default::default()
        });
        for (i, group) in groups.into_iter().enumerate() {
            pass.set_bind_group(i as u32, group, &[]);
        }
        for r in self.regions.values() {
            let Some(g) = &r.gpu else { continue };
            let [nx, nz] = g.size;
            pass.set_bind_group(3, &g.depth_group, &[]);
            if self.cull_on {
                let mesh = g.mesh.as_ref().expect("water mesh encoded before mask");
                pass.set_pipeline(&optics.depth_pipelines[usize::from(r.look.blocky)]);
                pass.set_index_buffer(mesh.draw_indices().slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed_indirect(mesh.draw_args(), 0);
            } else if r.look.blocky {
                pass.set_pipeline(&optics.depth_pipelines[1]);
                let per = r.look.per.max(1);
                pass.draw(0..(nx / per) * (nz / per) * 18, 0..1);
            } else if let Some(indices) = self.indices.get(&g.size) {
                pass.set_pipeline(&optics.depth_pipelines[0]);
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..(nx - 1) * (nz - 1) * 6, 0, 0..1);
            }
            if !r.look.blocky {
                pass.set_pipeline(&optics.depth_pipelines[4]);
                pass.set_vertex_buffer(0, g.ledges.indices.slice(..));
                pass.draw_indirect(&g.ledges.args, 0);
            }
            if r.surround_on {
                pass.set_pipeline(&optics.depth_pipelines[2]);
                pass.draw(0..24, 0..1);
                pass.set_pipeline(&optics.depth_pipelines[3]);
                pass.draw(0..6 * (2 * (nx - 1) + 2 * (nz - 1) + 4), 0..1);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn medium(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        profiler: &mut crate::profiler::GpuProfiler,
        targets: &crate::post::Targets,
        view: &wgpu::Buffer,
        sh: &wgpu::Buffer,
    ) {
        let fields: Vec<_> = self
            .regions
            .iter()
            .filter_map(|(id, r)| {
                r.gpu
                    .as_ref()
                    .map(|g| (*id, &g.uniform, &g.columns, &g.ledges.args, &g.ledges.mask))
            })
            .collect();
        if fields.is_empty() {
            return;
        }
        self.medium
            .get_or_insert_with(|| crate::water_medium::WaterMedium::new(device))
            .encode(device, queue, encoder, profiler, targets, view, sh, &fields);
    }

    pub fn layer(&self, front: bool) -> (&wgpu::RenderPipeline, &wgpu::BindGroup) {
        let optics = self
            .optics
            .as_ref()
            .expect("water optics prepared before drawing");
        (
            &optics.layer_pipeline,
            &optics.input.layers[usize::from(front)],
        )
    }

    pub fn background(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &crate::post::Targets,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        self.optics
            .as_ref()
            .expect("water optics prepared before drawing")
            .background(encoder, targets, profiler);
    }

    /// Draws every region, groups 0 to 2 bound as for the lit forward pipelines.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        for r in self.regions.values() {
            let Some(g) = &r.gpu else {
                continue;
            };
            let [nx, nz] = g.size;
            pass.set_bind_group(3, &g.group, &[]);
            if self.cull_on {
                let mesh = g.mesh.as_ref().expect("water mesh encoded before draw");
                pass.set_pipeline(if r.look.blocky {
                    &self.blocky
                } else {
                    &self.smooth
                });
                pass.set_index_buffer(mesh.draw_indices().slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed_indirect(mesh.draw_args(), 0);
            } else if r.look.blocky {
                let per = r.look.per.max(1);
                let blocks = (nx / per) * (nz / per);
                pass.set_pipeline(&self.blocky);
                pass.draw(0..blocks * 18, 0..1);
            } else if let Some(indices) = self.indices.get(&g.size) {
                pass.set_pipeline(&self.smooth);
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..(nx - 1) * (nz - 1) * 6, 0, 0..1);
            }
            if !r.look.blocky {
                pass.set_pipeline(&self.ledge_surface);
                pass.set_vertex_buffer(0, g.ledges.indices.slice(..));
                pass.draw_indirect(&g.ledges.args, 0);
            }
            if r.surround_on {
                pass.set_pipeline(&self.surround);
                pass.draw(0..24, 0..1);
                // The stitch: a quad a pair of neighbouring edge columns along each edge, and two
                // triangles at each corner.
                pass.set_pipeline(&self.stitch);
                pass.draw(0..6 * (2 * (nx - 1) + 2 * (nz - 1) + 4), 0..1);
            }
        }
    }

    pub fn ledge_readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> impl std::future::Future<Output = Result<Vec<(u64, crate::water_ledge::Stats)>, String>> + use<>
    {
        let staged = self
            .regions
            .iter()
            .filter_map(|(&id, r)| {
                r.gpu
                    .as_ref()
                    .map(|g| (id, g.ledges.copy_stats(device, queue)))
            })
            .collect::<Vec<_>>();
        async move {
            let mut out = Vec::new();
            for (id, s) in staged {
                out.push((id, s.read().await?));
            }
            Ok(out)
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn ledge_stats(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<(u64, crate::water_ledge::Stats)>, String> {
        self.regions
            .iter()
            .filter_map(|(&id, r)| r.gpu.as_ref().map(|g| (id, &g.ledges)))
            .map(|(id, l)| {
                l.copy_stats(device, queue)
                    .read_blocking(device)
                    .map(|s| (id, s))
            })
            .collect()
    }

    pub fn spray_readback(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> impl std::future::Future<Output = Result<Vec<(u64, crate::water_spray::Stats)>, String>> + use<>
    {
        let staged = self
            .regions
            .iter()
            .filter_map(|(&id, r)| {
                r.gpu
                    .as_ref()?
                    .spray
                    .as_ref()
                    .map(|p| (id, p.copy_stats(device, queue)))
            })
            .collect::<Vec<_>>();
        async move {
            let mut out = Vec::new();
            for (id, s) in staged {
                out.push((id, s.read().await?));
            }
            Ok(out)
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spray_stats(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<(u64, crate::water_spray::Stats)>, String> {
        self.regions
            .iter()
            .filter_map(|(&id, r)| r.gpu.as_ref()?.spray.as_ref().map(|p| (id, p)))
            .map(|(id, p)| {
                p.copy_stats(device, queue)
                    .read_blocking(device)
                    .map(|s| (id, s))
            })
            .collect()
    }
    pub fn draw_spray(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.spray_on {
            return;
        }
        if let Some(p) = &self.spray {
            for r in self.regions.values() {
                if let Some(pool) = r.gpu.as_ref().and_then(|g| g.spray.as_ref()) {
                    p.draw(pass, pool);
                }
            }
        }
    }
    /// Whether any region is held.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

fn optical_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    (uniform, columns): (&wgpu::Buffer, &wgpu::Buffer),
    foam: &wgpu::TextureView,
    surface: &crate::water_surface::Surface,
    (geometry, ledge, mask): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::TextureView),
    optics: &crate::water_optics::Optics,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("water optics"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 10,
                resource: ledge.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(mask),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: geometry.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&surface.normal_depth),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&surface.flow_height),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: columns.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(foam),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&optics.input.color_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&optics.input.depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&optics.sampler),
            },
        ],
    })
}

fn depth_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    columns: &wgpu::Buffer,
    surface: &crate::water_surface::Surface,
    (geometry, ledge, mask): (&wgpu::Buffer, &wgpu::Buffer, &wgpu::TextureView),
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("water depth geometry"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 10,
                resource: ledge.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(mask),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: columns.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&surface.normal_depth),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&surface.flow_height),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: geometry.as_entire_binding(),
            },
        ],
    })
}
