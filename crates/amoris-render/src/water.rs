//! Water regions (docs/spec/water.md 9): the columns of each `WaterRegion` the feed carries, drawn
//! in the transparent layer by forward.wgsl's `vs_shallow` (Smooth: a vertex at each column's centre,
//! two triangles between four centres from an index buffer) or `vs_shallow_blocky` (Blocky: flat
//! over each block, with vertical sides where neighbouring blocks differ), both shaded by
//! `fs_shallow`. Every column is drawn (level 1; levels of detail are slice D). The renderer keeps a
//! copy of each region's columns, tile by tile, and uploads only the tiles a frame changed; the
//! copy also answers whether the camera is under a surface (the region's fog). Nothing here
//! simulates.
//!
//! The shallow-water spike's host-made field (`ShallowWaterView`, its examples) is drawn the same
//! way as one more region, its columns in rows instead of tiles.

use std::collections::BTreeMap;

use amoris_assets::frame::{WaterLookView, WaterUpdate};
use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};

/// Columns along a tile's edge.
const TILE: u32 = 16;
/// A column wetter than this is wet (the shader's `SHALLOW_WET`).
const WET: f32 = 0.01;

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
}

struct RegionGpu {
    uniform: wgpu::Buffer,
    columns: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// The column counts the buffers were made for.
    size: [u32; 2],
}

impl Region {
    fn column(&self, i: u32, k: u32) -> [f32; 4] {
        let at = if self.tiled {
            let tx = self.size[0] / TILE;
            ((i / TILE) + tx * (k / TILE)) * TILE * TILE + (i % TILE) + TILE * (k % TILE)
        } else {
            i + self.size[0] * k
        };
        self.columns
            .get(at as usize)
            .copied()
            .unwrap_or([f32::MIN, 0.0, 0.0, 0.0])
    }

    /// The column under world `(x, z)`.
    fn column_at(&self, x: f32, z: f32) -> Option<[f32; 4]> {
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
            buffers: &[],
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
            depth_write_enabled: Some(false),
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
    smooth: wgpu::RenderPipeline,
    blocky: wgpu::RenderPipeline,
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
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                wgpu::BindGroupLayoutEntry {
                    visibility: wgpu::ShaderStages::VERTEX,
                    ..entry(2, wgpu::BufferBindingType::Storage { read_only: true })
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
            smooth: pipeline(device, module, &layout, format, "vs_shallow"),
            blocky: pipeline(device, module, &layout, format, "vs_shallow_blocky"),
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
        self.smooth = pipeline(device, module, &self.layout, format, "vs_shallow");
        self.blocky = pipeline(device, module, &self.layout, format, "vs_shallow_blocky");
    }

    /// Forgets every region the feed sent (a reset frame); the spike's field stays.
    pub fn clear(&mut self) {
        self.regions.retain(|&id, _| id == SPIKE);
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
            });
            if r.size != u.size || r.columns.len() != (tiles * TILE * TILE) as usize {
                r.size = u.size;
                r.columns = vec![[f32::MIN, 0.0, 0.0, 0.0]; (tiles * TILE * TILE) as usize];
                r.all_dirty = true;
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
            },
            columns: Vec::new(),
            tiled: false,
            dirty: Vec::new(),
            all_dirty: true,
            gpu: None,
        });
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
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, t: f32) {
        for r in self.regions.values_mut() {
            let [nx, nz] = r.size;
            if nx < 2 || nz < 2 || r.columns.is_empty() {
                continue;
            }
            let bytes = (r.columns.len() * 16) as u64;
            if r.gpu.as_ref().is_none_or(|g| g.size != r.size) {
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
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water"),
                    layout: &self.group_layout,
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
                r.gpu = Some(RegionGpu {
                    uniform,
                    columns,
                    group,
                    size: r.size,
                });
                r.all_dirty = true;
            }
            let Some(g) = &r.gpu else {
                continue;
            };
            if r.all_dirty {
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
            let u = Uniform {
                origin: r.origin,
                cell: r.cell_m,
                time: t,
                count: [nx, nz, u32::from(r.tiled), r.look.per.max(1)],
                absorb: [a[0], a[1], a[2], 0.0],
                scatter: [s[0], s[1], s[2], 0.0],
            };
            queue.write_buffer(&g.uniform, 0, bytemuck::bytes_of(&u));
            if !r.look.blocky && !self.indices.contains_key(&r.size) {
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

    /// Draws every region, groups 0 to 2 bound as for the lit forward pipelines.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        for r in self.regions.values() {
            let Some(g) = &r.gpu else {
                continue;
            };
            let [nx, nz] = g.size;
            pass.set_bind_group(3, &g.group, &[]);
            if r.look.blocky {
                let per = r.look.per.max(1);
                let blocks = (nx / per) * (nz / per);
                pass.set_pipeline(&self.blocky);
                pass.draw(0..blocks * 18, 0..1);
            } else if let Some(indices) = self.indices.get(&g.size) {
                pass.set_pipeline(&self.smooth);
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..(nx - 1) * (nz - 1) * 6, 0, 0..1);
            }
        }
    }

    /// Whether any region is held.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}
