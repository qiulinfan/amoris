//! The transparent stage (charter 4.4, Pioneer 2026-10-16; docs/spec/transparency.md): instances
//! whose material blends (glTF BLEND, a look's alpha below 1) or transmits
//! (KHR_materials_transmission) leave the GPU-driven opaque batches (the culling pass keeps them out
//! of the camera's list, so out of the depth prepass, the depth pyramid and occlusion culling's
//! visible set) and are drawn here: culled against the camera's frustum and sorted back to front
//! by the view depth of their bounding sphere's centre on the CPU each frame, then drawn after the
//! sky, before the particles, with premultiplied blending, depth tested and not written
//! (forward.wgsl `fs_transparent`): in the opaque pass, or with GTAO on in a pass of their own
//! after GTAO has darkened the opaque image (renderer.rs).
//! With water present, a depth mask partitions its fragments into two draws, before and after
//! the water optical stage; both consume the same sorted list.
//!
//! The draws read their instances from a list of `Drawn` records this stage writes in draw order
//! (the culling pass writes the opaque ones' into `drawn`): the frame group's `drawn` binding is
//! this list in the stage's own frame groups, and a draw's records are its instances
//! `first_instance..` of a direct draw, which WebGPU's core allows on every draw path (only an
//! indirect draw's `first_instance` needs `indirect-first-instance`). Neighbours in the order with
//! the same mesh row and the same sidedness draw as one instanced draw.
//!
//! A double-sided instance must draw its back faces, then its front faces, before the next instance.
//! Its draw uses a two-sided index list (`TwoSided`): the mesh row's indices twice, the first copy's
//! triangles reversed (transparent_index.wgsl), so that through the one pipeline, which culls back
//! faces, the first copy draws the back faces and the second the front faces; primitives of an
//! instanced draw rasterize and blend in order, instance by instance.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3, Vec4};

use crate::batches::Batches;
use crate::meshes::MeshPool;
use crate::post::{DEPTH, SceneFormat};
use crate::scene::{FLAG_ALIVE, FLAG_VISIBLE, Scene, VARIANT_SHIFT};

/// One instance as the vertex shaders read it (forward.wgsl `Drawn`, 48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct DrawnGpu {
    pos: [f32; 3],
    material: u32,
    rot: [f32; 4],
    scale: [f32; 3],
    slot: u32,
}

/// Instances `first..first + count` of the list, all of mesh row `row`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Draw {
    row: u32,
    first: u32,
    count: u32,
    /// Drawn from the row's two-sided index list: back faces, then front faces.
    double_sided: bool,
}

/// An instance to sort: its view depth, slot and record.
struct Item {
    depth: f32,
    slot: u32,
    row: u32,
    double_sided: bool,
    record: DrawnGpu,
}

/// What the camera sees this frame (the stage's culling and levels of detail).
pub struct View {
    /// The jittered camera's frustum planes (`camera::frustum_planes`), pointing in.
    pub planes: [Vec4; 6],
    pub eye: Vec3,
    /// The camera's forward direction (view depth is along it).
    pub forward: Vec3,
    /// The interpolation between the slots' last two poses.
    pub alpha: f32,
    /// Levels of detail when they are on: the error bound per metre of distance (perspective) or
    /// in all (orthographic), as cull.wgsl's `eye.w`, and whether the camera is orthographic.
    pub lod: Option<(f32, bool)>,
}

/// What the stage drew in the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransparentStats {
    /// Transparent instances in the frustum (drawn).
    pub instances: u32,
    /// Draw calls of the color pass.
    pub draws: u32,
}

/// A reversal for transparent_index.wgsl (`Job`): the reversed copy's first index, its triangles.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct Job {
    first: u32,
    triangles: u32,
}

/// Where the two-sided lists lie in their buffer (`TwoSided`'s bookkeeping, apart from the GPU).
#[derive(Debug, Default)]
struct Placement {
    /// Indices in use.
    len: u64,
    /// Per mesh row, where its list starts (twice the row's indices).
    rows: HashMap<u32, u32>,
    /// Rows whose list alone is larger than the buffer may be: never made.
    too_large: HashSet<u32>,
}

