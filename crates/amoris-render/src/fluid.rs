//! A liquid in a box (docs/spikes/fluid-box.md): an experiment with a real fluid solver in the
//! renderer, MLS-MPM on the GPU (shaders/fluid_mpm.wgsl), drawn as a smoothed surface
//! (shaders/fluid_surface.wgsl: the particles' depth and thickness in textures of their own before
//! the opaque pass, a bilateral filter, then a full-screen pass in the transparent layer) or, for
//! looking at the particles, as sphere impostors in the opaque pass (shaders/fluid_draw.wgsl).
//! Presentation only, like the particles: it runs on the GPU's float arithmetic, which differs
//! between vendors, so nothing here reaches the world or its hash.

use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};
use crate::shaders;

/// What to simulate: a box of `grid` cells of `cell_m` metres with its corner at `origin`, a block
/// of liquid filling cells `block_lo..block_hi` with particles `spacing` cells apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FluidSpec {
    pub origin: [f32; 3],
    pub cell_m: f32,
    pub grid: [u32; 3],
    pub block_lo: [f32; 3],
    pub block_hi: [f32; 3],
    pub spacing: f32,
    /// Substeps per frame.
    pub substeps: u32,
    pub tuning: FluidTuning,
}

/// The solver's constants in physical units; the solver works in cells and seconds. A frame is
/// 1/60 s of simulated time, split into the spec's substeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FluidTuning {
    /// m/s2, down.
    pub gravity_mps2: f32,
    /// The speed of sound as a share of a cell per substep (the CFL number): the equation of
    /// state's stiffness follows, so more substeps make a stiffer, less compressible liquid.
    pub sound_cfl: f32,
    /// Particle mass per cell at rest (eight particles a cell at spacing 0.5).
    pub rest_density: f32,
    /// Dynamic viscosity, in the solver's units (mass per cell per second).
    pub viscosity: f32,
}

impl Default for FluidTuning {
    fn default() -> Self {
        FluidTuning {
            gravity_mps2: 9.81,
            sound_cfl: 0.6,
            rest_density: 8.0,
            viscosity: 1.0,
        }
    }
}

impl FluidSpec {
    /// Seconds a substep.
    pub fn dt(&self) -> f32 {
        1.0 / (60.0 * self.substeps.max(1) as f32)
    }

    /// The speed of sound, cells per second.
    pub fn sound(&self) -> f32 {
        self.tuning.sound_cfl / self.dt()
    }

