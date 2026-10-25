//! Explicit standalone experiment buffers; no new production solver option or state.
use amoris_assets::water_field::FieldCheckpoint;
use amoris_render::Gpu;
pub struct Field<'a> {
    gpu: &'a Gpu,
    size: [u32; 2],
    state: [wgpu::Buffer; 2],
    params: wgpu::Buffer,
    velocity: wgpu::Buffer,
    current: usize,
    /// Advection, face pressure and depth transport families, as in production.
    groups: [[wgpu::BindGroup; 2]; 3],
    pipelines: [wgpu::ComputePipeline; 5],
}
const BINDINGS: [&[u32]; 3] = [
    &[0, 1, 3, 7, 8],
    &[0, 1, 3, 4, 7, 8, 9],
    &[0, 1, 2, 3, 4, 5, 6, 10],
];
const FAMILY: [usize; 5] = [0, 0, 1, 2, 2];
const PARAMS_WORDS: usize = 168;
impl<'a> Field<'a> {
    pub fn new(gpu: &'a Gpu, initial: &FieldCheckpoint, mode: &str) -> Self {
        let [nx, nz] = initial.size;
        let n = u64::from(nx) * u64::from(nz);
        let buffer = |bytes, usage| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("depth-transport research"),
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // Production's Params (water_compute.wgsl), wind and rain included.
        let params = buffer(
            PARAMS_WORDS as u64 * 4,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let state =
            std::array::from_fn(|_| buffer(n * 16 + 4096, storage | wgpu::BufferUsages::COPY_SRC));
        gpu.queue
            .write_buffer(&state[0], 0, bytemuck::cast_slice(&initial.columns));
        let awake = buffer(n * 4, storage);
        gpu.queue
            .write_buffer(&awake, 0, bytemuck::cast_slice(&vec![1u32; n as usize]));
        let velocity = buffer(n * 8, storage);
        let scale = buffer(n * 4, storage);
        let edges = buffer(u64::from(nx + nz) * 32, storage);
        let predicted = buffer(n * 8, storage);
        let reversed = buffer(n * 8, storage);
        // No displaced head and no falls: zero storage satisfies the production bindings.
        let occupancy = buffer(n * 4, storage);
        let falls = buffer(n * 12, storage);
        let layouts = BINDINGS.map(|bindings| {
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("depth-transport research"),
                    entries: &bindings
                        .iter()
                        .map(|&i| wgpu::BindGroupLayoutEntry {
                            binding: i,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: if i == 0 {
                                    wgpu::BufferBindingType::Uniform
                                } else {
                                    wgpu::BufferBindingType::Storage {
                                        read_only: matches!(i, 1 | 3 | 9),
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
        let groups = std::array::from_fn(|family| {
            std::array::from_fn(|current| {
                let resource = |i: u32| match i {
                    0 => &params,
                    1 => &state[current],
                    2 => &state[1 - current],
                    3 => &awake,
                    4 => &velocity,
                    5 => &scale,
                    6 => &edges,
                    7 => &predicted,
                    8 => &reversed,
                    9 => &occupancy,
                    _ => &falls,
                };
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("depth-transport research"),
                    layout: &layouts[family],
                    entries: &BINDINGS[family]
                        .iter()
                        .map(|&i| wgpu::BindGroupEntry {
                            binding: i,
                            resource: resource(i).as_entire_binding(),
                        })
                        .collect::<Vec<_>>(),
                })
            })
        });
        let pipeline_layouts = layouts.each_ref().map(|layout| {
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("depth-transport research"),
                    bind_group_layouts: &[Some(layout)],
                    immediate_size: 0,
                })
        });
        let muscl = mode.starts_with("muscl");
        let mut base = include_str!("../../shaders/water_compute.wgsl").to_owned();
        if mode.contains("raw") {
            assert_eq!(
                base.matches("let delta = clamp(eb - ea, -(ha + oa), hb + ob);")
                    .count(),
                1
            );
            base = base.replace(
                "let delta = clamp(eb - ea, -(ha + oa), hb + ob);",
                "let delta = eb - ea;",
            );
        }
        if mode.contains("thin") {
            assert_eq!(base.matches("0.0001").count(), 4);
            base = base.replace("0.0001", "0.00000001");
        }
        let module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("depth-transport research"),
                source: wgpu::ShaderSource::Wgsl(
                    format!(
                        "{}\n{}\n{}",
                        base,
                        include_str!("../../shaders/water_maccormack.wgsl"),
                        include_str!("../../shaders/water_depth_first_order.wgsl")
                    )
                    .into(),
                ),
            });
        let names = if muscl {
            [
                "mac_forward",
                "mac_reverse",
                "mac_faces",
                "donors",
                "transport",
            ]
        } else {
            [
                "mac_forward",
                "mac_reverse",
                "mac_faces",
                "depth_first_donors",
                "depth_first_transport",
            ]
        };
        let mut stage = 0;
        let pipelines = names.map(|name| {
            let family = FAMILY[stage];
            stage += 1;
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(name),
                    layout: Some(&pipeline_layouts[family]),
                    module: &module,
                    entry_point: Some(name),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        zero_initialize_workgroup_memory: false,
                        ..Default::default()
                    },
                    cache: None,
                })
        });
        Self {
            gpu,
            size: initial.size,
            state,
            params,
            velocity,
            current: 0,
            groups,
            pipelines,
        }
    }
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, dx: f32, dt: f32, substeps: u32) {
        self.encode_profiled(encoder, dx, dt, substeps, None);
    }
    pub fn encode_profiled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        dx: f32,
        dt: f32,
        substeps: u32,
        mut profiler: Option<&mut amoris_render::profiler::GpuProfiler>,
    ) {
        let mut words = [0u32; PARAMS_WORDS];
        words[0] = self.size[0];
        words[1] = self.size[1];
        words[2] = dx.to_bits();
        words[3] = dt.to_bits();
        self.gpu
            .queue
            .write_buffer(&self.params, 0, bytemuck::cast_slice(&words));
        for _ in 0..substeps {
            for (stage, p) in self.pipelines.iter().enumerate() {
                let label = [
                    "water advect forward",
                    "water advect reverse",
                    "water faces",
                    "water donors",
                    "water transport",
                ][stage];
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: profiler.as_deref_mut().and_then(|p| p.compute_scope(label)),
                });
                pass.set_pipeline(p);
                pass.set_bind_group(0, &self.groups[FAMILY[stage]][self.current], &[]);
                pass.dispatch_workgroups(self.size[0].div_ceil(8), self.size[1].div_ceil(8), 1);
            }
            self.current = 1 - self.current;
        }
    }
    pub fn transport(&mut self, dx: f32, dt: f32, velocity: &[[f32; 2]]) {
        assert_eq!(velocity.len(), (self.size[0] * self.size[1]) as usize);
        self.gpu
            .queue
            .write_buffer(&self.velocity, 0, bytemuck::cast_slice(velocity));
        let mut words = [0u32; PARAMS_WORDS];
        words[0] = self.size[0];
        words[1] = self.size[1];
        words[2] = dx.to_bits();
        words[3] = dt.to_bits();
        self.gpu
            .queue
            .write_buffer(&self.params, 0, bytemuck::cast_slice(&words));
        let mut e = self.gpu.device.create_command_encoder(&Default::default());
        for pipeline in &self.pipelines[3..] {
            let mut pass = e.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.groups[2][self.current], &[]);
            pass.dispatch_workgroups(self.size[0].div_ceil(8), self.size[1].div_ceil(8), 1);
        }
        self.gpu.queue.submit([e.finish()]);
        self.current = 1 - self.current;
    }
    pub fn checkpoint(&self) -> FieldCheckpoint {
        let n = u64::from(self.size[0]) * u64::from(self.size[1]);
        let stage = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth experiment diagnostic"),
            size: n * 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = self.gpu.device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(&self.state[self.current], 0, &stage, 0, n * 16);
        self.gpu.queue.submit([e.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        stage.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let columns =
            bytemuck::cast_slice::<u8, [u32; 4]>(&stage.slice(..).get_mapped_range().unwrap())
                .to_vec();
        stage.unmap();
        FieldCheckpoint::new(self.size, 0.0, columns)
    }
}