impl Placement {
    /// Places the lists of this frame's `wanted` rows (sorted, without repeats; `size` indices
    /// each) in a buffer of `capacity` indices that may grow to `limit`. Returns the capacity of a
    /// new buffer to make, when the lists do not fit and a larger buffer helps (it then holds this
    /// frame's lists only; the others are made again when drawn), and the lists to fill: (row,
    /// where its list starts). When the buffer is as large as it may be, it is refilled with this
    /// frame's lists if they fit together; else the lists that do not fit are not made (their
    /// draws draw front faces only), and the next frames with the same rows change nothing.
    fn reserve(
        &mut self,
        wanted: &[u32],
        size: impl Fn(u32) -> u64,
        capacity: u64,
        limit: u64,
    ) -> (Option<u64>, Vec<(u32, u32)>) {
        for &row in wanted {
            if size(row) > limit {
                self.too_large.insert(row);
            }
        }
        let makeable: Vec<u32> = wanted
            .iter()
            .copied()
            .filter(|r| size(*r) > 0 && !self.too_large.contains(r))
            .collect();
        let missing: u64 = makeable
            .iter()
            .filter(|r| !self.rows.contains_key(r))
            .map(|&r| size(r))
            .sum();
        if missing == 0 {
            return (None, Vec::new());
        }
        let mut capacity = capacity;
        let mut grown = None;
        if self.len + missing > capacity {
            let frame: u64 = makeable.iter().map(|&r| size(r)).sum();
            let larger = frame.max(2 * capacity).min(limit);
            if larger > capacity {
                grown = Some(larger);
                capacity = larger;
            }
            if grown.is_some() || frame <= capacity {
                self.len = 0;
                self.rows.clear();
            }
        }
        let mut placed = Vec::new();
        for row in makeable {
            if self.rows.contains_key(&row) {
                continue;
            }
            let end = self.len + size(row);
            if end > capacity {
                continue;
            }
            self.rows.insert(row, self.len as u32);
            placed.push((row, self.len as u32));
            self.len = end;
        }
        (grown, placed)
    }

    /// Forgets the lists of rows whose mesh was replaced (`MeshPool::replace`): they hold the old
    /// indices and are made again when next drawn. Their space is reclaimed when the buffer is next
    /// refilled.
    fn forget(&mut self, rows: &[u32]) {
        for row in rows {
            self.rows.remove(row);
            self.too_large.remove(row);
        }
    }
}

/// The two-sided index lists of the mesh rows double-sided transparent instances have drawn, made
/// on the GPU from the mesh pool's indices when a row is first drawn so (`Transparent::encode`).
struct TwoSided {
    /// Index, storage (the reversal) and copy destination.
    indices: wgpu::Buffer,
    placement: Placement,
    /// The rows to fill this frame: (row, where its list starts).
    pending: Vec<(u32, u32)>,
    reverse: Option<wgpu::ComputePipeline>,
}

const INDEX: u64 = 4;

/// The most indices a two-sided buffer may hold: the reversal binds it whole as storage.
fn two_sided_limit(device: &wgpu::Device) -> u64 {
    (device.limits().max_storage_buffer_binding_size & !255) / INDEX
}

