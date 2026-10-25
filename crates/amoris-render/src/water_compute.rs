//! GPU shallow-water substeps. The core keeps its field on the GPU between dispatches; the native
//! game adapter uploads and reads it once per tick to preserve same-tick queries and snapshots.
//! f32, three classic or five bounded-MacCormack dispatches per substep. No atomics or optional
//! WebGPU features. Modern scratch/pipelines are created lazily, within the baseline storage limit.

use amoris_assets::water_field::{
    CheckedCheckpoint, FieldBoundary, FieldCheckpoint, FieldExterior, FieldPhysics, FieldSea,
    FieldStep, FieldWind, MAX_SEA_WAVES, PointVolume, field_f32,
};
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WaveParams {
    phase: [f32; 4],
    data: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    size: [u32; 2],
    cell: f32,
    dt: f32,
    friction: f32,
    breaking: f32,
    fall_min: f32,
    occupancy_on: f32,
    calm: f32,
    /// How the resident kernel transports its face velocities (water.md 61.1).
    advection: f32,
    /// 1: plunging breakers (water.md 63.1).
    plunge: f32,
    /// WGSL aligns `edge_kind` to 16 bytes.
    _pad: [f32; 1],
    edge_kind: [u32; 4],
    edge_rate: [f32; 4],
    edge_normal: [f32; 4],
    sea_meta: [f32; 4],
    waves: [WaveParams; MAX_SEA_WAVES],
    /// The wind 10 m up along x and z, and 1 when it blows (water.md 67).
    wind: [f32; 4],
    /// The gusts' share, their phase at the corner, rad, and rad per metre downwind.
    gust: [f32; 4],
    /// Depth rain adds to every column each substep, m.
    rain: [f32; 4],
}

/// How the resident kernel transports its face velocities (water.md 61.1). Regions always run
/// `Hybrid`; the others are there to compare the schemes (the `water_swash` example).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Advection {
    /// Bounded MacCormack everywhere (water.md 26).
    MacCormack,
    /// Stelling and Duinmeijer's momentum-conserving upwind form everywhere.
    Conservative,
    /// The momentum-conserving form where the flow varies rapidly (thin water and steep steps),
    /// MacCormack elsewhere.
    #[default]
    Hybrid,
}

/// A reusable GPU field. State is row-major [bed, depth, -x velocity, -z velocity]. Outer faces
/// are walls. Upload all inputs after a restore or a fork; no hidden simulation history is used.
pub struct GpuWater {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: [u32; 2],
    capacity: u64,
    initialized: bool,
    state: [wgpu::Buffer; 2],
    active: wgpu::Buffer,
    params: wgpu::Buffer,
    groups: [wgpu::BindGroup; 2],
    pipelines: [wgpu::ComputePipeline; 3],
    velocity: wgpu::Buffer,
    scale: wgpu::Buffer,
    maccormack: Option<MacCormack>,
    occupancy: crate::water_occupancy::WaterOccupancy,
    falls: wgpu::Buffer,
    waves: Option<crate::water_waves::WaterWaves>,
    nh: Option<crate::water_nh::WaterNh>,
    edge_flux: wgpu::Buffer,
    boundaries: Option<BoundaryPipelines>,
    has_boundary_report: bool,
    /// The last step had a Joined side: its edge columns can be exported.
    joined: bool,
    current: usize,
    readback: wgpu::Buffer,
    advection: Advection,
}

/// The faces of the boundary flux buffer's first segment: every side of a 512² field.
const JOIN_IN: u64 = 2 * (512 + 512);

/// Three bind layouts keep each stage within WebGPU's eight storage buffers: advection, face
/// pressure (with the occupancy head) and depth transport (with ballistic falls).
struct MacCormack {
    layouts: [wgpu::BindGroupLayout; 3],
    capacity: u64,
    groups: [[wgpu::BindGroup; 2]; 3],
    pipelines: [wgpu::ComputePipeline; 7],
}
/// The bindings of each MacCormack layout, by stage family.
const MAC_BINDINGS: [&[u32]; 3] = [
    &[0, 1, 3, 7, 8],
    &[0, 1, 3, 4, 7, 8, 9],
    &[0, 1, 2, 3, 4, 5, 6, 10],
];
/// Pipelines and their layout family: forward, reverse, faces, donors, transport, landing.
const MAC_STAGES: [(&str, usize, &str); 7] = [
    ("mac_forward", 0, "water advect forward"),
    ("mac_reverse", 0, "water advect reverse"),
    ("mac_faces", 1, "water faces"),
    ("donors", 2, "water donors"),
    ("transport", 2, "water transport"),
    ("fall_land_depth", 2, "water falls land"),
    ("fall_land_faces", 2, "water falls momentum"),
];
struct BoundaryPipelines {
    groups: [wgpu::BindGroup; 2],
    pipelines: [wgpu::ComputePipeline; 4],
    report: wgpu::Buffer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundaryReport {
    pub entered: f32,
    pub left: f32,
    pub edited: f32,
}
impl BoundaryReport {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 16 {
            return Err("water boundary receipt must have 16 bytes".into());
        }
        let v: [f32; 4] = bytemuck::pod_read_unaligned(bytes);
        if v.iter().any(|x| !x.is_finite())
            || v[0] < 0.0
            || v[1] < 0.0
            || v[2] != 0.0
            || v[3] != 0.0
        {
            return Err("water boundary receipt is invalid".into());
        }
        Ok(Self {
            entered: v[0],
            left: v[1],
            edited: 0.0,
        })
    }
}

