//! Static surface path tracing on wgpu ray queries: Metal, Vulkan (`VK_KHR_ray_query`) or
//! Direct3D 12 (DXR 1.1), wherever the adapter exposes `EXPERIMENTAL_RAY_QUERY`
//! ([`super::require_ray_query`]). Geometry is immutable; lighting scales and camera sampling are
//! parameters. Native batches retain accumulation and statistics on the GPU and read back once.
//! The ordinary renderer and earlier GI experiments remain independent.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use pocket_assets::mesh::{AlphaMode, ImageData};
use pocket_assets::{LightKind, SkyKind};
use serde::Serialize;
use wgpu::util::DeviceExt;

use super::bake::RayScene;
use super::nrc::{NrcConfig, NrcFrameStats, OnlineNrc};
use crate::Gpu;
use crate::sky::{Sky, SkyParams};

pub use super::rt::RayCamera;
const COUNTERS: u64 = 16;

/// Explicit upload settings for static scenes; these do not change path sampling.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PtSceneOptions {
    /// Maximum width and height of the common material texture array, in texels.
    pub texture_size: u32,
}

impl Default for PtSceneOptions {
    fn default() -> Self {
        Self { texture_size: 2048 }
    }
}

impl PtSceneOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=2048).contains(&self.texture_size) {
            return Err("PT texture size must be in 1..=2048".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PtOptions {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub frames: u32,
    pub bounces: u32,
    pub rr_start: u32,
    pub nee: bool,
    pub russian_roulette: bool,
    pub seed: u32,
    /// Diagnostic baseline: wait on every frame rather than queueing the complete batch.
    pub synchronize_frames: bool,
    /// Diagnostic baseline: copy/map the HDR image after every frame, discarding the interim data.
    pub readback_every_frame: bool,
    /// Emission, punctual lights, environment and overall radiance scales.
    pub lighting_scale: [f32; 4],
    pub workgroup: u32,
    /// Visibility needs any accepted blocker, whereas surface transport needs the nearest hit.
    pub shadow_any_hit: bool,
    /// GPU online training and approximate secondary-path termination; None is uncached PT.
    pub online_nrc: Option<NrcConfig>,
    /// Change emitted/punctual/environment light, retain the network, clear image accumulation.
    pub light_change_frame: Option<u32>,
    pub light_change_factor: f32,
    /// Diagnostic baseline that retains NRC shader code even for uncached PT.
    pub specialize_nrc: bool,
    /// Build the trace and training shaders without naga's loop bounding and ray-query
    /// initialization tracking (bounds and division checks stay). Their loops terminate, their ray
    /// queries follow the rules and the trace rejects nonfinite rays itself; on the RTX 5060 these
    /// two checks cost a quarter of the trace and most of Direct3D 12's NRC time
    /// (docs/bench/path-tracing-nrc.md, Windows). Off by default, as the M5 numbers were taken.
    pub lean_shaders: bool,
    /// Count committed hits whose hardware front face disagrees with their triangle's winding
    /// (`PtFrameStats::facing_mismatches`): the convention the sidedness test relies on. Rendering
    /// is the same either way; the check costs 2% to 3% of the trace with Vulkan and 7% to 8% with
    /// Direct3D 12 on the RTX 5060, so it is off by default and on in the GPU test.
    pub check_facing: bool,
}
impl Default for PtOptions {
    fn default() -> Self {
        Self {
            width: 320,
            height: 240,
            samples: 1,
            frames: 64,
            bounces: 12,
            rr_start: 3,
            nee: true,
            russian_roulette: true,
            seed: 42,
            synchronize_frames: false,
            readback_every_frame: false,
            lighting_scale: [1.0; 4],
            workgroup: 64,
            shadow_any_hit: false,
            online_nrc: None,
            light_change_frame: None,
            light_change_factor: 1.0,
            specialize_nrc: true,
            lean_shaders: false,
            check_facing: false,
        }
    }
}
impl PtOptions {
    pub fn validate(&self) -> Result<(), String> {
        let query_stride = if self.online_nrc.as_ref().is_some_and(|c| c.training) {
            4
        } else {
            2
        };
        if self.frames > wgpu::QUERY_SET_MAX_QUERIES / query_stride {
            return Err(format!(
                "PT timing batch exceeds {} frames; split the run",
                wgpu::QUERY_SET_MAX_QUERIES / query_stride
            ));
        }
        if ![32, 64, 128].contains(&self.workgroup)
            || !self.light_change_factor.is_finite()
            || !(0.0..=100.0).contains(&self.light_change_factor)
            || self
                .light_change_frame
                .is_some_and(|frame| frame == 0 || frame >= self.frames)
        {
            return Err("invalid PT workgroup or lighting-change settings".into());
        }
        if let Some(config) = &self.online_nrc {
            config.validate()?;
        }
        if self.width == 0
            || self.height == 0
            || self.width > 4096
            || self.height > 4096
            || self.frames == 0
            || self.frames > 4096
            || self.samples == 0
            || self.samples > 256
            || !(1..=64).contains(&self.bounces)
            || self.rr_start > 64
            || self
                .lighting_scale
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1e6).contains(v))
        {
            return Err("invalid PT dimensions, samples, depth or lighting scales".into());
        }
        if u64::from(self.width)
            * u64::from(self.height)
            * u64::from(self.frames)
            * u64::from(self.samples)
            > 2_000_000_000
        {
            return Err(
                "PT batch exceeds two billion camera paths; split it into smaller runs".into(),
            );
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Parameters {
    pub image: [u32; 4],
    pub limits: [u32; 4],
    pub camera_position: [f32; 4],
    pub camera_right: [f32; 4],
    pub camera_up: [f32; 4],
    pub camera_forward: [f32; 4],
    pub lighting_scale: [f32; 4],
    pub scene_min: [f32; 4],
    pub scene_extent: [f32; 4],
    pub controls: [u32; 4],
    pub ray: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TriangleGpu {
    positions: [[f32; 4]; 3],
    normals: [[f32; 4]; 3],
    uv: [[f32; 4]; 3],
    tangents: [[f32; 4]; 3],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialGpu {
    base_color: [f32; 4],
    emission: [f32; 4],
    surface: [f32; 4],
    tex: [u32; 4],
    flags: [u32; 4],
    extra: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightGpu {
    position_kind: [f32; 4],
    direction_range: [f32; 4],
    color_intensity: [f32; 4],
    cone: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EmitterGpu {
    triangle: u32,
    area: f32,
    power_cdf: f32,
    padding: u32,
}

#[derive(Debug, Serialize)]
pub struct PtFrameStats {
    pub frame: u32,
    pub cpu_submit_ms: f64,
    pub gpu_trace_ms: Option<f64>,
    pub gpu_training_ms: Option<f64>,
    pub nrc: Option<NrcFrameStats>,
    pub camera_paths: u32,
    pub path_rays: u32,
    pub continuation_rays: u32,
    pub shadow_rays: u32,
    pub surface_vertices: u32,
    pub roulette_terminations: u32,
    pub invalid_bsdf_samples: u32,
    pub emissive_hits: u32,
    pub environment_hits: u32,
    pub nee_samples: u32,
    pub alpha_rejections: u32,
    pub transparent_candidates: u32,
    pub nonfinite_paths: u32,
    pub transmission_samples: u32,
    pub metallic_samples: u32,
    pub textured_vertices: u32,
    /// Committed hits whose hardware front face disagrees with their triangle's winding (away from
    /// grazing incidence): the convention the candidate loop's sidedness test relies on. Zero;
    /// counted only with `PtOptions::check_facing`.
    pub facing_mismatches: u32,
    pub counters: [u32; 16],
}
#[derive(Debug, Serialize)]
pub struct NrcAdaptation {
    /// Identical real path features captured before the lighting change; diagnostic only.
    pub samples: usize,
    pub before_mean_rgb: [f64; 3],
    pub after_mean_rgb: [f64; 3],
}

#[derive(Debug, Serialize)]
pub struct PtStats {
    pub format: &'static str,
    pub adapter: String,
    pub backend: &'static str,
    pub scene_signature: String,
    pub options: PtOptions,
    pub scene_options: PtSceneOptions,
    pub triangles: usize,
    pub materials: usize,
    pub lights: usize,
    pub emitters: usize,
    pub texture_layers: usize,
    pub texture_resolution: [u32; 2],
    pub acceleration_build_ms: f64,
    pub trace_wall_ms: f64,
    pub final_readback_ms: f64,
    pub accumulated_frames: u32,
    pub nrc_parameter_count: usize,
    pub nrc_memory_bytes: u64,
    pub nrc_adaptation: Option<NrcAdaptation>,
    pub mean_radiance: [f64; 3],
    pub frames: Vec<PtFrameStats>,
    pub gpu_errors: Vec<String>,
}
pub struct PtOutput {
    pub radiance: Vec<[f32; 4]>,
    pub stats: PtStats,
}

pub(crate) fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}
fn upload<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        usage,
    })
}
fn storage_upload<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    data: &[T],
) -> Result<wgpu::Buffer, String> {
    let bytes = std::mem::size_of_val(data) as u64;
    validate_storage_bytes(label, bytes, &device.limits())?;
    Ok(upload(device, label, data, wgpu::BufferUsages::STORAGE))
}

fn validate_storage_bytes(label: &str, bytes: u64, limits: &wgpu::Limits) -> Result<(), String> {
    if bytes > limits.max_storage_buffer_binding_size || bytes > limits.max_buffer_size {
        return Err(format!(
            "{label} needs {bytes} bytes; GPU storage/buffer limits are {}/{} bytes",
            limits.max_storage_buffer_binding_size, limits.max_buffer_size
        ));
    }
    Ok(())
}

fn validate_triangle_capacity(count: usize, limits: &wgpu::Limits) -> Result<(), String> {
    let count = u64::try_from(count).map_err(|_| "PT triangle count overflow")?;
    if count == 0
        || count > u64::from(limits.max_blas_primitive_count)
        || count > u64::from(u32::MAX) / 3
    {
        return Err(format!(
            "PT triangle count {count} exceeds BLAS/vertex limits ({} primitives, {} vertices)",
            limits.max_blas_primitive_count,
            u32::MAX
        ));
    }
    validate_storage_bytes(
        "PT triangle shading data",
        count * std::mem::size_of::<TriangleGpu>() as u64,
        limits,
    )?;
    let vertex_bytes = count * 3 * std::mem::size_of::<[f32; 3]>() as u64;
    if vertex_bytes > limits.max_buffer_size {
        return Err(format!(
            "PT BLAS vertices need {vertex_bytes} bytes; GPU buffer limit is {} bytes",
            limits.max_buffer_size
        ));
    }
    Ok(())
}

fn texture_shape(
    source_width: u32,
    source_height: u32,
    layers: usize,
    options: &PtSceneOptions,
    limits: &wgpu::Limits,
) -> Result<[u32; 2], String> {
    options.validate()?;
    let layers = layers.max(1) as u64;
    if layers > u64::from(limits.max_texture_array_layers) {
        return Err(format!(
            "PT needs {layers} texture layers; GPU supports {}",
            limits.max_texture_array_layers
        ));
    }
    let width = source_width.max(1).min(options.texture_size);
    let height = source_height.max(1).min(options.texture_size);
    if width > limits.max_texture_dimension_2d || height > limits.max_texture_dimension_2d {
        return Err("PT texture dimensions exceed the GPU limit".into());
    }
    let bytes = u64::from(width) * u64::from(height) * layers * 4;
    if bytes > 512 * 1024 * 1024 {
        return Err(format!(
            "PT texture array needs {bytes} bytes, exceeding 512 MiB; choose a smaller explicit texture_size / --texture-size"
        ));
    }
    Ok([width, height])
}
pub(crate) fn wait(device: &wgpu::Device, submission: wgpu::SubmissionIndex) -> Result<(), String> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(120)),
        })
        .map(|_| ())
        .map_err(|e| format!("PT GPU wait: {e}"))
}
fn luminance(v: [f32; 3]) -> f32 {
    v[0] * 0.2126 + v[1] * 0.7152 + v[2] * 0.0722
}
fn srgb(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn srgb_encode(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let encoded = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

fn resize_texture(
    src: &ImageData,
    width: u32,
    height: u32,
    color: bool,
) -> Result<image::RgbaImage, String> {
    let source = image::RgbaImage::from_raw(src.width, src.height, src.rgba8.clone())
        .ok_or("bad RGBA image")?;
    if src.width == width && src.height == height {
        return Ok(source);
    }
    if !color {
        return Ok(image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::Triangle,
        ));
    }
    let linear = image::Rgba32FImage::from_fn(src.width, src.height, |x, y| {
        let p = source.get_pixel(x, y).0;
        image::Rgba([srgb(p[0]), srgb(p[1]), srgb(p[2]), f32::from(p[3]) / 255.0])
    });
    let filtered = image::imageops::resize(
        &linear,
        width,
        height,
        image::imageops::FilterType::Triangle,
    );
    Ok(image::RgbaImage::from_fn(width, height, |x, y| {
        let p = filtered.get_pixel(x, y).0;
        image::Rgba([
            srgb_encode(p[0]),
            srgb_encode(p[1]),
            srgb_encode(p[2]),
            (p[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ])
    }))
}
fn mean_emission(image: &ImageData) -> f32 {
    image
        .rgba8
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| luminance([srgb(p[0]), srgb(p[1]), srgb(p[2])]))
        .sum::<f32>()
        / (image.width as f32 * image.height as f32)
}

fn trace_pipeline(
    device: &wgpu::Device,
    scene: &wgpu::BindGroupLayout,
    nrc: &wgpu::BindGroupLayout,
    workgroup: u32,
    nrc_enabled: bool,
    lean_shaders: bool,
    check_facing: bool,
) -> wgpu::ComputePipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("PT and online NRC layout"),
        bind_group_layouts: &[Some(scene), Some(nrc)],
        immediate_size: 0,
    });
    let source = format!(
        "{}\n{}\n{}",
        include_str!("../../shaders/pt_bsdf.wgsl"),
        super::nrc::WGSL,
        include_str!("../../shaders/pt_trace.wgsl")
    );
    let desc = wgpu::ShaderModuleDescriptor {
        label: Some("PT BSDF, online cache and integrator"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    };
    let module = if lean_shaders {
        let mut checks = wgpu::ShaderRuntimeChecks::checked();
        checks.force_loop_bounding = false;
        checks.ray_query_initialization_tracking = false;
        // SAFETY: every loop in pt_bsdf.wgsl, online_nrc.wgsl and pt_trace.wgsl runs to a bound
        // (path depth, samples, network sizes, a halving binary search, ray-query traversal);
        // pt_trace initializes each query before proceeding, reads candidates only while
        // proceeding, confirms only triangles, reads the committed hit after traversal and returns
        // a miss for a nonfinite ray instead of tracing it. Bounds and division checks stay on.
        unsafe { device.create_shader_module_trusted(desc, checks) }
    } else {
        device.create_shader_module(desc)
    };
    let constants = [
        ("PT_WORKGROUP", f64::from(workgroup)),
        ("NRC_ENABLED", f64::from(nrc_enabled)),
        ("PT_CHECK_FACING", f64::from(check_facing)),
    ];
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("PT camera paths"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants: &constants,
            ..crate::shaders::compute_options()
        },
        cache: None,
    })
}