/// A buffer for `indices` two-sided indices at least (never past [`two_sided_limit`]).
fn two_sided_buffer(device: &wgpu::Device, indices: u64) -> wgpu::Buffer {
    let limit = two_sided_limit(device) * INDEX;
    let bytes = indices.max(1024) * INDEX;
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("transparent two-sided indices"),
        size: bytes.next_power_of_two().min(limit).max(bytes),
        usage: wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub struct Transparent {
    /// The records in draw order (the stage's frame groups bind it as `drawn`).
    pub drawn: wgpu::Buffer,
    /// What `drawn` holds (a frame that would write the same bytes writes nothing).
    held: Vec<DrawnGpu>,
    records: Vec<DrawnGpu>,
    draws: Vec<Draw>,
    items: Vec<Item>,
    two_sided: TwoSided,
    /// For the opaque pass's format: built with its other pipelines (renderer.rs, at start-up and
    /// when the format changes), so the first frame with transparent instances does not stall.
    pipeline: wgpu::RenderPipeline,
    pub stats: TransparentStats,
}

const RECORD: u64 = std::mem::size_of::<DrawnGpu>() as u64;

fn list(device: &wgpu::Device, records: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("transparent drawn"),
        size: (records.max(64) * RECORD).next_power_of_two(),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// common.wgsl `nlerp`.
fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    Quat::from_vec4(Vec4::from(a).lerp(Vec4::from(b), t).normalize())
}

/// The mesh row drawing an instance's level of detail: cull.wgsl's camera rule (the coarsest level
/// whose projected error over the bounding sphere stays within the bound) without its hysteresis.
fn level(meshes: &MeshPool, mesh: u32, centre: Vec3, radius: f32, smax: f32, view: &View) -> u32 {
    let (Some((per_metre, ortho)), Some(info)) = (view.lod, meshes.infos.get(mesh as usize)) else {
        return mesh;
    };
    let levels = info.levels().min(8);
    if levels < 2 {
        return mesh;
    }
    let mut dist = 1.0;
    if !ortho {
        let delta = centre - view.eye;
        let depth = delta.dot(view.forward);
        let near = (depth - radius).max(1e-4);
        let side = (delta - view.forward * depth).length() + radius;
        dist = near / (1.0 + (side / near) * (side / near)).sqrt();
    }
    let bound = dist * per_metre;
    let mut chosen = 0;
    for k in 1..levels {
        let row = info.level_row(k);
        if meshes
            .infos
            .get(row as usize)
            .is_some_and(|l| l.lod_error * smax <= bound)
        {
            chosen = k;
        }
    }
    if chosen == 0 {
        mesh
    } else {
        info.level_row(chosen)
    }
}

/// Sorts `items` back to front (equal depths in slot order, so a frame is the same every time) and
/// lays out their records and draws: neighbours with the same mesh row and sidedness share an
/// instanced draw.
fn order(items: &mut [Item], records: &mut Vec<DrawnGpu>, draws: &mut Vec<Draw>) {
    items.sort_unstable_by(|a, b| {
        b.depth
            .total_cmp(&a.depth)
            .then_with(|| a.slot.cmp(&b.slot))
    });
    records.clear();
    draws.clear();
    for (k, item) in items.iter().enumerate() {
        records.push(item.record);
        match draws.last_mut() {
            Some(d) if d.double_sided == item.double_sided && d.row == item.row => {
                d.count += 1;
            }
            _ => draws.push(Draw {
                row: item.row,
                first: k as u32,
                count: 1,
                double_sided: item.double_sided,
            }),
        }
    }
}

/// The color targets of a transparent pipeline: the HDR color blended premultiplied; the extra
/// targets left as they are: the motion and normals keep what is behind, and the indirect share is
/// read by GTAO before the stage draws (renderer.rs).
fn targets(format: SceneFormat) -> Vec<Option<wgpu::ColorTargetState>> {
    let premultiplied = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    format.targets(Some(premultiplied), false)
}

/// The stage's pipeline for an opaque pass of `format`: forward.wgsl's `vs` and `fs_transparent`,
/// back faces culled.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    pipeline_entry(device, module, layout, format, "fs_transparent")
}

