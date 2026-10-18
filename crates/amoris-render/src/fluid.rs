//! Liquid particles on the GPU (docs/spec/splashes.md 4, docs/spikes/fluid-box.md): an MLS-MPM
//! solver (shaders/fluid_mpm.wgsl) in up to four regions, each a box of solver cells with a mask of
//! the blocks in it, over one ring of particles; drawn as a smoothed surface
//! (shaders/fluid_surface.wgsl: the particles' depth and thickness in textures of their own before
//! the opaque pass, a bilateral filter, then a full-screen pass after the transparent instances).
//! The splashes over block liquids (splash.rs) and the fluid box of the spike both run on it.
//! Presentation only, like the particles: it runs on the GPU's float arithmetic, which differs
//! between backends, so nothing here reaches the world or its hash.

use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};
use crate::shaders;

/// The fluid box of the spike: a box of `grid` cells of `cell_m` metres with its corner at
/// `origin`, walls three cells in, a block of liquid filling cells `block_lo..block_hi` with
/// particles `spacing` cells apart.
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

/// The solver's constants in physical units; the solver works in cells and seconds.
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

impl FluidTuning {
    /// Tait's stiffness for a substep of `dt` seconds: the speed of sound is `sound_cfl` cells a
    /// substep, and dp/drho at rest is 4k/rho0 = c^2.
    pub fn stiffness(&self, dt: f32) -> f32 {
        let sound = self.sound_cfl / dt;
        sound * sound * self.rest_density / 4.0
    }
}

/// The solver's shape: every region is `dims` cells with `per_block` cells on a block's edge, over a
/// ring of `capacity` particles that live `lifetime_s` seconds (0: for ever).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolverSpec {
    pub dims: [u32; 3],
    pub per_block: u32,
    pub capacity: u32,
    pub substeps: u32,
    pub lifetime_s: f32,
    /// Particles' spacing at rest, cells (their drawn radius follows).
    pub spacing: f32,
    pub tuning: FluidTuning,
}

impl SolverSpec {
    /// Mask bytes per region and axis.
    pub fn blocks(&self) -> [u32; 3] {
        self.dims.map(|d| d.div_ceil(self.per_block))
    }

    fn mask_words(&self) -> u64 {
        let b = self.blocks();
        (u64::from(b[0]) * u64::from(b[1]) * u64::from(b[2])).div_ceil(4)
    }

    fn region_cells(&self) -> u64 {
        u64::from(self.dims[0]) * u64::from(self.dims[1]) * u64::from(self.dims[2])
    }
}

