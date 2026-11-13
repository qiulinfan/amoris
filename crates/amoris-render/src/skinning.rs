//! Skeletal animation on the GPU: each skinned part of an entity owns a range of the shared vertex
//! buffer (a mesh entry that reuses the source mesh's indices), rewritten every frame by a compute
//! pass from the rest vertices, the per-vertex joint weights and the joint matrices of the
//! entity's animation pose. Everything after that (culling, shadows, drawing) treats it as a mesh.
//!
//! For TAA's object motion the pass first copies what each part's range held (last frame's skinned
//! vertices) into [`History`], which the forward vertex shader reads (`skin_prev` in forward.wgsl).
//!
//! A mesh with morph targets (docs/spec/animation.md 5a) goes through the same pass: before
//! skinning, each vertex adds its active targets' offsets times their weights, at most
//! [`MAX_ACTIVE`] a part; a mesh with targets and no skin takes one identity joint.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use amoris_assets::animation::{
    clip_time, find_clip, global_pose, joint_matrices, morph_offsets, morph_weights,
};
use amoris_assets::frame::AnimView;
use amoris_assets::mesh::{ModelAsset, MorphTargets};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use crate::shaders;

/// A skinned part's job in the one dispatch (skin.wgsl `jobs`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Job {
    src: u32,
    dst: u32,
    count: u32,
    palette: u32,
    /// Its first workgroup.
    group: u32,
    /// Its source's first morph offset (a float index into the targets' offsets).
    morph: u32,
    /// Its first active target (an index into the active targets) and their count.
    active: u32,
    actives: u32,
}

/// The most morph targets a part applies in a frame: those with the largest magnitude of weight
/// (animation.md 5a).
pub const MAX_ACTIVE: usize = 16;

/// Vertices per workgroup (skin.wgsl).
const GROUP: u32 = 64;
/// Workgroups per dispatch row (skin.wgsl `ROW`).
const ROW: u32 = 65535;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkinVert {
    joints: [u32; 2],
    pad: [u32; 2],
    weights: [f32; 4],
}

/// A skinned or morphed source mesh's rest vertices, weights and targets' offsets, uploaded once.
pub struct Source {
    pub first: u32,
    pub count: u32,
    /// Whether its vertices carry skin weights (else they take one identity joint).
    pub skinned: bool,
    /// Its first morph offset and its target count (0 without targets).
    pub morph: u32,
    pub targets: u32,
}

/// One skinned or morphed part of an entity.
pub struct Part {
    pub entity: u64,
    pub asset: Arc<ModelAsset>,
    pub source: (String, usize),
    pub skin: Option<usize>,
    /// The file's node the mesh hangs from (its morph weights).
    pub node: Option<usize>,
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
    /// The table as last written, written again only when it changes.
    written: Vec<u32>,
}

pub struct Skinning {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    rest: Vec<f32>,
    weights: Vec<SkinVert>,
    /// Every source's targets' offsets (`MorphTargets::deltas`, one after another).
    deltas: Vec<f32>,
    rest_buf: Option<wgpu::Buffer>,
    weight_buf: Option<wgpu::Buffer>,
    delta_buf: Option<wgpu::Buffer>,
    /// The frame's active targets: per part its (target, weight bits) pairs.
    active: Option<wgpu::Buffer>,
    dirty: bool,
    sources: HashMap<(String, usize), Source>,
    pub parts: Vec<Part>,
    palette: Option<wgpu::Buffer>,
    /// The job count and the jobs (skin.wgsl `jobs`).
    jobs: Option<wgpu::Buffer>,
    /// The jobs as last written, written again only when they change.
    jobs_written: Vec<u8>,
    /// The pass's bind group and the buffers it holds: rebuilt when one is replaced, not every
    /// frame. Creating bind groups and staging writes per job cost about 0.9 ms a frame on Direct3D
    /// 12 for 40 skinned parts (docs/bench/dx12.md 10). One dispatch covers every job, where a
    /// dispatch per job bound each with a dynamic offset (docs/bench/dynamic-scenes.md: its GPU
    /// effect is not measured).
    group: Option<([wgpu::Buffer; 7], wgpu::BindGroup)>,
    pub history: History,
}

