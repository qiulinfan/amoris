//! Depth-integrated non-hydrostatic pressure for the resident field (water.md 55), after
//! Yamazaki, Kowalik and Cheung, "Depth-integrated, non-hydrostatic model for wave breaking and
//! run-up", International Journal for Numerical Methods in Fluids 61(5), 2009. In every substep,
//! between the MacCormack faces and the donors, a fixed number of Chebyshev iterations solve the
//! column pressure, whose force is the transpose of the discrete continuity so that it does no
//! work, and correct the faces the transport then carries. A dispatch runs `BLOCK` of the
//! iterations on a tile in its workgroup's memory (temporal blocking). Shader: `water_nh.wgsl`.

use amoris_assets::water_field::FieldBoundary;
use bytemuck::{Pod, Zeroable};

use crate::profiler::GpuProfiler;

/// Chebyshev iterations in a cycle. A substep runs two identical cycles from the previous
/// substep's pressure (from zero at a tick's first), so every mode's error is multiplied by the
/// square of the cycle's residual polynomial, which is never negative: the pressure then lies
/// between the hydrostatic and the exact one, and the field stays stable however loosely the
/// system is solved. One cycle's polynomial changes sign across the spectrum, and where it is
/// negative on a short wave in deep water, which the exact pressure all but holds still, the
/// remaining error reverses gravity on it: a basin 8.8 m deep at half-metre columns broke apart
/// within 4 s at 64 iterations and held at 96. Two cycles of 24 keep waves with kh up to 2 within
/// a percent of their phase speed there.
pub const CYCLE: u32 = 24;
const _: () = assert!(CYCLE <= 64, "water_nh.wgsl keeps 64 coefficients");
pub const ITERATIONS: u32 = 2 * CYCLE;
const _: () = assert!(
    ITERATIONS.is_multiple_of(2),
    "the last iteration writes the first pair"
);
/// Iterations a dispatch runs on a tile of `TILE` x `TILE` columns in its workgroup's memory
/// (water_nh.wgsl `nh_block`, whose constants these are).
const BLOCK: u32 = 4;
const TILE: u32 = 24;
const _: () = assert!(
    CYCLE.is_multiple_of(BLOCK) && (ITERATIONS / BLOCK).is_multiple_of(2),
    "a dispatch stays in one cycle, and the last writes the first pair"
);
const SLOT: u64 = 256;
/// Run the iterations one dispatch each (`nh_iterate`), the plain definition the blocked solve
/// matches bit for bit: a test switches it on to check that. It changes no result, only the time.
pub static REFERENCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    size: [u32; 2],
    cell: f32,
    dt: f32,
    open: [u32; 4],
    breaking: f32,
    limit: f32,
    unused: [f32; 2],
}

pub(crate) struct WaterNh {
    queue: wgpu::Queue,
    columns: u64,
    params: wgpu::Buffer,
    bounds: wgpu::Buffer,
    /// The pressure pair the system starts from, which the last iteration writes again.
    first: wgpu::Buffer,
    system: wgpu::ComputePipeline,
    spectrum: wgpu::ComputePipeline,
    chebyshev: wgpu::ComputePipeline,
    iterate: wgpu::ComputePipeline,
    block: wgpu::ComputePipeline,
    correct: wgpu::ComputePipeline,
    /// Per state buffer: from the first pressure pair into the second, and back.
    groups: [[wgpu::BindGroup; 2]; 2],
}