pub(crate) fn pipeline_entry(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
    fragment: &str,
) -> wgpu::RenderPipeline {
    // `vs` computes last frame's position only for the motion target, which this pipeline leaves
    // alone.
    let constants = [("SCENE_MOTION", 0.0)];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("transparent"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            buffers: &[Some(crate::renderer::vertex_layout())],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &targets(format),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
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

impl TwoSided {
    fn new(device: &wgpu::Device) -> TwoSided {
        TwoSided {
            indices: two_sided_buffer(device, 0),
            placement: Placement::default(),
            pending: Vec::new(),
            reverse: None,
        }
    }

    /// Makes room for the two-sided lists of `rows` (the double-sided draws' rows this frame) and
    /// lists those not made yet for `encode` (`Placement::reserve`). A list that does not fit under
    /// the device's storage binding limit is not made: its draws then draw their front faces only.
    fn reserve(&mut self, device: &wgpu::Device, meshes: &MeshPool, rows: &[u32]) {
        let size = |row: u32| {
            meshes
                .infos
                .get(row as usize)
                .map_or(0, |i| 2 * u64::from(i.index_count))
        };
        let mut wanted: Vec<u32> = rows.to_vec();
        wanted.sort_unstable();
        wanted.dedup();
        let capacity = self.indices.size() / INDEX;
        let limit = two_sided_limit(device);
        let (grown, placed) = self.placement.reserve(&wanted, size, capacity, limit);
        if let Some(indices) = grown {
            self.indices = two_sided_buffer(device, indices);
        }
        self.pending.extend(placed);
    }

    fn forget(&mut self, rows: &[u32]) {
        self.placement.forget(rows);
        self.pending.retain(|(r, _)| !rows.contains(r));
    }

    /// Where `row`'s two-sided list starts, if it is made.
    fn start(&self, row: u32) -> Option<u32> {
        self.placement.rows.get(&row).copied()
    }

    /// Fills the pending lists: each row's indices copied twice from the mesh pool, then the first
    /// copy's triangles reversed.
    fn encode(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder, meshes: &MeshPool) {
        if self.pending.is_empty() {
            return;
        }
        let mut jobs = Vec::with_capacity(self.pending.len());
        for (row, first) in self.pending.drain(..) {
            let Some(info) = meshes.infos.get(row as usize) else {
                continue;
            };
            let bytes = u64::from(info.index_count) * INDEX;
            let from = u64::from(info.first_index) * INDEX;
            let to = u64::from(first) * INDEX;
            enc.copy_buffer_to_buffer(&meshes.indices, from, &self.indices, to, bytes);
            enc.copy_buffer_to_buffer(&meshes.indices, from, &self.indices, to + bytes, bytes);
            jobs.push(Job {
                first,
                triangles: info.index_count / 3,
            });
        }
        if jobs.is_empty() {
            return;
        }
        let pipeline = self.reverse.get_or_insert_with(|| {
            let module = crate::shaders::module(device, "transparent_index");
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("transparent two-sided indices"),
                layout: None,
                module: &module,
                entry_point: Some("reverse"),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        let limit = device.limits().max_compute_workgroups_per_dimension;
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("transparent two-sided indices"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        // A row of workgroups per job (the shader strides a job's triangles over its row).
        for chunk in jobs.chunks(limit as usize) {
            let job_buffer = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("transparent two-sided jobs"),
                    contents: bytemuck::cast_slice(chunk),
                    usage: wgpu::BufferUsages::STORAGE,
                },
            );
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("transparent two-sided indices"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.indices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: job_buffer.as_entire_binding(),
                    },
                ],
            });
            let most = chunk.iter().map(|j| j.triangles).max().unwrap_or(0);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(most.div_ceil(64).clamp(1, limit), chunk.len() as u32, 1);
        }
    }
}

impl Transparent {
    /// Rows whose mesh was replaced since the last frame (`MeshPool::take_replaced`): their
    /// two-sided lists are made again.
    pub fn forget_rows(&mut self, rows: &[u32]) {
        self.two_sided.forget(rows);
    }

    /// With its `pipeline` ([`pipeline`]) for the opaque pass's format.
    pub fn new(device: &wgpu::Device, pipeline: wgpu::RenderPipeline) -> Transparent {
        Transparent {
            drawn: list(device, 64),
            held: Vec::new(),
            records: Vec::new(),
            draws: Vec::new(),
            items: Vec::new(),
            two_sided: TwoSided::new(device),
            pipeline,
            stats: TransparentStats::default(),
        }
    }

    /// Whether this frame draws transparent instances.
    pub fn any(&self) -> bool {
        !self.draws.is_empty()
    }