impl Skinning {
    pub fn new(device: &wgpu::Device) -> Skinning {
        let m = shaders::module(device, "skin");
        let cs = wgpu::ShaderStages::COMPUTE;
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cs,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skinning"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, false),
                storage(5, true),
                storage(6, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skinning"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Skinning {
            pipeline: device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("skinning"),
                layout: Some(&pipeline_layout),
                module: &m,
                entry_point: Some("main"),
                compilation_options: shaders::compute_options(),
                cache: None,
            }),
            rest: Vec::new(),
            weights: Vec::new(),
            deltas: Vec::new(),
            rest_buf: None,
            weight_buf: None,
            delta_buf: None,
            active: None,
            dirty: false,
            sources: HashMap::new(),
            parts: Vec::new(),
            palette: None,
            jobs: None,
            jobs_written: Vec::new(),
            group: None,
            layout,
            history: History {
                buffer: history_buffer(device, 64),
                generation: 0,
                last: HashSet::new(),
                written: Vec::new(),
            },
        }
    }

    /// Registers a skinned (with `skinned`) or morphed mesh of an asset (once per path and mesh
    /// index); none for a mesh that is neither.
    pub fn add_source(
        &mut self,
        path: &str,
        index: usize,
        asset: &ModelAsset,
        skinned: bool,
    ) -> Option<u32> {
        let key = (path.to_owned(), index);
        if let Some(s) = self.sources.get(&key) {
            return Some(s.count);
        }
        let mesh = asset.meshes.get(index)?;
        let skin = mesh.skin.as_ref().filter(|_| skinned);
        let n = mesh.vertices.len();
        let morph = mesh
            .morph
            .as_ref()
            .filter(|m| !m.is_empty() && m.deltas.len() == m.len() * n * MorphTargets::STRIDE);
        if skin.is_none() && morph.is_none() {
            return None;
        }
        let first = (self.rest.len() / 12) as u32;
        for v in &mesh.vertices {
            self.rest.extend_from_slice(&v.position);
            self.rest.extend_from_slice(&v.normal);
            self.rest.extend_from_slice(&v.uv);
            self.rest.extend_from_slice(&v.tangent);
        }
        match skin {
            Some(skin) => {
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
            }
            // One identity joint.
            None => self.weights.extend((0..n).map(|_| SkinVert {
                joints: [0; 2],
                pad: [0; 2],
                weights: [1.0, 0.0, 0.0, 0.0],
            })),
        }
        let (morph_at, targets) = match morph {
            Some(m) => {
                let at = self.deltas.len() as u32;
                self.deltas.extend_from_slice(&m.deltas);
                (at, m.len() as u32)
            }
            None => (0, 0),
        };
        let count = n as u32;
        self.sources.insert(
            key,
            Source {
                first,
                count,
                skinned: skin.is_some(),
                morph: morph_at,
                targets,
            },
        );
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
            h.written.clear();
        }
        if table != h.written {
            queue.write_buffer(&h.buffer, 0, bytemuck::cast_slice(&table));
            h.written = table;
        }
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
            self.delta_buf = Some(storage(
                "morph target offsets",
                bytemuck::cast_slice(&self.deltas),
            ));
            self.dirty = false;
        }
        let (Some(rest), Some(weights), Some(deltas)) =
            (&self.rest_buf, &self.weight_buf, &self.delta_buf)
        else {
            return;
        };
        let mut palette: Vec<[f32; 16]> = Vec::new();
        let mut jobs: Vec<Job> = Vec::new();
        let mut active: Vec<[u32; 2]> = Vec::new();
        let mut groups = 0u32;
        // One pose per entity (its parts share the skeleton).
        let mut poses: HashMap<u64, Vec<Mat4>> = HashMap::new();
        for p in &self.parts {
            let Some(src) = self.sources.get(&p.source) else {
                continue;
            };
            let a = anims.get(&p.entity);
            let first_active = active.len() as u32;
            if src.targets > 0 {
                let w = part_weights(p, a, since_tick_s);
                active.extend(
                    active_targets(&w, src.targets as usize)
                        .into_iter()
                        .map(|(t, w)| [t, w.to_bits()]),
                );
            }
            let actives = active.len() as u32 - first_active;
            let first = palette.len() as u32;
            let skin = p.skin.filter(|_| src.skinned);
            let Some(skin) = skin else {
                // A morphed mesh without a skin: one identity joint.
                palette.push(Mat4::IDENTITY.to_cols_array());
                jobs.push(Job {
                    src: src.first,
                    dst: p.dst,
                    count: src.count,
                    palette: first,
                    group: groups,
                    morph: src.morph,
                    active: first_active,
                    actives,
                });
                groups += src.count.div_ceil(GROUP);
                continue;
            };
            let pose = poses.entry(p.entity).or_insert_with(|| {
                let a = anims.get(&p.entity);
                // A pose the simulation evaluated (animation.md 5), between its last two ticks.
                if let Some(a) = a.filter(|a| !a.pose.is_empty()) {
                    let u = if a.tick_s > 0.0 {
                        (1.0 + since_tick_s / a.tick_s).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    if let Some(g) =
                        amoris_assets::animation::global_pose_from(&p.asset, &a.prev, &a.pose, u)
                    {
                        return g;
                    }
                }
                let (clip, t) = clip_at(p, a, since_tick_s);
                global_pose(&p.asset, clip, t)
            });
            for m in joint_matrices(&p.asset, skin, pose, p.node_global) {
                palette.push(m.to_cols_array());
            }
            jobs.push(Job {
                src: src.first,
                dst: p.dst,
                count: src.count,
                palette: first,
                group: groups,
                morph: src.morph,
                active: first_active,
                actives,
            });
            groups += src.count.div_ceil(GROUP);
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
        let mut job_bytes = (jobs.len() as u32).to_le_bytes().to_vec();
        job_bytes.extend_from_slice(bytemuck::cast_slice(&jobs));
        if self
            .jobs
            .as_ref()
            .is_none_or(|b| b.size() < job_bytes.len() as u64)
        {
            self.jobs = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("skinning jobs"),
                size: (job_bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.jobs_written.clear();
        }
        let Some(job_buf) = &self.jobs else { return };
        if job_bytes != self.jobs_written {
            queue.write_buffer(job_buf, 0, &job_bytes);
            self.jobs_written = job_bytes;
        }
        let active_bytes = bytemuck::cast_slice::<[u32; 2], u8>(&active);
        if self
            .active
            .as_ref()
            .is_none_or(|b| b.size() < active_bytes.len() as u64)
        {
            self.active = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("active morph targets"),
                size: (active_bytes.len() as u64).max(16).next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(active_buf) = &self.active else {
            return;
        };
        if !active_bytes.is_empty() {
            queue.write_buffer(active_buf, 0, active_bytes);
        }
        let buffers = [
            job_buf.clone(),
            rest.clone(),
            weights.clone(),
            pal.clone(),
            vertices.clone(),
            deltas.clone(),
            active_buf.clone(),
        ];
        if self.group.as_ref().is_none_or(|(held, _)| *held != buffers) {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skinning"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: job_buf.as_entire_binding(),
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
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: deltas.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: active_buf.as_entire_binding(),
                    },
                ],
            });
            self.group = Some((buffers, bg));
        }
        let Some((_, bg)) = &self.group else { return };
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        let (x, y) = if groups > ROW {
            (ROW, groups.div_ceil(ROW))
        } else {
            (groups, 1)
        };
        pass.dispatch_workgroups(x, y, 1);
    }
}