/// Regions open at once.
pub const SLOTS: usize = 4;
/// A mask byte: a solid block.
pub const SOLID: u8 = 255;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Particle {
    x: [f32; 3],
    age: f32,
    v: [f32; 3],
    tag: u32,
    c: [[f32; 4]; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    dims: [u32; 4],
    blocks: [u32; 4],
    dt: f32,
    gravity: f32,
    rest_density: f32,
    stiffness: f32,
    viscosity: f32,
    lifetime: f32,
    push: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct RegionGpu {
    origin: [f32; 4],
    absorb: [f32; 4],
    generation: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Look {
    size: [f32; 4],
    blur: [f32; 4],
}

/// An open region: its corner in the world, metres per solver cell, its liquid's absorption.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub origin: [f32; 3],
    pub cell_m: f32,
    pub absorb: [f32; 3],
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
                wgpu::TextureFormat::Rgba16Float,
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
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
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
            &[target(wgpu::TextureFormat::Rgba16Float, Some(add))],
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

/// The box's mask: solid within three cells of every face.
pub fn box_mask(grid: [u32; 3]) -> Vec<u8> {
    let wall = 3;
    let mut out = Vec::with_capacity((grid[0] * grid[1] * grid[2]) as usize);
    for z in 0..grid[2] {
        for y in 0..grid[1] {
            for x in 0..grid[0] {
                let c = [x, y, z];
                let inside = (0..3).all(|a| c[a] >= wall && c[a] + wall < grid[a]);
                out.push(if inside { 0 } else { SOLID });
            }
        }
    }
    out
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
    spec: SolverSpec,
    params: wgpu::Buffer,
    particles: wgpu::Buffer,
    regions_buf: wgpu::Buffer,
    masks: wgpu::Buffer,
    look: wgpu::Buffer,
    group: wgpu::BindGroup,
    clear: wgpu::ComputePipeline,
    p2g_mass: wgpu::ComputePipeline,
    p2g_force: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    g2p: wgpu::ComputePipeline,
    surface_module: wgpu::ShaderModule,
    pipes: SurfacePipes,
    surface: Option<Surface>,
    /// One look buffer per filter direction (the filter's passes run in one encoder).
    blur_looks: [wgpu::Buffer; 2],
    regions: [Option<Region>; SLOTS],
    gens: [u32; SLOTS],
    /// The ring's next slot.
    head: u32,
    /// Simulated seconds.
    clock: f64,
    /// Each slot's last spawn (simulated seconds).
    last_spawn: [f64; SLOTS],
    /// No particle can be alive from this time on.
    quiet_from: f64,
    /// A horizontal acceleration on every particle, m/s2 (the box tilted or shaken).
    pub push: [f32; 2],
    /// Frames stepped.
    pub frames: u64,
}

impl Fluid {
    pub fn new(device: &wgpu::Device, spec: SolverSpec, format: SceneFormat) -> Fluid {
        let buf = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let params = buf(
            "fluid params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let particles = buf(
            "fluid particles",
            u64::from(spec.capacity) * std::mem::size_of::<Particle>() as u64,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let grid = buf(
            "fluid grid",
            spec.region_cells() * SLOTS as u64 * 16,
            wgpu::BufferUsages::STORAGE,
        );
        let regions_buf = buf(
            "fluid regions",
            (std::mem::size_of::<RegionGpu>() * SLOTS) as u64,
            wgpu::BufferUsages::UNIFORM,
        );
        let masks = buf(
            "fluid masks",
            spec.mask_words() * SLOTS as u64 * 4,
            wgpu::BufferUsages::STORAGE,
        );
        let look_size = std::mem::size_of::<Look>() as u64;
        let look = buf("fluid look", look_size, wgpu::BufferUsages::UNIFORM);
        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fluid solver"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage(false)),
                entry(2, storage(false)),
                entry(3, wgpu::BufferBindingType::Uniform),
                entry(4, storage(true)),
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
                    resource: particles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: grid.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: regions_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: masks.as_entire_binding(),
                },
            ],
        });
        let surface_module = shaders::module(device, "fluid_surface");
        let pipes = surface_pipes(device, &surface_module, format);
        Fluid {
            spec,
            params,
            particles,
            regions_buf,
            masks,
            look,
            group,
            clear: compute("clear_grid"),
            p2g_mass: compute("p2g_mass"),
            p2g_force: compute("p2g_force"),
            update: compute("update_grid"),
            g2p: compute("g2p"),
            surface_module,
            pipes,
            surface: None,
            blur_looks: [
                buf("fluid filter x", look_size, wgpu::BufferUsages::UNIFORM),
                buf("fluid filter y", look_size, wgpu::BufferUsages::UNIFORM),
            ],
            regions: [None; SLOTS],
            gens: [0; SLOTS],
            head: 0,
            clock: 0.0,
            last_spawn: [f64::NEG_INFINITY; SLOTS],
            quiet_from: 0.0,
            push: [0.0; 2],
            frames: 0,
        }
    }

    /// The fluid box of the spike: one region, its walls in the mask, its block of liquid spawned.
    pub fn boxed(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spec: FluidSpec,
        format: SceneFormat,
    ) -> Fluid {
        let positions = initial_particles(&spec);
        let solver = SolverSpec {
            dims: spec.grid,
            per_block: 1,
            capacity: positions.len() as u32,
            substeps: spec.substeps,
            lifetime_s: 0.0,
            spacing: spec.spacing,
            tuning: spec.tuning,
        };
        let mut f = Fluid::new(device, solver, format);
        f.open(
            queue,
            0,
            Region {
                origin: spec.origin,
                cell_m: spec.cell_m,
                absorb: [6.0, 1.8, 0.9],
            },
            &box_mask(spec.grid),
        );
        let still: Vec<([f32; 3], [f32; 3])> = positions
            .iter()
            .map(|&x| (x.map(|c| c * spec.cell_m), [0.0; 3]))
            .map(|(x, v)| ([0, 1, 2].map(|a| spec.origin[a] + x[a]), v))
            .collect();
        f.spawn(queue, 0, &still);
        f
    }

    pub fn spec(&self) -> &SolverSpec {
        &self.spec
    }

    /// The particles' buffer (readbacks for checks): 80 bytes each, position xyz then age (s),
    /// velocity xyz (cells/s) then the region's tag (`u32::MAX`: dead), the affine field.
    pub fn particle_buffer(&self) -> &wgpu::Buffer {
        &self.particles
    }

    /// Simulated seconds.
    pub fn clock(&self) -> f64 {
        self.clock
    }

    /// The region open in `slot`.
    pub fn region(&self, slot: usize) -> Option<&Region> {
        self.regions.get(slot)?.as_ref()
    }

    /// When `slot` last spawned (simulated seconds; never: negative infinity).
    pub fn last_spawn(&self, slot: usize) -> f64 {
        self.last_spawn[slot]
    }

    /// Whether a particle may be alive: until then, every pass is skipped and frames are those of
    /// a renderer without liquid particles.
    pub fn active(&self) -> bool {
        self.clock < self.quiet_from
    }

    fn write_region(&self, queue: &wgpu::Queue, slot: usize) {
        let r = self.regions[slot].map_or(RegionGpu::default(), |r| RegionGpu {
            origin: [r.origin[0], r.origin[1], r.origin[2], r.cell_m],
            absorb: [r.absorb[0], r.absorb[1], r.absorb[2], 0.0],
            generation: [self.gens[slot], 0, 0, 0],
        });
        let at = (std::mem::size_of::<RegionGpu>() * slot) as u64;
        queue.write_buffer(&self.regions_buf, at, bytemuck::bytes_of(&r));
    }

    /// Opens `region` in `slot` (closing what was there: its particles die) with `mask`, a byte per
    /// block, x fastest, then y, then z.
    pub fn open(&mut self, queue: &wgpu::Queue, slot: usize, region: Region, mask: &[u8]) {
        self.gens[slot] = (self.gens[slot] + 1) & 0x1fff_ffff;
        self.regions[slot] = Some(region);
        self.last_spawn[slot] = f64::NEG_INFINITY;
        self.write_region(queue, slot);
        self.set_mask(queue, slot, mask);
    }

    /// Replaces `slot`'s mask.
    pub fn set_mask(&self, queue: &wgpu::Queue, slot: usize, mask: &[u8]) {
        let words = self.spec.mask_words() as usize;
        let mut bytes = mask.to_vec();
        bytes.resize(words * 4, SOLID);
        queue.write_buffer(&self.masks, (slot * words * 4) as u64, &bytes);
    }

    /// Closes `slot`: its particles die.
    pub fn close(&mut self, queue: &wgpu::Queue, slot: usize) {
        self.regions[slot] = None;
        self.write_region(queue, slot);
    }

    /// Closes every region (a reset frame).
    pub fn clear(&mut self, queue: &wgpu::Queue) {
        for slot in 0..SLOTS {
            if self.regions[slot].is_some() {
                self.close(queue, slot);
            }
        }
        self.quiet_from = self.clock;
    }

    /// New particles in `slot`'s region: world positions and velocities (m/s). They take the
    /// ring's oldest slots.
    pub fn spawn(&mut self, queue: &wgpu::Queue, slot: usize, particles: &[([f32; 3], [f32; 3])]) {
        let Some(r) = self.regions[slot] else {
            return;
        };
        let cap = self.spec.capacity;
        let tag = slot as u32 | (self.gens[slot] << 3);
        let new: Vec<Particle> = particles
            .iter()
            .take(cap as usize)
            .map(|(x, v)| Particle {
                x: [0, 1, 2].map(|a| (x[a] - r.origin[a]) / r.cell_m),
                age: 0.0,
                v: v.map(|c| c / r.cell_m),
                tag,
                c: [[0.0; 4]; 3],
            })
            .collect();
        if new.is_empty() {
            return;
        }
        let size = std::mem::size_of::<Particle>() as u64;
        let first = (cap - self.head).min(new.len() as u32) as usize;
        queue.write_buffer(
            &self.particles,
            u64::from(self.head) * size,
            bytemuck::cast_slice(&new[..first]),
        );
        if first < new.len() {
            queue.write_buffer(&self.particles, 0, bytemuck::cast_slice(&new[first..]));
        }
        self.head = ((u64::from(self.head) + new.len() as u64) % u64::from(cap)) as u32;
        self.last_spawn[slot] = self.clock;
        self.quiet_from = if self.spec.lifetime_s > 0.0 {
            self.quiet_from
                .max(self.clock + f64::from(self.spec.lifetime_s) + 0.1)
        } else {
            f64::INFINITY
        };
    }

    pub fn set_format(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.pipes = surface_pipes(device, &self.surface_module, format);
    }

    fn look(&self, blur: [f32; 2]) -> Look {
        let radius = 0.5 * self.spec.spacing * 1.6;
        let largest = self
            .regions
            .iter()
            .flatten()
            .map(|r| r.cell_m)
            .fold(0.0, f32::max);
        Look {
            size: [radius, self.spec.lifetime_s, radius * largest, 0.0],
            blur: [blur[0], blur[1], 0.0, 0.0],
        }
    }

    fn group(
        &self,
        device: &wgpu::Device,
        pipe: &wgpu::RenderPipeline,
        view: &wgpu::Buffer,
        look: Option<&wgpu::Buffer>,
        particles: bool,
    ) -> wgpu::BindGroup {
        // Automatic layouts hold only what an entry point reads: the composite reads no look.
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: view.as_entire_binding(),
        }];
        if let Some(look) = look {
            entries.push(wgpu::BindGroupEntry {
                binding: 2,
                resource: look.as_entire_binding(),
            });
        }
        if particles {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: self.particles.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 3,
                resource: self.regions_buf.as_entire_binding(),
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
    /// the filter (twice each way).
    pub fn prepare_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::Buffer,
        size: (u32, u32),
    ) {
        if !self.active() {
            return;
        }
        if self.surface.as_ref().is_none_or(|s| s.size != size) {
            self.surface = Some(Surface::new(device, size));
        }
        queue.write_buffer(&self.look, 0, bytemuck::bytes_of(&self.look([0.0; 2])));
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
        let count = self.spec.capacity;
        {
            let g = self.group(device, &self.pipes.depth, view, Some(&self.look), true);
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
            pass.draw(0..6, 0..count);
        }
        {
            let g = self.group(device, &self.pipes.thickness, view, Some(&self.look), true);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fluid thickness"),
                color_attachments: &[color(&t.thickness)],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipes.thickness);
            pass.set_bind_group(0, &g, &[]);
            pass.draw(0..6, 0..count);
        }
        // Twice each way: one pass of a separable bilateral filter leaves the particles' bumps.
        for (i, (from, to)) in [(0, 1), (1, 0), (0, 1), (1, 0)].into_iter().enumerate() {
            let g = self.group(
                device,
                &self.pipes.blur,
                view,
                Some(&self.blur_looks[i % 2]),
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

    /// After the transparent instances: the surface over the image, tested against its depth.
    pub fn draw_surface(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        view: &wgpu::Buffer,
    ) {
        let Some(t) = self.surface.as_ref().filter(|_| self.active()) else {
            return;
        };
        let g = self.group(device, &self.pipes.surface, view, None, false);
        let tx = self.textures(device, &self.pipes.surface, &t.depth[0], Some(&t.thickness));
        pass.set_pipeline(&self.pipes.surface);
        pass.set_bind_group(0, &g, &[]);
        pass.set_bind_group(1, &tx, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Encodes `dt` seconds of simulation (at most 1/30 s) in the spec's substeps.
    pub fn step(&mut self, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, dt: f32) {
        if !self.active() {
            return;
        }
        let s = self.spec;
        let substeps = s.substeps.max(1);
        let dt = dt.clamp(0.0, 1.0 / 30.0) / substeps as f32;
        self.clock += f64::from(dt * substeps as f32);
        self.frames += 1;
        if dt <= 0.0 {
            return;
        }
        let blocks = s.blocks();
        let params = Params {
            dims: [s.dims[0], s.dims[1], s.dims[2], s.capacity],
            blocks: [blocks[0], blocks[1], blocks[2], s.per_block],
            dt,
            gravity: s.tuning.gravity_mps2,
            rest_density: s.tuning.rest_density,
            stiffness: s.tuning.stiffness(dt),
            viscosity: s.tuning.viscosity,
            lifetime: s.lifetime_s,
            push: self.push,
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let cells = (s.region_cells() * SLOTS as u64) as u32;
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("fluid"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        for _ in 0..substeps {
            for (pipe, n) in [
                (&self.clear, cells),
                (&self.p2g_mass, s.capacity),
                (&self.p2g_force, s.capacity),
                (&self.update, cells),
                (&self.g2p, s.capacity),
            ] {
                pass.set_pipeline(pipe);
                pass.dispatch_workgroups(n.div_ceil(64), 1, 1);
            }
        }
    }
}
