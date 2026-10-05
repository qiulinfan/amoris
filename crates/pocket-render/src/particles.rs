//! GPU particles for `ParticleEmitter`s: the CPU only decides how many particles each emitter
//! releases this frame (rate and bursts) and where in a ring-buffer pool they go; spawning, motion
//! and drawing run on the GPU. Presentation: particles never feed back into the simulation.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use pocket_assets::frame::EmitterView;

use crate::post::{DEPTH, HDR, SAMPLES};
use crate::shaders;

pub const POOL: u32 = 1 << 17;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EmitterGpu {
    pos: [f32; 4],
    up: [f32; 4],
    accel: [f32; 4],
    size: [f32; 4],
    color_start: [f32; 4],
    color_end: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Spawn {
    first: u32,
    count: u32,
    emitter: u32,
    seed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SimParams {
    dt: f32,
    pool: u32,
    spawns: u32,
    _p: u32,
}

pub struct Particles {
    spawn: wgpu::ComputePipeline,
    simulate: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    pool: wgpu::Buffer,
    emitters: wgpu::Buffer,
    spawns: wgpu::Buffer,
    params: wgpu::Buffer,
    head: u32,
    /// Fractional particles carried to the next frame, and the last burst seen, per emitter.
    carry: HashMap<u64, (f32, u32)>,
    frame: u32,
    active: bool,
}

impl Particles {
    pub fn new(device: &wgpu::Device) -> Particles {
        let m = shaders::module(device, "particles");
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &m,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let draw = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("particles"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &m,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &m,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR,
                    blend: Some(premultiplied),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let buf = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Particles {
            spawn: compute("spawn"),
            simulate: compute("simulate"),
            draw,
            pool: buf("particle pool", u64::from(POOL) * 48, wgpu::BufferUsages::STORAGE),
            emitters: buf("emitters", 256 * 96, wgpu::BufferUsages::STORAGE),
            spawns: buf("spawns", 256 * 16, wgpu::BufferUsages::STORAGE),
            params: buf("particle params", 16, wgpu::BufferUsages::UNIFORM),
            head: 0,
            carry: HashMap::new(),
            frame: 0,
            active: false,
        }
    }

    /// Spawns this frame's particles and advances every particle by `dt` seconds.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        emitters: &[EmitterView],
        dt: f32,
    ) {
        if emitters.is_empty() && !self.active {
            return;
        }
        self.active = !emitters.is_empty() || self.active;
        self.frame = self.frame.wrapping_add(1);
        let mut gpu = Vec::with_capacity(emitters.len());
        let mut spawns = Vec::new();
        for (i, e) in emitters.iter().take(256).enumerate() {
            gpu.push(EmitterGpu {
                pos: [e.position[0], e.position[1], e.position[2], e.radius],
                up: [e.up[0], e.up[1], e.up[2], e.spread_deg.to_radians().cos()],
                accel: [e.acceleration[0], e.acceleration[1], e.acceleration[2], e.drag],
                size: [e.size[0], e.size[1], e.speed, e.lifetime.max(0.01)],
                color_start: e.color_start,
                color_end: e.color_end,
            });
            let (carry, last_burst) = self.carry.entry(e.id).or_insert((0.0, e.burst_id));
            let mut n = 0u32;
            if e.emitting {
                *carry += e.rate * dt;
                n = carry.floor() as u32;
                *carry -= n as f32;
            }
            if e.burst_id != *last_burst {
                *last_burst = e.burst_id;
                n += e.burst;
            }
            let n = n.min(POOL / 4);
            if n > 0 {
                spawns.push(Spawn {
                    first: self.head,
                    count: n,
                    emitter: i as u32,
                    seed: (self.frame.wrapping_mul(0x9e37_79b9)) ^ (e.id as u32).wrapping_mul(0x85eb_ca6b),
                });
                self.head = (self.head + n) % POOL;
            }
        }
        if spawns.len() > 256 {
            spawns.truncate(256);
        }
        let total: u32 = spawns.iter().map(|s| s.count).sum();
        if !gpu.is_empty() {
            queue.write_buffer(&self.emitters, 0, bytemuck::cast_slice(&gpu));
        }
        if !spawns.is_empty() {
            queue.write_buffer(&self.spawns, 0, bytemuck::cast_slice(&spawns));
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&SimParams {
                dt,
                pool: POOL,
                spawns: spawns.len() as u32,
                _p: 0,
            }),
        );
        let entries = |p: &wgpu::ComputePipeline| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particles"),
                layout: &p.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.pool.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.emitters.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: self.spawns.as_entire_binding() },
                ],
            })
        };
        let spawn_bg = (total > 0).then(|| entries(&self.spawn));
        let sim_bg = {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particles"),
                layout: &self.simulate.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.pool.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.emitters.as_entire_binding() },
                ],
            })
        };
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("particles"),
            timestamp_writes: None,
        });
        if let Some(bg) = &spawn_bg {
            pass.set_pipeline(&self.spawn);
            pass.set_bind_group(0, bg, &[]);
            pass.dispatch_workgroups(total.div_ceil(64), 1, 1);
        }
        pass.set_pipeline(&self.simulate);
        pass.set_bind_group(0, &sim_bg, &[]);
        pass.dispatch_workgroups(POOL.div_ceil(64), 1, 1);
    }

    /// Draws the live particles (inside the HDR pass, after opaque geometry).
    pub fn draw(&self, device: &wgpu::Device, pass: &mut wgpu::RenderPass<'_>, view: &wgpu::Buffer) {
        if !self.active {
            return;
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particles draw"),
            layout: &self.draw.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 4, resource: view.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: self.pool.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: self.emitters.as_entire_binding() },
            ],
        });
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..POOL);
    }
}