/// The clip a part plays and its time for the drawn moment: the view's clip (or the file's first)
/// at its interpolated time, or the first clip at 0 without a view.
fn clip_at<'a>(
    p: &'a Part,
    a: Option<&AnimView>,
    since_tick_s: f32,
) -> (Option<&'a amoris_assets::mesh::AnimationClip>, f32) {
    let clip = a
        .and_then(|a| find_clip(&p.asset, &a.clip))
        .or_else(|| p.asset.animations.first());
    let t = match (a, clip) {
        (Some(a), Some(c)) => clip_time(c, a.time + a.rate * since_tick_s, a.looped),
        _ => 0.0,
    };
    (clip, t)
}

/// A part's morph weights for the drawn moment: a pose's from the simulation between its last two
/// ticks, else its clip's, else its node's defaults (animation.md 5a).
fn part_weights(p: &Part, a: Option<&AnimView>, since_tick_s: f32) -> Vec<f32> {
    let Some(node) = p.node.filter(|&n| n < p.asset.skeleton.len()) else {
        return Vec::new();
    };
    if let Some(a) = a.filter(|a| !a.pose.is_empty()) {
        let at = morph_offsets(&p.asset);
        let (lo, hi) = (at[node], at[node + 1]);
        if a.morph.len() != at[at.len() - 1] || a.prev_morph.len() != a.morph.len() {
            return p.asset.skeleton[node].weights.clone();
        }
        let u = if a.tick_s > 0.0 {
            (1.0 + since_tick_s / a.tick_s).clamp(0.0, 1.0)
        } else {
            1.0
        };
        return (lo..hi)
            .map(|i| a.prev_morph[i] + (a.morph[i] - a.prev_morph[i]) * u)
            .collect();
    }
    let (clip, t) = clip_at(p, a, since_tick_s);
    morph_weights(&p.asset, clip, t, node)
}

