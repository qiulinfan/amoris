//! Bounded horizontal point impulses into the actual staggered field, with an explicit budget.
use crate::{shaders, water_compute::GpuWater};
use amoris_assets::water_field::{MAX_SAMPLES, PointImpulse};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReactionReport {
    pub applied: [f32; 2],
    pub rejected: [f32; 2],
    pub roundoff: [f32; 2],
}
impl ReactionReport {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 32 {
            return Err("water reaction budget must contain 32 bytes".into());
        }
        let words: [f32; 8] = bytemuck::pod_read_unaligned(bytes);
        if words.iter().any(|v| !v.is_finite()) || words[6] != 0.0 || words[7] != 0.0 {
            return Err("water reaction overflowed or has an invalid budget".into());
        }
        Ok(Self {
            applied: [words[0], words[1]],
            rejected: [words[2], words[3]],
            roundoff: [words[4], words[5]],
        })
    }
}

pub struct WaterReactions {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: [wgpu::ComputePipeline; 3],
    params: wgpu::Buffer,
    points: wgpu::Buffer,
    missing: wgpu::Buffer,
    runs: wgpu::Buffer,
    links: wgpu::Buffer,
    stats: wgpu::Buffer,
    report: wgpu::Buffer,
    group: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}
impl WaterReactions {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water reaction inputs"),
            entries: &std::array::from_fn::<_, 8, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: matches!(binding, 2 | 4 | 5),
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            }),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water reaction"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water reaction"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/det/water_reactions.wgsl").into(),
            ),
        });
        let pipelines = ["prepare", "apply", "summarize"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        });
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipelines,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("reaction parameters"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            points: buffer("reaction points", (MAX_SAMPLES * 16) as u64),
            missing: buffer("reaction unsupported weights", (MAX_SAMPLES * 8) as u64),
            runs: buffer("reaction face owners", (MAX_SAMPLES * 16 * 16) as u64),
            links: buffer("reaction point links", (MAX_SAMPLES * 16 * 4) as u64),
            stats: buffer("reaction face budgets", (MAX_SAMPLES * 16 * 16) as u64),
            report: buffer("reaction budget", 32),
            group: None,
        }
    }
    /// Returns false for an empty input. All CPU work is bounded by point count, not field size.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        cell_m: f64,
        impulses: &[PointImpulse],
    ) -> Result<bool, String> {
        self.encode_inner(encoder, field, cell_m, impulses, true)
    }
    pub(crate) fn encode_presentation(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        cell_m: f64,
        impulses: &[PointImpulse],
    ) -> Result<bool, String> {
        self.encode_inner(encoder, field, cell_m, impulses, false)
    }
    fn encode_inner(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        cell_m: f64,
        impulses: &[PointImpulse],
        report: bool,
    ) -> Result<bool, String> {
        if impulses.is_empty() {
            return Ok(false);
        }
        if !field.is_initialized() {
            return Err("water reactions need an initialized field".into());
        }
        if impulses.len() > MAX_SAMPLES || !(0.25..=4.0).contains(&cell_m) {
            return Err("water reaction capacity or cell size is invalid".into());
        }
        let size = field.size();
        let mut points = Vec::with_capacity(impulses.len());
        let mut owners = BTreeMap::<(u32, u32), Vec<u32>>::new();
        for (n, p) in impulses.iter().enumerate() {
            let point = [p.at[0], p.at[1], p.impulse[0], p.impulse[1]]
                .map(amoris_assets::water_field::field_f32);
            if point.iter().any(|v| !v.is_finite()) {
                return Err("water reaction must fit finite f32 values".into());
            }
            points.push(point);
            let q = std::array::from_fn::<_, 2, _>(|a| {
                (point[a] - 0.5).clamp(0.0, (size[a] - 1) as f32)
            });
            let lo = q.map(|v| v.floor() as u32);
            let hi = std::array::from_fn::<_, 2, _>(|a| (lo[a] + 1).min(size[a] - 1));
            let t = std::array::from_fn::<_, 2, _>(|a| q[a] - lo[a] as f32);
            let mut faces = BTreeSet::new();
            for z in 0..2 {
                for x in 0..2 {
                    let weight = if x == 0 { 1.0 - t[0] } else { t[0] }
                        * if z == 0 { 1.0 - t[1] } else { t[1] };
                    if weight <= 0.0 {
                        continue;
                    }
                    let at = [
                        if x == 0 { lo[0] } else { hi[0] },
                        if z == 0 { lo[1] } else { hi[1] },
                    ];
                    let c = at[0] + at[1] * size[0];
                    for axis in 0..2 {
                        if at[axis] > 0 {
                            faces.insert((c, axis as u32));
                        }
                        if at[axis] + 1 < size[axis] {
                            faces.insert((c + if axis == 0 { 1 } else { size[0] }, axis as u32));
                        }
                    }
                }
            }
            for face in faces {
                owners.entry(face).or_default().push(n as u32);
            }
        }
        let mut runs = Vec::with_capacity(owners.len());
        let mut links = Vec::new();
        for ((column, axis), list) in owners {
            runs.push([column, axis, links.len() as u32, list.len() as u32]);
            links.extend(list);
        }
        let mut params = [0u32; 8];
        params[..4].copy_from_slice(&[size[0], size[1], points.len() as u32, runs.len() as u32]);
        params[4] = (cell_m as f32).to_bits();
        self.queue
            .write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        self.queue
            .write_buffer(&self.points, 0, bytemuck::cast_slice(&points));
        self.queue
            .write_buffer(&self.runs, 0, bytemuck::cast_slice(&runs));
        self.queue
            .write_buffer(&self.links, 0, bytemuck::cast_slice(&links));
        if self
            .group
            .as_ref()
            .is_none_or(|(buffer, _)| buffer != field.buffer())
        {
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water reaction"),
                layout: &self.pipelines[0].get_bind_group_layout(0),
                entries: &[
                    &self.params,
                    field.buffer(),
                    &self.points,
                    &self.missing,
                    &self.runs,
                    &self.links,
                    &self.stats,
                    &self.report,
                ]
                .iter()
                .enumerate()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
            });
            self.group = Some((field.buffer().clone(), group));
        }
        for (index, (pipeline, count)) in self
            .pipelines
            .iter()
            .zip([points.len().div_ceil(64), runs.len().div_ceil(64), 1])
            .enumerate()
        {
            if !report && index != 1 {
                continue;
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("water reaction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.group.as_ref().unwrap().1, &[]);
            pass.dispatch_workgroups(count as u32, 1, 1);
        }
        Ok(true)
    }
    pub fn copy_to(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Buffer, offset: u64) {
        encoder.copy_buffer_to_buffer(&self.report, 0, target, offset, 32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_report(&self) -> Result<ReactionReport, String> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reaction budget readback"),
            size: 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.copy_to(&mut encoder, &staging, 0);
        self.queue.submit([encoder.finish()]);
        let mut words = [0u32; 8];
        crate::readback::read_blocking(&self.device, &staging, &mut words)?;
        ReactionReport::from_bytes(bytemuck::cast_slice(&words))
    }
}
