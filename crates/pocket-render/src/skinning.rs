//! Skeletal animation on the GPU: each skinned part of an entity owns a range of the shared vertex
//! buffer (a mesh entry that reuses the source mesh's indices), rewritten every frame by a compute
//! pass from the rest vertices, the per-vertex joint weights and the joint matrices of the
//! entity's animation pose. Everything after that (culling, shadows, drawing) treats it as a mesh.
//!
//! For TAA's object motion the pass first copies what each part's range held (last frame's skinned
//! vertices) into [`History`], which the forward vertex shader reads (`skin_prev` in forward.wgsl).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use pocket_assets::animation::{clip_time, find_clip, global_pose, joint_matrices};
use pocket_assets::frame::AnimView;
use pocket_assets::mesh::ModelAsset;

use crate::shaders;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Job {
    src: u32,
    dst: u32,
    count: u32,
    palette: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkinVert {
    joints: [u32; 2],
    pad: [u32; 2],
    weights: [f32; 4],
}

/// A skinned source mesh's rest vertices and weights, uploaded once.
pub struct Source {
    pub first: u32,
    pub count: u32,
}

/// One skinned part of an entity.
pub struct Part {
    pub entity: u64,
    pub asset: Arc<ModelAsset>,
    pub source: (String, usize),
    pub skin: usize,
    pub node_global: Mat4,
    /// The output range's first vertex (the dynamic mesh entry's base vertex).
    pub dst: u32,
    /// The dynamic mesh entry.
    pub mesh: u32,
}

/// Mark of a mesh without last frame's vertices in the history's table (`NO_TEXTURE` in WGSL).
const NONE: u32 = u32::MAX;

/// Last frame's skinned vertices: the table's length in meshes, per mesh id two words (the part's
/// first vertex in the pool or [`NONE`], where its copy starts in words), then the copies, 12 words
/// a vertex.
pub struct History {
    pub buffer: wgpu::Buffer,
    /// Bumped whenever `buffer` is replaced (bind groups hold it).
    pub generation: u64,
    /// The parts skinned last frame: (entity, mesh, first vertex).
    last: HashSet<(u64, u32, u32)>,
}

pub struct Skinning {
    pipeline: wgpu::ComputePipeline,
    rest: Vec<f32>,
    weights: Vec<SkinVert>,
    rest_buf: Option<wgpu::Buffer>,
    weight_buf: Option<wgpu::Buffer>,
    dirty: bool,
    sources: HashMap<(String, usize), Source>,
    pub parts: Vec<Part>,
    palette: Option<wgpu::Buffer>,
    jobs: Option<wgpu::Buffer>,
    pub history: History,
}

impl Skinning {
    pub fn new(device: &wgpu::Device) -> Skinning {
        let m = shaders::module(device, "skin");
        Skinning {
            pipeline: device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("skinning"),
                layout: None,
                module: &m,
                entry_point: Some("main"),
                compilation_options: shaders::compute_options(),
                cache: None,
            }),
            rest: Vec::new(),
            weights: Vec::new(),
            rest_buf: None,
            weight_buf: None,
            dirty: false,
            sources: HashMap::new(),
            parts: Vec::new(),
            palette: None,
            jobs: None,
            history: History {
                buffer: history_buffer(device, 64),
                generation: 0,
                last: HashSet::new(),
            },
        }
    }

    /// Registers a skinned mesh of an asset (once per path and mesh index).
    pub fn add_source(&mut self, path: &str, index: usize, asset: &ModelAsset) -> Option<u32> {
        let key = (path.to_owned(), index);
        if let Some(s) = self.sources.get(&key) {
            return Some(s.count);
        }
        let mesh = asset.meshes.get(index)?;
        let skin = mesh.skin.as_ref()?;
        let first = (self.rest.len() / 12) as u32;
        for v in &mesh.vertices {
            self.rest.extend_from_slice(&v.position);
            self.rest.extend_from_slice(&v.normal);
            self.rest.extend_from_slice(&v.uv);
            self.rest.extend_from_slice(&v.tangent);
        }
        for (j, w) in skin.joints.iter().zip(&skin.weights) {
            self.weights.push(SkinVert {
                joints: [
                    u32::from(j[0]) | (u32::from(j[1]) << 16),
                    u32::from(j[2]) | (u32::from(j[3]) << 16),
                ],
                pad: [0; 2],
                weights: *w,
            });
        }
        let count = mesh.vertices.len() as u32;
        self.sources.insert(key, Source { first, count });
        self.dirty = true;
        Some(count)
    }

    /// Forgets an entity's skinned parts (its look changed or it was removed).
    pub fn remove_entity(&mut self, entity: u64) {
        self.parts.retain(|p| p.entity != entity);
    }

    /// Copies what the parts' ranges hold (last frame's vertices) into the history, for parts
    /// skinned into the same range last frame; the others get no history (their motion reads as
    /// none for a frame). Without `motion` the table is emptied.
    fn keep_history(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        vertices: &wgpu::Buffer,
        motion: bool,
    ) {
        let h = &mut self.history;
        let now: HashSet<(u64, u32, u32)> = self
            .parts
            .iter()
            .map(|p| (p.entity, p.mesh, p.dst))
            .collect();
        let meshes = self.parts.iter().map(|p| p.mesh + 1).max().unwrap_or(0) as usize;
        let mut table = vec![NONE; 1 + meshes * 2];
        table[0] = meshes as u32;
        let mut words = table.len() as u64;
        let mut copies = Vec::new();
        if motion {
            for p in &self.parts {
                let Some(src) = self.sources.get(&p.source) else {
                    continue;
                };
                let at = 1 + p.mesh as usize * 2;
                if !h.last.contains(&(p.entity, p.mesh, p.dst)) || table[at] != NONE {
                    continue;
                }
                table[at] = p.dst;
                table[at + 1] = words as u32;
                copies.push((u64::from(p.dst) * 48, words * 4, u64::from(src.count) * 48));
                words += u64::from(src.count) * 12;
            }
        }
        h.last = now;
        let bytes = (words * 4).max(16);
        if bytes > h.buffer.size() {
            h.buffer = history_buffer(device, bytes.next_power_of_two());
            h.generation += 1;
        }
        queue.write_buffer(&h.buffer, 0, bytemuck::cast_slice(&table));
        for (from, to, size) in copies {
            if from + size <= vertices.size() {
                enc.copy_buffer_to_buffer(vertices, from, &h.buffer, to, size);
            }
        }
    }

    /// Evaluates every skinned part's pose at its interpolated clip time (`since_tick_s` is the
    /// drawn moment relative to the last tick: between -dt and 0) and encodes the skinning pass that
    /// writes their vertices into `vertices`, after keeping last frame's in the history when
    /// `motion` (TAA) wants them.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        vertices: &wgpu::Buffer,
        anims: &HashMap<u64, AnimView>,
        since_tick_s: f32,
        motion: bool,
    ) {
        self.keep_history(device, queue, enc, vertices, motion);
        if self.parts.is_empty() {
            return;
        }
        let storage = |label: &str, bytes: &[u8]| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (bytes.len() as u64).max(16).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&b, 0, bytes);
            b
        };
        if self.dirty {
            self.rest_buf = Some(storage(
                "skin rest vertices",
                bytemuck::cast_slice(&self.rest),
            ));
            self.weight_buf = Some(storage("skin weights", bytemuck::cast_slice(&self.weights)));
            self.dirty = false;
        }
        let (Some(rest), Some(weights)) = (&self.rest_buf, &self.weight_buf) else {
            return;
        };
        let mut palette: Vec<[f32; 16]> = Vec::new();
        let mut jobs: Vec<(Job, u32)> = Vec::new();
        // One pose per entity (its parts share the skeleton).
        let mut poses: HashMap<u64, Vec<Mat4>> = HashMap::new();
        for p in &self.parts {
            let Some(src) = self.sources.get(&p.source) else {
                continue;
            };
            let pose = poses.entry(p.entity).or_insert_with(|| {
                let a = anims.get(&p.entity);
                let clip = a
                    .and_then(|a| find_clip(&p.asset, &a.clip))
                    .or_else(|| p.asset.animations.first());
                let t = match (a, clip) {
                    (Some(a), Some(c)) => clip_time(c, a.time + a.rate * since_tick_s, a.looped),
                    _ => 0.0,
                };
                global_pose(&p.asset, clip, t)
            });
            let first = palette.len() as u32;
            for m in joint_matrices(&p.asset, p.skin, pose, p.node_global) {
                palette.push(m.to_cols_array());
            }
            jobs.push((
                Job {
                    src: src.first,
                    dst: p.dst,
                    count: src.count,
                    palette: first,
                },
                src.count,
            ));
        }
        if jobs.is_empty() || palette.is_empty() {
            return;
        }
        let pal_bytes = bytemuck::cast_slice::<[f32; 16], u8>(&palette);
        if self
            .palette
            .as_ref()
            .is_none_or(|b| b.size() < pal_bytes.len() as u64)
        {
            self.palette = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("joint matrices"),
                size: (pal_bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(pal) = &self.palette else { return };
        queue.write_buffer(pal, 0, pal_bytes);
        let job_bytes = (jobs.len() * 256) as u64;
        if self.jobs.as_ref().is_none_or(|b| b.size() < job_bytes) {
            self.jobs = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("skinning jobs"),
                size: job_bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(job_buf) = &self.jobs else { return };
        for (k, (j, _)) in jobs.iter().enumerate() {
            queue.write_buffer(job_buf, k as u64 * 256, bytemuck::bytes_of(j));
        }
        let layout = self.pipeline.get_bind_group_layout(0);
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (k, (_, count)) in jobs.iter().enumerate() {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skinning job"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: job_buf,
                            offset: k as u64 * 256,
                            size: wgpu::BufferSize::new(16),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: rest.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: weights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: pal.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: vertices.as_entire_binding(),
                    },
                ],
            });
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
    }
}

fn history_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("skinned vertices (last frame)"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
