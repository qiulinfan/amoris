//! Bodies' displaced volume as a per-column pressure head (two-way vertical coupling). The
//! resident step scatters each point's volume over its footprint disk in fixed point, then
//! resolves metres of head that the face pressure stage and the free surface read.
use amoris_assets::water_field::{MAX_SAMPLES, PointVolume};

pub(crate) struct WaterOccupancy {
    queue: wgpu::Queue,
    params: wgpu::Buffer,
    points: wgpu::Buffer,
    /// Kept with the bind group that writes it.
    _accum: wgpu::Buffer,
    head: wgpu::Buffer,
    /// Head added since presentation last took it, per column (spray at bodies' entries).
    rise: wgpu::Buffer,
    /// The state was just uploaded, with no head: the first gathering after it is a baseline.
    fresh: bool,
    pipelines: [wgpu::ComputePipeline; 3],
    group: wgpu::BindGroup,
    columns: u64,
    /// The head buffer may hold a nonzero value from the last step that had displacements.
    dirty: bool,
}

impl WaterOccupancy {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, columns: u64) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let params = buffer(
            "water occupancy params",
            16,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let points = buffer("water occupancy points", MAX_SAMPLES as u64 * 16, storage);
        let accum = buffer("water occupancy sums", columns * 4, storage);
        let head = buffer("water occupancy head", columns * 4, storage);
        // Copyable so a check can read what presentation would take.
        let rise = buffer(
            "water occupancy rise",
            columns * 4,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water occupancy"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/water_occupancy.wgsl").into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water occupancy"),
            entries: &std::array::from_fn::<_, 5, _>(|b| wgpu::BindGroupLayoutEntry {
                binding: b as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if b == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: b == 1 }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water occupancy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines =
            ["occupancy_clear", "occupancy_scatter", "occupancy_resolve"].map(|entry| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: crate::shaders::compute_options(),
                    cache: None,
                })
            });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water occupancy"),
            layout: &layout,
            entries: &[&params, &points, &accum, &head, &rise]
                .iter()
                .enumerate()
                .map(|(binding, b)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        Self {
            queue: queue.clone(),
            params,
            points,
            _accum: accum,
            head,
            rise,
            fresh: true,
            pipelines,
            group,
            columns,
            dirty: true,
        }
    }

    pub fn head(&self) -> &wgpu::Buffer {
        &self.head
    }

    pub fn rise(&self) -> &wgpu::Buffer {
        &self.rise
    }

    /// A newly uploaded state carries no displaced volume; queue order precedes later submits.
    pub fn reset(&mut self) {
        let zeros = vec![0f32; self.columns as usize];
        self.queue
            .write_buffer(&self.head, 0, bytemuck::cast_slice(&zeros));
        self.queue
            .write_buffer(&self.rise, 0, bytemuck::cast_slice(&zeros));
        self.dirty = false;
        self.fresh = true;
    }

    /// Gather this step's displaced volume, or clear what an earlier step left. Returns whether
    /// the face pressure stage should read the head.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        cell: f32,
        displacements: &[PointVolume],
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<bool, String> {
        if displacements.len() > MAX_SAMPLES {
            return Err("water displacements exceed the point budget".into());
        }
        let fresh = std::mem::take(&mut self.fresh);
        if displacements.is_empty() {
            if self.dirty {
                encoder.clear_buffer(&self.head, 0, None);
                self.dirty = false;
            }
            return Ok(false);
        }
        let columns = size[0] * size[1];
        let points: Vec<[f32; 4]> = displacements
            .iter()
            .map(|d| {
                [
                    d.at[0] as f32,
                    d.at[1] as f32,
                    d.volume_m3 as f32,
                    d.radius as f32,
                ]
            })
            .collect();
        self.queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[
                size[0],
                size[1],
                points.len() as u32,
                (cell * cell).to_bits(),
            ]),
        );
        self.queue
            .write_buffer(&self.points, 0, bytemuck::cast_slice(&points));
        for (stage, count) in [columns, points.len() as u32, columns]
            .into_iter()
            .enumerate()
        {
            let label = [
                "water occupancy clear",
                "water occupancy scatter",
                "water occupancy",
            ][stage];
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: profiler.as_deref_mut().and_then(|p| p.compute_scope(label)),
            });
            pass.set_pipeline(&self.pipelines[stage]);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        if fresh {
            // The bodies were in the water before the upload, which kept none of their head (a
            // viewport's replica re-seeds from a checkpoint at most compactions when its feed skips
            // ticks). Gathered again, the whole head is no entry: counted as one, every re-seed
            // threw a burst of spray around a board merely floating.
            encoder.clear_buffer(&self.rise, 0, None);
        }
        self.dirty = true;
        Ok(true)
    }
}
