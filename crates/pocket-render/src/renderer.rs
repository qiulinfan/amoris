//! The renderer: keeps the retained scene in step with the render feed and draws a frame.
//!
//! Per frame: write what changed (instances, materials, meshes, lights), then on the GPU
//! 1. cull every instance against the camera and the four shadow cascades in one dispatch, which
//!    writes the visible lists and the instance counts of every indirect draw;
//! 2. assign punctual lights to the cluster grid;
//! 3. draw each shadow cascade (one multi-draw each);
//! 4. draw the opaque scene and the sky into the multisampled HDR target (one multi-draw);
//! 5. bloom and the display transform into the output.
//! The CPU's work per frame does not grow with the number of instances.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use pocket_assets::frame::{LightKindView, Look, RenderFrame};
use pocket_assets::mesh::ModelAsset;
use pocket_assets::primitives::{PRIMITIVES, primitive};

use crate::blit::Blitter;
use crate::camera::{CameraState, frustum_planes};
use crate::gpu::Gpu;
use crate::loader::{AssetSource, NoAssets};
use crate::materials::MaterialPool;
use crate::meshes::MeshPool;
use crate::ocean::Ocean;
use crate::overlay::Overlays;
use crate::picking::{PickRequest, Picking, coverage};
use crate::post::{DEPTH, HDR, Post, SAMPLES, Targets};
use crate::profiler::GpuProfiler;
use crate::scene::{InstanceGpu, Part, Resolve, Scene, VARIANTS};
use crate::shaders;
use crate::shadows::{self, CASCADES, SHADOW_SIZE};
use crate::sky::{Sky, SkyParams};

