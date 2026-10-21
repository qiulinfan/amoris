//! GPU shallow-water substeps. The core keeps its field on the GPU between dispatches; the native
//! game adapter uploads and reads it once per tick to preserve same-tick queries and snapshots.
//! f32, three classic or five bounded-MacCormack dispatches per substep. No atomics or optional
//! WebGPU features. Modern scratch/pipelines are created lazily, within the baseline storage limit.

use amoris_assets::water_field::{
    CheckedCheckpoint, FieldBoundary, FieldCheckpoint, FieldSea, FieldStep, MAX_SEA_WAVES,
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
    padding: [f32; 3],
    edge_kind: [u32; 4],
    edge_rate: [f32; 4],
    edge_normal: [f32; 4],
    sea_meta: [f32; 4],
    waves: [WaveParams; MAX_SEA_WAVES],
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
    edge_flux: wgpu::Buffer,
    boundaries: Option<BoundaryPipelines>,
    has_boundary_report: bool,
    current: usize,
    readback: wgpu::Buffer,
}

struct MacCormack {
    layout: wgpu::BindGroupLayout,
    capacity: u64,
    groups: [wgpu::BindGroup; 2],
    pipelines: [wgpu::ComputePipeline; 5],
}
struct BoundaryPipelines {
    groups: [wgpu::BindGroup; 2],
    pipelines: [wgpu::ComputePipeline; 3],
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
        let edge_flux = buffer(
            "water boundary flux",
            2 * (512 + 512) * 16,
            storage | wgpu::BufferUsages::COPY_DST,
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
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../shaders/water_compute.wgsl"),
                    include_str!("../shaders/water_depth_first_order.wgsl")
                )
                .into(),
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
            edge_flux,
            boundaries: None,
            has_boundary_report: false,
            current: 0,
            readback: buffer(
                "water readback",
                state_bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
        })
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
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
            friction,
            false,
            &[FieldBoundary::Wall; 4],
            None,
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
            friction,
            true,
            &[FieldBoundary::Wall; 4],
            None,
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
            encoder, cell, dt, substeps, friction, true, edges, None, None,
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
            encoder, cell, dt, substeps, friction, true, edges, None, profiler,
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
            step.cell_m as f32,
            step.substep_dt as f32,
            step.substeps,
            step.friction as f32,
            true,
            &step.boundaries,
            step.sea.as_ref(),
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
        friction: f32,
        maccormack: bool,
        edges: &[FieldBoundary; 4],
        sea: Option<&FieldSea>,
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        self.has_boundary_report = false;
        if !self.initialized {
            return Err("GPU water has not been uploaded".into());
        }
        if !cell.is_finite()
            || cell <= 0.0
            || !dt.is_finite()
            || dt <= 0.0
            || !friction.is_finite()
            || friction < 0.0
            || !(1..=8).contains(&substeps)
        {
            return Err("invalid GPU water timestep, cell, friction or substep count".into());
        }
        if edges.contains(&FieldBoundary::Sea) != sea.is_some() {
            return Err("sea boundary needs its recorded descriptor".into());
        }
        if let Some(sea) = sea {
            sea.validate()?;
        }
        if maccormack {
            self.init_maccormack();
        }
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
                    rates[i] = h as f32;
                    normals[i] = u as f32;
                }
                FieldBoundary::Outflow => kinds[i] = 2,
                FieldBoundary::Stage {
                    surface_relative_m: h,
                } => {
                    kinds[i] = 3;
                    rates[i] = h as f32;
                }
                FieldBoundary::Inflow { m3s_per_m: q } => {
                    kinds[i] = 1;
                    rates[i] = q as f32;
                }
            }
        }
        if open {
            if self.size.iter().any(|n| n % 4 != 0) {
                return Err("open water windows must have dimensions divisible by four".into());
            }
            self.init_boundaries();
            encoder.clear_buffer(&self.edge_flux, 0, None);
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
                friction,
                padding: [0.0; 3],
                edge_kind: kinds,
                edge_rate: rates,
                edge_normal: normals,
                sea_meta: [
                    sea.map_or(0.0, |s| s.level_relative_m as f32),
                    sea.map_or(0.0, |s| s.waves.len() as f32),
                    0.0,
                    0.0,
                ],
                waves: std::array::from_fn(|i| {
                    sea.and_then(|s| s.waves.get(i))
                        .map_or(WaveParams::zeroed(), |w| WaveParams {
                            phase: [
                                w.wave_vector_rad_m[0] as f32,
                                w.wave_vector_rad_m[1] as f32,
                                w.angular_frequency_rad_s as f32,
                                w.phase_at_start_rad as f32,
                            ],
                            data: [w.amplitude_m as f32, 0.0, 0.0, 0.0],
                        })
                }),
            }),
        );
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
            for stage in 0..if maccormack { 5 } else { 3 } {
                if open && stage == if maccormack { 3 } else { 1 } {
                    self.boundary_dispatch(encoder, 0, false, profiler.as_deref_mut());
                }
                let (pipeline, group, label) = if maccormack {
                    let mac = self.maccormack.as_ref().expect("MacCormack pipelines");
                    (
                        &mac.pipelines[stage],
                        &mac.groups[self.current],
                        [
                            "water advect forward",
                            "water advect reverse",
                            "water faces",
                            "water donors",
                            "water transport",
                        ][stage],
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
            self.boundary_dispatch(encoder, 2, true, profiler);
        }
        Ok(())
    }

    fn init_maccormack(&mut self) {
        let bytes = u64::from(self.size[0]) * u64::from(self.size[1]) * 8;
        if let Some(mac) = &self.maccormack {
            if mac.capacity >= bytes {
                return;
            }
            let groups = self.maccormack_groups(&mac.layout, bytes);
            let mac = self
                .maccormack
                .as_mut()
                .expect("existing MacCormack pipelines");
            mac.groups = groups;
            mac.capacity = bytes;
            return;
        }
        let layout = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("water MacCormack"),
                entries: &std::array::from_fn::<_, 9, _>(|binding| wgpu::BindGroupLayoutEntry {
                    binding: binding as u32,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: if binding == 0 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: matches!(binding, 1 | 3),
                            }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }),
            });
        let groups = self.maccormack_groups(&layout, bytes);
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("water MacCormack"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("water MacCormack"),
                source: wgpu::ShaderSource::Wgsl(
                    format!(
                        "{}\n{}",
                        include_str!("../shaders/water_compute.wgsl"),
                        include_str!("../shaders/water_maccormack.wgsl")
                    )
                    .into(),
                ),
            });
        let pipelines = [
            "mac_forward",
            "mac_reverse",
            "mac_faces",
            "donors",
            "transport",
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
        self.maccormack = Some(MacCormack {
            layout,
            capacity: bytes,
            groups,
            pipelines,
        });
    }

    fn maccormack_groups(
        &self,
        layout: &wgpu::BindGroupLayout,
        bytes: u64,
    ) -> [wgpu::BindGroup; 2] {
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
        std::array::from_fn(|i| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water MacCormack"),
                layout,
                entries: &[
                    &self.params,
                    &self.state[i],
                    &self.state[1 - i],
                    &self.active,
                    &self.velocity,
                    &self.scale,
                    &self.edge_flux,
                    &forward,
                    &reverse,
                ]
                .iter()
                .enumerate()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
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
                    format!(
                        "{}\n{}",
                        include_str!("../shaders/water_compute.wgsl"),
                        include_str!("../shaders/water_boundaries.wgsl")
                    )
                    .into(),
                ),
            });
        let pipelines = ["boundary_predict", "boundary_limit", "boundary_reduce"].map(|entry| {
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
            } else if stage == 0 {
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
        friction: f32,
    ) -> Result<(), String> {
        self.upload(columns, active)?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode(&mut encoder, cell, dt, substeps, friction)?;
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
        self.advance_uploaded(columns, cell, dt, substeps, friction)
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
        friction: f32,
    ) -> Result<(), String> {
        self.advance_uploaded_mode(columns, cell, dt, substeps, friction, false)
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
        self.advance_uploaded_mode(columns, cell, dt, substeps, friction, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn advance_uploaded_mode(
        &mut self,
        columns: &mut [[f32; 4]],
        cell: f32,
        dt: f32,
        substeps: u32,
        friction: f32,
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
            friction,
            maccormack,
            &[FieldBoundary::Wall; 4],
            None,
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