impl GpuWater {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: [u32; 2]) -> Result<Self, String> {
        if size.iter().any(|n| !(2..=512).contains(n)) {
            return Err("GPU water window dimensions must be 2..=512".into());
        }
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 16;
        let state_bytes = bytes + 4096; // Maximum packed east/north tail at 512².
        if state_bytes > device.limits().max_storage_buffer_binding_size {
            return Err("GPU water field exceeds the device's storage binding limit".into());
        }
        let buffer = |label, bytes, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let state = std::array::from_fn(|_| {
            buffer(
                "water state",
                state_bytes,
                storage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            )
        });
        let active = buffer(
            "water active",
            bytes / 4,
            storage | wgpu::BufferUsages::COPY_DST,
        );
        let velocity = buffer("water faces", bytes / 2, storage);
        let scale = buffer("water donors", bytes / 4, storage);
        let falls = buffer(
            "water falls",
            bytes / 16 * 12,
            storage | wgpu::BufferUsages::COPY_DST,
        );
        let occupancy = crate::water_occupancy::WaterOccupancy::new(device, queue, bytes / 16);
        // Fluxes, then joined neighbours' columns, then this field's own edge columns
        // (water_boundaries.wgsl).
        let edge_flux = buffer(
            "water boundary flux",
            3 * JOIN_IN * 16,
            storage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let params = buffer(
            "water params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water compute"),
            entries: &std::array::from_fn::<_, 7, _>(|b| wgpu::BindGroupLayoutEntry {
                binding: b as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if b == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: b == 1 || b == 3,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water compute"),
                layout: &layout,
                entries: &[
                    &params,
                    &state[i],
                    &state[1 - i],
                    &active,
                    &velocity,
                    &scale,
                    &edge_flux,
                ]
                .iter()
                .enumerate()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water compute"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water compute"),
            // The deterministic kernels generated from water_compute.wgsl and
            // water_depth_first_order.wgsl (tests/water_det.rs, water.md 66).
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/det/water_compute.wgsl").into(),
            ),
        });
        let pipelines = ["faces", "depth_first_donors", "depth_first_transport"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            size,
            capacity: bytes,
            initialized: false,
            state,
            active,
            params,
            groups,
            pipelines,
            velocity,
            scale,
            maccormack: None,
            occupancy,
            falls,
            waves: None,
            nh: None,
            edge_flux,
            boundaries: None,
            has_boundary_report: false,
            joined: false,
            current: 0,
            readback: buffer(
                "water readback",
                state_bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            advection: Advection::default(),
        })
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Transports the resident kernel's face velocities by another scheme, to compare them.
    pub fn set_advection(&mut self, advection: Advection) {
        self.advection = advection;
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Change the window without reallocating buffers or recompiling pipelines.
    pub fn set_size(&mut self, size: [u32; 2]) -> Result<(), String> {
        if size.iter().any(|n| !(2..=512).contains(n))
            || u64::from(size[0]) * u64::from(size[1]) * 16 > self.capacity
        {
            return Err("GPU water window exceeds its capacity".into());
        }
        if self.size != size {
            self.initialized = false;
            self.has_boundary_report = false;
            self.joined = false;
        }
        self.size = size;
        Ok(())
    }

    /// Initializes or replaces the entire field. Heights may be relative to a common datum to
    /// avoid losing small surface slopes at large world coordinates.
    pub fn upload(&mut self, columns: &[[f32; 4]], active: &[u32]) -> Result<(), String> {
        self.upload_inner(columns, active, None)
    }

    /// Exact complete state, including all outside faces. Boundary controls belong to steps;
    /// loading a state must not replace its existing physical velocities with implicit walls.
    pub fn upload_checkpoint(&mut self, checkpoint: &FieldCheckpoint) -> Result<(), String> {
        checkpoint.validate()?;
        self.write_complete_checkpoint(checkpoint)
    }
    /// A sealed payload already proved finite complete data and has no mutable accessors.
    pub fn upload_checked_checkpoint(
        &mut self,
        checkpoint: &CheckedCheckpoint,
    ) -> Result<(), String> {
        self.write_complete_checkpoint(checkpoint.as_raw())
    }
    fn write_complete_checkpoint(&mut self, checkpoint: &FieldCheckpoint) -> Result<(), String> {
        if checkpoint.size != self.size {
            return Err("water checkpoint size differs from GPU window".into());
        }
        self.initialized = false;
        self.has_boundary_report = false;
        self.joined = false;
        self.current = 0;
        // u32 words are the exact f32 storage bits; no conversion vector or masked-face scan
        // is necessary for a validated complete checkpoint whose columns are all active.
        self.queue
            .write_buffer(&self.state[0], 0, bytemuck::cast_slice(&checkpoint.columns));
        let tail = checkpoint.packed_faces();
        self.queue.write_buffer(
            &self.state[0],
            checkpoint.columns.len() as u64 * 16,
            bytemuck::cast_slice(&tail),
        );
        self.queue.write_buffer(
            &self.state[1],
            checkpoint.columns.len() as u64 * 16,
            bytemuck::cast_slice(&vec![0u32; tail.len()]),
        );
        self.queue.write_buffer(
            &self.active,
            0,
            bytemuck::cast_slice(&vec![1u32; checkpoint.columns.len()]),
        );
        self.occupancy.reset();
        self.initialized = true;
        Ok(())
    }
    fn upload_inner(
        &mut self,
        columns: &[[f32; 4]],
        active: &[u32],
        far: Option<&[u32]>,
    ) -> Result<(), String> {
        self.initialized = false;
        self.has_boundary_report = false;
        self.joined = false;
        let n = (self.size[0] * self.size[1]) as usize;
        if columns.len() != n
            || active.len() != n
            || columns
                .iter()
                .any(|c| !c.iter().all(|v| v.is_finite()) || c[1] < 0.0)
            || active.iter().any(|&a| a > 1)
        {
            return Err("invalid GPU water field or active mask".into());
        }
        // Reject rather than discard input that crosses a wall or an inactive boundary.
        if columns.iter().enumerate().any(|(n, c)| {
            (far.is_none() && n % self.size[0] as usize == 0 && c[2] != 0.0)
                || (far.is_none() && n < self.size[0] as usize && c[3] != 0.0)
                || (n % self.size[0] as usize > 0 && active[n] != active[n - 1] && c[2] != 0.0)
                || (n >= self.size[0] as usize
                    && active[n] != active[n - self.size[0] as usize]
                    && c[3] != 0.0)
        }) {
            return Err("GPU water outer and inactive boundary faces must be walls".into());
        }
        self.current = 0;
        self.initialized = true;
        self.queue
            .write_buffer(&self.state[0], 0, bytemuck::cast_slice(columns));
        let mut tail = far.map_or_else(
            || vec![0; (self.size[0] + self.size[1]) as usize],
            <[u32]>::to_vec,
        );
        tail.resize(tail.len().div_ceil(4) * 4, 0);
        self.queue
            .write_buffer(&self.state[0], n as u64 * 16, bytemuck::cast_slice(&tail));
        self.queue.write_buffer(
            &self.state[1],
            n as u64 * 16,
            bytemuck::cast_slice(&vec![0u32; tail.len()]),
        );
        self.queue
            .write_buffer(&self.active, 0, bytemuck::cast_slice(active));
        self.occupancy.reset();
        Ok(())
    }

    /// Encode one tick. Submit before changing these parameters again: uniform writes are queue
    /// operations, not encoder commands. All substeps stay on the GPU with no CPU fence between.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
    ) -> Result<(), String> {
        self.encode_profiled(encoder, cell, dt, substeps, friction, None)
    }

    /// `encode` with the region's calm damping, per second (water.md 4).
    pub fn encode_damped(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        damping: [f32; 2],
    ) -> Result<(), String> {
        self.encode_inner(
            encoder,
            cell,
            dt,
            substeps,
            damping,
            false,
            &[FieldBoundary::Wall; 4],
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            None,
        )
    }

    /// The same work with pass timestamps, following the renderer's deferred-resolve lifecycle.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_profiled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.encode_inner(
            encoder,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
            false,
            &[FieldBoundary::Wall; 4],
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            profiler,
        )
    }

    /// Bounded predictor/reverse/correction velocity advection, then the same pressure,
    /// donor limit and conservative height transport. All intermediate fields remain on GPU.
    pub fn encode_maccormack(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
    ) -> Result<(), String> {
        self.encode_maccormack_profiled(encoder, cell, dt, substeps, friction, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_maccormack_profiled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.encode_inner(
            encoder,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
            true,
            &[FieldBoundary::Wall; 4],
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            profiler,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn encode_boundaries(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
        edges: &[FieldBoundary; 4],
    ) -> Result<(), String> {
        self.encode_inner(
            encoder,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
            true,
            edges,
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_boundaries_profiled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
        edges: &[FieldBoundary; 4],
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.encode_inner(
            encoder,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
            true,
            edges,
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            profiler,
        )
    }

    /// Transport part of a validated resident step; edits and impulses precede this call.
    pub fn encode_transport(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        step: &FieldStep,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.has_boundary_report = false;
        step.validate((self.size[0] * self.size[1]) as usize)?;
        self.encode_inner(
            encoder,
            field_f32(step.cell_m),
            field_f32(step.substep_dt),
            step.substeps,
            [field_f32(step.friction), field_f32(step.calm_per_s)],
            true,
            &step.boundaries,
            step.sea.as_ref(),
            step.physics,
            &step.displacements,
            &step.exterior,
            step.wind.as_ref(),
            step.rain_m,
            profiler,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_inner(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        cell: f32,
        dt: f32,
        substeps: u32,
        damping: [f32; 2],
        maccormack: bool,
        edges: &[FieldBoundary; 4],
        sea: Option<&FieldSea>,
        physics: FieldPhysics,
        displacements: &[PointVolume],
        exterior: &[FieldExterior],
        wind: Option<&FieldWind>,
        rain_m: f64,
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.has_boundary_report = false;
        self.joined = false;
        if !self.initialized {
            return Err("GPU water has not been uploaded".into());
        }
        if !cell.is_finite()
            || cell <= 0.0
            || !dt.is_finite()
            || dt <= 0.0
            || damping.iter().any(|v| !v.is_finite() || *v < 0.0)
            || !(1..=8).contains(&substeps)
        {
            return Err("invalid GPU water timestep, cell, damping or substep count".into());
        }
        if edges.contains(&FieldBoundary::Sea) != sea.is_some() {
            return Err("sea boundary needs its recorded descriptor".into());
        }
        if let Some(sea) = sea {
            sea.validate()?;
        }
        physics.validate()?;
        if !maccormack && (physics != FieldPhysics::default() || !displacements.is_empty()) {
            return Err("water physics extensions need the MacCormack resident scheme".into());
        }
        if maccormack {
            self.init_maccormack();
        }
        let occupied = self.occupancy.encode(
            encoder,
            self.size,
            cell,
            displacements,
            profiler.as_deref_mut(),
        )?;
        // A plunging crest lands as a fall does.
        let falls = physics.fall_min_m > 0.0 || physics.plunging;
        let open = edges.iter().any(|e| *e != FieldBoundary::Wall);
        let sea_clock = sea.is_some_and(|s| !s.waves.is_empty());
        self.has_boundary_report = open;
        let mut kinds = [0; 4];
        let mut rates = [0.0; 4];
        let mut normals = [0.0; 4];
        for (i, edge) in edges.iter().copied().enumerate() {
            edge.validate()?;
            match edge {
                FieldBoundary::Wall => {}
                FieldBoundary::Sea => kinds[i] = 5,
                FieldBoundary::Radiation {
                    surface_relative_m: h,
                    outward_velocity_mps: u,
                } => {
                    kinds[i] = 4;
                    rates[i] = field_f32(h);
                    normals[i] = field_f32(u);
                }
                FieldBoundary::Outflow => kinds[i] = 2,
                FieldBoundary::Stage {
                    surface_relative_m: h,
                } => {
                    kinds[i] = 3;
                    rates[i] = field_f32(h);
                }
                FieldBoundary::Inflow { m3s_per_m: q } => {
                    kinds[i] = 1;
                    rates[i] = field_f32(q);
                }
                FieldBoundary::Joined => kinds[i] = 6,
            }
        }
        self.joined = edges.contains(&FieldBoundary::Joined);
        if self.joined {
            // Each face's seam; a face no exterior covers stays a wall.
            let [nx, nz] = self.size.map(|n| n as usize);
            let mut faces = vec![[0.0f32; 4]; 2 * (nx + nz)];
            for e in exterior {
                let side = usize::from(e.side);
                let (start, len) = [(0, nz), (nz, nz), (2 * nz, nx), (2 * nz + nx, nx)][side];
                if edges[side] != FieldBoundary::Joined || e.first as usize + e.faces.len() > len {
                    return Err("joined exterior does not fit its side".into());
                }
                for (j, c) in e.faces.iter().enumerate() {
                    faces[start + e.first as usize + j] = [c[0], c[1], 0.0, 1.0];
                }
            }
            self.queue
                .write_buffer(&self.edge_flux, JOIN_IN * 16, bytemuck::cast_slice(&faces));
        } else if !exterior.is_empty() {
            return Err("a joined exterior needs a Joined side".into());
        }
        if open {
            if self.size.iter().any(|n| n % 4 != 0) {
                return Err("open water windows must have dimensions divisible by four".into());
            }
            self.init_boundaries();
            // The fluxes only: queued exterior columns are written before this submission runs.
            encoder.clear_buffer(&self.edge_flux, 0, Some(JOIN_IN * 16));
            encoder.clear_buffer(
                &self
                    .boundaries
                    .as_ref()
                    .expect("initialized boundaries")
                    .report,
                0,
                None,
            );
        }
        self.queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                size: self.size,
                cell,
                dt,
                friction: damping[0],
                breaking: if physics.breaking { 1.0 } else { 0.0 },
                fall_min: field_f32(physics.fall_min_m),
                occupancy_on: if occupied { 1.0 } else { 0.0 },
                calm: damping[1],
                advection: match self.advection {
                    Advection::MacCormack => 0.0,
                    Advection::Conservative => 1.0,
                    Advection::Hybrid => 2.0,
                },
                plunge: if physics.plunging { 1.0 } else { 0.0 },
                _pad: [0.0; 1],
                edge_kind: kinds,
                edge_rate: rates,
                edge_normal: normals,
                sea_meta: [
                    sea.map_or(0.0, |s| field_f32(s.level_relative_m)),
                    sea.map_or(0.0, |s| s.waves.len() as f32),
                    if physics.dispersion { 1.0 } else { 0.0 },
                    0.0,
                ],
                waves: std::array::from_fn(|i| {
                    sea.and_then(|s| s.waves.get(i))
                        .map_or(WaveParams::zeroed(), |w| WaveParams {
                            phase: [
                                field_f32(w.wave_vector_rad_m[0]),
                                field_f32(w.wave_vector_rad_m[1]),
                                field_f32(w.angular_frequency_rad_s),
                                field_f32(w.phase_at_start_rad),
                            ],
                            data: [field_f32(w.amplitude_m), 0.0, 0.0, 0.0],
                        })
                }),
                wind: wind.map_or([0.0; 4], |w| {
                    [
                        field_f32(w.velocity_mps[0]),
                        field_f32(w.velocity_mps[1]),
                        0.0,
                        1.0,
                    ]
                }),
                gust: wind.map_or([0.0; 4], |w| {
                    [
                        field_f32(w.gust),
                        field_f32(w.gust_phase_rad),
                        field_f32(w.gust_rad_per_m),
                        0.0,
                    ]
                }),
                rain: [field_f32(rain_m / f64::from(substeps)), 0.0, 0.0, 0.0],
            }),
        );
        if physics.dispersion {
            if self.waves.as_ref().is_none_or(|w| !w.fits(self.size)) {
                self.waves = Some(crate::water_waves::WaterWaves::new(
                    &self.device,
                    &self.queue,
                    [&self.state[0], &self.state[1]],
                    self.capacity / 16,
                ));
            }
            let waves = self.waves.as_ref().expect("dispersive waves");
            waves.prepare(
                self.size,
                cell,
                dt * substeps as f32,
                dt,
                edges,
                physics.breaking,
            )?;
            waves.encode_split(encoder, self.size, self.current, profiler.as_deref_mut());
        }
        if physics.nonhydrostatic {
            if !maccormack {
                return Err("non-hydrostatic water needs the MacCormack resident scheme".into());
            }
            if self.nh.is_none() {
                self.nh = Some(crate::water_nh::WaterNh::new(
                    &self.device,
                    &self.queue,
                    [&self.state[0], &self.state[1]],
                    &self.velocity,
                    &self.active,
                    self.capacity / 16,
                ));
            }
            let nh = self.nh.as_ref().expect("non-hydrostatic pressure");
            nh.prepare(encoder, self.size, cell, dt, edges, physics.breaking)?;
        }
        // A new pass for each stage makes all storage writes visible to its consumers, on every
        // native backend and WebGPU. No workgroup barrier can synchronize different workgroups.
        for _ in 0..substeps {
            // Current encode APIs have closed boundary controls. Explicitly close the packed
            // output faces as well; a future open step must write its own complete face tail.
            if !open {
                encoder.clear_buffer(
                    &self.state[1 - self.current],
                    u64::from(self.size[0]) * u64::from(self.size[1]) * 16,
                    Some(u64::from(self.size[0] + self.size[1]).div_ceil(4) * 16),
                );
            }
            let stages = if !maccormack {
                3
            } else if falls {
                7
            } else {
                5
            };
            for stage in 0..stages {
                if physics.nonhydrostatic && stage == 3 {
                    // The faces' non-hydrostatic correction, which the boundaries, the donors and
                    // the transport then carry (water_nh.rs).
                    let nh = self.nh.as_ref().expect("non-hydrostatic pressure");
                    nh.encode_solve(encoder, self.size, self.current, profiler.as_deref_mut());
                }
                if open && stage == if maccormack { 3 } else { 1 } {
                    self.boundary_dispatch(encoder, 0, false, profiler.as_deref_mut());
                }
                if falls && stage == 4 {
                    encoder.clear_buffer(
                        &self.falls,
                        0,
                        Some(u64::from(self.size[0] * self.size[1]) * 12),
                    );
                }
                let (pipeline, group, label) = if maccormack {
                    let mac = self.maccormack.as_ref().expect("MacCormack pipelines");
                    let (_, family, label) = MAC_STAGES[stage];
                    (
                        &mac.pipelines[stage],
                        &mac.groups[family][self.current],
                        label,
                    )
                } else {
                    (
                        &self.pipelines[stage],
                        &self.groups[self.current],
                        ["water faces", "water donors", "water transport"][stage],
                    )
                };
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("water compute"),
                    timestamp_writes: profiler.as_deref_mut().and_then(|p| p.compute_scope(label)),
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(self.size[0].div_ceil(8), self.size[1].div_ceil(8), 1);
                drop(pass);
                if open && stage == if maccormack { 3 } else { 1 } {
                    self.boundary_dispatch(encoder, 1, false, profiler.as_deref_mut());
                }
            }
            self.current = 1 - self.current;
            if sea_clock {
                self.boundary_dispatch(encoder, 2, true, profiler.as_deref_mut());
            }
        }
        if open && !sea_clock {
            self.boundary_dispatch(encoder, 2, true, profiler.as_deref_mut());
        }
        if physics.dispersion {
            let waves = self.waves.as_ref().expect("dispersive waves");
            waves.encode_surface(encoder, self.size, self.current, profiler);
        }
        Ok(())
    }

    fn init_maccormack(&mut self) {
        let bytes = u64::from(self.size[0]) * u64::from(self.size[1]) * 8;
        if let Some(mac) = &self.maccormack {
            if mac.capacity >= bytes {
                return;
            }
            let groups = self.maccormack_groups(&mac.layouts, bytes);
            let mac = self
                .maccormack
                .as_mut()
                .expect("existing MacCormack pipelines");
            mac.groups = groups;
            mac.capacity = bytes;
            return;
        }
        let layouts = MAC_BINDINGS.map(|bindings| {
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("water MacCormack"),
                    entries: &bindings
                        .iter()
                        .map(|&binding| wgpu::BindGroupLayoutEntry {
                            binding,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: if binding == 0 {
                                    wgpu::BufferBindingType::Uniform
                                } else {
                                    wgpu::BufferBindingType::Storage {
                                        read_only: matches!(binding, 1 | 3 | 9),
                                    }
                                },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        })
                        .collect::<Vec<_>>(),
                })
        });
        let groups = self.maccormack_groups(&layouts, bytes);
        let pipeline_layouts = layouts.each_ref().map(|layout| {
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("water MacCormack"),
                    bind_group_layouts: &[Some(layout)],
                    immediate_size: 0,
                })
        });
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("water MacCormack"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../shaders/det/water_maccormack.wgsl").into(),
                ),
            });
        let pipelines = MAC_STAGES.map(|(entry, family, _)| {
            self.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layouts[family]),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: crate::shaders::compute_options(),
                    cache: None,
                })
        });
        self.maccormack = Some(MacCormack {
            layouts,
            capacity: bytes,
            groups,
            pipelines,
        });
    }

    fn maccormack_groups(
        &self,
        layouts: &[wgpu::BindGroupLayout; 3],
        bytes: u64,
    ) -> [[wgpu::BindGroup; 2]; 3] {
        let scratch = |label| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let forward = scratch("water forward advection");
        let reverse = scratch("water reverse advection");
        std::array::from_fn(|family| {
            std::array::from_fn(|i| {
                let resource = |binding: u32| match binding {
                    0 => &self.params,
                    1 => &self.state[i],
                    2 => &self.state[1 - i],
                    3 => &self.active,
                    4 => &self.velocity,
                    5 => &self.scale,
                    6 => &self.edge_flux,
                    7 => &forward,
                    8 => &reverse,
                    9 => self.occupancy.head(),
                    _ => &self.falls,
                };
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water MacCormack"),
                    layout: &layouts[family],
                    entries: &MAC_BINDINGS[family]
                        .iter()
                        .map(|&binding| wgpu::BindGroupEntry {
                            binding,
                            resource: resource(binding).as_entire_binding(),
                        })
                        .collect::<Vec<_>>(),
                })
            })
        })
    }

    fn init_boundaries(&mut self) {
        if self.boundaries.is_some() {
            return;
        }
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("water boundaries"),
                entries: &std::array::from_fn::<_, 8, _>(|b| wgpu::BindGroupLayoutEntry {
                    binding: b as u32,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: wgpu::BindingType::Buffer {
                        ty: if b == 0 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: matches!(b, 1 | 3),
                            }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                }),
            });
        let report = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water boundary receipt"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let groups = std::array::from_fn(|i| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water boundaries"),
                layout: &layout,
                entries: &[
                    &self.params,
                    &self.state[i],
                    &self.state[1 - i],
                    &self.active,
                    &self.velocity,
                    &self.scale,
                    &self.edge_flux,
                    &report,
                ]
                .iter()
                .enumerate()
                .map(|(b, buffer)| wgpu::BindGroupEntry {
                    binding: b as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
            })
        });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("water boundaries"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("water boundaries"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../shaders/det/water_boundaries.wgsl").into(),
                ),
            });
        let pipelines = [
            "boundary_predict",
            "boundary_limit",
            "boundary_reduce",
            "boundary_export",
        ]
        .map(|entry| {
            self.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: crate::shaders::compute_options(),
                    cache: None,
                })
        });
        self.boundaries = Some(BoundaryPipelines {
            groups,
            pipelines,
            report,
        });
    }
    fn boundary_dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stage: usize,
        reduce: bool,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) {
        let b = self.boundaries.as_ref().expect("boundary pipelines");
        let label = [
            "water boundary predict",
            "water boundary limit",
            "water boundary receipt",
            "water boundary export",
        ][stage];
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: profiler.and_then(|p| p.compute_scope(label)),
        });
        pass.set_pipeline(&b.pipelines[stage]);
        pass.set_bind_group(0, &b.groups[self.current], &[]);
        let n = 2 * (self.size[0] + self.size[1]);
        pass.dispatch_workgroups(
            if reduce {
                1
            } else if stage == 0 || stage == 3 {
                n.div_ceil(64)
            } else {
                n.div_ceil(256)
            },
            1,
            1,
        );
    }
    pub fn copy_boundary_report(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Buffer,
        offset: u64,
    ) -> Result<(), String> {
        if !self.has_boundary_report {
            return Err("this water step has no boundary receipt".into());
        }
        let b = self
            .boundaries
            .as_ref()
            .ok_or("boundary report is unavailable")?;
        encoder.copy_buffer_to_buffer(&b.report, 0, target, offset, 16);
        Ok(())
    }
    /// The bytes `copy_edges` writes after a step with a Joined side.
    pub fn edge_bytes(&self) -> u64 {
        2 * u64::from(self.size[0] + self.size[1]) * 16
    }
    /// The finished field's edge columns, one per boundary face in `edge_address` order: free
    /// surface and bed relative to the datum, then the inner face's and the boundary face's
    /// velocities along the side's axis (water.md 64.3).
    pub fn copy_edges(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Buffer,
        offset: u64,
    ) -> Result<(), String> {
        if !self.joined {
            return Err("this water step has no joined side".into());
        }
        self.boundary_dispatch(encoder, 3, false, None);
        encoder.copy_buffer_to_buffer(
            &self.edge_flux,
            2 * JOIN_IN * 16,
            target,
            offset,
            self.edge_bytes(),
        );
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_boundary_report(&mut self) -> Result<BoundaryReport, String> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.copy_boundary_report(&mut encoder, &self.readback, 0)?;
        self.queue.submit([encoder.finish()]);
        let mut words = [0u32; 4];
        crate::readback::read_blocking(&self.device, &self.readback, &mut words)?;
        BoundaryReport::from_bytes(bytemuck::cast_slice(&words))
    }

    /// The current GPU field, for downstream GPU consumers or an asynchronous staging copy.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.state[self.current]
    }
    /// Bodies' displaced head of the last step, metres per column (zero without displacements).
    pub fn occupancy_buffer(&self) -> &wgpu::Buffer {
        self.occupancy.head()
    }

    fn encode_checkpoint(&self, complete: bool) -> Result<(), String> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_checkpoint_copy(&mut encoder, complete)?;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }
    pub(crate) fn encode_checkpoint_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        complete: bool,
    ) -> Result<(), String> {
        if !self.initialized {
            return Err("GPU water has not been uploaded".into());
        }
        encoder.copy_buffer_to_buffer(
            self.buffer(),
            0,
            &self.readback,
            0,
            self.state_bytes(complete),
        );
        Ok(())
    }
    pub(crate) fn checkpoint_buffer(&self) -> &wgpu::Buffer {
        &self.readback
    }
    /// Explicit exact checkpoint; not part of a normal resident step.
    pub async fn checkpoint_async(&mut self) -> Result<Vec<[u32; 4]>, String> {
        self.encode_checkpoint(false)?;
        let mut bits = vec![[0u32; 4]; (self.size[0] * self.size[1]) as usize];
        crate::readback::read(&self.readback, &mut bits).await?;
        Ok(bits)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn checkpoint(&mut self) -> Result<Vec<[u32; 4]>, String> {
        self.encode_checkpoint(false)?;
        let mut bits = vec![[0u32; 4]; (self.size[0] * self.size[1]) as usize];
        crate::readback::read_blocking(&self.device, &self.readback, &mut bits)?;
        Ok(bits)
    }

    pub fn state_words(&self) -> u32 {
        self.size[0] * self.size[1] * 4 + (self.size[0] + self.size[1]).div_ceil(4) * 4
    }
    fn state_bytes(&self, complete: bool) -> u64 {
        if complete {
            u64::from(self.state_words()) * 4
        } else {
            u64::from(self.size[0]) * u64::from(self.size[1]) * 16
        }
    }
    pub(crate) fn unpack_checkpoint(
        &self,
        datum: f64,
        mut packed: Vec<[u32; 4]>,
    ) -> FieldCheckpoint {
        let faces = packed.split_off((self.size[0] * self.size[1]) as usize);
        FieldCheckpoint {
            size: self.size,
            datum,
            columns: packed,
            far_faces: faces
                .into_iter()
                .flatten()
                .take((self.size[0] + self.size[1]) as usize)
                .collect(),
        }
    }
    pub async fn checkpoint_complete_async(
        &mut self,
        datum: f64,
    ) -> Result<FieldCheckpoint, String> {
        self.encode_checkpoint(true)?;
        let mut packed = vec![[0; 4]; self.state_words() as usize / 4];
        crate::readback::read(&self.readback, &mut packed).await?;
        Ok(self.unpack_checkpoint(datum, packed))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn checkpoint_complete(&mut self, datum: f64) -> Result<FieldCheckpoint, String> {
        self.encode_checkpoint(true)?;
        let mut packed = vec![[0; 4]; self.state_words() as usize / 4];
        crate::readback::read_blocking(&self.device, &self.readback, &mut packed)?;
        Ok(self.unpack_checkpoint(datum, packed))
    }

    /// Asynchronous complete-window bridge for a browser game worker. Native executors must poll
    /// the device themselves; a WebGPU event loop drives the map callback without any device wait.
    pub async fn advance_async(
        &mut self,
        columns: &mut [[f32; 4]],
        active: &[u32],
        cell: f32,
        dt: f32,
        substeps: u32,
        damping: [f32; 2],
    ) -> Result<(), String> {
        self.upload(columns, active)?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_damped(&mut encoder, cell, dt, substeps, damping)?;
        let bytes = (columns.len() * 16) as u64;
        encoder.copy_buffer_to_buffer(self.buffer(), 0, &self.readback, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let result = crate::readback::read(&self.readback, columns).await;
        if result.is_err() {
            self.initialized = false;
        }
        result?;
        if columns
            .iter()
            .any(|c| c[1] < 0.0 || c.iter().any(|v| !v.is_finite()))
        {
            self.initialized = false;
            return Err("GPU water produced invalid state".into());
        }
        Ok(())
    }

    /// Native game bridge. One upload, one submission and one readback per tick. The browser core
    /// uses `upload`, `encode`, `buffer` with an asynchronous staging copy instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn advance(
        &mut self,
        columns: &mut [[f32; 4]],
        active: &[u32],
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
    ) -> Result<(), String> {
        self.upload(columns, active)?;
        self.advance_uploaded(
            columns,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
        )
    }

    /// Step an already resident field and read the result. `columns` is output, not input.
    /// The caller uploads edits and masks first; a host may reuse inputs only after verifying
    /// that they match the last successfully read GPU state bit for bit.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn advance_uploaded(
        &mut self,
        columns: &mut [[f32; 4]],
        cell: f32,
        dt: f32,
        substeps: u32,
        damping: [f32; 2],
    ) -> Result<(), String> {
        self.advance_uploaded_mode(columns, cell, dt, substeps, damping, false)
    }

    /// Complete readback reference for the resident scheme; gameplay residency uses encode and
    /// sparse readbacks instead. Kept separate from the original complete-window solver contract.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub fn advance_maccormack(
        &mut self,
        columns: &mut [[f32; 4]],
        active: &[u32],
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
    ) -> Result<(), String> {
        self.upload(columns, active)?;
        self.advance_uploaded_mode(
            columns,
            cell,
            dt,
            substeps,
            [
                friction,
                amoris_assets::water_field::DEFAULT_CALM_PER_S as f32,
            ],
            true,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn advance_uploaded_mode(
        &mut self,
        columns: &mut [[f32; 4]],
        cell: f32,
        dt: f32,
        substeps: u32,
        [friction, calm]: [f32; 2],
        maccormack: bool,
    ) -> Result<(), String> {
        if columns.len() != (self.size[0] * self.size[1]) as usize {
            return Err("GPU water output does not match its window".into());
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("water tick"),
            });
        self.encode_inner(
            &mut encoder,
            cell,
            dt,
            substeps,
            [friction, calm],
            maccormack,
            &[FieldBoundary::Wall; 4],
            None,
            FieldPhysics::default(),
            &[],
            &[],
            None,
            0.0,
            None,
        )?;
        let bytes = (columns.len() * 16) as u64;
        encoder.copy_buffer_to_buffer(self.buffer(), 0, &self.readback, 0, bytes);
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.readback
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        let result = (|| -> Result<(), String> {
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(10)),
                })
                .map_err(|e| format!("GPU water polling failed: {e}"))?;
            rx.recv_timeout(std::time::Duration::from_secs(1))
                .map_err(|e| format!("GPU water readback did not complete: {e}"))?
                .map_err(|e| format!("GPU water readback failed: {e}"))?;
            {
                let mapped = self
                    .readback
                    .slice(..bytes)
                    .get_mapped_range()
                    .map_err(|e| format!("GPU water mapped range failed: {e}"))?;
                columns.copy_from_slice(bytemuck::cast_slice(&mapped));
            }
            Ok(())
        })();
        self.readback.unmap();
        if result.is_err() {
            self.initialized = false;
        }
        result?;
        if columns
            .iter()
            .any(|c| !c.iter().all(|v| v.is_finite()) || c[1] < 0.0)
        {
            self.initialized = false;
            return Err("GPU water produced non-finite state or negative depth".into());
        }
        Ok(())
    }
}