const VIEWS: u32 = 1 + CASCADES as u32;
const CLUSTER_X: u32 = 16;
const CLUSTER_Y: u32 = 9;
const CLUSTER_Z: u32 = 24;
const CLUSTER_MAX: u32 = 64;
const CLUSTER_FAR: f32 = 400.0;
const SHADOW_DISTANCE: f32 = 150.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    viewport: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    fog: [f32; 4],
    params: [f32; 4],
    sky_color: [f32; 4],
    cascade_splits: [f32; 4],
    cascades: [[[f32; 4]; 4]; 4],
    counts: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CullUniform {
    planes: [[[f32; 4]; 6]; 5],
    instance_count: u32,
    view_count: u32,
    mesh_count: u32,
    view_stride: u32,
    alpha: f32,
    lod_scale: f32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightGpu {
    pos: [f32; 3],
    range: f32,
    color: [f32; 3],
    kind: u32,
    dir: [f32; 3],
    cos_outer: f32,
    cos_inner: f32,
    _p: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ClusterUniform {
    grid: [u32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// What a frame cost and drew.
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    pub cpu_ms: f32,
    pub gpu_ms: f32,
    pub passes: Vec<(&'static str, f32)>,
    pub instances: usize,
    pub entities: usize,
    pub meshes: usize,
    pub materials: usize,
    pub lights: usize,
    pub pending_assets: usize,
    pub tick: u64,
    pub backend: &'static str,
}

/// A loaded model: its meshes by name and index, and its scene's nodes.
struct Model {
    /// (mesh id, material key or empty) per mesh index.
    meshes: Vec<(u32, String)>,
    names: HashMap<String, usize>,
    /// (mesh index, local TRS) per drawn node.
    nodes: Vec<(usize, (Vec3, Quat, Vec3))>,
}

struct Pools<'a> {
    meshes: &'a mut MeshPool,
    materials: &'a mut MaterialPool,
    models: &'a HashMap<String, Model>,
    loader: &'a mut dyn AssetSource,
    requested: &'a mut HashSet<String>,
    failed: &'a HashSet<String>,
}

impl Resolve for Pools<'_> {
    fn parts(&mut self, look: &Look) -> Option<Vec<Part>> {
        if let Some(mesh) = self
            .meshes
            .get(&look.mesh)
            .filter(|_| !look.mesh.contains('.'))
        {
            let material = self.materials.resolve(look)?;
            return Some(vec![Part {
                mesh,
                material,
                variant: self.materials.variant(material),
                local: None,
            }]);
        }
        let (path, sub) = match look.mesh.split_once('#') {
            Some((p, s)) => (p, Some(s)),
            None => (look.mesh.as_str(), None),
        };
        if self.failed.contains(path) {
            return Some(Vec::new());
        }
        let Some(model) = self.models.get(path) else {
            if self.requested.insert(path.to_owned()) {
                self.loader.request(path);
            }
            return None;
        };
        let mut material_for = |key: &str| -> Option<u32> {
            if !look.material.is_empty() || key.is_empty() {
                self.materials.resolve(look)
            } else {
                let l = Look {
                    material: key.to_owned(),
                    ..look.clone()
                };
                self.materials.resolve(&l)
            }
        };
        match sub {
            None => {
                let mut parts = Vec::with_capacity(model.nodes.len());
                for (mi, local) in &model.nodes {
                    let (mesh, key) = &model.meshes[*mi];
                    let material = material_for(key)?;
                    parts.push(Part {
                        mesh: *mesh,
                        material,
                        variant: 0,
                        local: Some(*local),
                    });
                }
                for p in &mut parts {
                    p.variant = self.materials.variant(p.material);
                }
                Some(parts)
            }
            Some(s) => {
                let i = model
                    .names
                    .get(s)
                    .copied()
                    .or_else(|| s.parse::<usize>().ok())
                    .filter(|&i| i < model.meshes.len())?;
                let (mesh, key) = &model.meshes[i];
                let material = material_for(key)?;
                Some(vec![Part {
                    mesh: *mesh,
                    material,
                    variant: self.materials.variant(material),
                    local: None,
                }])
            }
        }
    }
}

pub struct Renderer {
    gpu: Gpu,
    output: wgpu::TextureFormat,
    pub scene: Scene,
    meshes: MeshPool,
    materials: MaterialPool,
    models: HashMap<String, Model>,
    loader: Box<dyn AssetSource>,
    requested: HashSet<String>,
    failed: HashSet<String>,
    blitter: Blitter,
    targets: Targets,
    post: Post,
    sky: Sky,
    profiler: GpuProfiler,

    view_buf: wgpu::Buffer,
    cull_buf: wgpu::Buffer,
    instances: wgpu::Buffer,
    visible: wgpu::Buffer,
    /// The camera view's visible instances with interpolated poses (48 bytes each).
    drawn: wgpu::Buffer,
    draws: wgpu::Buffer,
    draw_template: wgpu::Buffer,
    lights: wgpu::Buffer,
    cluster_buf: wgpu::Buffer,
    cluster_lights: wgpu::Buffer,
    cluster_overflow: wgpu::Buffer,

    cull_pipeline: wgpu::ComputePipeline,
    cluster_pipeline: wgpu::ComputePipeline,
    forward: [wgpu::RenderPipeline; 4],
    shadow: [wgpu::RenderPipeline; 2],
    empty_group: wgpu::BindGroup,
    ocean: Ocean,
    picking: Picking,
    /// Editor overlays: gizmo shapes, selection outline, grid and axes.
    pub overlays: Overlays,
    /// The last finished pick: `Some(None)` when the pixel shows no entity.
    last_pick: Option<Option<u64>>,
    /// The last finished coverage read: (entity, share of the view's pixels), largest first.
    last_visible: Option<Vec<(u64, f32)>>,
    batch_offsets: wgpu::Buffer,
    sky_pipeline: wgpu::RenderPipeline,
    layouts: Layouts,
    shadow_tex: wgpu::Texture,
    shadow_views: Vec<wgpu::TextureView>,
    shadow_array: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    tex_sampler: wgpu::Sampler,

    bind: Option<Binds>,
    bind_key: (u64, u64, u64),
    view_stride: u32,
    draw_meshes: u32,
    light_count: u32,

    camera_override: Option<CameraState>,
    tick_arrived: f64,
    pub last: FrameStats,
}

struct Layouts {
    frame: wgpu::BindGroupLayout,
    lighting: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
}

struct Binds {
    cull: wgpu::BindGroup,
    cluster: wgpu::BindGroup,
    frame: wgpu::BindGroup,
    lighting: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    sky: wgpu::BindGroup,
}

fn storage(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    extra: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(64),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | extra,
        mapped_at_creation: false,
    })
}

fn uniform(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn entry(
    binding: u32,
    vis: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty,
        count: None,
    }
}