/// The targets a part applies: the nonzero weights of its first `targets`, at most [`MAX_ACTIVE`]
/// of the largest magnitude (the lower index first among equals), in their order.
fn active_targets(weights: &[f32], targets: usize) -> Vec<(u32, f32)> {
    let mut a: Vec<(u32, f32)> = weights
        .iter()
        .take(targets)
        .enumerate()
        .filter(|(_, w)| **w != 0.0 && w.is_finite())
        .map(|(i, w)| (i as u32, *w))
        .collect();
    if a.len() > MAX_ACTIVE {
        a.sort_by(|x, y| y.1.abs().total_cmp(&x.1.abs()).then(x.0.cmp(&y.0)));
        a.truncate(MAX_ACTIVE);
        a.sort_by_key(|x| x.0);
    }
    a
}

fn history_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("skinned vertices (last frame)"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_the_heaviest_targets_are_active_in_their_order() {
        assert!(active_targets(&[0.0, 0.0], 2).is_empty());
        assert_eq!(
            active_targets(&[0.0, 0.5, f32::NAN, -1.0], 4),
            vec![(1, 0.5), (3, -1.0)]
        );
        // Weights past the mesh's targets are not its own.
        assert_eq!(active_targets(&[0.0, 0.5, 1.0], 2), vec![(1, 0.5)]);
        let w: Vec<f32> = (0..20).map(|i| i as f32 * 0.1 - 1.0).collect();
        let a = active_targets(&w, 20);
        assert_eq!(a.len(), MAX_ACTIVE);
        assert!(a.windows(2).all(|p| p[0].0 < p[1].0));
        // Those left out weigh no more than any kept.
        let least = a.iter().map(|x| x.1.abs()).fold(f32::INFINITY, f32::min);
        for i in (0..20).filter(|i| a.iter().all(|x| x.0 != *i)) {
            assert!(w[i as usize].abs() <= least, "{i}: {}", w[i as usize]);
        }
    }
}
