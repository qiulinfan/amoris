//! The renderer: keeps the retained scene in step with the render feed and draws a frame.
//!
//! Per frame: write what changed (instances, materials, meshes, lights), then on the GPU
//! 1. cull every instance against the camera and the four shadow cascades in one dispatch, which
//!    writes the visible lists and the instance counts of every indirect draw;
//! 2. assign punctual lights to the cluster grid;
//! 3. draw each shadow cascade (one multi-draw per pipeline variant natively);
//! 4. draw the opaque scene and the sky into the multisampled HDR target (likewise);
//! 5. bloom and the display transform into the output.
//!
//! With occlusion culling (occlusion.rs) step 1 keeps only what the camera saw last frame, and
//! step 4 is two passes: those instances, then a depth pyramid and a late culling pass, then the
//! instances it finds newly visible with the sky and the rest.
//!
//! Step 1 also picks each instance's level of detail per view (lod.rs, docs/spec/lod.md); a level
//! is a row of the mesh table, so the passes draw levels as they draw meshes.
//!
//! The CPU's work per frame does not grow with the number of instances. How the indirect draws
//! are issued (multi-draw, or WebGPU's baseline without `indirect-first-instance`) is batches.rs.

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use pocket_assets::frame::{LightKindView, Look, RenderFrame};
use pocket_assets::mesh::ModelAsset;
use pocket_assets::neural::{NeuralLayout, NeuralTexture};
use pocket_assets::primitives::{PRIMITIVES, primitive};

use crate::batches::{Batches, LATE, VIEWS};
use crate::blit::Blitter;
use crate::camera::{CameraState, frustum_planes};
use crate::gpu::Gpu;
use crate::loader::{AssetSource, NoAssets, is_neural_texture};
use crate::lod::{LodMode, LodSettings};
use crate::materials::MaterialPool;
use crate::meshes::{MeshPool, SKINNED_GROW};
use crate::neural::NeuralTable;
use crate::occlusion::{LateInputs, Occlusion, OcclusionMode, OcclusionStats};
use crate::ocean::Ocean;
use crate::overlay::Overlays;
use crate::particles::Particles;
use crate::picking::{PickRequest, Picking, coverage};
use crate::post::{DEPTH, HDR, Post, SAMPLES, Targets};
use crate::profiler::GpuProfiler;
use crate::scene::{InstanceGpu, NEURAL_VARIANT, Part, Resolve, Scene, VARIANTS};
use crate::shaders;
use crate::shadows::{self, CASCADES, SHADOW_SIZE};
use crate::skinning::Skinning;
use crate::sky::{Sky, SkyParams};
use crate::ui::Ui;

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
    view_proj: [[f32; 4]; 4],
    instance_count: u32,
    view_count: u32,
    mesh_count: u32,
    view_stride: u32,
    alpha: f32,
    lod_on: u32,
    occlusion: u32,
    hiz_levels: u32,
    viewport: [f32; 2],
    _pad: [u32; 2],
    eye: [f32; 4],
    lod: [f32; 4],
    lod_cascades: [f32; 4],
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
    /// How the indirect draws were issued (`multi-draw`, `first-instance` or `baseline`).
    pub draw_path: &'static str,
    /// Draw calls the shadow and opaque passes issued for the GPU-driven batches.
    pub draw_calls: u32,
    /// Occlusion culling this frame: `off`, `on`, `auto-on` or `auto-off` (occlusion.rs).
    pub occlusion: &'static str,
    /// The latest reading of the late culling pass's counters (a few frames old).
    pub occlusion_stats: Option<OcclusionStats>,
    /// Levels of detail this frame: `on`, `off`, or `off-limit` (asked for, but the views' lists
    /// would pass the device's binding limit with them; docs/spec/lod.md 5).
    pub lod: &'static str,
}

/// A drawn node: (mesh index, local TRS, skin).
type DrawnNode = (usize, (Vec3, Quat, Vec3), Option<usize>);

/// A loaded model: its meshes by name and index, and its scene's nodes.
struct Model {
    /// (mesh id, material key or empty) per mesh index.
    meshes: Vec<(u32, String)>,
    names: HashMap<String, usize>,
    /// (mesh index, local TRS, skin) per drawn node.
    nodes: Vec<DrawnNode>,
    /// The asset itself when it has skins (poses are evaluated from it every frame).
    skinned: Option<std::sync::Arc<ModelAsset>>,
}

struct Pools<'a> {
    meshes: &'a mut MeshPool,
    materials: &'a mut MaterialPool,
    models: &'a HashMap<String, Model>,
    loader: &'a mut dyn AssetSource,
    requested: &'a mut HashSet<String>,
    failed: &'a HashSet<String>,
    skinning: &'a mut Skinning,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
}

impl Pools<'_> {
    /// The mesh an entity draws for node `mi` of `path`: the shared mesh, or for a skinned mesh
    /// the entity's own skinned copy.
    fn mesh_for(
        &mut self,
        entity: u64,
        path: &str,
        model: &Model,
        mi: usize,
        skin: Option<usize>,
        local: (Vec3, Quat, Vec3),
    ) -> u32 {
        let (mesh, _) = model.meshes[mi];
        let (Some(skin), Some(asset)) = (skin, &model.skinned) else {
            return mesh;
        };
        let Some(count) = self.skinning.add_source(path, mi, asset) else {
            return mesh;
        };
        let key = format!("{path}#{mi}@{entity}");
        let Some(dynamic) =
            self.meshes
                .add_dynamic(self.device, self.queue, &key, mesh, count, SKINNED_GROW)
        else {
            return mesh;
        };
        let dst = self.meshes.infos[dynamic as usize].base_vertex as u32;
        self.skinning.parts.push(crate::skinning::Part {
            entity,
            asset: asset.clone(),
            source: (path.to_owned(), mi),
            skin,
            node_global: Mat4::from_scale_rotation_translation(local.2, local.1, local.0),
            dst,
        });
        dynamic
    }
}

