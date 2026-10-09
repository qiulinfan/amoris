//! The indirect draws of the GPU-driven passes. A batch is one (view, variant, mesh): the culling
//! pass counts its visible instances into its arguments and writes them to the batch's region of
//! the view's visible list, so instance `k` of a batch is entry `base + k`, where `base` is the
//! view's region plus the batch's region in it. The vertex shaders read entry
//! `instance_index + batch.x` (forward.wgsl), and the base reaches them one of two ways:
//!
//! - in the arguments' `first_instance` (`batch.x` is 0). This needs WebGPU's optional
//!   `indirect-first-instance`: without it a nonzero `first_instance` makes the draw a no-op.
//!   Natively each view and variant is then one multi-draw over every mesh; in the browser, where
//!   wgpu's multi-draw is a loop of single draws, one draw per batch that holds instances;
//! - on WebGPU's baseline, `first_instance` is 0 and the base is a uniform at a dynamic offset
//!   (group 3) set before each batch's draw.
//!
//! The CPU's work per frame grows with the batches that hold instances, not with the instances.
//!
//! With occlusion culling (docs/spec/occlusion.md) the camera view has a second set of arguments,
//! [`LATE`]: the instances the late culling pass finds newly visible, written to the same regions
//! of the camera's list (the early draws are done with them by then), so a late batch has the
//! camera's base on every path.
//!
//! A "mesh" here is a row of the mesh table, so a mesh's levels of detail (docs/spec/lod.md) are
//! batches of their own, with regions sized by the mesh's instances: nothing in this file knows
//! about levels.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

use crate::gpu::Capabilities;
use crate::meshes::MeshInfo;
use crate::scene::VARIANTS;
use crate::shadows::CASCADES;

/// The camera and the shadow cascades.
pub const VIEWS: u32 = 1 + CASCADES as u32;
/// The argument set of the camera's late draws (occlusion culling's second phase), after the
/// views' sets (`LATE` in cull.wgsl).
pub const LATE: u32 = VIEWS;
/// Argument sets: the views' and the late one.
const SETS: u32 = VIEWS + 1;
/// Bytes of one set of indexed indirect arguments.
const ARGS: u64 = 20;
/// Bytes of the base uniform the shaders read (`vec4u`).
const BASE: u64 = 16;
/// The live arguments: counted into by the culling passes, drawn from, read back by checks.
const DRAWS_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE
    .union(wgpu::BufferUsages::INDIRECT)
    .union(wgpu::BufferUsages::COPY_SRC);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// How the passes issue their batches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawPath {
    /// `indirect-first-instance` and native multi-draw: one multi-draw per view and variant.
    MultiDraw,
    /// `indirect-first-instance` without native multi-draw (the browser): one draw per batch
    /// that holds instances, its base in the arguments.
    FirstInstance,
    /// WebGPU's baseline: one draw per batch that holds instances, `first_instance` 0, its base
    /// in a dynamic-offset uniform.
    Baseline,
}

impl DrawPath {
    pub fn for_caps(caps: &Capabilities) -> DrawPath {
        match (caps.indirect_first_instance, caps.multi_draw_indirect) {
            (true, true) => DrawPath::MultiDraw,
            (true, false) => DrawPath::FirstInstance,
            (false, _) => DrawPath::Baseline,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DrawPath::MultiDraw => "multi-draw",
            DrawPath::FirstInstance => "first-instance",
            DrawPath::Baseline => "baseline",
        }
    }
}

pub struct Batches {
    pub path: DrawPath,
    /// Group 3 of the forward, shadow and id pipelines: the batch's base (binding 1; binding 0 of
    /// group 3 is the ocean's, in its own pipeline).
    pub layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    bases: wgpu::Buffer,
    /// Bytes between bases (the device's uniform offset alignment).
    align: u64,
    /// The arguments each frame starts from (instance counts 0).
    template: wgpu::Buffer,
    /// The arguments the culling pass counts into and the passes draw.
    pub draws: wgpu::Buffer,
    meshes: u32,
    /// The meshes of the batches holding instances, variant by variant; variant `v`'s are
    /// `live[live_at[v]..live_at[v + 1]]`.
    live: Vec<u32>,
    live_at: [usize; VARIANTS as usize + 1],
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(64),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    bases: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("batch base"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: bases,
                offset: 0,
                size: NonZeroU64::new(BASE),
            }),
        }],
    })
}

/// The view whose list an argument set draws from: the late set draws the camera's.
fn view_of(set: u32) -> u32 {
    if set == LATE { 0 } else { set }
}

