//! Dilute GPU spray from current displayed water columns. Presentation only, no field readback.
use crate::{
    post::{DEPTH, SceneFormat},
    profiler::GpuProfiler,
};
use amoris_assets::frame::StirView;
use bytemuck::{Pod, Zeroable};
/// The ring of spray parcels, a multiple of 64 (the draw's list is built in blocks of 64 slots).
pub const CAPACITY: u32 = 16384;
pub const PER_FRAME: u32 = 4096;
/// Births a frame left to splashes and breaking lips (matches water_spray_compute.wgsl): sustained
/// white water must not overwrite a waterfall's sheet in the ring before its drops land.
pub const SPLASH_PER_FRAME: u32 = 64;
/// The sprites a parcel of spray is drawn with (water_spray_draw.wgsl): its chunk of water, the
/// cloud of its drops too fine to draw, then up to this less two of its drops one by one.
pub const SPRITES: u32 = 22;
/// The most stirring points and bodies whose entries a frame reads (water_spray_compute.wgsl
/// `Crowns`); past them, in body order, they throw no crown.
pub const ENTERING_POINTS: usize = 64;
pub const ENTERING_BODIES: usize = 16;
/// The slowest a point goes down to throw a crown, m/s.
const ENTRY_SPEED: f64 = 1.2;
/// Two of the feed's lists further apart than this give no speed: a still scene sends none, and
/// the point may have moved only in the last of the ticks between.
const ENTRY_GAP_S: f64 = 0.1;
/// A stirring point going down between two of the feed's lists (water.md 52): where its centre
/// was and is, its height and volume (a waterplane of `volume / size`), its velocity, and its
/// body, whose points throw one crown. The crown pass tests it against the drawn surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entering {
    pub was: [f32; 3],
    pub now: [f32; 3],
    pub size: f32,
    pub volume: f32,
    pub velocity: [f32; 3],
    pub body: u64,
}
/// The points of `after` going down at [`ENTRY_SPEED`] or faster since `before`, each list with
/// its simulated time: the same entity's same point in both, in `after`'s order.
pub fn entering(before: (f64, &[StirView]), after: (f64, &[StirView])) -> Vec<Entering> {
    let dt = after.0 - before.0;
    if dt <= 0.0 || dt > ENTRY_GAP_S {
        return Vec::new();
    }
    let was: std::collections::HashMap<(u64, u32), [f32; 3]> =
        before.1.iter().map(|s| ((s.id, s.point), s.at)).collect();
    after
        .1
        .iter()
        .filter_map(|s| {
            let w = was.get(&(s.id, s.point))?;
            let velocity =
                std::array::from_fn(|a| ((f64::from(s.at[a]) - f64::from(w[a])) / dt) as f32);
            (f64::from(velocity[1]) <= -ENTRY_SPEED).then_some(Entering {
                was: *w,
                now: s.at,
                size: s.size,
                volume: s.volume,
                velocity,
                body: s.id,
            })
        })
        .collect()
}
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CrownPoint {
    was: [f32; 4],
    now: [f32; 4],
    velocity: [f32; 4],
}
/// The crown pass's points, a body's together (water_spray_compute.wgsl `Crowns`).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Crowns {
    count: [u32; 4],
    points: [CrownPoint; ENTERING_POINTS],
    bodies: [[u32; 4]; ENTERING_BODIES],
}
impl Crowns {
    /// `entering` by body (each body's points in their order), within the pass's bounds.
    fn new(entering: &[Entering]) -> Crowns {
        let mut sorted = entering.to_vec();
        sorted.sort_by_key(|e| e.body);
        let mut c = Crowns::zeroed();
        let (mut points, mut bodies) = (0usize, 0usize);
        for e in sorted {
            let same = bodies > 0 && c.bodies[bodies - 1][2..] == key(e.body);
            if points == ENTERING_POINTS || (!same && bodies == ENTERING_BODIES) {
                continue;
            }
            if !same {
                let [lo, hi] = key(e.body);
                c.bodies[bodies] = [points as u32, 0, lo, hi];
                bodies += 1;
            }
            c.bodies[bodies - 1][1] += 1;
            c.points[points] = CrownPoint {
                was: [e.was[0], e.was[1], e.was[2], e.size],
                now: [e.now[0], e.now[1], e.now[2], e.volume],
                velocity: [e.velocity[0], e.velocity[1], e.velocity[2], 0.0],
            };
            points += 1;
        }
        c.count = [points as u32, bodies as u32, 0, 0];
        c
    }
}
/// A body's id as the crown pass keys it: its low and high words.
fn key(body: u64) -> [u32; 2] {
    [body as u32, (body >> 32) as u32]
}
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
    sound: [f32; 4],
}
pub struct Advance<'a> {
    pub field: FieldSpec,
    pub dt: f32,
    pub changed: bool,
    pub clear: bool,
    /// The stirring points that went down fast since the last advance (water.md 52).
    pub entering: &'a [Entering],
    /// Tally what the sources sound like from the camera (water_acoustics.rs).
    pub listen: bool,
}
pub struct Pipelines {
    layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    compute: [wgpu::ComputePipeline; 5],
    draw: wgpu::RenderPipeline,
    module: wgpu::ShaderModule,
    pub budget: wgpu::Buffer,
    /// The frame's tally of sound-making events, shared by every region (water_acoustics.rs).
    pub acoustics: wgpu::Buffer,
}
pub struct Pool {
    pub(crate) params: wgpu::Buffer,
    crowns: wgpu::Buffer,
    pub particles: wgpu::Buffer,
    pub state: wgpu::Buffer,
    /// The liquid surface's indirect draw of the parcels alive (fluid_surface.rs): a quad each.
    pub(crate) splat_args: wgpu::Buffer,
    compute: wgpu::BindGroup,
    last_spec: Option<FieldSpec>,
    frame: u32,
}
fn source(body: &str) -> String {
    format!(
        "{}\n{}\nconst SPRAY_SPRITES: u32 = {SPRITES}u;\nconst SPRAY_CAPACITY: u32 = {CAPACITY}u;\n{body}",
        include_str!("../shaders/common.wgsl"),
        include_str!("../shaders/water_spray_common.wgsl")
    )
}