fn buf_ty(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn uni_ty() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn mat(m: Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
}

impl Renderer {
    /// A renderer drawing into textures of `output` format at `width` x `height`.
    pub fn new(gpu: &Gpu, output: wgpu::TextureFormat, width: u32, height: u32) -> Renderer {
        let device = &gpu.device;
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let frag = wgpu::ShaderStages::FRAGMENT;
        let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                entry(0, vf, uni_ty()),
                entry(1, vf, buf_ty(true)),
                entry(2, vf, buf_ty(true)),
                entry(3, vf, buf_ty(true)),
                entry(4, vf, buf_ty(true)),
            ],
        });
        let lighting = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lighting"),
            entries: &[
                entry(
                    0,
                    frag,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    frag,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                entry(
                    2,
                    frag,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                ),
                entry(
                    3,
                    frag,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(4, frag, uni_ty()),
                entry(5, frag, buf_ty(true)),
                entry(6, frag, uni_ty()),
                entry(7, frag, buf_ty(true)),
            ],
        });
        let tex_entry = |b: u32| {
            entry(
                b,
                frag,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            )
        };
        let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                entry(
                    2,
                    frag,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let fwd_module = shaders::module(device, "forward");
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forward"),
            bind_group_layouts: &[Some(&frame), Some(&lighting), Some(&textures)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<pocket_assets::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
        };
        let forward_pipe = |cull: Option<wgpu::Face>, fs: &str, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &fwd_module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fwd_module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(HDR.into())],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
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
            })
        };
        // Variants: 0 opaque, 1 alpha-masked, 2 double-sided, 3 both.
        let forward = [
            forward_pipe(Some(wgpu::Face::Back), "fs", "forward"),
            forward_pipe(Some(wgpu::Face::Back), "fs_masked", "forward (masked)"),
            forward_pipe(None, "fs", "forward (double sided)"),
            forward_pipe(None, "fs_masked", "forward (masked, double sided)"),
        ];
        // Shadow passes write the cascades, so group 1 (which samples them) is an empty group there.
        let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("empty"),
            entries: &[],
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&frame), Some(&empty), Some(&textures)],
            immediate_size: 0,
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("empty"),
            layout: &empty,
            entries: &[],
        });
        let shadow_pipe = |masked: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if masked {
                    "shadow casters (masked)"
                } else {
                    "shadow casters"
                }),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: &fwd_module,
                    entry_point: Some(if masked {
                        "vs_shadow_masked"
                    } else {
                        "vs_shadow"
                    }),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                fragment: masked.then(|| wgpu::FragmentState {
                    module: &fwd_module,
                    entry_point: Some("fs_shadow_masked"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    unclipped_depth: false,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.5,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let shadow = [shadow_pipe(false), shadow_pipe(true)];
        let picking = Picking::new(device, &fwd_module, &frame, vertex_layout.clone());
        let ocean = Ocean::new(
            device,
            &fwd_module,
            [&frame, &lighting, &textures],
            HDR,
            DEPTH,
            SAMPLES,
        );
        let sky_module = shaders::module(device, "sky");
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &sky_module,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sky_module,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(HDR.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
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
        let compute = |name: &str, entry: &str| {
            let m = shaders::module(device, name);
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &m,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        };
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow cascades"),
            size: wgpu::Extent3d {
                width: SHADOW_SIZE,
                height: SHADOW_SIZE,
                depth_or_array_layers: CASCADES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_views = (0..CASCADES as u32)
            .map(|l| {
                shadow_tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: l,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_array = shadow_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let cells = u64::from(CLUSTER_X * CLUSTER_Y * CLUSTER_Z);
        let mut meshes = MeshPool::new(device);
        for name in PRIMITIVES {
            if let Some(m) = primitive(name) {
                meshes.add(device, &gpu.queue, name, &m);
            }
        }
        Renderer {
            output,
            scene: Scene::new(),
            meshes,
            materials: MaterialPool::new(device),
            models: HashMap::new(),
            loader: Box::new(NoAssets),
            requested: HashSet::new(),
            failed: HashSet::new(),
            blitter: Blitter::new(device),
            targets: Targets::new(device, width, height),
            post: Post::new(device, output),
            sky: Sky::new(device),
            profiler: GpuProfiler::new(device, &gpu.queue, gpu.caps.timestamps),
            view_buf: uniform(device, "view", std::mem::size_of::<ViewUniform>() as u64),
            cull_buf: uniform(device, "cull", std::mem::size_of::<CullUniform>() as u64),
            instances: storage(device, "instances", 1024 * 80, wgpu::BufferUsages::empty()),
            visible: storage(
                device,
                "visible",
                1024 * 4 * u64::from(VIEWS),
                wgpu::BufferUsages::empty(),
            ),
            drawn: storage(device, "drawn", 1024 * 48, wgpu::BufferUsages::empty()),
            draws: storage(device, "draws", 64 * 20, wgpu::BufferUsages::INDIRECT),
            draw_template: storage(
                device,
                "draw template",
                64 * 20,
                wgpu::BufferUsages::COPY_SRC,
            ),
            lights: storage(device, "lights", 64 * 64, wgpu::BufferUsages::empty()),
            cluster_buf: uniform(device, "clusters", 32),
            cluster_lights: storage(
                device,
                "cluster lights",
                cells * u64::from(CLUSTER_MAX + 1) * 4,
                wgpu::BufferUsages::empty(),
            ),
            cluster_overflow: storage(device, "cluster overflow", 16, wgpu::BufferUsages::empty()),
            cull_pipeline: compute("cull", "main"),
            cluster_pipeline: compute("cluster", "assign"),
            forward,
            shadow,
            ocean,
            picking,
            overlays: Overlays::new(device, output),
            last_pick: None,
            last_visible: None,
            empty_group,
            batch_offsets: storage(device, "batch offsets", 64 * 4, wgpu::BufferUsages::empty()),
            sky_pipeline,
            layouts: Layouts {
                frame,
                lighting,
                textures,
            },
            shadow_tex,
            shadow_views,
            shadow_array,
            shadow_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("shadow"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            }),
            tex_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("material textures"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: 16,
                ..Default::default()
            }),
            bind: None,
            bind_key: (u64::MAX, 0, 0),
            view_stride: 1,
            draw_meshes: 0,
            light_count: 0,
            camera_override: None,
            tick_arrived: 0.0,
            last: FrameStats::default(),
            gpu: gpu.clone(),
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    pub fn set_asset_source(&mut self, source: Box<dyn AssetSource>) {
        self.loader = source;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == (self.targets.width, self.targets.height)
        {
            return;
        }
        self.targets = Targets::new(&self.gpu.device, width, height);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Draws from this camera instead of the scene's active one (`None`: the scene's).
    pub fn set_camera_override(&mut self, camera: Option<CameraState>) {
        self.camera_override = camera;
    }

    /// The camera the next frame is drawn from.
    pub fn camera(&self) -> CameraState {
        if let Some(c) = self.camera_override {
            return c;
        }
        self.scene
            .cameras
            .iter()
            .filter(|c| c.active)
            .min_by_key(|c| c.id)
            .map(|c| CameraState {
                position: Vec3::from(c.position),
                rotation: Quat::from_array(c.rotation).normalize(),
                fov_y: c.fov_deg.to_radians(),
                near: c.near.max(0.01),
                exposure_ev: c.exposure_ev,
                ortho_height: None,
            })
            .unwrap_or_default()
    }

    /// Applies a frame of the render feed; `now_s` is the wall clock (interpolation).
    pub fn apply(&mut self, frame: RenderFrame, now_s: f64) {
        if frame.tick != self.scene.tick || frame.reset {
            self.tick_arrived = now_s;
        }
        let mut pools = Pools {
            meshes: &mut self.meshes,
            materials: &mut self.materials,
            models: &self.models,
            loader: self.loader.as_mut(),
            requested: &mut self.requested,
            failed: &self.failed,
        };
        self.scene.apply(frame, &mut pools);
    }

    /// Registers a loaded model under `path` (natively the loader does this; the browser calls it
    /// with fetched assets).
    pub fn add_model(&mut self, path: &str, asset: &ModelAsset) {
        let device = &self.gpu.device;
        let queue = &self.gpu.queue;
        self.materials.add_asset(
            device,
            queue,
            &mut self.blitter,
            path,
            &asset.materials,
            &asset.images,
        );
        let mut meshes = Vec::with_capacity(asset.meshes.len());
        let mut names = HashMap::new();
        for (i, m) in asset.meshes.iter().enumerate() {
            let key = format!("{path}#{}", m.name);
            let id = self.meshes.add(device, queue, &key, m);
            let mat = m
                .material
                .map(|k| format!("{path}#{k}"))
                .unwrap_or_default();
            meshes.push((id, mat));
            names.insert(m.name.clone(), i);
        }
        let nodes = asset
            .nodes
            .iter()
            .map(|n| {
                let (s, r, t) = Mat4::from_cols_array(&n.transform).to_scale_rotation_translation();
                (n.mesh, (t, r, s))
            })
            .collect();
        self.models.insert(
            path.to_owned(),
            Model {
                meshes,
                names,
                nodes,
            },
        );
    }

    fn poll_assets(&mut self) {
        for (path, r) in self.loader.poll() {
            match r {
                Ok(asset) => self.add_model(&path, &asset),
                Err(e) => {
                    log::warn!("{path}: {e}");
                    self.failed.insert(path);
                }
            }
        }
        let mut pools = Pools {
            meshes: &mut self.meshes,
            materials: &mut self.materials,
            models: &self.models,
            loader: self.loader.as_mut(),
            requested: &mut self.requested,
            failed: &self.failed,
        };
        self.scene.retry_pending(&mut pools);
    }

    /// Writes the scene's changes to the GPU, growing buffers as needed.
    fn sync(&mut self) {
        let device = &self.gpu.device;
        let queue = &self.gpu.queue;
        let n = self.scene.instance_count() as u64;
        let mut grown = false;
        if n * 80 > self.instances.size() {
            self.instances = storage(
                device,
                "instances",
                (n * 80).next_power_of_two(),
                wgpu::BufferUsages::empty(),
            );
            self.scene.full_upload = true;
            grown = true;
        }
        for run in self.scene.take_dirty_runs() {
            let slots: &[InstanceGpu] = &self.scene.slots[run.start as usize..run.end as usize];
            queue.write_buffer(
                &self.instances,
                u64::from(run.start) * 80,
                bytemuck::cast_slice(slots),
            );
        }
        let mesh_count = self.meshes.len() as u32;
        if self.scene.counts_changed || mesh_count != self.draw_meshes {
            self.scene.counts_changed = false;
            let (offsets, stride) = self.scene.batch_offsets(self.meshes.len());
            let ob = (offsets.len() * 4) as u64;
            if ob > self.batch_offsets.size() {
                self.batch_offsets = storage(
                    device,
                    "batch offsets",
                    ob.next_power_of_two(),
                    wgpu::BufferUsages::empty(),
                );
                grown = true;
            }
            queue.write_buffer(&self.batch_offsets, 0, bytemuck::cast_slice(&offsets));
            let vis = u64::from(stride) * u64::from(VIEWS) * 4;
            if vis > self.visible.size() || stride != self.view_stride {
                if vis > self.visible.size() {
                    self.visible = storage(
                        device,
                        "visible",
                        vis.next_power_of_two(),
                        wgpu::BufferUsages::empty(),
                    );
                    grown = true;
                }
                self.view_stride = stride;
            }
            let drawn = u64::from(stride) * 48;
            if drawn > self.drawn.size() {
                self.drawn = storage(
                    device,
                    "drawn",
                    drawn.next_power_of_two(),
                    wgpu::BufferUsages::empty(),
                );
                grown = true;
            }
            let mut template = Vec::with_capacity((mesh_count * VIEWS * VARIANTS) as usize);
            for v in 0..VIEWS {
                for variant in 0..VARIANTS {
                    for (m, info) in self.meshes.infos.iter().enumerate() {
                        template.push(DrawArgs {
                            index_count: info.index_count,
                            instance_count: 0,
                            first_index: info.first_index,
                            base_vertex: info.base_vertex,
                            first_instance: v * stride
                                + offsets[(variant * mesh_count) as usize + m],
                        });
                    }
                }
            }
            let bytes = (template.len() * 20) as u64;
            if bytes > self.draws.size() {
                self.draws = storage(
                    device,
                    "draws",
                    bytes.next_power_of_two(),
                    wgpu::BufferUsages::INDIRECT,
                );
                self.draw_template = storage(
                    device,
                    "draw template",
                    bytes.next_power_of_two(),
                    wgpu::BufferUsages::COPY_SRC,
                );
                grown = true;
            }
            queue.write_buffer(&self.draw_template, 0, bytemuck::cast_slice(&template));
            self.draw_meshes = mesh_count;
        }
        grown |= self.meshes.flush(device, queue);
        grown |= self.materials.flush(device, queue);
        if self.scene.lights_changed {
            self.scene.lights_changed = false;
            let lights: Vec<LightGpu> = self
                .scene
                .lights
                .iter()
                .filter(|l| l.kind != LightKindView::Directional)
                .map(|l| LightGpu {
                    pos: l.position,
                    range: l.range.max(0.01),
                    color: [
                        l.color[0] * l.intensity,
                        l.color[1] * l.intensity,
                        l.color[2] * l.intensity,
                    ],
                    kind: if l.kind == LightKindView::Spot { 2 } else { 1 },
                    dir: l.direction,
                    cos_outer: l.outer_deg.to_radians().cos(),
                    cos_inner: l.inner_deg.to_radians().cos(),
                    _p: [0.0; 3],
                })
                .collect();
            let bytes = (lights.len() * 64) as u64;
            if bytes > self.lights.size() {
                self.lights = storage(
                    device,
                    "lights",
                    bytes.next_power_of_two(),
                    wgpu::BufferUsages::empty(),
                );
                grown = true;
            }
            if !lights.is_empty() {
                queue.write_buffer(&self.lights, 0, bytemuck::cast_slice(&lights));
            }
            self.light_count = lights.len() as u32;
        }
        let key = (self.meshes.generation, self.materials.generation, 0);
        if grown || self.bind.is_none() || key != self.bind_key {
            self.bind_key = key;
            self.rebind();
        }
    }

    fn rebind(&mut self) {
        let device = &self.gpu.device;
        fn b(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buf.as_entire_binding(),
            }
        }
        let cull = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cull"),
            layout: &self.cull_pipeline.get_bind_group_layout(0),
            entries: &[
                b(0, &self.cull_buf),
                b(1, &self.instances),
                b(2, &self.meshes.info_buffer),
                b(3, &self.draws),
                b(4, &self.visible),
                b(5, &self.batch_offsets),
                b(6, &self.drawn),
            ],
        });
        let cluster = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clusters"),
            layout: &self.cluster_pipeline.get_bind_group_layout(0),
            entries: &[
                b(0, &self.view_buf),
                b(1, &self.lights),
                b(2, &self.cluster_buf),
                b(3, &self.cluster_lights),
                b(4, &self.cluster_overflow),
            ],
        });
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &self.layouts.frame,
            entries: &[
                b(0, &self.view_buf),
                b(1, &self.instances),
                b(2, &self.visible),
                b(3, &self.materials.buffer),
                b(4, &self.drawn),
            ],
        });
        let lighting = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lighting"),
            layout: &self.layouts.lighting,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.shadow_array),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.sky.env_cube),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sky.sampler),
                },
                b(4, &self.sky.sh),
                b(5, &self.lights),
                b(6, &self.cluster_buf),
                b(7, &self.cluster_lights),
            ],
        });
        let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("textures"),
            layout: &self.layouts.textures,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.materials.srgb.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.materials.linear.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.tex_sampler),
                },
            ],
        });
        let sky = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &self.sky_pipeline.get_bind_group_layout(0),
            entries: &[
                b(0, &self.view_buf),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.sky.raw_cube),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sky.sampler),
                },
            ],
        });
        self.bind = Some(Binds {
            cull,
            cluster,
            frame,
            lighting,
            textures,
            sky,
        });
    }

    /// Draws a frame into `output` (a texture view of the renderer's output format).
    pub fn render(&mut self, output: &wgpu::TextureView, now_s: f64) -> FrameStats {
        let cpu = web_time();
        self.collect_pick();
        self.poll_assets();
        self.sync();
        let device = self.gpu.device.clone();
        let queue = self.gpu.queue.clone();
        let (w, h) = (self.targets.width, self.targets.height);
        let aspect = w as f32 / h.max(1) as f32;
        let cam = self.camera();
        let view = cam.view();
        let proj = cam.proj(aspect);
        let vp = proj * view;
        let alpha =
            ((now_s - self.tick_arrived) / self.scene.dt_s.max(1e-6)).clamp(0.0, 1.0) as f32;

        // The sun: the lowest-id directional light.
        let sun = self
            .scene
            .lights
            .iter()
            .filter(|l| l.kind == LightKindView::Directional)
            .min_by_key(|l| l.id)
            .cloned();
        let env = self.scene.environment.clone();
        let (to_sun, sun_color, sun_lux, shadows) = match &sun {
            Some(l) => (
                -Vec3::from(l.direction).normalize_or(Vec3::NEG_Y),
                l.color,
                l.intensity,
                l.shadows,
            ),
            None => (
                Vec3::new(0.3, 0.8, 0.4).normalize(),
                [1.0, 0.96, 0.9],
                0.0,
                false,
            ),
        };
        let sky_kind = env.as_ref().map_or(0, |e| e.sky);
        let flat = env.as_ref().map_or([0.35, 0.5, 0.75], |e| e.sky_color);
        self.sky.update(
            &device,
            &queue,
            SkyParams {
                sun: [
                    to_sun.x,
                    to_sun.y,
                    to_sun.z,
                    if sun.is_some() { sun_lux } else { 6.0 },
                ],
                color: [sun_color[0], sun_color[1], sun_color[2], sky_kind as f32],
                flat_color: [flat[0], flat[1], flat[2], 0.0],
                size: [0; 4],
            },
        );
        let casc = shadows::cascades(&cam, aspect, to_sun, SHADOW_DISTANCE);
        let ev = env.as_ref().map_or(0.0, |e| e.exposure_ev) + cam.exposure_ev;
        let exposure = 0.9 * 2f32.powf(ev);
        let vu = ViewUniform {
            view_proj: mat(vp),
            view: mat(view),
            proj: mat(proj),
            inv_view_proj: mat(vp.inverse()),
            camera_pos: [
                cam.position.x,
                cam.position.y,
                cam.position.z,
                self.scene.t_s as f32,
            ],
            viewport: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h.max(1) as f32],
            sun_dir: [to_sun.x, to_sun.y, to_sun.z, sun_lux],
            sun_color: [
                sun_color[0],
                sun_color[1],
                sun_color[2],
                env.as_ref().map_or(1.0, |e| e.ambient),
            ],
            fog: env.as_ref().map_or([0.0; 4], |e| {
                [
                    e.fog_color[0],
                    e.fog_color[1],
                    e.fog_color[2],
                    e.fog_density,
                ]
            }),
            params: [
                alpha,
                exposure,
                env.as_ref().map_or(0.15, |e| e.bloom),
                sky_kind as f32,
            ],
            sky_color: [flat[0], flat[1], flat[2], if shadows { 1.0 } else { 0.0 }],
            cascade_splits: casc.splits,
            cascades: casc.view_proj.map(mat),
            counts: [self.view_stride, 0, 0, 0],
        };
        queue.write_buffer(&self.view_buf, 0, bytemuck::bytes_of(&vu));
        self.overlays.write_params(&queue);
        if let Some(sea) = &self.scene.sea {
            // Drawn time matches the interpolated poses: between the last two ticks.
            let t = self.scene.t_s - self.scene.dt_s * (1.0 - f64::from(alpha));
            self.ocean.update(&queue, sea, t as f32);
        }
        let mut planes = [[[0.0f32; 4]; 6]; 5];
        for (i, p) in frustum_planes(vp, true).iter().enumerate() {
            planes[0][i] = p.to_array();
        }
        for c in 0..CASCADES {
            for (i, p) in shadows::planes(casc.view_proj[c]).iter().enumerate() {
                planes[c + 1][i] = p.to_array();
            }
        }
        let views = if shadows { VIEWS } else { 1 };
        let n = self.scene.instance_count() as u32;
        let cu = CullUniform {
            planes,
            instance_count: n,
            view_count: views,
            mesh_count: self.draw_meshes,
            view_stride: self.view_stride,
            alpha,
            lod_scale: 0.0,
            _pad: [0; 2],
        };
        queue.write_buffer(&self.cull_buf, 0, bytemuck::bytes_of(&cu));
        let near = cam.near.max(0.01);
        queue.write_buffer(
            &self.cluster_buf,
            0,
            bytemuck::bytes_of(&ClusterUniform {
                grid: [CLUSTER_X, CLUSTER_Y, CLUSTER_Z, CLUSTER_MAX],
                params: [
                    near,
                    CLUSTER_Z as f32 / (CLUSTER_FAR / near).ln(),
                    CLUSTER_FAR,
                    self.light_count as f32,
                ],
            }),
        );

        let binds = self.bind.as_ref().unwrap_or_else(|| unreachable!());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        let draw_bytes = u64::from(self.draw_meshes * VIEWS * VARIANTS) * 20;
        if draw_bytes > 0 {
            enc.copy_buffer_to_buffer(&self.draw_template, 0, &self.draws, 0, draw_bytes);
        }
        if n > 0 {
            let groups = n.div_ceil(256);
            let (gx, gy) = if groups > 65535 {
                (65535, groups.div_ceil(65535))
            } else {
                (groups, 1)
            };
            let ts = self.profiler.compute_scope("cull");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cull"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.cull_pipeline);
            pass.set_bind_group(0, &binds.cull, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        {
            let ts = self.profiler.compute_scope("lights");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cluster lights"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.cluster_pipeline);
            pass.set_bind_group(0, &binds.cluster, &[]);
            pass.dispatch_workgroups((CLUSTER_X * CLUSTER_Y * CLUSTER_Z).div_ceil(64), 1, 1);
        }
        // Draws one variant's batches of one view: a contiguous range of the indirect buffer.
        let draw = |pass: &mut wgpu::RenderPass<'_>,
                    view_index: u32,
                    variant: u32,
                    meshes: u32,
                    first_instance: bool| {
            if meshes == 0 {
                return;
            }
            let offset = u64::from((view_index * VARIANTS + variant) * meshes) * 20;
            if first_instance {
                pass.multi_draw_indexed_indirect(&self.draws, offset, meshes);
            } else {
                for m in 0..meshes {
                    pass.draw_indexed_indirect(&self.draws, offset + u64::from(m) * 20);
                }
            }
        };
        let fi = self.gpu.caps.indirect_first_instance;
        // Without shadows the cascades are never sampled (`shadow_factor` returns 1): skip them.
        for c in 0..if shadows { CASCADES } else { 0 } {
            let ts = self.profiler.render_scope("shadows");
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow cascade"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_views[c],
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts,
                ..Default::default()
            });
            pass.set_bind_group(0, &binds.frame, &[]);
            pass.set_bind_group(1, &self.empty_group, &[]);
            pass.set_bind_group(2, &binds.textures, &[]);
            pass.set_vertex_buffer(0, self.meshes.vertices.slice(..));
            pass.set_index_buffer(self.meshes.indices.slice(..), wgpu::IndexFormat::Uint32);
            for variant in 0..VARIANTS {
                pass.set_pipeline(&self.shadow[(variant & 1) as usize]);
                draw(&mut pass, c as u32 + 1, variant, self.draw_meshes, fi);
            }
        }
        {
            let ts = self.profiler.render_scope("opaque+sky");
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_msaa,
                    depth_slice: None,
                    resolve_target: Some(&self.targets.hdr_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts,
                ..Default::default()
            });
            pass.set_bind_group(0, &binds.frame, &[]);
            pass.set_bind_group(1, &binds.lighting, &[]);
            pass.set_bind_group(2, &binds.textures, &[]);
            pass.set_vertex_buffer(0, self.meshes.vertices.slice(..));
            pass.set_index_buffer(self.meshes.indices.slice(..), wgpu::IndexFormat::Uint32);
            for variant in 0..VARIANTS {
                pass.set_pipeline(&self.forward[variant as usize]);
                draw(&mut pass, 0, variant, self.draw_meshes, fi);
            }
            if self.scene.sea.is_some() {
                self.ocean.draw(&mut pass);
            }
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &binds.sky, &[]);
            pass.draw(0..3, 0..1);
            self.overlays.draw_grid(&device, &mut pass, &self.view_buf);
        }
        if self.picking.wanted() {
            let meshes = self.draw_meshes;
            let draws = &self.draws;
            let id_draw = |pass: &mut wgpu::RenderPass<'_>| {
                for variant in 0..VARIANTS {
                    if meshes == 0 {
                        return;
                    }
                    let offset = u64::from(variant * meshes) * 20;
                    if fi {
                        pass.multi_draw_indexed_indirect(draws, offset, meshes);
                    } else {
                        for m in 0..meshes {
                            pass.draw_indexed_indirect(draws, offset + u64::from(m) * 20);
                        }
                    }
                }
            };
            self.picking.encode(
                &device,
                &mut enc,
                (w, h),
                &binds.frame,
                &self.meshes.vertices,
                &self.meshes.indices,
                &id_draw,
            );
        }
        let bloom = env.as_ref().map_or(0.15, |e| e.bloom);
        self.post.run(
            &device,
            &queue,
            &mut enc,
            &self.targets,
            output,
            exposure,
            bloom,
        );
        if self.overlays.any() {
            let mut selected = Vec::new();
            let hovered = self.overlays.hovered;
            for (e, hov) in self
                .overlays
                .selection
                .iter()
                .map(|e| (*e, false))
                .chain(hovered.map(|h| (h, true)))
            {
                for &slot in self.scene.slots_of(e) {
                    let mesh = self.scene.slots[slot as usize].mesh as usize;
                    if let Some(info) = self.meshes.infos.get(mesh) {
                        selected.push((
                            slot,
                            info.first_index..info.first_index + info.index_count,
                            info.base_vertex,
                            hov,
                        ));
                    }
                }
            }
            self.overlays.draw(
                &device,
                &queue,
                &mut enc,
                output,
                (w, h),
                &self.view_buf,
                &self.instances,
                &self.meshes.vertices,
                &self.meshes.indices,
                &selected,
            );
        }
        self.profiler.resolve(&mut enc);
        queue.submit([enc.finish()]);
        self.profiler.after_submit();
        self.picking.after_submit();
        self.last = FrameStats {
            cpu_ms: (web_time() - cpu) as f32 * 1000.0,
            gpu_ms: self.profiler.total_ms(),
            passes: merge_passes(&self.profiler.last),
            instances: self.scene.instance_count(),
            entities: self.scene.entity_count(),
            meshes: self.meshes.len(),
            materials: self.materials.len(),
            lights: self.light_count as usize,
            pending_assets: self.scene.pending(),
            tick: self.scene.tick,
            backend: self.gpu.backend_name(),
        };
        self.last.clone()
    }

    /// Asks which entity is at pixel (x, y) of the output (answered a frame or two later by
    /// `take_pick`).
    pub fn request_pick(&mut self, x: u32, y: u32) {
        self.picking.request(PickRequest::Pixel(x, y));
    }

    /// The answer to the last `request_pick`: `None` until it is ready, then `Some(entity)` once.
    pub fn take_pick(&mut self) -> Option<Option<u64>> {
        self.collect_pick();
        self.last_pick.take()
    }

    /// Asks which entities the view shows and how much of it each covers.
    pub fn request_visible(&mut self) {
        self.picking.request(PickRequest::Full);
    }

    /// The answer to the last `request_visible`: (entity, share of pixels), largest first.
    pub fn take_visible(&mut self) -> Option<Vec<(u64, f32)>> {
        self.collect_pick();
        self.last_visible.take()
    }

    fn collect_pick(&mut self) {
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);
        let Some(r) = self.picking.take() else {
            return;
        };
        match r.request {
            PickRequest::Pixel(..) => {
                let id = r.ids.first().copied().unwrap_or(0);
                self.last_pick = Some(if id == 0 {
                    None
                } else {
                    self.scene.entity_of_slot(id - 1)
                });
            }
            PickRequest::Full => {
                let total = r.ids.len().max(1) as f32;
                let mut by_entity: HashMap<u64, u32> = HashMap::new();
                for (slot, n) in coverage(&r.ids) {
                    if let Some(e) = self.scene.entity_of_slot(slot) {
                        *by_entity.entry(e).or_insert(0) += n;
                    }
                }
                let mut v: Vec<(u64, f32)> = by_entity
                    .into_iter()
                    .map(|(e, n)| (e, n as f32 / total))
                    .collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                self.last_visible = Some(v);
            }
        }
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output
    }

    /// Draws a frame offscreen and reads it back as tightly packed RGBA8 (sRGB) pixels: the
    /// `capture` an agent or a test asks for. Blocks until the GPU is done.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_rgba(&mut self, now_s: f64) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.targets.width, self.targets.height);
        let device = self.gpu.device.clone();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.output,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        self.render(&view, now_s);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        if let Ok(Ok(())) = rx.recv() {
            if let Ok(data) = slice.get_mapped_range() {
                let bgra = matches!(
                    self.output,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                );
                for y in 0..h {
                    let r = &data[(y * row) as usize..(y * row + w * 4) as usize];
                    if bgra {
                        for px in r.chunks_exact(4) {
                            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                        }
                    } else {
                        out.extend_from_slice(r);
                    }
                }
            }
        }
        buf.unmap();
        (w, h, out)
    }
}

fn merge_passes(p: &[(&'static str, f32)]) -> Vec<(&'static str, f32)> {
    let mut out: Vec<(&'static str, f32)> = Vec::new();
    for (l, ms) in p {
        match out.iter_mut().find(|(k, _)| k == l) {
            Some(e) => e.1 += ms,
            None => out.push((l, *ms)),
        }
    }
    out
}

/// Seconds from an arbitrary origin, monotonic.
pub fn web_time() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}
