//! The liquid surface (docs/spec/splashes.md 5; shaders/fluid_surface.wgsl): liquid particles drawn
//! as one refracting surface once the frame's last colour pass has drawn. Their spheres go into
//! textures of their own, tested against the scene's depth (the nearest front with its filter radius
//! and air, and the summed thickness); a separable bilateral filter smooths depth and thickness; the
//! frame is copied, and a full-screen composite shades the liquid over it, seeing the copy through
//! the liquid along the refracted ray. Screen-space fluid rendering after van der Laan, Green and
//! Sainz (I3D 2009) and S. Lague's Fluid-Sim (github.com/SebLague/Fluid-Sim at 4717b72, MIT).
//! Its particles are MLS-MPM solvers' and the spray's entry crowns (water.md 52), whose sheets it
//! draws where they are wide enough on the screen, the spray's streaks the rest. Presentation only:
//! nothing here reaches the world; the same GPU and backend draw the same frames.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::fluid::Fluid;
use crate::post::{DEPTH, HDR, Targets};
use crate::profiler::GpuProfiler;
use crate::shaders;
use crate::water_spray::Pool;

/// The filter's horizontal and vertical pass pairs: one pair leaves the particles' bumps.
const FILTER_PAIRS: usize = 3;
/// The smoothed depth and air, and the thickness (rgb optical depth, a length).
const SMOOTH: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;
const THICK: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const ATTRS: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A source of liquid particles.
pub enum Source<'a> {
    /// An MLS-MPM solver's particles, `air` 0 for clear water to 1 for white.
    Mpm { fluid: &'a Fluid, air: f32 },
    /// A water region's spray, of which the entry crowns' parcels are drawn.
    Spray { pool: &'a Pool },
}

/// The light the composite reads.
pub struct Light<'a> {
    pub env_cube: &'a wgpu::TextureView,
    pub env_sampler: &'a wgpu::Sampler,
    pub sh: &'a wgpu::Buffer,
    pub shadows: &'a wgpu::TextureView,
    pub shadow_sampler: &'a wgpu::Sampler,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MpmSplat {
    size: [f32; 4],
    look: [f32; 4],
}

impl MpmSplat {
    /// A solver's spheres: the depth sphere as before (0.4 of a cell at spacing 0.5), the thickness
    /// sphere the particle's own volume (spacing^3 cells: a radius of 0.62 spacings), and a filter
    /// reach of 5.4 depth radii, which keeps the old filter's two radii of deviation a pass.
    fn new(fluid: &Fluid, air: f32) -> MpmSplat {
        let spec = fluid.spec();
        let depth = 0.5 * spec.spacing * 1.6;
        let thick = spec.spacing * (3.0 / (4.0 * std::f32::consts::PI)).cbrt();
        MpmSplat {
            size: [depth, spec.lifetime_s, thick, 5.4],
            look: [air, 0.0, 0.0, 0.0],
        }
    }
}

struct Textures {
    size: (u32, u32),
    /// The nearest front's distance and air, as drawn.
    raw: wgpu::TextureView,
    /// Its filter radius and share.
    attrs: wgpu::TextureView,
    /// The filter's ping-pong pairs; the splats add their thickness into `thick[1]`.
    smooth: [wgpu::TextureView; 2],
    thick: [wgpu::TextureView; 2],
    zbuffer: wgpu::TextureView,
    /// The frame as it stood before the composite.
    background: wgpu::Texture,
    background_view: wgpu::TextureView,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
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
        usage,
        view_formats: &[],
    })
}

impl Textures {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Textures {
        let target = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let view = |label, format, usage| {
            texture(device, label, size, format, usage).create_view(&Default::default())
        };
        let filtered = target | wgpu::TextureUsages::STORAGE_BINDING;
        let background = texture(
            device,
            "liquid background",
            size,
            HDR,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Textures {
            size,
            raw: view("liquid depth", SMOOTH, target),
            attrs: view("liquid attributes", ATTRS, target),
            smooth: [
                view("liquid smoothed depth a", SMOOTH, filtered),
                view("liquid smoothed depth b", SMOOTH, filtered),
            ],
            thick: [
                view("liquid thickness a", THICK, filtered),
                view("liquid thickness b", THICK, filtered),
            ],
            zbuffer: texture(
                device,
                "liquid z",
                size,
                DEPTH,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default()),
            background_view: background.create_view(&Default::default()),
            background,
        }
    }
}

struct Pipes {
    samples: u32,
    frame: wgpu::BindGroupLayout,
    mpm: wgpu::BindGroupLayout,
    spray: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
    filter_textures: wgpu::BindGroupLayout,
    light: wgpu::BindGroupLayout,
    mpm_thick: wgpu::RenderPipeline,
    mpm_depth: wgpu::RenderPipeline,
    spray_thick: wgpu::RenderPipeline,
    spray_depth: wgpu::RenderPipeline,
    filter: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
}

fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn uniform() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn float_texture(filterable: bool, dimension: wgpu::TextureViewDimension) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable },
        view_dimension: dimension,
        multisampled: false,
    }
}