/// Independent full surface integrator. Static BLAS/TLAS and texture uploads are retained across
/// renders; final-only readback allows the GPU to run without a CPU fence after each dispatch.
pub struct PathTracer {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    _blas: wgpu::Blas,
    tlas: wgpu::Tlas,
    triangles: wgpu::Buffer,
    materials: wgpu::Buffer,
    lights: wgpu::Buffer,
    emitters: wgpu::Buffer,
    _textures: wgpu::Texture,
    textures_view: wgpu::TextureView,
    textures_color_view: wgpu::TextureView,
    textures_sampler: wgpu::Sampler,
    sky: Sky,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) pipeline: wgpu::ComputePipeline,
    online_nrc: OnlineNrc,
    pub(crate) timestamp_enabled: bool,
    pub(crate) errors: Arc<Mutex<Vec<String>>>,
    adapter: String,
    backend: &'static str,
    scene_signature: String,
    scene_options: PtSceneOptions,
    triangle_count: usize,
    material_count: usize,
    light_count: usize,
    emitter_count: usize,
    texture_layers: usize,
    texture_resolution: [u32; 2],
    pub(crate) bounds_min: [f32; 3],
    pub(crate) bounds_extent: [f32; 3],
    environment_ambient: f32,
    build_ms: f64,
}