impl Batches {
    pub fn new(device: &wgpu::Device, caps: &Capabilities) -> Batches {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("batch base"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(BASE),
                },
                count: None,
            }],
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(BASE);
        let usage = wgpu::BufferUsages::UNIFORM;
        let bases = buffer(device, "batch bases", align, usage);
        Batches {
            path: DrawPath::for_caps(caps),
            group: bind(device, &layout, &bases),
            layout,
            bases,
            align,
            template: buffer(
                device,
                "draw template",
                64 * ARGS,
                wgpu::BufferUsages::COPY_SRC,
            ),
            draws: buffer(device, "draws", 64 * ARGS, DRAWS_USAGE),
            meshes: 0,
            live: Vec::new(),
            live_at: [0; VARIANTS as usize + 1],
        }
    }

    /// Rebuilds the arguments' template and the bases after the meshes or the instance counts
    /// changed: `offsets` and `sizes` are each batch's region and instance count in a view's list
    /// (variant-major: `variant * meshes + mesh`), `stride` a view's entries. Returns whether
    /// `draws` was replaced (bind groups holding it need rebuilding).
    pub fn rebuild(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        infos: &[MeshInfo],
        offsets: &[u32],
        sizes: &[u32],
        stride: u32,
    ) -> bool {
        let meshes = infos.len() as u32;
        self.meshes = meshes;
        self.live.clear();
        for variant in 0..VARIANTS {
            self.live_at[variant as usize] = self.live.len();
            for m in 0..meshes {
                if sizes
                    .get((variant * meshes + m) as usize)
                    .is_some_and(|&n| n > 0)
                {
                    self.live.push(m);
                }
            }
        }
        self.live_at[VARIANTS as usize] = self.live.len();
        let baseline = self.path == DrawPath::Baseline;
        let base = |v: u32, variant: u32, m: u32| {
            v * stride
                + offsets
                    .get((variant * meshes + m) as usize)
                    .copied()
                    .unwrap_or(0)
        };
        let mut template = Vec::with_capacity((meshes * SETS * VARIANTS) as usize);
        for set in 0..SETS {
            let v = view_of(set);
            for variant in 0..VARIANTS {
                for (m, info) in infos.iter().enumerate() {
                    template.push(DrawArgs {
                        index_count: info.index_count,
                        instance_count: 0,
                        first_index: info.first_index,
                        base_vertex: info.base_vertex,
                        first_instance: if baseline {
                            0
                        } else {
                            base(v, variant, m as u32)
                        },
                    });
                }
            }
        }
        let mut grown = false;
        let bytes = template.len() as u64 * ARGS;
        if bytes > self.draws.size() {
            let size = bytes.next_power_of_two();
            self.draws = buffer(device, "draws", size, DRAWS_USAGE);
            self.template = buffer(device, "draw template", size, wgpu::BufferUsages::COPY_SRC);
            grown = true;
        }
        if !template.is_empty() {
            queue.write_buffer(&self.template, 0, bytemuck::cast_slice(&template));
        }
        if baseline {
            // Base 0 first (unused here), then view by view the live batches' bases.
            let entries = 1 + u64::from(VIEWS) * self.live.len() as u64;
            let words = (self.align / 4) as usize;
            let mut data = vec![0u32; entries as usize * words];
            for v in 0..VIEWS {
                for variant in 0..VARIANTS {
                    let run = self.live_at[variant as usize]..self.live_at[variant as usize + 1];
                    for j in run {
                        let at = (1 + v as usize * self.live.len() + j) * words;
                        data[at] = base(v, variant, self.live[j]);
                    }
                }
            }
            let bytes = entries * self.align;
            if bytes > self.bases.size() {
                self.bases = buffer(
                    device,
                    "batch bases",
                    bytes.next_power_of_two(),
                    wgpu::BufferUsages::UNIFORM,
                );
                self.group = bind(device, &self.layout, &self.bases);
            }
            queue.write_buffer(&self.bases, 0, bytemuck::cast_slice(&data));
        }
        grown
    }

    /// The arguments the last submitted frame drew, as `[index_count, instance_count, first_index,
    /// base_vertex, first_instance]` per (set, variant, mesh row) in that order. Blocks until the
    /// GPU is done (checks and benchmarks only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_back(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<[u32; 5]> {
        let bytes = u64::from(self.meshes * SETS * VARIANTS) * ARGS;
        if bytes == 0 {
            return Vec::new();
        }
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draws readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&self.draws, 0, &buf, 0, bytes);
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mut out = Vec::new();
        if let Ok(Ok(())) = rx.recv()
            && let Ok(data) = slice.get_mapped_range()
        {
            let words: &[u32] = bytemuck::cast_slice(&data);
            out = words.as_chunks::<5>().0.to_vec();
        }
        buf.unmap();
        out
    }

    /// Mesh rows per argument set and variant (the readback's layout).
    pub fn rows(&self) -> u32 {
        self.meshes
    }

    /// Starts a frame's arguments from the template (the culling pass then counts into them).
    pub fn reset(&self, enc: &mut wgpu::CommandEncoder) {
        let bytes = u64::from(self.meshes * SETS * VARIANTS) * ARGS;
        if bytes > 0 {
            enc.copy_buffer_to_buffer(&self.template, 0, &self.draws, 0, bytes);
        }
    }

    /// Draws one variant's batches of one argument set (a view, or [`LATE`]) with the pass's
    /// current pipeline; returns the number of draw calls issued.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, set: u32, variant: u32) -> u32 {
        if self.meshes == 0 {
            return 0;
        }
        let view = view_of(set);
        let first = u64::from((set * VARIANTS + variant) * self.meshes) * ARGS;
        let run = self.live_at[variant as usize]..self.live_at[variant as usize + 1];
        match self.path {
            DrawPath::MultiDraw => {
                pass.set_bind_group(3, &self.group, &[0]);
                pass.multi_draw_indexed_indirect(&self.draws, first, self.meshes);
                1
            }
            DrawPath::FirstInstance => {
                if run.is_empty() {
                    return 0;
                }
                pass.set_bind_group(3, &self.group, &[0]);
                for &m in &self.live[run.clone()] {
                    pass.draw_indexed_indirect(&self.draws, first + u64::from(m) * ARGS);
                }
                run.len() as u32
            }
            DrawPath::Baseline => {
                let row = 1 + view as usize * self.live.len();
                for j in run.clone() {
                    let offset = (row + j) as u64 * self.align;
                    pass.set_bind_group(3, &self.group, &[offset as u32]);
                    pass.draw_indexed_indirect(&self.draws, first + u64::from(self.live[j]) * ARGS);
                }
                run.len() as u32
            }
        }
    }
}