    /// Culls and sorts the scene's transparent instances for `view`, lays out the draws and writes
    /// the list. Returns whether `drawn` was replaced (the frame groups holding it need rebuilding).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        meshes: &MeshPool,
        view: &View,
    ) -> bool {
        self.items.clear();
        let need = FLAG_ALIVE | FLAG_VISIBLE;
        for &slot in scene.transparent_slots() {
            let Some(inst) = scene.slots.get(slot as usize) else {
                continue;
            };
            if inst.flags & need != need {
                continue;
            }
            let Some(info) = meshes.infos.get(inst.mesh as usize) else {
                continue;
            };
            let pos = Vec3::from(inst.prev_pos).lerp(Vec3::from(inst.pos), view.alpha);
            let rot = nlerp(
                Quat::from_array(inst.prev_rot),
                Quat::from_array(inst.rot),
                view.alpha,
            );
            let scale = Vec3::from(inst.scale);
            let smax = scale.abs().max_element();
            let centre = pos + rot * (Vec3::from(info.center) * scale);
            let radius = info.radius * smax;
            if view
                .planes
                .iter()
                .any(|p| p.truncate().dot(centre) + p.w < -radius)
            {
                continue;
            }
            self.items.push(Item {
                depth: (centre - view.eye).dot(view.forward),
                slot,
                row: level(meshes, inst.mesh, centre, radius, smax, view),
                double_sided: (inst.flags >> VARIANT_SHIFT) & 2 != 0,
                record: DrawnGpu {
                    pos: pos.to_array(),
                    material: inst.material,
                    rot: rot.to_array(),
                    scale: inst.scale,
                    slot,
                },
            });
        }
        order(&mut self.items, &mut self.records, &mut self.draws);
        let rows: Vec<u32> = self
            .draws
            .iter()
            .filter(|d| d.double_sided)
            .map(|d| d.row)
            .collect();
        if !rows.is_empty() {
            self.two_sided.reserve(device, meshes, &rows);
        }
        self.stats = TransparentStats {
            instances: self.records.len() as u32,
            draws: self.draws.len() as u32,
        };
        let mut grown = false;
        let bytes = self.records.len() as u64 * RECORD;
        if bytes > self.drawn.size() {
            self.drawn = list(device, self.records.len() as u64);
            self.held.clear();
            grown = true;
        }
        if !self.records.is_empty() && self.records != self.held {
            queue.write_buffer(&self.drawn, 0, bytemuck::cast_slice(&self.records));
            self.held.clone_from(&self.records);
        }
        grown
    }

    /// Makes the two-sided index lists this frame's draws need for the first time (before the
    /// opaque pass, in its command encoder).
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        meshes: &MeshPool,
    ) {
        self.two_sided.encode(device, enc, meshes);
    }

    /// The pipeline for the opaque pass's new format ([`pipeline`]).
    pub fn set_pipeline(&mut self, pipeline: wgpu::RenderPipeline) {
        self.pipeline = pipeline;
    }

    /// Draws the sorted instances after the sky (into the opaque pass, or the pass after GTAO):
    /// `frame` is the frame group over this stage's list. Returns the draw calls.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frame: &wgpu::BindGroup,
        lighting: &wgpu::BindGroup,
        textures: &wgpu::BindGroup,
        batches: &Batches,
        meshes: &MeshPool,
    ) -> u32 {
        self.draw_layer(pass, frame, lighting, textures, batches, meshes, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_layer(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frame: &wgpu::BindGroup,
        lighting: &wgpu::BindGroup,
        textures: &wgpu::BindGroup,
        batches: &Batches,
        meshes: &MeshPool,
        layer: Option<(&wgpu::RenderPipeline, &wgpu::BindGroup)>,
    ) -> u32 {
        if !self.any() {
            return 0;
        }
        pass.set_pipeline(layer.map_or(&self.pipeline, |(p, _)| p));
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(1, lighting, &[]);
        pass.set_bind_group(2, textures, &[]);
        if let Some((_, group)) = layer {
            pass.set_bind_group(3, group, &[]);
        } else {
            batches.bind_zero_base(pass);
        }
        pass.set_vertex_buffer(0, meshes.vertices.slice(..));
        let mut calls = 0;
        // Whether the pass's index buffer is the two-sided lists' (else the mesh pool's).
        let mut two_sided = None;
        for d in &self.draws {
            let Some(info) = meshes.infos.get(d.row as usize) else {
                continue;
            };
            // A double-sided draw whose list could not be made draws its front faces only.
            let start = self.two_sided.start(d.row).filter(|_| d.double_sided);
            if two_sided != Some(start.is_some()) {
                let indices = if start.is_some() {
                    &self.two_sided.indices
                } else {
                    &meshes.indices
                };
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                two_sided = Some(start.is_some());
            }
            let indices = match start {
                Some(first) => first..first + 2 * info.index_count,
                None => info.first_index..info.first_index + info.index_count,
            };
            pass.draw_indexed(indices, info.base_vertex, d.first..d.first + d.count);
            calls += 1;
        }
        calls
    }

    /// Draws the sorted instances into the entity-id pass with its pipeline (set by the caller,
    /// front faces), after the opaque ones: `frame` is the unjittered frame group over this stage's
    /// list.
    pub fn draw_ids(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frame: &wgpu::BindGroup,
        batches: &Batches,
        meshes: &MeshPool,
    ) {
        if !self.any() {
            return;
        }
        pass.set_bind_group(0, frame, &[]);
        batches.bind_zero_base(pass);
        for d in &self.draws {
            if let Some(info) = meshes.infos.get(d.row as usize) {
                pass.draw_indexed(
                    info.first_index..info.first_index + info.index_count,
                    info.base_vertex,
                    d.first..d.first + d.count,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlerp_matches_the_shader() {
        let a = Quat::from_rotation_y(0.3);
        let b = Quat::from_rotation_y(1.1);
        assert!(nlerp(a, b, 0.0).abs_diff_eq(a, 1e-6));
        assert!(nlerp(a, b, 1.0).abs_diff_eq(b, 1e-6));
        // The shorter way round: `-b` is the same rotation as `b`.
        assert!(nlerp(a, -b, 1.0).abs_diff_eq(b, 1e-6));
    }

    fn item(depth: f32, slot: u32, row: u32, double_sided: bool) -> Item {
        Item {
            depth,
            slot,
            row,
            double_sided,
            record: DrawnGpu {
                slot,
                ..DrawnGpu::zeroed()
            },
        }
    }

    #[test]
    fn instances_draw_back_to_front_and_neighbours_alike_share_a_draw() {
        let mut items = vec![
            item(2.0, 5, 0, false),
            item(9.0, 1, 0, false),
            item(4.0, 3, 1, true),
            item(4.0, 2, 1, true),
            item(3.0, 7, 0, true),
            item(7.0, 4, 0, false),
            item(1.0, 6, 0, false),
        ];
        let (mut records, mut draws) = (Vec::new(), Vec::new());
        order(&mut items, &mut records, &mut draws);
        // Farthest first; the two at depth 4 in slot order.
        let slots: Vec<u32> = records.iter().map(|r| r.slot).collect();
        assert_eq!(slots, [1, 4, 2, 3, 7, 5, 6]);
        let d = |row, first, count, double_sided| Draw {
            row,
            first,
            count,
            double_sided,
        };
        assert_eq!(
            draws,
            [
                d(0, 0, 2, false),
                d(1, 2, 2, true),
                d(0, 4, 1, true),
                d(0, 5, 2, false),
            ]
        );
    }

    #[test]
    fn two_sided_lists_grow_refill_or_wait_and_never_churn() {
        // Rows 1, 3 and 4 have lists of 6, 12 and 10 indices; row 2's (30) is larger than the
        // buffer may be.
        let size = |row: u32| [0, 6, 30, 12, 10][row as usize];
        let (limit, mut capacity) = (24, 16);
        let mut p = Placement::default();
        let reserve = |p: &mut Placement, wanted: &[u32], capacity: &mut u64| {
            let (grown, placed) = p.reserve(wanted, size, *capacity, limit);
            if let Some(c) = grown {
                *capacity = c;
            }
            (grown, placed)
        };
        assert_eq!(
            reserve(&mut p, &[1, 2], &mut capacity),
            (None, vec![(1, 0)])
        );
        assert!(p.too_large.contains(&2));
        // The same rows again: nothing to make, no new buffer (row 2 is not missing every frame).
        assert_eq!(reserve(&mut p, &[1, 2], &mut capacity), (None, vec![]));
        // Row 3 does not fit beside row 1: a larger buffer with this frame's lists.
        assert_eq!(
            reserve(&mut p, &[1, 3], &mut capacity),
            (Some(24), vec![(1, 0), (3, 6)])
        );
        // Row 4 fits neither beside them nor in a larger buffer: not made, frame after frame.
        assert_eq!(reserve(&mut p, &[1, 3, 4], &mut capacity), (None, vec![]));
        assert_eq!(reserve(&mut p, &[1, 3, 4], &mut capacity), (None, vec![]));
        assert_eq!(p.rows.len(), 2);
        // Without row 1 this frame's lists fit together: the full buffer is refilled with them.
        assert_eq!(
            reserve(&mut p, &[3, 4], &mut capacity),
            (None, vec![(3, 0), (4, 12)])
        );
        assert_eq!(reserve(&mut p, &[3, 4], &mut capacity), (None, vec![]));
        assert_eq!(capacity, 24);
    }

    #[test]
    fn a_replaced_row_gets_its_list_made_again() {
        let mut sizes = [0, 6, 12];
        let mut p = Placement::default();
        let size = |sizes: [u64; 3]| move |row: u32| sizes[row as usize];
        assert_eq!(
            p.reserve(&[1, 2], size(sizes), 64, 64),
            (None, vec![(1, 0), (2, 6)])
        );
        // Row 1's mesh is replaced by a larger one: its old list is stale.
        sizes[1] = 10;
        p.forget(&[1]);
        assert_eq!(
            p.reserve(&[1, 2], size(sizes), 64, 64),
            (None, vec![(1, 18)])
        );
        assert_eq!(p.reserve(&[1, 2], size(sizes), 64, 64), (None, vec![]));
    }

    #[test]
    fn the_index_reversal_shader_validates() {
        let source = crate::shaders::source("transparent_index");
        let module = naga::front::wgsl::parse_str(&source).expect("transparent_index.wgsl parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("transparent_index.wgsl validates");
    }
}