impl Resolve for Pools<'_> {
    fn released(&mut self, entity: u64) {
        self.skinning.remove_entity(entity);
    }

    fn parts(&mut self, entity: u64, look: &Look) -> Option<Vec<Part>> {
        // A neural texture material (`materials/brick.ntex`) loads like a model; until it has, the
        // entity waits (a failed load registers the default material under its path).
        if is_neural_texture(&look.material) && !self.materials.has_asset(&look.material) {
            if self.requested.insert(look.material.clone()) {
                self.loader.request_neural_texture(&look.material);
            }
            return None;
        }
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
        let models = self.models;
        let Some(model) = models.get(path) else {
            if self.requested.insert(path.to_owned()) {
                self.loader.request(path);
            }
            return None;
        };
        let material_for = |materials: &mut MaterialPool, key: &str| -> Option<u32> {
            if !look.material.is_empty() || key.is_empty() {
                materials.resolve(look)
            } else {
                let l = Look {
                    material: key.to_owned(),
                    ..look.clone()
                };
                materials.resolve(&l)
            }
        };
        match sub {
            None => {
                let mut parts = Vec::with_capacity(model.nodes.len());
                for (mi, local, skin) in &model.nodes {
                    let (_, key) = &model.meshes[*mi];
                    let material = material_for(self.materials, key)?;
                    let mesh = self.mesh_for(entity, path, model, *mi, *skin, *local);
                    parts.push(Part {
                        mesh,
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
                let (_, key) = &model.meshes[i];
                let material = material_for(self.materials, key)?;
                let skin = model.nodes.iter().find(|n| n.0 == i).and_then(|n| n.2);
                let mesh = self.mesh_for(
                    entity,
                    path,
                    model,
                    i,
                    skin,
                    (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
                );
                Some(vec![Part {
                    mesh,
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
    gi: crate::gi::ProbeVolume,
    /// Ray-traced sun shadows in place of the cascades (rt_shadows.rs, `POCKET_RT_SHADOWS=1`).
    rt_shadows: Option<crate::rt_shadows::RtShadows>,
    profiler: GpuProfiler,
    /// Gaussian splats (splat/): prepared before the opaque pass, drawn after it.
    pub splats: crate::splat::Splats,

    view_buf: wgpu::Buffer,
    cull_buf: wgpu::Buffer,
    instances: wgpu::Buffer,
    visible: wgpu::Buffer,
    /// The camera view's visible instances with interpolated poses (48 bytes each).
    drawn: wgpu::Buffer,
    /// The indirect draws and how they are issued.
    batches: Batches,
    /// Occlusion culling: the depth pyramid, the late culling pass and the auto mode.
    occlusion: Occlusion,
    /// Per instance slot, the occlusion state (`state` in cull.wgsl).
    vis_state: wgpu::Buffer,
    lights: wgpu::Buffer,
    cluster_buf: wgpu::Buffer,
    cluster_lights: wgpu::Buffer,
    cluster_overflow: wgpu::Buffer,

    cull_pipeline: wgpu::ComputePipeline,
    cluster_pipeline: wgpu::ComputePipeline,
    forward: [wgpu::RenderPipeline; 4],
    /// The neural-texture variants' pipelines (variants 4..8), built when the first neural texture
    /// loads, for its network's shape (neural.rs).
    neural_forward: Option<[wgpu::RenderPipeline; 4]>,
    forward_layout: wgpu::PipelineLayout,
    /// Loaded neural textures: their latents and networks (neural.rs).
    neural: NeuralTable,
    /// Whether neural textures decode in half precision (shader-f16 and not
    /// `POCKET_NEURAL_PRECISION=f32`).
    neural_f16: bool,
    shadow: [wgpu::RenderPipeline; 2],
    empty_group: wgpu::BindGroup,
    ocean: Ocean,
    picking: Picking,
    skinning: Skinning,
    ui: Ui,
    particles: Particles,
    /// The wall clock of the last frame (particle steps).
    last_frame_s: Option<f64>,
    /// Editor overlays: gizmo shapes, selection outline, grid and axes.
    pub overlays: Overlays,
    /// The last finished pick: `Some(None)` when the pixel shows no entity.
    last_pick: Option<Option<u64>>,
    /// The last finished coverage read: (entity, share of the view's pixels), largest first.
    last_visible: Option<Vec<(u64, f32)>>,
    /// That read's slot ids per pixel (`slot + 1`, 0 for nothing), row by row, when
    /// `request_id_image` asked to keep them.
    last_ids: Option<Vec<u32>>,
    keep_ids: bool,
    /// The splat clouds' entities when the pending id pass was drawn (its `SPLAT_PICK` ids index
    /// them).
    pick_splats: Vec<u64>,
    batch_offsets: wgpu::Buffer,
    sky_pipeline: wgpu::RenderPipeline,
    layouts: Layouts,
    /// The shadow cascades' texture, kept with its views.
    _shadow_tex: wgpu::Texture,
    shadow_views: Vec<wgpu::TextureView>,
    shadow_array: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    tex_sampler: wgpu::Sampler,

    bind: Option<Binds>,
    bind_key: (u64, u64, u64, u64),
    view_stride: u32,
    draw_meshes: u32,
    /// How levels of detail are picked (lod.rs); whether levels were asked for when the batches'
    /// regions were last laid out, and whether those regions hold level rows (not when the lists
    /// would pass the device's binding limit, docs/spec/lod.md 5).
    lod: LodSettings,
    regions_want: Option<bool>,
    regions_lod: Option<bool>,
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

/// The bytes of the views' lists for `stride` entries per view: the camera's `drawn` records (48
/// bytes each) and the `visible` indices (4 bytes per entry and view).
fn list_bytes(stride: u32) -> (u64, u64) {
    (
        u64::from(stride) * 48,
        u64::from(stride) * 4 * u64::from(VIEWS),
    )
}

/// The largest list the device can bind whole (a multiple of 256 bytes).
fn list_limit(device: &wgpu::Device) -> u64 {
    let l = device.limits();
    l.max_storage_buffer_binding_size.min(l.max_buffer_size) & !255
}

/// Grows `buf` to hold `need` bytes, doubling up to `limit`; also replaces a buffer past `limit`
/// once `need` fits. Returns whether it was replaced.
fn fit_list(
    device: &wgpu::Device,
    buf: &mut wgpu::Buffer,
    label: &str,
    need: u64,
    limit: u64,
) -> bool {
    if need <= buf.size() && (buf.size() <= limit || need > limit) {
        return false;
    }
    let size = need.next_power_of_two().min(limit.max(need));
    *buf = storage(device, label, size, wgpu::BufferUsages::empty());
    true
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
        let rt_shadows = crate::rt_shadows::RtShadows::new(gpu);
        let mut lighting_entries = vec![
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
            entry(8, frag, uni_ty()),
            entry(9, frag, buf_ty(true)),
            entry(10, frag, buf_ty(true)),
        ];
        // Ray-traced shadows: the casters' top level and table, and the mesh pool (rt_shadows.rs).
        if rt_shadows.is_some() {
            lighting_entries.extend(crate::rt_shadows::RtShadows::layout_entries());
        }
        let lighting = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lighting"),
            entries: &lighting_entries,
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
                // Neural textures (forward_neural.wgsl): latents, then the data uniform twice (the
                // second view is the half-precision decoder's).
                entry(
                    3,
                    frag,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(4, frag, uni_ty()),
                entry(5, frag, uni_ty()),
            ],
        });
        let fwd_module = shaders::module(device, "forward");
        // With ray-traced shadows the lit pipelines (forward, ocean) trace the sun's shadow rays.
        let rt_module = rt_shadows
            .as_ref()
            .map(|_| crate::rt_shadows::RtShadows::forward_module(device));
        let lit_module = rt_module.as_ref().unwrap_or(&fwd_module);
        let batches = Batches::new(device, &gpu.caps);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("forward"),
            bind_group_layouts: &[
                Some(&frame),
                Some(&lighting),
                Some(&textures),
                Some(&batches.layout),
            ],
            immediate_size: 0,
        });
        let vertex_layout = vertex_layout();
        let forward_pipe = |cull: Option<wgpu::Face>, fs: &str, label: &str| {
            forward_pipeline(device, &layout, lit_module, cull, fs, label)
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
            bind_group_layouts: &[
                Some(&frame),
                Some(&empty),
                Some(&textures),
                Some(&batches.layout),
            ],
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
        let picking = Picking::new(
            device,
            &fwd_module,
            [&frame, &empty, &empty, &batches.layout],
            vertex_layout.clone(),
        );
        let ocean = Ocean::new(
            device,
            lit_module,
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
            if let Some(mut m) = primitive(name) {
                pocket_assets::lod::build(&mut m, &pocket_assets::lod::LodOptions::default());
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
            gi: crate::gi::ProbeVolume::new(device, &gpu.queue),
            rt_shadows,
            profiler: GpuProfiler::new(device, &gpu.queue, gpu.caps.timestamps),
            splats: crate::splat::Splats::new(gpu),
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
            batches,
            occlusion: Occlusion::new(device, OcclusionMode::from_env()),
            vis_state: storage(
                device,
                "occlusion state",
                1024 * 4,
                wgpu::BufferUsages::empty(),
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
            neural_forward: None,
            forward_layout: layout,
            neural: NeuralTable::new(device),
            neural_f16: gpu.caps.shader_f16
                && std::env::var("POCKET_NEURAL_PRECISION").map_or(true, |v| v.trim() != "f32"),
            shadow,
            ocean,
            picking,
            skinning: Skinning::new(device),
            ui: Ui::new(device, output),
            particles: Particles::new(device),
            last_frame_s: None,
            overlays: Overlays::new(device, output),
            last_pick: None,
            last_visible: None,
            last_ids: None,
            keep_ids: false,
            pick_splats: Vec::new(),
            empty_group,
            batch_offsets: storage(device, "batch offsets", 64 * 4, wgpu::BufferUsages::empty()),
            sky_pipeline,
            layouts: Layouts {
                frame,
                lighting,
                textures,
            },
            _shadow_tex: shadow_tex,
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
            bind_key: (u64::MAX, 0, 0, 0),
            view_stride: 1,
            draw_meshes: 0,
            lod: LodSettings::from_env(),
            regions_want: None,
            regions_lod: None,
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

    /// How the GPU-driven passes issue their indirect draws (`multi-draw`, `first-instance` or
    /// `baseline`; batches.rs).
    pub fn draw_path(&self) -> &'static str {
        self.batches.path.name()
    }

    /// The ray-traced sun shadows' last frame, when they replace the cascades (rt_shadows.rs).
    pub fn rt_shadow_stats(&self) -> Option<crate::rt_shadows::RtShadowStats> {
        self.rt_shadows.as_ref().map(|rt| rt.stats)
    }

    pub fn set_asset_source(&mut self, source: Box<dyn AssetSource>) {
        self.loader = source;
        self.gi.asset.clear();
        self.gi.loading = false;
        self.gi.clear(&self.gpu.queue);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == (self.targets.width, self.targets.height)
        {
            return;
        }
        self.targets = Targets::new(&self.gpu.device, width, height);
        // The pyramid's first level reads the old depth target.
        self.occlusion.invalidate(true);
    }

    /// Whether the camera view is occlusion culled (occlusion.rs; `POCKET_OCCLUSION` sets the
    /// starting mode).
    pub fn set_occlusion(&mut self, mode: OcclusionMode) {
        self.occlusion.set_mode(mode);
    }

    pub fn occlusion(&self) -> OcclusionMode {
        self.occlusion.mode
    }

    /// Whether instances draw coarser levels of detail (lod.rs; `POCKET_LOD` sets the starting
    /// mode). Turning them off draws every instance's full mesh from the next frame.
    pub fn set_lod(&mut self, mode: LodMode) {
        self.lod.mode = mode;
    }

    /// How levels are picked: the mode, the pixel and texel bounds and the hysteresis.
    pub fn set_lod_settings(&mut self, settings: LodSettings) {
        self.lod = settings;
    }

    pub fn lod(&self) -> LodSettings {
        self.lod
    }

    /// Each shadow cascade's texel in metres when drawn from `cam` at this renderer's size: a
    /// cascade draws the coarsest level whose error is at most `shadow_texels` of them (lod.rs).
    pub fn cascade_texels(&self, cam: &CameraState) -> [f32; CASCADES] {
        let aspect = self.targets.width as f32 / self.targets.height.max(1) as f32;
        shadows::cascades(cam, aspect, Vec3::Y, SHADOW_DISTANCE).texel
    }

    /// The bytes the views' lists need for the current regions: the camera's `drawn` records (48
    /// bytes per entry) and the cascades' `visible` indices (4 bytes per entry and view). Levels of
    /// detail multiply a mesh's entries by its levels (docs/spec/lod.md 5).
    pub fn list_bytes(&self) -> u64 {
        let (drawn, visible) = list_bytes(self.view_stride);
        drawn + visible
    }

    /// What the last submitted frame drew, per argument set and level of detail, read back from
    /// the GPU's indirect arguments. Blocks until the GPU is done (checks and benchmarks).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn draw_counts(&self) -> crate::lod::DrawCounts {
        let args = self.batches.read_back(&self.gpu.device, &self.gpu.queue);
        let rows = self.batches.rows() as usize;
        let mut out = crate::lod::DrawCounts::default();
        for (i, a) in args.iter().enumerate() {
            let set = i / (rows * VARIANTS as usize);
            let level = (self.meshes.level_of((i % rows) as u32) as usize).min(7);
            let counts = match set {
                0 => &mut out.camera,
                s if (s as u32) < VIEWS => &mut out.cascades[s - 1],
                _ => &mut out.late,
            };
            counts.instances[level] += u64::from(a[1]);
            counts.triangles[level] += u64::from(a[1]) * u64::from(a[0] / 3);
        }
        out
    }

    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Device pixels per logical pixel for the game UI (the window's scale factor).
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui.scale = scale.max(0.5);
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
        if frame.reset {
            self.gi.asset.clear();
            self.gi.loading = false;
            self.gi.clear(&self.gpu.queue);
        }
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
            skinning: &mut self.skinning,
            device: &self.gpu.device,
            queue: &self.gpu.queue,
        };
        self.scene.apply(frame, &mut pools);
    }

    /// Registers a neural texture as the material `path` (natively the loader does this when a
    /// look names a `.ntex`; tools and the browser may call it directly). The first one fixes the
    /// network shape the neural pipelines are compiled for; another shape is refused.
    pub fn add_neural_texture(
        &mut self,
        path: &str,
        texture: &NeuralTexture,
    ) -> Result<(), String> {
        let descriptor = self
            .neural
            .add(&self.gpu.device, &self.gpu.queue, texture)?;
        if self.neural_forward.is_none() {
            self.neural_forward = Some(self.neural_pipelines(&texture.layout));
        }
        self.materials.add_neural(path, descriptor, &texture.layout);
        log::info!(
            "neural texture {path}: {}x{}, {} ({} precision)",
            texture.width,
            texture.height,
            crate::neural::describe(&texture.layout),
            if self.neural_f16 { "half" } else { "single" }
        );
        Ok(())
    }

    /// Decode neural textures in half precision (when the device has shader-f16) or single;
    /// rebuilds their pipelines. Returns the precision in use.
    pub fn set_neural_half_precision(&mut self, f16: bool) -> bool {
        self.neural_f16 = f16 && self.gpu.caps.shader_f16;
        if let Some(layout) = self.neural.profile.clone() {
            self.neural_forward = Some(self.neural_pipelines(&layout));
        }
        self.neural_f16
    }

    /// Whether neural textures decode in half precision.
    pub fn neural_half_precision(&self) -> bool {
        self.neural_f16
    }

    fn neural_pipelines(&self, layout: &NeuralLayout) -> [wgpu::RenderPipeline; 4] {
        let device = &self.gpu.device;
        let source = shaders::forward_neural(layout, self.neural_f16, self.rt_shadows.is_some());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("forward (neural textures)"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipe = |cull: Option<wgpu::Face>, label: &str| {
            forward_pipeline(
                device,
                &self.forward_layout,
                &module,
                cull,
                "fs_neural",
                label,
            )
        };
        // Neural materials are never alpha-masked: variants 5 and 7 hold no instances.
        [
            pipe(Some(wgpu::Face::Back), "forward (neural)"),
            pipe(Some(wgpu::Face::Back), "forward (neural, unused masked)"),
            pipe(None, "forward (neural, double sided)"),
            pipe(None, "forward (neural, unused masked double sided)"),
        ]
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
                (
                    n.mesh,
                    (t, r, s),
                    n.skin.filter(|_| !asset.skins.is_empty()),
                )
            })
            .collect();
        let skinned = (!asset.skins.is_empty()).then(|| std::sync::Arc::new(asset.clone()));
        self.models.insert(
            path.to_owned(),
            Model {
                meshes,
                names,
                nodes,
                skinned,
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
        for (path, r) in self.loader.poll_neural_textures() {
            if let Err(e) = r.and_then(|t| self.add_neural_texture(&path, &t)) {
                log::error!("neural texture {path}: {e}; drawn with the default material");
                self.materials.add_fallback(&path);
            }
        }
        let mut pools = Pools {
            meshes: &mut self.meshes,
            materials: &mut self.materials,
            models: &self.models,
            loader: self.loader.as_mut(),
            requested: &mut self.requested,
            failed: &self.failed,
            skinning: &mut self.skinning,
            device: &self.gpu.device,
            queue: &self.gpu.queue,
        };
        self.scene.retry_pending(&mut pools);
    }

    fn poll_gi(&mut self) {
        let neural_path = self
            .scene
            .environment
            .as_ref()
            .map_or("", |e| e.neural_gi.as_str());
        let is_neural = !neural_path.is_empty();
        let path = if is_neural {
            neural_path
        } else {
            self.scene
                .environment
                .as_ref()
                .map_or("", |e| e.baked_gi.as_str())
        }
        .to_owned();
        let intensity = self
            .scene
            .environment
            .as_ref()
            .map_or(1.0, |e| e.gi_intensity);
        self.gi.set_intensity(&self.gpu.queue, intensity);
        if path != self.gi.asset || is_neural != self.gi.requested_neural {
            self.gi.asset = path.clone();
            self.gi.requested_neural = is_neural;
            self.gi.clear(&self.gpu.queue);
            self.gi.loading = !path.is_empty();
            if !path.is_empty() {
                if is_neural {
                    self.loader.request_neural_gi(&path);
                } else {
                    self.loader.request_baked_gi(&path);
                }
            }
        }
        for (loaded_path, result) in self.loader.poll_baked_gi() {
            if loaded_path != self.gi.asset || is_neural {
                continue;
            }
            self.gi.loading = false;
            match result.and_then(|data| self.gi.upload(&self.gpu.device, &self.gpu.queue, data)) {
                Ok(()) => log::info!("loaded baked GI: {loaded_path}"),
                Err(error) => {
                    log::error!("baked GI {loaded_path}: {error}");
                    self.gi.error = Some(error);
                }
            }
        }
        for (loaded_path, result) in self.loader.poll_neural_gi() {
            if loaded_path != self.gi.asset || !is_neural {
                continue;
            }
            self.gi.loading = false;
            match result.and_then(|data| {
                self.gi
                    .upload_neural(&self.gpu.device, &self.gpu.queue, data)
            }) {
                Ok(()) => log::info!("loaded neural GI: {loaded_path}"),
                Err(error) => {
                    log::error!("neural GI {loaded_path}: {error}");
                    self.gi.error = Some(error);
                }
            }
        }
    }

    /// Loads a validated GI field explicitly (capture tools and GPU experiments).
    pub fn set_baked_gi(&mut self, data: pocket_assets::gi::BakedGi) -> Result<(), String> {
        self.gi.upload(&self.gpu.device, &self.gpu.queue, data)
    }

    /// A failed GI asset must not be mistaken for a successfully measured lighting mode.
    pub fn gi_error(&self) -> Option<&str> {
        self.gi.error.as_deref()
    }

    /// Whether a validated lighting field is resident and its asynchronous load succeeded.
    pub fn gi_loaded(&self) -> bool {
        self.gi.has_data() && !self.gi.loading && self.gi.error.is_none()
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
        if n * 4 > self.vis_state.size() {
            // Zeroed: nothing counts as visible last frame, so the next frame draws late.
            self.vis_state = storage(
                device,
                "occlusion state",
                (n * 4).next_power_of_two(),
                wgpu::BufferUsages::empty(),
            );
            grown = true;
        }
        // A full upload (a reset, possibly to an empty scene, or grown buffers) changes the casters
        // even when it leaves no dirty run behind.
        let full_upload = self.scene.full_upload;
        let runs = self.scene.take_dirty_runs();
        if let Some(rt) = &mut self.rt_shadows {
            rt.slots_changed(full_upload || !runs.is_empty());
        }
        for run in runs {
            let slots: &[InstanceGpu] = &self.scene.slots[run.start as usize..run.end as usize];
            queue.write_buffer(
                &self.instances,
                u64::from(run.start) * 80,
                bytemuck::cast_slice(slots),
            );
        }
        let mesh_count = self.meshes.len() as u32;
        // Ray-traced shadows meet the full meshes: a receiver drawn coarser than the mesh its rays
        // start from shadows itself (docs/spec/lod.md 7), so they draw full meshes.
        let want_lod = self.lod.on() && self.rt_shadows.is_none();
        if self.scene.counts_changed
            || mesh_count != self.draw_meshes
            || self.regions_want != Some(want_lod)
        {
            self.scene.counts_changed = false;
            // Whether the last layout already gave levels up at the limit (warn once, not at every
            // layout of a scene that keeps changing).
            let was_limited = self.regions_want == Some(true) && self.regions_lod == Some(false);
            self.regions_want = Some(want_lod);
            // A level row draws its mesh's instances while levels are on, none while they are off.
            let owners_with = |lod_on: bool| -> Vec<u32> {
                self.meshes
                    .owner
                    .iter()
                    .enumerate()
                    .map(|(row, &o)| {
                        if lod_on || o == row as u32 {
                            o
                        } else {
                            u32::MAX
                        }
                    })
                    .collect()
            };
            // The lists are bound whole: a level row's region holds every instance of its mesh,
            // so levels multiply them, and past the device's limit nothing would draw. Then the
            // scene draws full meshes with the lists it had without levels (docs/spec/lod.md 5).
            let limit = list_limit(device);
            let largest = |stride: u32| {
                let (drawn, visible) = list_bytes(stride);
                drawn.max(visible)
            };
            let mut lod_on = want_lod;
            let mut owners = owners_with(lod_on);
            let (mut offsets, mut stride) = self.scene.batch_offsets(&owners);
            if lod_on && largest(stride) > limit {
                let with = largest(stride);
                lod_on = false;
                owners = owners_with(false);
                (offsets, stride) = self.scene.batch_offsets(&owners);
                if !was_limited {
                    log::warn!(
                        "levels of detail off: with them a view list would take {with} bytes, \
                         over this device's {limit}-byte binding limit ({} without them)",
                        largest(stride)
                    );
                }
            } else if lod_on && was_limited {
                log::info!("levels of detail back on: the views' lists fit the binding limit");
            }
            if largest(stride) > limit {
                log::error!(
                    "a view list takes {} bytes, over this device's {limit}-byte binding limit: \
                     instances will not draw",
                    largest(stride)
                );
            }
            self.regions_lod = Some(lod_on);
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
            let (drawn, vis) = list_bytes(stride);
            grown |= fit_list(device, &mut self.visible, "visible", vis, limit);
            grown |= fit_list(device, &mut self.drawn, "drawn", drawn, limit);
            self.view_stride = stride;
            let sizes = self.scene.batch_sizes(&owners);
            grown |=
                self.batches
                    .rebuild(device, queue, &self.meshes.infos, &offsets, &sizes, stride);
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
        if let Some(rt) = &mut self.rt_shadows {
            grown |= rt.prepare(device, queue, &self.meshes, self.scene.slots.len());
        }
        let key = (
            self.meshes.generation,
            self.materials.generation,
            self.gi.generation,
            self.neural.generation,
        );
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
                b(3, &self.batches.draws),
                b(4, &self.visible),
                b(5, &self.batch_offsets),
                b(6, &self.drawn),
                b(7, &self.vis_state),
            ],
        });
        self.occlusion.invalidate(false);
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
        let mut lighting_entries = vec![
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
            b(8, &self.gi.params),
            b(9, &self.gi.radiance),
            b(10, &self.gi.distances),
        ];
        if let Some(rt) = &self.rt_shadows {
            lighting_entries.extend(rt.bind_entries(&self.meshes));
        }
        let lighting = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lighting"),
            layout: &self.layouts.lighting,
            entries: &lighting_entries,
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.neural.latents_view),
                },
                b(4, &self.neural.data),
                b(5, &self.neural.data),
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
        self.poll_gi();
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
        // Ray-traced shadows replace the cascades: no cascade views to cull or draw.
        let cascades = shadows && self.rt_shadows.is_none();
        let views = if cascades { VIEWS } else { 1 };
        let n = self.scene.instance_count() as u32;
        // Two phases this frame (occlusion.rs)?
        let occl = self.occlusion.begin_frame(n > 0);
        // Levels of detail: the regions were sized for this mode in `sync` (lod.rs).
        let lod_on = self.regions_lod == Some(true);
        let (eye, ortho) = self.lod.camera_terms(&cam, h);
        let cu = CullUniform {
            planes,
            view_proj: mat(vp),
            instance_count: n,
            view_count: views,
            mesh_count: self.draw_meshes,
            view_stride: self.view_stride,
            alpha,
            lod_on: u32::from(lod_on),
            occlusion: u32::from(occl),
            hiz_levels: crate::occlusion::levels(w, h),
            viewport: [w as f32, h as f32],
            _pad: [0; 2],
            eye,
            lod: [
                if ortho { 1.0 } else { 0.0 },
                self.lod.hysteresis.max(0.0),
                0.0,
                0.0,
            ],
            lod_cascades: self.lod.cascade_terms(&casc.texel),
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
        self.batches.reset(&mut enc);
        let frame_dt = self
            .last_frame_s
            .map_or(1.0 / 60.0, |t| (now_s - t).clamp(0.0, 0.1)) as f32;
        self.last_frame_s = Some(now_s);
        self.particles
            .update(&device, &queue, &mut enc, &self.scene.emitters, frame_dt);
        // Skinned parts' vertices for the drawn moment (between the last two ticks).
        let since_tick = (f64::from(alpha) - 1.0) * self.scene.dt_s;
        self.skinning.encode(
            &device,
            &queue,
            &mut enc,
            &self.meshes.vertices,
            &self.scene.anims,
            since_tick as f32,
        );
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
        if let Some(rt) = &mut self.rt_shadows {
            rt.encode(&mut enc, &queue, &self.meshes, &self.scene, alpha);
        }
        let mut draw_calls = 0;
        // Without shadows the cascades are never sampled (`shadow_factor` returns 1): skip them.
        for c in 0..if cascades { CASCADES } else { 0 } {
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
                draw_calls += self.batches.draw(&mut pass, c as u32 + 1, variant);
            }
        }
        // Gaussian splats (splat/): preprocess and sort; the opaque pass keeps its depth for them.
        self.splats
            .prepare(&mut enc, &mut self.profiler, &mut self.scene, &cam, (w, h));
        let picking = self.picking.begin(&device, (w, h));
        // The opaque pass, or with occlusion culling its early half: what the camera saw last frame.
        {
            let ts = self
                .profiler
                .render_scope(if occl { "opaque early" } else { "opaque+sky" });
            let mut pass = opaque_pass(
                &mut enc,
                &self.targets,
                ts,
                true,
                !occl,
                self.splats.active(),
            );
            pass.set_bind_group(0, &binds.frame, &[]);
            pass.set_bind_group(1, &binds.lighting, &[]);
            pass.set_bind_group(2, &binds.textures, &[]);
            pass.set_vertex_buffer(0, self.meshes.vertices.slice(..));
            pass.set_index_buffer(self.meshes.indices.slice(..), wgpu::IndexFormat::Uint32);
            for variant in 0..VARIANTS {
                let Some(p) = forward_for(&self.forward, &self.neural_forward, variant) else {
                    continue;
                };
                pass.set_pipeline(p);
                draw_calls += self.batches.draw(&mut pass, 0, variant);
            }
            if !occl {
                self.draw_after_opaque(&device, &mut pass, binds);
            }
        }
        let ids = IdPass {
            frame: &binds.frame,
            empty: &self.empty_group,
            batches: &self.batches,
            meshes: &self.meshes,
            splats: &self.splats,
        };
        if picking {
            ids.draw(
                &mut self.picking,
                &mut self.profiler,
                &mut enc,
                0,
                true,
                !occl,
            );
        }
        if occl {
            self.occlusion.encode_pyramid(
                &device,
                &mut enc,
                &mut self.profiler,
                &self.targets.depth,
                (w, h),
            );
            self.occlusion.encode_late(
                &device,
                &mut enc,
                &mut self.profiler,
                &LateInputs {
                    cull: &self.cull_buf,
                    instances: &self.instances,
                    meshes: &self.meshes.info_buffer,
                    draws: &self.batches.draws,
                    offsets: &self.batch_offsets,
                    drawn: &self.drawn,
                    state: &self.vis_state,
                },
                n,
            );
            // The late half: the newly visible instances, then the sky and the rest.
            let ts = self.profiler.render_scope("opaque+sky");
            let mut pass = opaque_pass(
                &mut enc,
                &self.targets,
                ts,
                false,
                true,
                self.splats.active(),
            );
            pass.set_bind_group(0, &binds.frame, &[]);
            pass.set_bind_group(1, &binds.lighting, &[]);
            pass.set_bind_group(2, &binds.textures, &[]);
            pass.set_vertex_buffer(0, self.meshes.vertices.slice(..));
            pass.set_index_buffer(self.meshes.indices.slice(..), wgpu::IndexFormat::Uint32);
            for variant in 0..VARIANTS {
                let Some(p) = forward_for(&self.forward, &self.neural_forward, variant) else {
                    continue;
                };
                pass.set_pipeline(p);
                draw_calls += self.batches.draw(&mut pass, LATE, variant);
            }
            self.draw_after_opaque(&device, &mut pass, binds);
            drop(pass);
            if picking {
                ids.draw(
                    &mut self.picking,
                    &mut self.profiler,
                    &mut enc,
                    LATE,
                    false,
                    true,
                );
            }
        }
        if picking {
            self.picking.finish(&device, &mut enc);
            self.pick_splats = self.splats.drawn_entities().to_vec();
        }
        // Gaussian splats (splat/): drawn over the resolved image, tested against the depth.
        self.splats
            .draw(&mut enc, &mut self.profiler, &self.targets);
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
        if !self.scene.ui.is_empty() {
            self.ui.items = self.scene.ui.clone();
            self.ui.draw(&device, &queue, &mut enc, output, (w, h), vp);
        }
        self.profiler.resolve(&mut enc);
        queue.submit([enc.finish()]);
        self.profiler.after_submit();
        self.picking.after_submit();
        self.occlusion.after_submit();
        self.last = FrameStats {
            cpu_ms: (web_time() - cpu) as f32 * 1000.0,
            gpu_ms: self.profiler.total_ms(),
            passes: merge_passes(&self.profiler.last),
            instances: self.scene.instance_count(),
            entities: self.scene.entity_count(),
            meshes: self.meshes.len(),
            materials: self.materials.len(),
            lights: self.light_count as usize,
            pending_assets: self.scene.pending() + usize::from(self.gi.loading),
            tick: self.scene.tick,
            backend: self.gpu.backend_name(),
            draw_path: self.batches.path.name(),
            draw_calls,
            occlusion: self.occlusion.label(),
            occlusion_stats: self.occlusion.last,
            lod: match (lod_on, self.regions_want) {
                (true, _) => "on",
                (false, Some(true)) => "off-limit",
                _ => "off",
            },
        };
        self.last.clone()
    }

    /// What the opaque pass draws after the instances: the ocean, the sky, particles and the
    /// editor's grid.
    fn draw_after_opaque(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        binds: &Binds,
    ) {
        if self.scene.sea.is_some() {
            self.ocean.draw(pass);
        }
        pass.set_pipeline(&self.sky_pipeline);
        pass.set_bind_group(0, &binds.sky, &[]);
        pass.draw(0..3, 0..1);
        self.particles.draw(device, pass, &self.view_buf);
        self.overlays.draw_grid(device, pass, &self.view_buf);
    }

    /// The entity a ray through pixel (x, y) of the output hits first, with the distance and the
    /// world point: a CPU test of each drawn part's oriented bounding box, answered at once (the
    /// editor's click; the GPU id pass answers coverage questions).
    pub fn pick_ray(&self, x: f32, y: f32) -> Option<(u64, f32, [f32; 3])> {
        let (w, h) = (self.targets.width as f32, self.targets.height as f32);
        let cam = self.camera();
        let vp = cam.proj(w / h.max(1.0)) * cam.view();
        let inv = vp.inverse();
        let ndc = glam::Vec2::new(x / w * 2.0 - 1.0, 1.0 - y / h * 2.0);
        let near = inv.project_point3(glam::Vec3::new(ndc.x, ndc.y, 1.0));
        let far = inv.project_point3(glam::Vec3::new(ndc.x, ndc.y, 1e-4));
        let origin = near;
        let dir = (far - near).normalize_or_zero();
        if dir == glam::Vec3::ZERO {
            return None;
        }
        let mut best: Option<(u64, f32)> = None;
        for (slot, inst) in self.scene.slots.iter().enumerate() {
            if inst.flags & crate::scene::FLAG_ALIVE == 0
                || inst.flags & crate::scene::FLAG_VISIBLE == 0
            {
                continue;
            }
            let Some(&(lo, hi)) = self.meshes.boxes.get(inst.mesh as usize) else {
                continue;
            };
            let rot = Quat::from_array(inst.rot);
            let inv_rot = rot.inverse();
            let scale = Vec3::from(inst.scale);
            let safe = |v: f32| {
                if v.abs() < 1e-6 {
                    1e-6f32.copysign(v)
                } else {
                    v
                }
            };
            let s = Vec3::new(safe(scale.x), safe(scale.y), safe(scale.z));
            // The ray in the part's unscaled local frame.
            let o = inv_rot * (origin - Vec3::from(inst.pos)) / s;
            let d = inv_rot * dir / s;
            let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
            let t1 = (lo - o) / d;
            let t2 = (hi - o) / d;
            let tmin = t1.min(t2).max_element();
            let tmax = t1.max(t2).min_element();
            if tmax >= tmin.max(0.0) {
                let t = tmin.max(0.0);
                if best.is_none_or(|(_, bt)| t < bt)
                    && let Some(e) = self.scene.entity_of_slot(slot as u32)
                {
                    best = Some((e, t));
                }
            }
        }
        best.map(|(e, t)| {
            let p = origin + dir * t;
            (e, t, p.to_array())
        })
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

    /// The picking state, for diagnostics.
    pub fn pick_debug(&self) -> String {
        self.picking.debug()
    }

    /// Asks which entities the view shows and how much of it each covers.
    pub fn request_visible(&mut self) {
        self.picking.request(PickRequest::Full);
    }

    /// [`Renderer::request_visible`], also keeping the read's id at every pixel for
    /// [`Renderer::take_id_image`] (checks compare whole id images).
    pub fn request_id_image(&mut self) {
        self.keep_ids = true;
        self.request_visible();
    }

    /// The entity at every pixel (0 for none), row by row, from the read that gave the last
    /// [`Renderer::take_visible`] answer after [`Renderer::request_id_image`].
    pub fn take_id_image(&mut self) -> Option<Vec<u64>> {
        let ids = self.last_ids.take()?;
        Some(
            ids.iter()
                .map(|&id| self.entity_of_id(id).unwrap_or(0))
                .collect(),
        )
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
                self.last_pick = Some(self.entity_of_id(id));
            }
            PickRequest::Full => {
                let total = r.ids.len().max(1) as f32;
                let mut by_entity: HashMap<u64, u32> = HashMap::new();
                for (slot, n) in coverage(&r.ids) {
                    if let Some(e) = self.entity_of_id(slot + 1) {
                        *by_entity.entry(e).or_insert(0) += n;
                    }
                }
                let mut v: Vec<(u64, f32)> = by_entity
                    .into_iter()
                    .map(|(e, n)| (e, n as f32 / total))
                    .collect();
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                self.last_visible = Some(v);
                if std::mem::take(&mut self.keep_ids) {
                    self.last_ids = Some(r.ids);
                }
            }
        }
    }

    /// The entity an id of the entity-id pass names: a mesh slot + 1, or a splat cloud
    /// (`SPLAT_PICK` and its index among the clouds drawn with that pass); 0 is nothing.
    fn entity_of_id(&self, id: u32) -> Option<u64> {
        if id == 0 {
            None
        } else if id & crate::splat::SPLAT_PICK != 0 {
            self.pick_splats
                .get((id & !crate::splat::SPLAT_PICK) as usize)
                .copied()
        } else {
            self.scene.entity_of_slot(id - 1)
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
        if let Ok(Ok(())) = rx.recv()
            && let Ok(data) = slice.get_mapped_range()
        {
            let bgra = matches!(
                self.output,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            );
            for y in 0..h {
                let r = &data[(y * row) as usize..(y * row + w * 4) as usize];
                if bgra {
                    for &[blue, green, red, _] in r.as_chunks::<4>().0 {
                        out.extend_from_slice(&[red, green, blue, 255]);
                    }
                } else {
                    out.extend_from_slice(r);
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

/// The entity-id pass over one argument set of the camera's batches (the camera's, or the late
/// one), every variant through the one id pipeline; the last call adds the splats.
struct IdPass<'a> {
    frame: &'a wgpu::BindGroup,
    empty: &'a wgpu::BindGroup,
    batches: &'a Batches,
    meshes: &'a MeshPool,
    splats: &'a crate::splat::Splats,
}

impl IdPass<'_> {
    fn draw(
        &self,
        picking: &mut Picking,
        profiler: &mut GpuProfiler,
        enc: &mut wgpu::CommandEncoder,
        set: u32,
        first: bool,
        last: bool,
    ) {
        let id_draw = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_bind_group(1, self.empty, &[]);
            pass.set_bind_group(2, self.empty, &[]);
            for variant in 0..VARIANTS {
                self.batches.draw(pass, set, variant);
            }
            // Gaussian splats (splat/): after the meshes, against their depth.
            if last {
                self.splats.draw_ids(pass);
            }
        };
        picking.draw(
            enc,
            self.frame,
            &self.meshes.vertices,
            &self.meshes.indices,
            &id_draw,
            profiler.render_scope("entity ids"),
            first,
            last,
        );
    }
}

/// A pass into the multisampled HDR target and its depth: `first` clears them (else they are
/// loaded), `last` resolves the color into the HDR image; the depth is kept unless it is the last
/// pass and nothing reads it afterwards (`keep_depth`: the splats).
fn opaque_pass<'e>(
    enc: &'e mut wgpu::CommandEncoder,
    t: &Targets,
    timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    first: bool,
    last: bool,
    keep_depth: bool,
) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(if last { "opaque" } else { "opaque (early)" }),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &t.color_msaa,
            depth_slice: None,
            resolve_target: last.then_some(&t.hdr_view),
            ops: wgpu::Operations {
                load: if first {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: if last {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                },
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &t.depth,
            depth_ops: Some(wgpu::Operations {
                load: if first {
                    wgpu::LoadOp::Clear(0.0)
                } else {
                    wgpu::LoadOp::Load
                },
                store: if last && !keep_depth {
                    wgpu::StoreOp::Discard
                } else {
                    wgpu::StoreOp::Store
                },
            }),
            stencil_ops: None,
        }),
        timestamp_writes: timestamps,
        ..Default::default()
    })
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

/// The forward pass's vertex buffer: the shared mesh vertices.
fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<pocket_assets::Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

/// A forward pipeline: `vs` and fragment entry `fs` of `module` into the multisampled HDR target.
fn forward_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    cull: Option<wgpu::Face>,
    fs: &str,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(vertex_layout())],
        },
        fragment: Some(wgpu::FragmentState {
            module,
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
}

/// The forward pipeline of `variant`: the neural ones only once a neural texture has loaded.
fn forward_for<'a>(
    forward: &'a [wgpu::RenderPipeline; 4],
    neural: &'a Option<[wgpu::RenderPipeline; 4]>,
    variant: u32,
) -> Option<&'a wgpu::RenderPipeline> {
    if variant < NEURAL_VARIANT {
        Some(&forward[variant as usize])
    } else {
        neural
            .as_ref()
            .map(|n| &n[(variant - NEURAL_VARIANT) as usize])
    }
}