/// The surface's source for an opaque pass of `samples` samples, with what it shares with the
/// spray (the crowns' parcels and their hand-off).
fn source(samples: u32) -> String {
    format!(
        "{}\n{}",
        shaders::depth_source("fluid_surface", samples),
        include_str!("../shaders/water_spray_common.wgsl")
    )
}

impl Pipes {
    fn new(device: &wgpu::Device, samples: u32) -> Pipes {
        use wgpu::ShaderStages as S;
        let both = S::VERTEX_FRAGMENT;
        let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid frame"),
            entries: &[
                layout_entry(0, both | S::COMPUTE, uniform()),
                layout_entry(
                    1,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: samples > 1,
                    },
                ),
            ],
        });
        let read = || wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let mpm = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid mpm"),
            entries: &[
                layout_entry(0, S::VERTEX, read()),
                layout_entry(1, S::VERTEX, uniform()),
                layout_entry(2, S::VERTEX, uniform()),
            ],
        });
        let spray = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid spray"),
            entries: &[
                layout_entry(0, S::VERTEX, read()),
                layout_entry(1, S::VERTEX, read()),
                layout_entry(2, S::VERTEX, uniform()),
            ],
        });
        let d2 = wgpu::TextureViewDimension::D2;
        let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid textures"),
            entries: &[
                layout_entry(0, S::FRAGMENT, float_texture(false, d2)),
                layout_entry(1, S::FRAGMENT, float_texture(false, d2)),
                layout_entry(2, S::FRAGMENT, float_texture(false, d2)),
                layout_entry(3, S::FRAGMENT, float_texture(false, d2)),
                layout_entry(4, S::FRAGMENT, uniform()),
            ],
        });
        let storage = |format| wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: d2,
        };
        let filter_textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid filter"),
            entries: &[
                layout_entry(0, S::COMPUTE, float_texture(false, d2)),
                layout_entry(1, S::COMPUTE, float_texture(false, d2)),
                layout_entry(2, S::COMPUTE, float_texture(false, d2)),
                layout_entry(3, S::COMPUTE, float_texture(false, d2)),
                layout_entry(4, S::COMPUTE, uniform()),
                layout_entry(5, S::COMPUTE, storage(SMOOTH)),
                layout_entry(6, S::COMPUTE, storage(THICK)),
            ],
        });
        let light = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid light"),
            entries: &[
                layout_entry(0, S::FRAGMENT, float_texture(true, d2)),
                layout_entry(
                    1,
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                layout_entry(
                    2,
                    S::FRAGMENT,
                    float_texture(true, wgpu::TextureViewDimension::Cube),
                ),
                layout_entry(
                    3,
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                layout_entry(4, S::FRAGMENT, uniform()),
                layout_entry(
                    5,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    6,
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fluid_surface"),
            source: wgpu::ShaderSource::Wgsl(source(samples).into()),
        });
        let layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
            let groups: Vec<Option<&wgpu::BindGroupLayout>> =
                groups.iter().map(|g| Some(*g)).collect();
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            })
        };
        let splat_layout = layout("liquid splats", &[&frame, &mpm]);
        let spray_layout = layout("liquid crowns", &[&frame, &spray]);
        let filter_layout = layout("liquid filter", &[&frame, &filter_textures]);
        let composite_layout = layout("liquid composite", &[&frame, &textures, &light]);
        let target = |format, blend, write_mask| {
            Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask,
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
        let all = wgpu::ColorWrites::ALL;
        let none = wgpu::ColorWrites::empty();
        let pipe = |label: &str,
                    layout: &wgpu::PipelineLayout,
                    vs: &str,
                    fs: &str,
                    targets: &[Option<wgpu::ColorTargetState>],
                    depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                primitive: Default::default(),
                depth_stencil: depth,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let zbuffer = |compare, write| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
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
        Pipes {
            samples,
            mpm_thick: pipe(
                "liquid thickness",
                &splat_layout,
                "mpm_thick_vs",
                "splat_thick_fs",
                &[
                    target(SMOOTH, None, none),
                    target(ATTRS, None, none),
                    target(THICK, Some(add), all),
                ],
                zbuffer(wgpu::CompareFunction::Always, false),
            ),
            mpm_depth: pipe(
                "liquid depth",
                &splat_layout,
                "mpm_depth_vs",
                "splat_depth_fs",
                &[
                    target(SMOOTH, None, all),
                    target(ATTRS, None, all),
                    target(THICK, None, none),
                ],
                zbuffer(wgpu::CompareFunction::Greater, true),
            ),
            spray_thick: pipe(
                "liquid crown thickness",
                &spray_layout,
                "spray_vs",
                "splat_thick_fs",
                &[
                    target(SMOOTH, None, none),
                    target(ATTRS, None, none),
                    target(THICK, Some(add), all),
                ],
                zbuffer(wgpu::CompareFunction::Always, false),
            ),
            spray_depth: pipe(
                "liquid crown depth",
                &spray_layout,
                "spray_vs",
                "splat_depth_fs",
                &[
                    target(SMOOTH, None, all),
                    target(ATTRS, None, all),
                    target(THICK, None, none),
                ],
                zbuffer(wgpu::CompareFunction::Greater, true),
            ),
            filter: device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("liquid filter"),
                layout: Some(&filter_layout),
                module: &module,
                entry_point: Some("filter_cs"),
                compilation_options: shaders::compute_options(),
                cache: None,
            }),
            composite: pipe(
                "liquid composite",
                &composite_layout,
                "screen_vs",
                "composite_fs",
                &[target(HDR, Some(premultiplied), all)],
                None,
            ),
            frame,
            mpm,
            spray,
            textures,
            filter_textures,
            light,
        }
    }
}