/// The light the spray is drawn in: the sky's irradiance (spherical harmonics) and the sun's
/// shadow cascades with their comparison sampler.
pub struct Light<'a> {
    pub sh: &'a wgpu::Buffer,
    pub shadows: &'a wgpu::TextureView,
    pub shadow_sampler: &'a wgpu::Sampler,
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
            // The view and the parameters, the columns, then the spray's own storage, then the
            // crowns' points.
            entries: &std::array::from_fn::<_, 9, _>(|i| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if i < 2 || i == 8 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: i == 2 }
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
        let compute = ["simulate", "crown", "emit", "count", "list"].map(|name| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(name),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water spray draw"),
            entries: &[
                buffer(wgpu::BufferBindingType::Uniform),
                buffer(wgpu::BufferBindingType::Uniform),
                buffer(wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(wgpu::BufferBindingType::Uniform),
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                buffer(wgpu::BufferBindingType::Storage { read_only: true }),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, ty)| wgpu::BindGroupLayoutEntry {
                binding: i as u32,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty,
                count: None,
            })
            .collect::<Vec<_>>(),
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
        Self {
            layout,
            draw_layout,
            compute,
            draw,
            module,
            budget,
            acoustics,
        }
    }
    pub fn reformat(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.draw = draw_pipeline(device, &self.module, &self.draw_layout, format);
    }
    pub fn draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        pool: &Pool,
        view: &wgpu::Buffer,
        light: &Light<'_>,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water spray draw"),
            layout: &self.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pool.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pool.particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: light.sh.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(light.shadows),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(light.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: pool.state.as_entire_binding(),
                },
            ],
        });
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(0, &group, &[]);
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
        let crowns = buffer(
            "water spray crowns",
            std::mem::size_of::<Crowns>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let particles = buffer(
            "water spray particles",
            u64::from(CAPACITY) * 48,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        // The counters and the indirect draw's arguments, a slot a parcel drawn, and a count a
        // block of 64 slots (water_spray_compute.wgsl `SprayState`).
        let state = buffer(
            "water spray state and indirect draw",
            32 + 4 * u64::from(CAPACITY + CAPACITY / 64),
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let splat_args = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water spray liquid splats"),
                contents: bytemuck::cast_slice(&[6u32, 0, 0, 0]),
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            },
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
                &p.acoustics,
                &crowns,
            ]
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
            crowns,
            particles,
            state,
            splat_args,
            compute,
            last_spec: None,
            frame: 0,
        }
    }
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        p: &Pipelines,
        a: Advance<'_>,
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
                // Whether the sources are heard (water_acoustics.rs).
                sound: [f32::from(u8::from(a.listen)), 0.0, 0.0, 0.0],
            }),
        );
        // Crowns only where something went in: a frame without leaves the pass out.
        let crowning = !a.entering.is_empty() && dt > 0.0;
        if crowning {
            queue.write_buffer(
                &self.crowns,
                0,
                bytemuck::bytes_of(&Crowns::new(a.entering)),
            );
        }
        for stage in 0..5 {
            if stage == 1 && !crowning {
                continue;
            }
            let label = [
                "water spray simulate",
                "water spray crowns",
                "water spray emit",
                "water spray count",
                "water spray list",
            ][stage];
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: profiler.compute_scope(label),
            });
            pass.set_pipeline(&p.compute[stage]);
            pass.set_bind_group(0, &self.compute, &[]);
            match stage {
                0 => pass.dispatch_workgroups(CAPACITY.div_ceil(64), 1, 1),
                1 => pass.dispatch_workgroups(1, 1, 1),
                2 => pass.dispatch_workgroups(f.size[0].div_ceil(8), f.size[1].div_ceil(8), 1),
                _ => pass.dispatch_workgroups(CAPACITY / 64, 1, 1),
            }
        }
        // The draw's instances, the parcels listed, for the liquid surface's quads.
        encoder.copy_buffer_to_buffer(&self.state, 20, &self.splat_args, 4, 4);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stir(id: u64, point: u32, y: f32) -> StirView {
        StirView {
            id,
            point,
            at: [1.0, y, 2.0],
            size: 0.3,
            volume: 0.018,
        }
    }

    #[test]
    fn points_going_down_fast_are_entering_and_others_not() {
        let before = [stir(1, 0, 1.2), stir(1, 1, 1.2), stir(2, 0, 1.2)];
        // Point (1, 0) falls 0.1 m in a tick (6 m/s), (1, 1) 0.01 m (0.6 m/s), (2, 0) rises, and
        // (3, 0) has no place before.
        let after = [
            stir(1, 0, 1.1),
            stir(1, 1, 1.19),
            stir(2, 0, 1.3),
            stir(3, 0, 0.0),
        ];
        let tick = 1.0 / 60.0;
        let going = entering((1.0, &before), (1.0 + tick, &after));
        assert_eq!(going.len(), 1);
        let e = going[0];
        assert_eq!((e.body, e.was[1], e.now[1]), (1, 1.2, 1.1));
        assert!((e.velocity[1] + 6.0).abs() < 1e-3, "{e:?}");
        // Lists a still scene left far apart, or out of order, give no speed.
        assert!(entering((1.0, &before), (1.5, &after)).is_empty());
        assert!(entering((1.0, &before), (1.0, &after)).is_empty());
    }

    #[test]
    fn a_bodys_points_go_together_within_the_pass_bounds() {
        let point = |body| Entering {
            was: [0.0; 3],
            now: [0.0; 3],
            size: 0.3,
            volume: 0.018,
            velocity: [0.0, -3.0, 0.0],
            body,
        };
        // Bodies 9, 4, 9: body 4's point first, then body 9's two.
        let c = Crowns::new(&[point(9), point(4), point(9)]);
        assert_eq!(c.count[..2], [3, 2]);
        assert_eq!(c.bodies[0], [0, 1, 4, 0]);
        assert_eq!(c.bodies[1], [1, 2, 9, 0]);
        // Past 16 bodies or 64 points, the rest throw nothing.
        let many: Vec<Entering> = (0..20u64).flat_map(|b| [point(b); 4]).collect();
        let c = Crowns::new(&many);
        assert_eq!(c.count[..2], [64, 16]);
        let c = Crowns::new(&[point(1); 70]);
        assert_eq!(c.count[..2], [64, 1]);
        assert_eq!(c.bodies[0][1], 64);
    }
}