    /// Tait's stiffness for that speed of sound: dp/drho at rest is 4k/rho0 = c^2.
    pub fn stiffness(&self) -> f32 {
        self.sound() * self.sound() * self.tuning.rest_density / 4.0
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Particle {
    x: [f32; 3],
    _a: f32,
    v: [f32; 3],
    _b: f32,
    c: [[f32; 4]; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    grid: [u32; 4],
    dt: f32,
    gravity: f32,
    rest_density: f32,
    stiffness: f32,
    viscosity: f32,
    wall: f32,
    push: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Look {
    origin: [f32; 4],
    color: [f32; 4],
    foam: [f32; 4],
}

/// The surface's textures, at the opaque pass's size.
struct Surface {
    size: (u32, u32),
    /// The particles' nearest depth, then the filter's two passes ping-ponging.
    depth: [wgpu::TextureView; 2],
    zbuffer: wgpu::TextureView,
    thickness: wgpu::TextureView,
}

fn view_of(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl Surface {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Surface {
        let r32 = wgpu::TextureFormat::R32Float;
        Surface {
            size,
            depth: [
                view_of(device, "fluid depth", size, r32),
                view_of(device, "fluid depth (filtered)", size, r32),
            ],
            zbuffer: view_of(device, "fluid z", size, DEPTH),
            thickness: view_of(
                device,
                "fluid thickness",
                size,
                wgpu::TextureFormat::R16Float,
            ),
        }
    }
}

/// The surface's pipelines.
struct SurfacePipes {
    depth: wgpu::RenderPipeline,
    thickness: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    surface: wgpu::RenderPipeline,
}

fn surface_pipes(
    device: &wgpu::Device,
    m: &wgpu::ShaderModule,
    format: SceneFormat,
) -> SurfacePipes {
    let pipe = |label: &str,
                vs: &str,
                fs: &str,
                targets: &[Option<wgpu::ColorTargetState>],
                depth: Option<wgpu::DepthStencilState>,
                multisample: wgpu::MultisampleState| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: None,
            vertex: wgpu::VertexState {
                module: m,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: m,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets,
            }),
            primitive: Default::default(),
            depth_stencil: depth,
            multisample,
            multiview_mask: None,
            cache: None,
        })
    };
    let target = |format, blend| {
        Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })
    };
    let add = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    let premultiplied = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    let one = wgpu::MultisampleState::default();
    SurfacePipes {
        depth: pipe(
            "fluid depth",
            "depth_vs",
            "depth_fs",
            &[target(wgpu::TextureFormat::R32Float, None)],
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            one,
        ),
        thickness: pipe(
            "fluid thickness",
            "thick_vs",
            "thick_fs",
            &[target(wgpu::TextureFormat::R16Float, Some(add))],
            None,
            one,
        ),
        blur: pipe(
            "fluid filter",
            "screen_vs",
            "blur_fs",
            &[target(wgpu::TextureFormat::R32Float, None)],
            None,
            one,
        ),
        surface: pipe(
            "fluid surface",
            "screen_vs",
            "surface_fs",
            &format.targets(Some(premultiplied), false),
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            format.multisample(),
        ),
    }
}

/// The particles of `spec`'s block, in a fixed order with a small deterministic jitter.
pub fn initial_particles(spec: &FluidSpec) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let s = spec.spacing;
    let count = |a: usize| ((spec.block_hi[a] - spec.block_lo[a]) / s).floor().max(0.0) as u32;
    let mut h: u32 = 0x9e37_79b9;
    let mut jitter = || {
        h ^= h << 13;
        h ^= h >> 17;
        h ^= h << 5;
        (h as f32 / u32::MAX as f32 - 0.5) * 0.1 * s
    };
    for k in 0..count(2) {
        for j in 0..count(1) {
            for i in 0..count(0) {
                out.push([
                    spec.block_lo[0] + (i as f32 + 0.5) * s + jitter(),
                    spec.block_lo[1] + (j as f32 + 0.5) * s + jitter(),
                    spec.block_lo[2] + (k as f32 + 0.5) * s + jitter(),
                ]);
            }
        }
    }
    out
}

pub struct Fluid {
    spec: FluidSpec,
    count: u32,
    params: wgpu::Buffer,
    particles: wgpu::Buffer,
    look: wgpu::Buffer,
    group: wgpu::BindGroup,
    clear: wgpu::ComputePipeline,
    p2g_mass: wgpu::ComputePipeline,
    p2g_force: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    g2p: wgpu::ComputePipeline,
    draw_module: wgpu::ShaderModule,
    draw: wgpu::RenderPipeline,
    surface_module: wgpu::ShaderModule,
    pipes: SurfacePipes,
    surface: Option<Surface>,
    /// One look buffer per filter direction (the filter's passes run in one encoder).
    blur_looks: [wgpu::Buffer; 2],
    /// Draw the particles as spheres instead of the surface.
    pub spheres: bool,
    /// A horizontal acceleration on the whole liquid, m/s2 (the box tilted or shaken).
    pub push: [f32; 2],
    /// Frames stepped.
    pub frames: u64,
}

fn draw_pipeline(
    device: &wgpu::Device,
    m: &wgpu::ShaderModule,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fluid"),
        layout: None,
        vertex: wgpu::VertexState {
            module: m,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: m,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &format.targets(None, false),
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: format.multisample(),
        multiview_mask: None,
        cache: None,
    })
}