impl PathTracer {
    pub fn new(gpu: &Gpu, scene: &RayScene) -> Result<Self, String> {
        Self::with_scene_options(gpu, scene, &PtSceneOptions::default())
    }

    pub fn with_scene_options(
        gpu: &Gpu,
        scene: &RayScene,
        options: &PtSceneOptions,
    ) -> Result<Self, String> {
        options.validate()?;
        super::require_ray_query(gpu)?;
        let have = gpu.adapter.features();
        let timestamp_enabled = have.contains(wgpu::Features::TIMESTAMP_QUERY);
        let adapter_limits = gpu.adapter.limits();
        let mut limits =
            wgpu::Limits::default().using_acceleration_structure_values(adapter_limits.clone());
        // Native scene data can exceed WebGPU's default 128 MiB storage limit. Request the real
        // adapter limits explicitly, then validate resource bytes before building/uploading them.
        limits.max_storage_buffer_binding_size = adapter_limits.max_storage_buffer_binding_size;
        limits.max_buffer_size = adapter_limits.max_buffer_size;
        limits.max_texture_array_layers = adapter_limits.max_texture_array_layers;
        validate_triangle_capacity(scene.triangle_count(), &limits)?;
        // Reserve enough native storage slots for the toy online NRC group added by the caller.
        limits.max_storage_buffers_per_shader_stage =
            16.min(gpu.adapter.limits().max_storage_buffers_per_shader_stage);
        if limits.max_storage_buffers_per_shader_stage < 12 {
            return Err("PT/NRC needs at least twelve storage slots".into());
        }
        let (device, queue) =
            pollster::block_on(gpu.adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("Amoris surface path tracer"),
                required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY
                    | if timestamp_enabled {
                        wgpu::Features::TIMESTAMP_QUERY
                    } else {
                        wgpu::Features::empty()
                    },
                required_limits: limits,
                // SAFETY: this native tool explicitly opts into experimental ray queries.
                experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                ..Default::default()
            }))
            .map_err(|e| e.to_string())?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let captured = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            captured.lock().unwrap().push(e.to_string());
        }));
        let captured = errors.clone();
        device.set_device_lost_callback(move |reason, message| {
            captured
                .lock()
                .unwrap()
                .push(format!("device lost {reason:?}: {message}"));
        });
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let world_materials: Vec<_> = scene.world_materials().collect();
        let mut unique = BTreeMap::<(usize, usize, bool), u32>::new();
        let mut images = Vec::<(&ImageData, bool)>::new();
        let mut materials = Vec::new();
        let mut emission_powers = Vec::new();
        for surface in &world_materials {
            let mat = &surface.material;
            let mut layer = |index: Option<usize>, color: bool| -> Result<u32, String> {
                let Some(index) = index else {
                    return Ok(u32::MAX);
                };
                let key = (Arc::as_ptr(&surface.images) as usize, index, color);
                if let Some(layer) = unique.get(&key) {
                    return Ok(*layer);
                }
                let image = surface.images.get(index).ok_or("invalid texture index")?;
                let result = images.len() as u32;
                unique.insert(key, result);
                images.push((image, color));
                Ok(result)
            };
            materials.push(MaterialGpu {
                base_color: mat.base_color,
                emission: [
                    mat.emissive[0],
                    mat.emissive[1],
                    mat.emissive[2],
                    f32::from(mat.double_sided),
                ],
                surface: [mat.metallic, mat.roughness, mat.transmission, mat.ior],
                tex: [
                    layer(mat.base_color_texture, true)?,
                    layer(mat.metallic_roughness_texture, false)?,
                    layer(mat.normal_texture, false)?,
                    layer(mat.emissive_texture, true)?,
                ],
                flags: [
                    match mat.alpha_mode {
                        AlphaMode::Opaque => 0,
                        AlphaMode::Mask => 1,
                        AlphaMode::Blend => 2,
                    },
                    layer(mat.occlusion_texture, false)?,
                    layer(mat.transmission_texture, false)?,
                    u32::from(surface.casts_shadows),
                ],
                extra: [
                    mat.alpha_cutoff,
                    mat.normal_scale,
                    mat.occlusion_strength,
                    0.0,
                ],
            });
            let power = luminance(mat.emissive)
                * mat
                    .emissive_texture
                    .map_or(1.0, |i| mean_emission(&surface.images[i]));
            emission_powers.push(power.max(0.0));
        }
        if materials.is_empty() {
            return Err("PT scene needs at least one material".into());
        }
        validate_storage_bytes(
            "PT full materials",
            (materials.len() * std::mem::size_of::<MaterialGpu>()) as u64,
            &device.limits(),
        )?;
        let texture_layers = images.len();
        // A single filtered array keeps material lookup bindless without optional binding-array features.
        // The original image aspect is retained through UV coordinates; resizing to common dimensions
        // changes texel density only. Bound upload size so test assets cannot exhaust unified memory.
        let source_width = images.iter().map(|(i, _)| i.width).max().unwrap_or(1);
        let source_height = images.iter().map(|(i, _)| i.height).max().unwrap_or(1);
        let [width, height] = texture_shape(
            source_width,
            source_height,
            texture_layers,
            options,
            &device.limits(),
        )?;
        let textures = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PT linear/sRGB source image array"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: texture_layers.max(1) as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        for (index, (src, color)) in images.iter().enumerate() {
            let resized = resize_texture(src, width, height, *color)?;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &textures,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: index as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                resized.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        if images.is_empty() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &textures,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[255, 255, 255, 255],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        let textures_view = textures.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let textures_color_view = textures.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let textures_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("PT repeat linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Stream world triangles directly; a large scene otherwise needs another full CPU copy.
        let triangle_count = scene.triangle_count();
        let mut positions = Vec::with_capacity(triangle_count * 3);
        let mut triangles = Vec::with_capacity(triangle_count);
        let mut emitters = Vec::new();
        let mut power_cdf = 0.0;
        let mut bounds_min = Vec3::splat(f32::INFINITY);
        let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
        for (index, tri) in scene.world_triangles().enumerate() {
            if tri.material >= materials.len() {
                return Err("triangle material index out of range".into());
            }
            let p = tri.positions.map(Vec3::from_array);
            let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            if !area.is_finite() || area <= 1e-12 {
                return Err("degenerate PT triangle".into());
            }
            for v in p {
                bounds_min = bounds_min.min(v);
                bounds_max = bounds_max.max(v);
            }
            positions.extend(tri.positions);
            let mut normals = tri.normals.map(|n| [n[0], n[1], n[2], 0.0]);
            normals[0][3] = tri.material as f32;
            let power = area * emission_powers[tri.material];
            if power > 0.0 {
                power_cdf += power;
                normals[1][3] = emitters.len() as f32 + 1.0;
                emitters.push(EmitterGpu {
                    triangle: index as u32,
                    area,
                    power_cdf,
                    padding: 0,
                });
            }
            triangles.push(TriangleGpu {
                positions: tri.positions.map(|p| [p[0], p[1], p[2], 0.0]),
                normals,
                uv: tri.uv.map(|u| [u[0], u[1], 0.0, 0.0]),
                tangents: tri.tangents,
            });
        }
        let emitter_count = emitters.len();
        if emitters.is_empty() {
            emitters.push(EmitterGpu::zeroed());
        }
        let lights: Vec<_> = scene
            .world_lights()
            .map(|src| LightGpu {
                position_kind: [
                    src.position[0],
                    src.position[1],
                    src.position[2],
                    match src.light.kind {
                        LightKind::Directional => 0.0,
                        LightKind::Point => 1.0,
                        LightKind::Spot => 2.0,
                    },
                ],
                direction_range: [
                    src.direction[0],
                    src.direction[1],
                    src.direction[2],
                    src.light.range as f32,
                ],
                color_intensity: [
                    src.light.color[0] as f32,
                    src.light.color[1] as f32,
                    src.light.color[2] as f32,
                    src.light.intensity as f32,
                ],
                cone: [
                    (src.light.inner_deg as f32).to_radians().cos(),
                    (src.light.outer_deg as f32).to_radians().cos(),
                    f32::from(src.light.shadows),
                    0.0,
                ],
            })
            .collect();
        let light_count = lights.len();
        let light_upload = if lights.is_empty() {
            vec![LightGpu::zeroed()]
        } else {
            lights
        };
        let triangle_buffer = storage_upload(&device, "PT triangle shading data", &triangles)?;
        let material_buffer = storage_upload(&device, "PT full materials", &materials)?;
        let light_buffer = storage_upload(&device, "PT punctual lights", &light_upload)?;
        let emitter_buffer = storage_upload(&device, "PT power sampled emitters", &emitters)?;
        let vertices = upload(
            &device,
            "PT BLAS vertices",
            &positions,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        let geometry = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: positions.len() as u32,
            index_format: None,
            index_count: None,
            // Candidate commitment must evaluate sidedness and alpha, including cutouts.
            flags: wgpu::AccelerationStructureGeometryFlags::empty(),
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("PT static triangles"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![geometry.clone()],
            },
        );
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("PT world"),
            max_instances: 1,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        tlas[0] = Some(wgpu::TlasInstance::new(
            &blas,
            [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            0,
            255,
        ));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("PT AS build"),
        });
        let build = wgpu::BlasBuildEntry {
            blas: &blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                size: &geometry,
                vertex_buffer: &vertices,
                first_vertex: 0,
                vertex_stride: 12,
                index_buffer: None,
                first_index: None,
                transform_buffer: None,
                transform_buffer_offset: None,
            }]),
        };
        let start = Instant::now();
        if gpu.info.backend == wgpu::Backend::Metal {
            // wgpu-hal 30.0.1 leaves the Metal BLAS-to-TLAS barrier empty (#9215).
            // Complete this immutable BLAS before encoding the dependent TLAS build.
            encoder.build_acceleration_structures([&build], std::iter::empty());
            wait(&device, queue.submit([encoder.finish()]))?;
            encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("PT TLAS build after completed BLAS"),
            });
            encoder.build_acceleration_structures(std::iter::empty(), [&tlas]);
        } else {
            encoder.build_acceleration_structures([&build], [&tlas]);
        }
        wait(&device, queue.submit([encoder.finish()]))?;
        let build_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut sky = Sky::new(&device);
        let env = scene.world_environment();
        let sun = scene
            .world_lights()
            .find(|l| l.light.kind == LightKind::Directional);
        let direction = sun
            .as_ref()
            .map_or(Vec3::new(0.3, 0.8, 0.4).normalize(), |l| {
                -Vec3::from_array(l.direction)
            });
        let sun_color = sun
            .as_ref()
            .map_or([1.0, 0.96, 0.9], |l| l.light.color.map(|v| v as f32));
        let sun_lux = sun.as_ref().map_or(6.0, |l| l.light.intensity as f32);
        sky.update(
            &device,
            &queue,
            SkyParams {
                sun: [direction.x, direction.y, direction.z, sun_lux],
                color: [
                    sun_color[0],
                    sun_color[1],
                    sun_color[2],
                    f32::from(env.sky == SkyKind::Color),
                ],
                flat_color: [
                    env.sky_color[0] as f32,
                    env.sky_color[1] as f32,
                    env.sky_color[2] as f32,
                    0.0,
                ],
                size: [0; 4],
            },
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("PT scene resources"),
            entries: &(0..13)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: match binding {
                        0 => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        1 => wgpu::BindingType::AccelerationStructure {
                            vertex_return: false,
                        },
                        8 | 10 | 12 => wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: if binding != 10 {
                                wgpu::TextureViewDimension::D2Array
                            } else {
                                wgpu::TextureViewDimension::Cube
                            },
                            multisampled: false,
                        },
                        9 | 11 => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        _ => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage {
                                read_only: binding < 6,
                            },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                    },
                })
                .collect::<Vec<_>>(),
        });
        let online_nrc = OnlineNrc::new(
            &device,
            &queue,
            NrcConfig {
                batch_size: 1,
                training: false,
                querying: false,
                ..Default::default()
            },
        )?;
        let pipeline = trace_pipeline(
            &device,
            &layout,
            &online_nrc.layout,
            64,
            false,
            false,
            false,
        );
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("PT setup: {e}"));
        }
        Ok(Self {
            device,
            queue,
            _blas: blas,
            tlas,
            triangles: triangle_buffer,
            materials: material_buffer,
            lights: light_buffer,
            emitters: emitter_buffer,
            _textures: textures,
            textures_view,
            textures_color_view,
            textures_sampler,
            sky,
            layout,
            pipeline,
            online_nrc,
            timestamp_enabled,
            errors,
            adapter: gpu.info.name.clone(),
            backend: gpu.backend_name(),
            scene_signature: scene.scene_signature().into(),
            scene_options: *options,
            triangle_count: triangles.len(),
            material_count: materials.len(),
            light_count,
            emitter_count,
            texture_layers,
            texture_resolution: [width, height],
            bounds_min: bounds_min.to_array(),
            bounds_extent: (bounds_max - bounds_min).max(Vec3::splat(1e-4)).to_array(),
            environment_ambient: env.ambient as f32,
            build_ms,
        })
    }

    pub fn scene_bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.bounds_min, self.bounds_extent)
    }
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub(crate) fn parameters(
        &self,
        camera: &RayCamera,
        options: &PtOptions,
    ) -> Result<Parameters, String> {
        options.validate()?;
        let p = Vec3::from_array(camera.position);
        let f = Vec3::from_array(camera.forward);
        let u = Vec3::from_array(camera.up);
        if !p.is_finite()
            || !f.is_finite()
            || !u.is_finite()
            || (f.length_squared() - 1.0).abs() > 1e-3
            || (u.length_squared() - 1.0).abs() > 1e-3
            || f.dot(u).abs() > 1e-3
            || !camera.vertical_fov_degrees.is_finite()
            || !(1.0..=170.0).contains(&camera.vertical_fov_degrees)
        {
            return Err(
                "PT camera needs finite position, orthogonal unit axes and FOV 1..=170".into(),
            );
        }
        let r = f.cross(u);
        let lens = (camera.vertical_fov_degrees.to_radians() * 0.5).tan();
        let mut scale = options.lighting_scale;
        scale[2] *= self.environment_ambient;
        Ok(Parameters {
            image: [options.width, options.height, options.samples, 0],
            limits: [
                options.bounces,
                options.rr_start,
                self.light_count as u32,
                self.emitter_count as u32,
            ],
            camera_position: [p.x, p.y, p.z, 0.0],
            camera_right: [
                r.x,
                r.y,
                r.z,
                lens * options.width as f32 / options.height as f32,
            ],
            camera_up: [u.x, u.y, u.z, lens],
            camera_forward: [f.x, f.y, f.z, 0.0],
            lighting_scale: scale,
            scene_min: [
                self.bounds_min[0],
                self.bounds_min[1],
                self.bounds_min[2],
                0.0,
            ],
            scene_extent: [
                self.bounds_extent[0],
                self.bounds_extent[1],
                self.bounds_extent[2],
                0.0,
            ],
            controls: [
                u32::from(options.nee),
                u32::from(options.russian_roulette),
                options.seed,
                u32::from(options.shadow_any_hit),
            ],
            ray: [1e-4, 1000000.0, 0.0, 0.0],
        })
    }
    pub(crate) fn bindings(
        &self,
        parameters: &wgpu::Buffer,
        image: &wgpu::Buffer,
        counters: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PT frame bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.tlas.as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.triangles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.materials.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.emitters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: image.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: counters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&self.textures_view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::Sampler(&self.textures_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&self.sky.raw_cube),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(&self.sky.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(&self.textures_color_view),
                },
            ],
        })
    }

    pub fn render(&mut self, camera: &RayCamera, options: &PtOptions) -> Result<PtOutput, String> {
        let mut params = self.parameters(camera, options)?;
        let image_size = u64::from(options.width) * u64::from(options.height) * 16;
        if image_size > self.device.limits().max_storage_buffer_binding_size {
            return Err("PT image exceeds storage binding limit".into());
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut config = options.online_nrc.clone().unwrap_or_else(|| NrcConfig {
            batch_size: 1,
            training: false,
            querying: false,
            ..Default::default()
        });
        config.origin = self.bounds_min;
        config.extent = self.bounds_extent;
        self.online_nrc =
            OnlineNrc::with_shaders(&self.device, &self.queue, config, options.lean_shaders)?;
        self.pipeline = trace_pipeline(
            &self.device,
            &self.layout,
            &self.online_nrc.layout,
            options.workgroup,
            options.online_nrc.is_some() || !options.specialize_nrc,
            options.lean_shaders,
            options.check_facing,
        );
        let nrc_enabled = options.online_nrc.is_some();
        let training = nrc_enabled && self.online_nrc.config.training;
        let query_stride = if training { 4 } else { 2 };
        let nrc_stride = 32 + u64::from(self.online_nrc.config.batch_size) * 4;
        let nrc_size = nrc_stride * u64::from(options.frames);
        let nrc_history = nrc_enabled.then(|| {
            buffer(
                &self.device,
                "Online NRC frame history",
                nrc_size,
                wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            )
        });
        let adaptation_enabled = training && options.light_change_frame.is_some();
        let weight_bytes = self.online_nrc.weights.size();
        let adaptation_weights = adaptation_enabled.then(|| {
            buffer(
                &self.device,
                "NRC before/after diagnostic weights",
                weight_bytes * 2,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )
        });
        let adaptation_records = adaptation_enabled.then(|| {
            buffer(
                &self.device,
                "NRC fixed real-path diagnostic features",
                self.online_nrc.records.size(),
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )
        });
        let image = buffer(
            &self.device,
            "PT HDR accumulation",
            image_size,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let params_buffer = upload(
            &self.device,
            "PT frame parameters",
            &[params],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let counters = buffer(
            &self.device,
            "PT counters",
            COUNTERS * 4,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let statistics_size = u64::from(options.frames) * COUNTERS * 4;
        let statistics = buffer(
            &self.device,
            "PT statistics history",
            statistics_size,
            wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let bindings = self.bindings(&params_buffer, &image, &counters);
        let queries = self.timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("PT timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count: options.frames * query_stride,
            })
        });
        let timestamp_size = u64::from(options.frames * query_stride) * 8;
        let timestamps = queries.as_ref().map(|_| {
            buffer(
                &self.device,
                "PT resolved timestamps",
                timestamp_size,
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            )
        });
        let interim_read = options.readback_every_frame.then(|| {
            buffer(
                &self.device,
                "PT per-frame baseline readback",
                image_size,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )
        });
        let mut submit_times = Vec::new();
        let start = Instant::now();
        for frame in 0..options.frames {
            let submit_start = Instant::now();
            params.image[3] = frame;
            if options.light_change_frame == Some(frame) {
                for scale in &mut params.lighting_scale[..3] {
                    *scale *= options.light_change_factor;
                }
            }
            self.online_nrc.begin_frame(&self.queue)?;
            self.queue
                .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("PT progressive frame"),
                });
            encoder.clear_buffer(&counters, 0, None);
            if frame == 0 || options.light_change_frame == Some(frame) {
                encoder.clear_buffer(&image, 0, None);
            }
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("PT camera transport"),
                    timestamp_writes: queries.as_ref().map(|q| wgpu::ComputePassTimestampWrites {
                        query_set: q,
                        beginning_of_pass_write_index: Some(frame * query_stride),
                        end_of_pass_write_index: Some(frame * query_stride + 1),
                    }),
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                pass.set_bind_group(1, &self.online_nrc.bind_group, &[]);
                pass.dispatch_workgroups(
                    (options.width * options.height).div_ceil(options.workgroup),
                    1,
                    1,
                );
            }
            self.online_nrc.encode_training_with_timestamps(
                &mut encoder,
                queries
                    .as_ref()
                    .filter(|_| training)
                    .map(|q| (q, frame * query_stride + 2)),
            )?;
            if let Some(history) = &nrc_history {
                let offset = u64::from(frame) * nrc_stride;
                encoder.copy_buffer_to_buffer(&self.online_nrc.stats, 0, history, offset, 32);
                encoder.copy_buffer_to_buffer(
                    &self.online_nrc.losses,
                    0,
                    history,
                    offset + 32,
                    nrc_stride - 32,
                );
            }
            if options.light_change_frame == Some(frame + 1)
                && let (Some(weights), Some(records)) = (&adaptation_weights, &adaptation_records)
            {
                encoder.copy_buffer_to_buffer(
                    &self.online_nrc.weights,
                    0,
                    weights,
                    0,
                    weight_bytes,
                );
                encoder.copy_buffer_to_buffer(
                    &self.online_nrc.records,
                    0,
                    records,
                    0,
                    records.size(),
                );
            }
            encoder.copy_buffer_to_buffer(
                &counters,
                0,
                &statistics,
                u64::from(frame) * COUNTERS * 4,
                COUNTERS * 4,
            );
            if let Some(read) = &interim_read {
                encoder.copy_buffer_to_buffer(&image, 0, read, 0, image_size);
            }
            let submission = self.queue.submit([encoder.finish()]);
            if let Some(read) = &interim_read {
                let (tx, rx) = mpsc::channel();
                read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
                wait(&self.device, submission)?;
                rx.recv_timeout(Duration::from_secs(1))
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                // Make the baseline perform real host access, while accumulation remains untouched.
                let data = read
                    .slice(..)
                    .get_mapped_range()
                    .map_err(|e| e.to_string())?;
                std::hint::black_box(data.len());
                drop(data);
                read.unmap();
            } else if options.synchronize_frames {
                wait(&self.device, submission)?;
            }
            submit_times.push(submit_start.elapsed().as_secs_f64() * 1000.0);
        }
        // One explicit fence for the queued batch, followed by one final HDR/statistics readback.
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let trace_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        let read_start = Instant::now();
        let image_read = buffer(
            &self.device,
            "PT final HDR readback",
            image_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let statistics_read = buffer(
            &self.device,
            "PT final counter readback",
            statistics_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let timestamp_read = queries.as_ref().map(|_| {
            buffer(
                &self.device,
                "PT final timestamp readback",
                timestamp_size,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )
        });
        let nrc_read = nrc_history.as_ref().map(|_| {
            buffer(
                &self.device,
                "Online NRC final history",
                nrc_size,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            )
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("PT final readback"),
            });
        encoder.copy_buffer_to_buffer(&image, 0, &image_read, 0, image_size);
        encoder.copy_buffer_to_buffer(&statistics, 0, &statistics_read, 0, statistics_size);
        if let (Some(q), Some(t), Some(r)) = (&queries, &timestamps, &timestamp_read) {
            encoder.resolve_query_set(q, 0..options.frames * query_stride, t, 0);
            encoder.copy_buffer_to_buffer(t, 0, r, 0, timestamp_size);
        }
        if let (Some(history), Some(read)) = (&nrc_history, &nrc_read) {
            encoder.copy_buffer_to_buffer(history, 0, read, 0, nrc_size);
        }
        if let Some(weights) = &adaptation_weights {
            encoder.copy_buffer_to_buffer(
                &self.online_nrc.weights,
                0,
                weights,
                weight_bytes,
                weight_bytes,
            );
        }
        let submission = self.queue.submit([encoder.finish()]);
        let readbacks: Vec<_> = [&image_read, &statistics_read]
            .into_iter()
            .chain(timestamp_read.as_ref())
            .chain(nrc_read.as_ref())
            .chain(adaptation_weights.as_ref())
            .chain(adaptation_records.as_ref())
            .collect();
        let mut receivers = Vec::new();
        for read in &readbacks {
            let (tx, rx) = mpsc::channel();
            read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            receivers.push(rx);
        }
        wait(&self.device, submission)?;
        for rx in receivers {
            rx.recv_timeout(Duration::from_secs(1))
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("PT render: {e}"));
        }
        let data = image_read
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let radiance: Vec<[f32; 4]> = data
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| {
                let c: [f32; 4] = bytemuck::pod_read_unaligned(b);
                [
                    c[0] / c[3].max(1.0),
                    c[1] / c[3].max(1.0),
                    c[2] / c[3].max(1.0),
                    1.0,
                ]
            })
            .collect();
        drop(data);
        image_read.unmap();
        if radiance
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("PT produced negative or nonfinite HDR".into());
        }
        let data = statistics_read
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let counts: Vec<[u32; 16]> = data
            .as_chunks::<64>()
            .0
            .iter()
            .map(|b| bytemuck::pod_read_unaligned(b.as_slice()))
            .collect();
        drop(data);
        statistics_read.unmap();
        let times = if let Some(read) = &timestamp_read {
            let data = read
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let t: Vec<u64> = data
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| u64::from_le_bytes(*b))
                .collect();
            drop(data);
            read.unmap();
            Some(t)
        } else {
            None
        };
        let period = f64::from(self.queue.get_timestamp_period()) / 1e6;
        let nrc_frames = if let Some(read) = &nrc_read {
            let data = read
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let frames: Vec<_> = data
                .chunks_exact(nrc_stride as usize)
                .map(|chunk| {
                    let c: [u32; 8] = bytemuck::pod_read_unaligned(&chunk[..32]);
                    let count = c[5].min(self.online_nrc.config.batch_size);
                    let loss = chunk[32..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .take(count as usize)
                        .map(|b| f32::from_le_bytes(*b) as f64)
                        .sum::<f64>()
                        / f64::from(count.max(1));
                    NrcFrameStats {
                        requested_records: c[0],
                        cache_queries: c[1],
                        invalid_records: c[2],
                        invalid_updates: c[3],
                        overflow_records: c[4],
                        trained_records: c[5],
                        invalid_predictions: c[6],
                        mean_log_mse: loss,
                        updates: c[7],
                    }
                })
                .collect();
            drop(data);
            read.unmap();
            Some(frames)
        } else {
            None
        };
        let nrc_adaptation = if let (Some(weights), Some(records), Some(frames), Some(change)) = (
            &adaptation_weights,
            &adaptation_records,
            &nrc_frames,
            options.light_change_frame,
        ) {
            let weight_view = weights
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let w: Vec<f32> = weight_view
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            drop(weight_view);
            let r = records
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let count = frames[change as usize - 1].trained_records as usize;
            let mut before = [0.0; 3];
            let mut after = [0.0; 3];
            for chunk in r.as_chunks::<80>().0.iter().take(count) {
                let record: super::nrc::NrcRecord = bytemuck::pod_read_unaligned(chunk);
                let (_, _, a) =
                    super::nrc::reference_forward(&w[..super::nrc::PARAMETER_COUNT], &record);
                let (_, _, b) =
                    super::nrc::reference_forward(&w[super::nrc::PARAMETER_COUNT..], &record);
                for c in 0..3 {
                    let decode = |v: f64| {
                        v.clamp(0.0, f64::from(self.online_nrc.config.max_log_prediction))
                            .exp_m1()
                            * f64::from(self.online_nrc.config.radiance_scale)
                    };
                    before[c] += decode(a[c]) / count.max(1) as f64;
                    after[c] += decode(b[c]) / count.max(1) as f64;
                }
            }
            drop(r);
            records.unmap();
            weights.unmap();
            Some(NrcAdaptation {
                samples: count,
                before_mean_rgb: before,
                after_mean_rgb: after,
            })
        } else {
            None
        };
        let query_stride = query_stride as usize;
        let frames = counts
            .into_iter()
            .enumerate()
            .map(|(i, c)| PtFrameStats {
                frame: i as u32,
                cpu_submit_ms: submit_times[i],
                gpu_trace_ms: times.as_ref().map(|t| {
                    t[i * query_stride + 1].saturating_sub(t[i * query_stride]) as f64 * period
                }),
                gpu_training_ms: if training {
                    times.as_ref().map(|t| {
                        t[i * query_stride + 3].saturating_sub(t[i * query_stride + 2]) as f64
                            * period
                    })
                } else {
                    None
                },
                nrc: nrc_frames.as_ref().map(|frames| frames[i].clone()),
                camera_paths: c[0],
                path_rays: c[1],
                continuation_rays: c[1].saturating_sub(c[0]),
                shadow_rays: c[2],
                surface_vertices: c[3],
                roulette_terminations: c[4],
                invalid_bsdf_samples: c[5],
                emissive_hits: c[6],
                environment_hits: c[7],
                nee_samples: c[8],
                alpha_rejections: c[9],
                transparent_candidates: c[10],
                facing_mismatches: c[11],
                nonfinite_paths: c[12],
                transmission_samples: c[13],
                metallic_samples: c[14],
                textured_vertices: c[15],
                counters: c,
            })
            .collect();
        let mut mean = [0.0; 3];
        for p in &radiance {
            for k in 0..3 {
                mean[k] += p[k] as f64 / radiance.len() as f64;
            }
        }
        let stats = PtStats {
            format: "amoris-surface-pt-v1",
            adapter: self.adapter.clone(),
            backend: self.backend,
            scene_signature: self.scene_signature.clone(),
            options: options.clone(),
            scene_options: self.scene_options,
            triangles: self.triangle_count,
            materials: self.material_count,
            lights: self.light_count,
            emitters: self.emitter_count,
            texture_layers: self.texture_layers,
            texture_resolution: self.texture_resolution,
            acceleration_build_ms: self.build_ms,
            trace_wall_ms,
            final_readback_ms: read_start.elapsed().as_secs_f64() * 1000.0,
            accumulated_frames: options.frames - options.light_change_frame.unwrap_or(0),
            nrc_parameter_count: if nrc_enabled {
                super::nrc::PARAMETER_COUNT
            } else {
                0
            },
            nrc_memory_bytes: {
                self.online_nrc.params.size()
                    + self.online_nrc.weights.size()
                    + self.online_nrc.records.size()
                    + self.online_nrc.gradients.size()
                    + self.online_nrc.moments.size()
                    + self.online_nrc.stats.size()
                    + self.online_nrc.losses.size()
            },
            nrc_adaptation,
            mean_radiance: mean,
            frames,
            gpu_errors: self.errors.lock().unwrap().clone(),
        };
        Ok(PtOutput { radiance, stats })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_native_scene_capacity_uses_device_bytes_and_blas_limits() {
        let mut limits =
            wgpu::Limits::default().using_minimum_supported_acceleration_structure_values();
        limits.max_storage_buffer_binding_size = 4 * 1024 * 1024 * 1024;
        limits.max_buffer_size = 4 * 1024 * 1024 * 1024;
        assert!(validate_triangle_capacity(2_829_226, &limits).is_ok());
        limits.max_storage_buffer_binding_size = 128 * 1024 * 1024;
        assert!(validate_triangle_capacity(2_829_226, &limits).is_err());
        limits.max_storage_buffer_binding_size = 4 * 1024 * 1024 * 1024;
        limits.max_blas_primitive_count = 2_000_000;
        assert!(validate_triangle_capacity(2_829_226, &limits).is_err());
        assert!(validate_triangle_capacity(0, &limits).is_err());
        assert!(validate_triangle_capacity(u32::MAX as usize, &limits).is_err());
    }

    #[test]
    fn texture_size_is_explicit_and_checked_against_layers_and_upload_bytes() {
        let mut limits = wgpu::Limits {
            max_texture_array_layers: 2048,
            ..wgpu::Limits::default()
        };
        let options = PtSceneOptions { texture_size: 512 };
        assert_eq!(
            texture_shape(2048, 4096, 405, &options, &limits).unwrap(),
            [512, 512]
        );
        assert!(texture_shape(2048, 4096, 405, &PtSceneOptions::default(), &limits).is_err());
        limits.max_texture_array_layers = 256;
        assert!(texture_shape(2048, 4096, 405, &options, &limits).is_err());
        assert_eq!(
            texture_shape(32, 16, 0, &options, &limits).unwrap(),
            [32, 16]
        );
        assert!(PtSceneOptions { texture_size: 0 }.validate().is_err());
        assert!(PtSceneOptions { texture_size: 2049 }.validate().is_err());
    }
    #[test]
    fn pt_gpu_abi_and_limits_are_explicit() {
        assert_eq!(std::mem::size_of::<TriangleGpu>(), 192);
        assert_eq!(std::mem::size_of::<MaterialGpu>(), 96);
        assert_eq!(std::mem::size_of::<LightGpu>(), 64);
        assert_eq!(std::mem::size_of::<Parameters>(), 176);
        assert!(PtOptions::default().validate().is_ok());
        let mut opts = PtOptions {
            bounces: 0,
            ..PtOptions::default()
        };
        assert!(opts.validate().is_err());
        opts = PtOptions::default();
        opts.frames = 2049;
        assert!(opts.validate().is_err());
        opts.frames = 1025;
        opts.online_nrc = Some(NrcConfig::default());
        assert!(opts.validate().is_err());
    }

    #[test]
    fn color_resize_filters_linear_light_and_preserves_alpha() {
        let src = ImageData {
            name: String::new(),
            width: 2,
            height: 1,
            srgb: true,
            rgba8: vec![0, 0, 0, 0, 255, 255, 255, 255],
        };
        let color = resize_texture(&src, 1, 1, true).unwrap().get_pixel(0, 0).0;
        let linear = resize_texture(&src, 1, 1, false).unwrap().get_pixel(0, 0).0;
        assert!((i32::from(color[0]) - 188).abs() <= 1);
        assert!((i32::from(linear[0]) - 128).abs() <= 1);
        assert_eq!(color[3], linear[3]);
    }

    /// Complete PT and actual online training, on every native backend whose adapter exposes ray
    /// queries; skipped elsewhere.
    #[test]
    fn ray_query_pt_transport_training_and_accumulation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/pt-lab");
        let scene = RayScene::for_path_tracing(&root).unwrap();
        for gpu in crate::gi::ray_query_gpus() {
            pt_transport_training_and_accumulation(&gpu, &scene);
        }
    }

    fn mean_rgb(output: &PtOutput) -> [f64; 3] {
        let n = output.radiance.len().max(1) as f64;
        [0, 1, 2].map(|c| output.radiance.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
    }

    fn pt_transport_training_and_accumulation(gpu: &Gpu, scene: &RayScene) {
        let backend = gpu.backend_name();
        let mut tracer = PathTracer::new(gpu, scene).unwrap();
        let camera = RayCamera {
            position: [0.0, 1.7, 7.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            vertical_fov_degrees: 42.0,
        };
        let mut options = PtOptions {
            width: 64,
            height: 48,
            frames: 4,
            check_facing: true,
            ..Default::default()
        };
        let raw = tracer.render(&camera, &options).unwrap();
        let frames = &raw.stats.frames;
        let mean = mean_rgb(&raw);
        eprintln!(
            "{backend} on {}: pt-lab mean radiance {mean:?}",
            gpu.info.name
        );
        assert!(
            frames.iter().all(|f| f.facing_mismatches == 0),
            "{backend}: hardware front faces follow the triangles' counterclockwise winding: {:?}",
            frames
                .iter()
                .map(|f| f.facing_mismatches)
                .collect::<Vec<_>>()
        );
        // pt-lab from this camera, 64x48, 4 frames, the default seed: the RTX 5060 and the Radeon
        // 780M give this mean to 1e-5 with Vulkan and Direct3D 12, and so does the sidedness test
        // on candidate.front_face that the winding test replaced (Metal's references). Eight other
        // seeds move the channel sum by 2% (standard deviation); culling front instead of back
        // faces drops it to 0.10, AMD's Direct3D 12 miscompile of the old test to 0.26.
        const PT_LAB_SUM: f64 = 0.6945;
        let sum: f64 = mean.iter().sum();
        assert!(
            (sum / PT_LAB_SUM - 1.0).abs() < 0.1,
            "{backend}: pt-lab's mean radiance {mean:?} (sum {sum:.4}) is that of single-sided \
             surfaces seen from their front (sum {PT_LAB_SUM})"
        );
        assert!(
            frames.iter().any(|f| f.transmission_samples > 0),
            "{backend}: no transmission samples"
        );
        assert!(
            frames.iter().any(|f| f.metallic_samples > 0),
            "{backend}: no metallic samples"
        );
        assert!(
            frames.iter().any(|f| f.textured_vertices > 0),
            "{backend}: no textured vertices"
        );
        assert!(
            frames
                .iter()
                .any(|f| f.alpha_rejections > 0 && f.transparent_candidates > 0),
            "{backend}: no alpha rejections or transparent candidates"
        );
        options.readback_every_frame = true;
        let blocking = tracer.render(&camera, &options).unwrap();
        assert_eq!(
            raw.radiance, blocking.radiance,
            "{backend}: readback optimization preserves all samples"
        );
        options.readback_every_frame = false;
        let max_relative = |a: &PtOutput, b: &PtOutput| {
            a.radiance
                .iter()
                .zip(&b.radiance)
                .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs() / (1.0 + a[c])))
                .fold(0.0, f32::max)
        };
        options.check_facing = false;
        let unchecked = tracer.render(&camera, &options).unwrap();
        let unchecked_error = max_relative(&raw, &unchecked);
        assert!(
            unchecked_error < 1e-3,
            "{backend}: the facing check changes no path: {unchecked_error}"
        );
        options.check_facing = true;
        options.lean_shaders = true;
        let lean = tracer.render(&camera, &options).unwrap();
        // Compiled differently, so rounding may differ (2e-4 on the Radeon 780M with Vulkan);
        // a path that took another branch would differ by far more.
        let lean_error = max_relative(&raw, &lean);
        assert!(
            lean_error < 1e-3,
            "{backend}: shaders without loop bounds and query tracking trace the same paths: \
             {lean_error}"
        );
        options.online_nrc = Some(NrcConfig {
            querying: false,
            min_query_depth: 1,
            ..Default::default()
        });
        let lean_training = tracer.render(&camera, &options).unwrap();
        options.lean_shaders = false;
        let training = tracer.render(&camera, &options).unwrap();
        let max_error = max_relative(&raw, &training);
        assert!(
            max_relative(&training, &lean_training) < 1e-3,
            "{backend}: the lean trainer's teacher paths match"
        );
        assert!(
            max_error < 2e-5,
            "{backend}: uncached teacher paths preserve camera transport: {max_error}"
        );
        assert!(
            training
                .stats
                .frames
                .iter()
                .all(|f| f.nrc.as_ref().unwrap().cache_queries == 0)
        );
        assert!(
            training
                .stats
                .frames
                .iter()
                .any(|f| f.nrc.as_ref().unwrap().trained_records > 0)
        );
        options.online_nrc.as_mut().unwrap().querying = true;
        options.online_nrc.as_mut().unwrap().warmup_updates = 1;
        options.light_change_frame = Some(2);
        options.light_change_factor = 2.0;
        let changed = tracer.render(&camera, &options).unwrap();
        assert_eq!(changed.stats.accumulated_frames, 2);
        let adaptation = changed.stats.nrc_adaptation.as_ref().unwrap();
        assert!(adaptation.samples > 0);
        assert!(
            adaptation
                .before_mean_rgb
                .iter()
                .chain(&adaptation.after_mean_rgb)
                .all(|v| v.is_finite())
        );
        assert_eq!(
            changed.stats.frames[0].nrc.as_ref().unwrap().cache_queries,
            0
        );
        assert!(
            changed.stats.frames[1..]
                .iter()
                .any(|f| f.nrc.as_ref().unwrap().cache_queries > 0)
        );
        assert!(changed.stats.gpu_errors.is_empty());
        assert!(changed.stats.frames.iter().all(|f| f.nonfinite_paths == 0));
    }
}