/// How a source's splats are drawn: a quad for each of a solver's particles, or for each parcel
/// the spray lists (its count on the GPU).
enum Draw<'a> {
    Particles(u32),
    Crowns(&'a wgpu::Buffer),
}

pub struct FluidSurface {
    pipes: Option<Pipes>,
    textures: Option<Textures>,
    /// One splat uniform per source, in the order given.
    splats: Vec<wgpu::Buffer>,
    /// The filter's step directions, horizontal then vertical.
    steps: [wgpu::Buffer; 2],
    linear: wgpu::Sampler,
}

impl FluidSurface {
    pub fn new(device: &wgpu::Device) -> FluidSurface {
        let step = |label, dir: [f32; 4]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&dir),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        FluidSurface {
            pipes: None,
            textures: None,
            splats: Vec::new(),
            steps: [
                step("liquid filter x", [1.0, 0.0, 0.0, 0.0]),
                step("liquid filter y", [0.0, 1.0, 0.0, 0.0]),
            ],
            linear: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("liquid background"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
        }
    }

    /// Draws `sources` as one surface into `targets.color` (resolved), after the frame's last colour
    /// pass and before the water's medium and TAA. `view` is the frame's view uniform.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        targets: &Targets,
        view: &wgpu::Buffer,
        light: &Light<'_>,
        sources: &[Source<'_>],
    ) {
        if sources.is_empty() {
            return;
        }
        let samples = targets.format.samples;
        if self.pipes.as_ref().is_none_or(|p| p.samples != samples) {
            self.pipes = Some(Pipes::new(device, samples));
        }
        if self
            .textures
            .as_ref()
            .is_none_or(|t| t.size != targets.scene)
        {
            self.textures = Some(Textures::new(device, targets.scene));
        }
        while self.splats.len() < sources.len() {
            self.splats
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("liquid splats"),
                    size: std::mem::size_of::<MpmSplat>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let (Some(p), Some(t)) = (self.pipes.as_ref(), self.textures.as_ref()) else {
            return;
        };
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquid frame"),
            layout: &p.frame,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.depth),
                },
            ],
        });
        let mut groups = Vec::with_capacity(sources.len());
        for (i, s) in sources.iter().enumerate() {
            match s {
                Source::Spray { pool } => {
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("liquid spray"),
                        layout: &p.spray,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: pool.particles.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: pool.state.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: pool.params.as_entire_binding(),
                            },
                        ],
                    });
                    groups.push((group, Draw::Crowns(&pool.splat_args)));
                }
                Source::Mpm { fluid, air } => {
                    queue.write_buffer(
                        &self.splats[i],
                        0,
                        bytemuck::bytes_of(&MpmSplat::new(fluid, *air)),
                    );
                    groups.push((
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("liquid mpm"),
                            layout: &p.mpm,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: fluid.particle_buffer().as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: fluid.regions_buffer().as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: self.splats[i].as_entire_binding(),
                                },
                            ],
                        }),
                        Draw::Particles(fluid.spec().capacity),
                    ));
                }
            }
        }
        fn clear(view: &wgpu::TextureView) -> Option<wgpu::RenderPassColorAttachment<'_>> {
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
        // The splats: thickness first (every sphere's chord added up), then the nearest front.
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid splats"),
                color_attachments: &[clear(&t.raw), clear(&t.attrs), clear(&t.thick[1])],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.zbuffer,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: profiler.render_scope("liquid splats"),
                ..Default::default()
            });
            pass.set_bind_group(0, &frame, &[]);
            for thick in [true, false] {
                for (group, draw) in &groups {
                    pass.set_bind_group(1, group, &[]);
                    match draw {
                        Draw::Particles(count) => {
                            pass.set_pipeline(if thick { &p.mpm_thick } else { &p.mpm_depth });
                            pass.draw(0..6, 0..*count);
                        }
                        Draw::Crowns(args) => {
                            pass.set_pipeline(if thick {
                                &p.spray_thick
                            } else {
                                &p.spray_depth
                            });
                            pass.draw_indirect(args, 0);
                        }
                    }
                }
            }
        }
        let tex = |smooth: &wgpu::TextureView, thick: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("liquid textures"),
                layout: &p.textures,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&t.raw),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&t.attrs),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(smooth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(thick),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.steps[0].as_entire_binding(),
                    },
                ],
            })
        };
        // The filter: pass k reads what pass k - 1 wrote (the first, the splats as drawn), and
        // writes the other of each pair; the last writes `smooth[1]` and `thick[1]`.
        let passes = 2 * FILTER_PAIRS;
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("liquid filter"),
                timestamp_writes: profiler.compute_scope("liquid filter"),
            });
            pass.set_pipeline(&p.filter);
            pass.set_bind_group(0, &frame, &[]);
            for k in 0..passes {
                let to = k % 2;
                let (smooth_in, thick_in) = if k == 0 {
                    (&t.raw, &t.thick[1])
                } else {
                    (&t.smooth[1 - to], &t.thick[1 - to])
                };
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("liquid filter"),
                    layout: &p.filter_textures,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&t.raw),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&t.attrs),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(smooth_in),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(thick_in),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self.steps[k % 2].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(&t.smooth[to]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(&t.thick[to]),
                        },
                    ],
                });
                pass.set_bind_group(1, &g, &[]);
                pass.dispatch_workgroups(t.size.0.div_ceil(8), t.size.1.div_ceil(8), 1);
            }
        }
        let last = (passes - 1) % 2;
        // The frame as it stands, for the refraction to see through the liquid.
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.color.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &t.background,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: t.size.0,
                height: t.size.1,
                depth_or_array_layers: 1,
            },
        );
        let g = tex(&t.smooth[last], &t.thick[last]);
        let lit = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("liquid light"),
            layout: &p.light,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&t.background_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(light.env_cube),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(light.env_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: light.sh.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(light.shadows),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(light.shadow_sampler),
                },
            ],
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("liquid composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.color.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes: profiler.render_scope("liquid composite"),
            ..Default::default()
        });
        pass.set_pipeline(&p.composite);
        pass.set_bind_group(0, &frame, &[]);
        pass.set_bind_group(1, &g, &[]);
        pass.set_bind_group(2, &lit, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    /// The composed source validates for an opaque pass of one sample and of four (the scene's
    /// depth is then `texture_depth_2d` or multisampled). Kept here: shaders.rs is part of the
    /// water's execution key.
    #[test]
    fn the_surface_validates_for_single_and_multisample_depth() {
        for samples in [1, 4] {
            let source = super::source(samples);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{samples}: {}", e.emit_to_string(&source)));
            Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{samples}: {e:?}"));
        }
    }
}