impl Fluid {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spec: FluidSpec,
        format: SceneFormat,
    ) -> Fluid {
        let positions = initial_particles(&spec);
        let count = positions.len() as u32;
        let particles: Vec<Particle> = positions
            .iter()
            .map(|&x| Particle {
                x,
                _a: 0.0,
                v: [0.0; 3],
                _b: 0.0,
                c: [[0.0; 4]; 3],
            })
            .collect();
        let buf = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let cells = u64::from(spec.grid[0]) * u64::from(spec.grid[1]) * u64::from(spec.grid[2]);
        let params = buf(
            "fluid params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let particle_buf = buf(
            "fluid particles",
            u64::from(count) * std::mem::size_of::<Particle>() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        queue.write_buffer(&particle_buf, 0, bytemuck::cast_slice(&particles));
        let grid = buf("fluid grid", cells * 16, wgpu::BufferUsages::STORAGE);
        let look = buf(
            "fluid look",
            std::mem::size_of::<Look>() as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fluid solver"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fluid solver"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let m = shaders::module(device, "fluid_mpm");
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &m,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid solver"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particle_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: grid.as_entire_binding(),
                },
            ],
        });
        let draw_module = shaders::module(device, "fluid_draw");
        let draw = draw_pipeline(device, &draw_module, format);
        let surface_module = shaders::module(device, "fluid_surface");
        let pipes = surface_pipes(device, &surface_module, format);
        let blur_looks = [
            buf(
                "fluid filter x",
                std::mem::size_of::<Look>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            buf(
                "fluid filter y",
                std::mem::size_of::<Look>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
        ];
        Fluid {
            spec,
            count,
            params,
            particles: particle_buf,
            look,
            group,
            clear: compute("clear_grid"),
            p2g_mass: compute("p2g_mass"),
            p2g_force: compute("p2g_force"),
            update: compute("update_grid"),
            g2p: compute("g2p"),
            draw_module,
            draw,
            surface_module,
            pipes,
            surface: None,
            blur_looks,
            spheres: false,
            push: [0.0; 2],
            frames: 0,
        }
    }

    pub fn spec(&self) -> &FluidSpec {
        &self.spec
    }

    /// The particle count.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The particles' buffer (readbacks for checks).
    pub fn particle_buffer(&self) -> &wgpu::Buffer {
        &self.particles
    }

    pub fn set_format(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.draw = draw_pipeline(device, &self.draw_module, format);
        self.pipes = surface_pipes(device, &self.surface_module, format);
    }

    fn look(&self, blur: [f32; 2]) -> Look {
        let s = &self.spec;
        Look {
            origin: [s.origin[0], s.origin[1], s.origin[2], s.cell_m],
            // Absorption per metre: red goes first, so a thick body is blue-green.
            color: [6.0, 1.8, 0.9, 0.5 * s.spacing * s.cell_m * 1.6],
            foam: [blur[0], blur[1], 0.0, 0.0],
        }
    }

    fn group(
        &self,
        device: &wgpu::Device,
        pipe: &wgpu::RenderPipeline,
        view: &wgpu::Buffer,
        look: &wgpu::Buffer,
        particles: bool,
    ) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: view.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: look.as_entire_binding(),
            },
        ];
        if particles {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: self.particles.as_entire_binding(),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid surface"),
            layout: &pipe.get_bind_group_layout(0),
            entries: &entries,
        })
    }

    /// The depth and, for the surface (the filter reads none), the thickness.
    fn textures(
        &self,
        device: &wgpu::Device,
        pipe: &wgpu::RenderPipeline,
        depth: &wgpu::TextureView,
        thickness: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(depth),
        }];
        if let Some(t) = thickness {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(t),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid surface textures"),
            layout: &pipe.get_bind_group_layout(1),
            entries: &entries,
        })
    }

    /// Before the opaque pass: the particles' depth and thickness at the opaque pass's `size`, and
    /// the filter (two passes, each direction once).
    pub fn prepare_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::Buffer,
        size: (u32, u32),
    ) {
        if self.spheres {
            return;
        }
        if self.surface.as_ref().is_none_or(|s| s.size != size) {
            self.surface = Some(Surface::new(device, size));
        }
        for (i, dir) in [[1.0, 0.0], [0.0, 1.0]].into_iter().enumerate() {
            queue.write_buffer(&self.blur_looks[i], 0, bytemuck::bytes_of(&self.look(dir)));
        }
        let Some(t) = self.surface.as_ref() else {
            return;
        };
        fn color(view: &wgpu::TextureView) -> Option<wgpu::RenderPassColorAttachment<'_>> {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        }
        {
            let g = self.group(device, &self.pipes.depth, view, &self.look, true);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fluid depth"),
                color_attachments: &[color(&t.depth[0])],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.zbuffer,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipes.depth);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..6, 0..self.count);
        }
        {
            let g = self.group(device, &self.pipes.thickness, view, &self.look, true);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fluid thickness"),
                color_attachments: &[color(&t.thickness)],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipes.thickness);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..6, 0..self.count);
        }
        // Twice each way: one pass of a separable bilateral filter leaves the particles' bumps.
        for (i, (from, to)) in [(0, 1), (1, 0), (0, 1), (1, 0)].into_iter().enumerate() {
            let g = self.group(
                device,
                &self.pipes.blur,
                view,
                &self.blur_looks[i % 2],
                false,
            );
            let tx = self.textures(device, &self.pipes.blur, &t.depth[from], None);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fluid filter"),
                color_attachments: &[color(&t.depth[to])],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipes.blur);
            pass.set_bind_group(0, &g, &[]);
            pass.set_bind_group(1, &tx, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// In the transparent layer: the surface over the opaque image, tested against its depth.
    pub fn draw_surface(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        view: &wgpu::Buffer,
    ) {
        let Some(t) = self.surface.as_ref().filter(|_| !self.spheres) else {
            return;
        };
        let g = self.group(device, &self.pipes.surface, view, &self.look, false);
        let tx = self.textures(device, &self.pipes.surface, &t.depth[0], Some(&t.thickness));
        pass.set_pipeline(&self.pipes.surface);
        pass.set_bind_group(0, &g, &[]);
        pass.set_bind_group(1, &tx, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Encodes one frame's substeps.
    pub fn step(&mut self, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder) {
        let s = &self.spec;
        let t = s.tuning;
        let params = Params {
            grid: [s.grid[0], s.grid[1], s.grid[2], self.count],
            dt: s.dt(),
            gravity: -t.gravity_mps2 / s.cell_m,
            rest_density: t.rest_density,
            stiffness: s.stiffness(),
            viscosity: t.viscosity,
            wall: 3.0,
            push: [self.push[0] / s.cell_m, self.push[1] / s.cell_m],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let look = if self.spheres {
            Look {
                origin: [s.origin[0], s.origin[1], s.origin[2], s.cell_m],
                color: [0.04, 0.22, 0.55, 0.5 * s.spacing * s.cell_m * 1.6],
                // Full foam at 2.5 m/s.
                foam: [0.75, 0.85, 0.95, 2.5 / s.cell_m],
            }
        } else {
            self.look([0.0; 2])
        };
        queue.write_buffer(&self.look, 0, bytemuck::bytes_of(&look));
        let cells = s.grid[0] * s.grid[1] * s.grid[2];
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fluid"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        for _ in 0..s.substeps.max(1) {
            for (pipe, n) in [
                (&self.clear, cells),
                (&self.p2g_mass, self.count),
                (&self.p2g_force, self.count),
                (&self.update, cells),
                (&self.g2p, self.count),
            ] {
                pass.set_pipeline(pipe);
                pass.dispatch_workgroups(n.div_ceil(64), 1, 1);
            }
        }
        self.frames += 1;
    }

    /// Draws the particles as spheres in the opaque pass, when asked to (`spheres`).
    pub fn draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        view: &wgpu::Buffer,
    ) {
        if !self.spheres {
            return;
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid draw"),
            layout: &self.draw.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.look.as_entire_binding(),
                },
            ],
        });
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..self.count);
    }
}
