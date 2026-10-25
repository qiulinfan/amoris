//! Dilute GPU spray from current displayed water columns. Presentation only, no field readback.
use crate::{
    post::{DEPTH, SceneFormat},
    profiler::GpuProfiler,
};
use bytemuck::{Pod, Zeroable};
pub const CAPACITY: u32 = 16384;
pub const PER_FRAME: u32 = 4096;
/// Births a frame left to splashes and breaking lips (matches water_spray_compute.wgsl): sustained
/// white water must not overwrite a waterfall's sheet in the ring before its drops land.
pub const SPLASH_PER_FRAME: u32 = 64;
#[derive(Clone, Copy, PartialEq)]
pub struct FieldSpec {
    pub origin: [f32; 2],
    pub size: [u32; 2],
    pub cell: f32,
    pub tiled: bool,
    pub absorb: [f32; 3],
}
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Params {
    count: [u32; 4],
    origin_cell_dt: [f32; 4],
    flags: [u32; 4],
    rates: [f32; 4],
    absorb: [f32; 4],
    entry: [f32; 4],
}
pub struct Advance {
    pub field: FieldSpec,
    pub dt: f32,
    pub changed: bool,
    pub clear: bool,
    /// Simulated seconds the occupancy rise gathered over since the last advance; zero for none.
    pub rise_s: f32,
    /// Tally what the sources sound like from the camera (water_acoustics.rs).
    pub listen: bool,
}
pub struct Pipelines {
    layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    compute: [wgpu::ComputePipeline; 2],
    draw: wgpu::RenderPipeline,
    module: wgpu::ShaderModule,
    pub budget: wgpu::Buffer,
    /// The frame's tally of sound-making events, shared by every region (water_acoustics.rs).
    pub acoustics: wgpu::Buffer,
    /// Bound in place of the occupancy head and rise of a field that has none.
    pub no_occupancy: [wgpu::Buffer; 2],
}
pub struct Pool {
    params: wgpu::Buffer,
    pub particles: wgpu::Buffer,
    pub state: wgpu::Buffer,
    compute: wgpu::BindGroup,
    draw: wgpu::BindGroup,
    last_spec: Option<FieldSpec>,
    frame: u32,
}
fn source(body: &str) -> String {
    format!("{}\n{body}", include_str!("../shaders/common.wgsl"))
}
fn draw_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water spray draw"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("water spray"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &format.targets(Some(blend), false),
        }),
        primitive: Default::default(),
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
impl Pipelines {
    pub fn new(device: &wgpu::Device, format: SceneFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water spray compute"),
            entries: &std::array::from_fn::<_, 10, _>(|i| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if i < 2 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: i == 2 || i == 7,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water spray compute"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water spray compute"),
            source: wgpu::ShaderSource::Wgsl(
                source(include_str!("../shaders/water_spray_compute.wgsl")).into(),
            ),
        });
        let compute = ["simulate", "emit"].map(|name| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(name),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water spray draw"),
            entries: &std::array::from_fn::<_, 3, _>(|i| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: if i < 2 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: true }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water spray draw"),
            source: wgpu::ShaderSource::Wgsl(
                source(include_str!("../shaders/water_spray_draw.wgsl")).into(),
            ),
        });
        let draw = draw_pipeline(device, &module, &draw_layout, format);
        let budget = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water spray global birth budget"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let acoustics = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water spray acoustics"),
            size: crate::water_acoustics::TALLY_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let no_occupancy =
            ["water spray without occupancy", "water spray without rise"].map(|label| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: 16,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            });
        Self {
            layout,
            draw_layout,
            compute,
            draw,
            module,
            budget,
            acoustics,
            no_occupancy,
        }
    }
    pub fn reformat(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.draw = draw_pipeline(device, &self.module, &self.draw_layout, format);
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pool: &Pool) {
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(0, &pool.draw, &[]);
        pass.draw_indirect(&pool.state, 16);
    }
}
impl Pool {
    pub fn new(
        device: &wgpu::Device,
        p: &Pipelines,
        view: &wgpu::Buffer,
        columns: &wgpu::Buffer,
        impacts: &wgpu::Buffer,
        occupancy: Option<(&wgpu::Buffer, &wgpu::Buffer)>,
    ) -> Self {
        let buffer = |name, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(name),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = buffer(
            "water spray params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let particles = buffer(
            "water spray particles",
            u64::from(CAPACITY) * 48,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let state = buffer(
            "water spray state and indirect draw",
            32,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let compute = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water spray compute"),
            layout: &p.layout,
            entries: &[
                view,
                &params,
                columns,
                &particles,
                &state,
                &p.budget,
                impacts,
                occupancy.map_or(&p.no_occupancy[0], |o| o.0),
                occupancy.map_or(&p.no_occupancy[1], |o| o.1),
                &p.acoustics,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
        });
        let draw = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water spray draw"),
            layout: &p.draw_layout,
            entries: &[view, &params, &particles]
                .into_iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        Self {
            params,
            particles,
            state,
            compute,
            draw,
            last_spec: None,
            frame: 0,
        }
    }
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        p: &Pipelines,
        a: Advance,
        profiler: &mut GpuProfiler,
    ) {
        let reset = a.clear || a.dt > 0.25 || self.last_spec != Some(a.field);
        let dt = if a.dt > 0.25 { 0.0 } else { a.dt.max(0.0) };
        if reset {
            encoder.clear_buffer(&self.particles, 0, None);
            encoder.clear_buffer(&self.state, 0, None);
            self.frame = 0;
        }
        let changed = a.changed || reset;
        if !reset {
            if changed {
                // Keep only the ring head; invocation zero restores the constant vertex count.
                encoder.clear_buffer(&self.state, 4, Some(20));
            } else {
                encoder.clear_buffer(&self.state, 4, Some(8));
                encoder.clear_buffer(&self.state, 20, Some(4));
            }
        }
        if dt > 0.0 {
            self.frame = self.frame.wrapping_add(1);
        }
        let f = a.field;
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                count: [f.size[0], f.size[1], u32::from(f.tiled), CAPACITY],
                origin_cell_dt: [f.origin[0], f.origin[1], f.cell, dt],
                flags: [self.frame, u32::from(changed), u32::from(reset), PER_FRAME],
                // Fall particle volume (m^3), fall source reach (m), the rate of rise that throws a
                // splash (m/s) and the reach of splashes and breaking lips (m).
                rates: [0.00195, 48.0, 0.9, 160.0],
                absorb: [f.absorb[0], f.absorb[1], f.absorb[2], 0.0],
                // Whether bodies' entries are read this advance, and over how many seconds; then
                // whether the sources are heard (water_acoustics.rs).
                entry: [
                    f32::from(u8::from(a.rise_s > 0.0 && !f.tiled)),
                    if a.rise_s > 0.0 && !f.tiled {
                        1.0 / a.rise_s
                    } else {
                        0.0
                    },
                    f32::from(u8::from(a.listen)),
                    0.0,
                ],
            }),
        );
        for stage in 0..2 {
            let label = ["water spray simulate", "water spray emit"][stage];
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: profiler.compute_scope(label),
            });
            pass.set_pipeline(&p.compute[stage]);
            pass.set_bind_group(0, &self.compute, &[]);
            match stage {
                0 => pass.dispatch_workgroups(CAPACITY.div_ceil(64), 1, 1),
                1 => pass.dispatch_workgroups(f.size[0].div_ceil(8), f.size[1].div_ceil(8), 1),
                _ => pass.dispatch_workgroups(1, 1, 1),
            }
        }
        self.last_spec = Some(f);
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Stats {
    pub frame: u32,
    pub ring_head: u32,
    pub born: u32,
    pub survivors_before_spawn: u32,
    pub active_upper_bound: u32,
    pub sources: u32,
    pub draw_instances: u32,
    pub capacity: u32,
}
pub struct Snapshot {
    stage: wgpu::Buffer,
    frame: u32,
}
impl Snapshot {
    fn decode(&self, w: [u32; 8]) -> Stats {
        Stats {
            frame: self.frame,
            ring_head: w[0],
            born: w[1],
            survivors_before_spawn: w[2],
            active_upper_bound: w[1].saturating_add(w[2]).min(CAPACITY),
            sources: w[3],
            draw_instances: w[5],
            capacity: CAPACITY,
        }
    }
    pub async fn read(self) -> Result<Stats, String> {
        let mut w = [0u32; 8];
        crate::readback::read(&self.stage, &mut w).await?;
        Ok(self.decode(w))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_blocking(self, device: &wgpu::Device) -> Result<Stats, String> {
        let mut w = [0u32; 8];
        crate::readback::read_blocking(device, &self.stage, &mut w)?;
        Ok(self.decode(w))
    }
}
impl Pool {
    pub fn copy_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Snapshot {
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water spray diagnostic counters"),
            size: 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = device.create_command_encoder(&Default::default());
        e.copy_buffer_to_buffer(&self.state, 0, &stage, 0, 32);
        queue.submit([e.finish()]);
        Snapshot {
            stage,
            frame: self.frame,
        }
    }
}