impl WaterNh {
    /// `state` are the field's two buffers, `velocity` the MacCormack faces and `awake` the
    /// active columns (water_compute.rs), all at least `columns` large.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        state: [&wgpu::Buffer; 2],
        velocity: &wgpu::Buffer,
        awake: &wgpu::Buffer,
        columns: u64,
    ) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let params = buffer("water nh params", 48, uniform);
        let iterations = buffer("water nh iterations", SLOT * u64::from(CYCLE), uniform);
        let mut slots = vec![0u8; (SLOT * u64::from(CYCLE)) as usize];
        for k in 0..CYCLE {
            let at = (SLOT * u64::from(k)) as usize;
            slots[at..at + 4].copy_from_slice(&k.to_le_bytes());
        }
        queue.write_buffer(&iterations, 0, &slots);
        let system = buffer("water nh system", columns * 16, storage);
        let pressure = [0, 1].map(|_| {
            buffer(
                "water nh pressure",
                columns * 8,
                storage | wgpu::BufferUsages::COPY_DST,
            )
        });
        let bounds = buffer(
            "water nh spectrum bounds",
            16,
            storage | wgpu::BufferUsages::COPY_DST,
        );
        let chebyshev = buffer("water nh chebyshev", 64 * 8, storage);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water nh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_nh.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water nh"),
            entries: &(0..10u32)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if matches!(binding, 0 | 8) {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: matches!(binding, 1 | 7),
                            }
                        },
                        has_dynamic_offset: binding == 8,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect::<Vec<_>>(),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water nh"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        };
        let group = |s: &wgpu::Buffer, input: usize| {
            let entries: [(u32, wgpu::BindingResource<'_>); 10] = [
                (0, params.as_entire_binding()),
                (1, s.as_entire_binding()),
                (2, system.as_entire_binding()),
                (3, pressure[input].as_entire_binding()),
                (4, pressure[1 - input].as_entire_binding()),
                (5, velocity.as_entire_binding()),
                (6, bounds.as_entire_binding()),
                (7, awake.as_entire_binding()),
                (
                    8,
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &iterations,
                        offset: 0,
                        size: wgpu::BufferSize::new(16),
                    }),
                ),
                (9, chebyshev.as_entire_binding()),
            ];
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water nh"),
                layout: &layout,
                entries: &entries
                    .into_iter()
                    .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource })
                    .collect::<Vec<_>>(),
            })
        };
        let groups = state.map(|s| [group(s, 0), group(s, 1)]);
        let [first, _] = pressure;
        Self {
            queue: queue.clone(),
            columns,
            params,
            bounds,
            first,
            system: pipeline("nh_system"),
            spectrum: pipeline("nh_bounds"),
            chebyshev: pipeline("nh_chebyshev"),
            iterate: pipeline("nh_iterate"),
            block: pipeline("nh_block"),
            correct: pipeline("nh_correct"),
            groups,
        }
    }

    pub fn fits(&self, size: [u32; 2]) -> bool {
        u64::from(size[0]) * u64::from(size[1]) <= self.columns
    }

    /// Write this step's parameters and start its pressure from zero. Call once per submission,
    /// before the substeps.
    pub fn prepare(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        cell: f32,
        substep_dt: f32,
        edges: &[FieldBoundary; 4],
        breaking: bool,
    ) -> Result<(), String> {
        if !self.fits(size) {
            return Err("non-hydrostatic water exceeds its allocated capacity".into());
        }
        let params = Params {
            size,
            cell,
            dt: substep_dt,
            open: std::array::from_fn(|i| u32::from(edges[i] != FieldBoundary::Wall)),
            breaking: if breaking { 1.0 } else { 0.0 },
            limit: 0.5 * cell / substep_dt,
            unused: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        encoder.clear_buffer(&self.first, 0, None);
        Ok(())
    }

    /// One substep's solve and correction of the faces `mac_faces` wrote, from `state[current]`,
    /// the substep's source, in one pass.
    pub fn encode_solve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        current: usize,
        profiler: Option<&mut GpuProfiler>,
    ) {
        encoder.clear_buffer(&self.bounds, 0, None);
        // Each dispatch is a WebGPU usage scope (spec 3.4.4); wgpu inserts the storage barriers
        // between these dependent dispatches within the one pass (as water_summary.rs does).
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water nh"),
            timestamp_writes: profiler.and_then(|p| p.compute_scope("water nh")),
        });
        let columns = [size[0].div_ceil(8), size[1].div_ceil(8)];
        let mut dispatch =
            |pipeline: &wgpu::ComputePipeline, group, offset, workgroups: [u32; 2]| {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.groups[current][group], &[offset as u32]);
                pass.dispatch_workgroups(workgroups[0], workgroups[1], 1);
            };
        dispatch(&self.system, 0, 0, columns);
        dispatch(&self.spectrum, 0, 0, columns);
        dispatch(&self.chebyshev, 0, 0, [1, 1]);
        if REFERENCE.load(std::sync::atomic::Ordering::Relaxed) {
            for k in 0..ITERATIONS {
                let slot = SLOT * u64::from(k % CYCLE);
                dispatch(&self.iterate, (k % 2) as usize, slot, columns);
            }
        } else {
            let tiles = [size[0].div_ceil(TILE), size[1].div_ceil(TILE)];
            for b in 0..ITERATIONS / BLOCK {
                let slot = SLOT * u64::from(b * BLOCK % CYCLE);
                dispatch(&self.block, (b % 2) as usize, slot, tiles);
            }
        }
        dispatch(&self.correct, 0, 0, columns);
    }
}
