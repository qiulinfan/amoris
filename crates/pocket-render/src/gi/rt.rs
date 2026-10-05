//! Isolated Metal diffuse path tracer, SHaRC-style cache and bounded secondary-surface RIS/ReSTIR GI.
//!
//! Update traces independent sparse paths, Resolve combines history, and Query terminates eligible
//! secondary path suffixes. This implements the algorithm's three responsibilities in WGSL rather
//! than incorporating NVIDIA SDK code. Reference: NVIDIA-RTX/SHARC `docs/Integration.md`, accessed
//! 2026-10-05. Unlike the SDK, history is float32, exact spatial keys are collision checked, and new
//! hash entries accept only their insertion winner until the next frame.
//!
//! This tool accepts static opaque constant Lambertian materials, emissive triangles and a black
//! environment. It rejects textures, alpha, metallic transport and explicit lights. It owns a second
//! device requested from the supplied Metal adapter, because the ordinary renderer's device does
//! not enable experimental ray queries. It is not yet a pass in the interactive renderer.
//! ReSTIR's fresh RIS baseline is available without reuse; temporal/spatial source reuse is an
//! explicitly biased experiment with incomplete support/M correction, not a ReSTIR PT implementation.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use pocket_assets::mesh::AlphaMode;
use serde::Serialize;
use wgpu::util::DeviceExt;

use super::bake::RayScene;
use crate::Gpu;

const CACHE_SLOTS: u32 = 1 << 16;
const COUNTER_COUNT: u64 = 32;
const TIMESTAMPS_PER_FRAME: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceMode {
    Raw,
    Sharc,
    Restir,
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires real Metal ray-query hardware; verifies cache age and reset against raw rays"]
    fn metal_cache_requires_history_and_resets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/gi-room");
        let scene = RayScene::from_project(&root).expect("static room");
        let gpu = Gpu::headless(crate::BackendChoice::Metal).expect("Metal GPU");
        let mut lighting = RayLighting::new(&gpu, &scene).expect("Metal ray query");
        let camera = RayCamera {
            position: [0.0, 2.0, 7.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            vertical_fov_degrees: 44.0,
        };
        let mut options = TraceOptions {
            width: 32,
            height: 24,
            frames: 2,
            minimum_cache_samples: 1,
            ..Default::default()
        };
        let raw = lighting
            .render(&camera, &options, TraceMode::Raw)
            .expect("raw");
        let cached = lighting
            .render(&camera, &options, TraceMode::Sharc)
            .expect("empty cache");
        assert_eq!(
            raw.radiance, cached.radiance,
            "empty/ineligible cache must preserve raw path samples"
        );
        assert!(
            cached
                .stats
                .frames
                .iter()
                .all(|frame| frame.cache_hits == 0 && frame.cache_confident == 0)
        );
        assert!(
            cached
                .stats
                .frames
                .iter()
                .any(|frame| frame.cache_deposits > 0)
        );
        assert!(raw.radiance.iter().any(|pixel| pixel[0] > 0.0));
        options.frames = 32;
        let warmed = lighting
            .render(&camera, &options, TraceMode::Sharc)
            .expect("cache warmup");
        assert_eq!(
            warmed.stats.frames[0].cache_hits, 0,
            "new render clears previous history"
        );
        assert_eq!(
            warmed.stats.frames[1].cache_hits, 0,
            "at least three frames are required"
        );
        assert!(warmed.stats.frames.iter().any(|frame| frame.cache_hits > 0));
        assert!(warmed.stats.gpu_errors.is_empty());
    }

    #[test]
    #[ignore = "requires real Metal ray-query hardware; verifies RIS, M counting and history reset"]
    fn metal_ris_matches_raw_and_bounds_history() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/gi-room");
        let scene = RayScene::from_project(&root).expect("static room");
        let gpu = Gpu::headless(crate::BackendChoice::Metal).expect("Metal GPU");
        let mut lighting = RayLighting::new(&gpu, &scene).expect("Metal ray query");
        let camera = RayCamera {
            position: [0.0, 2.0, 7.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
            vertical_fov_degrees: 44.0,
        };
        let mut options = TraceOptions {
            width: 32,
            height: 24,
            frames: 4,
            candidates: 1,
            reuse: RestirReuse::None,
            history_m: 16,
            ..Default::default()
        };
        let raw = lighting
            .render(&camera, &options, TraceMode::Raw)
            .expect("raw");
        let ris = lighting
            .render(&camera, &options, TraceMode::Restir)
            .expect("RIS without reuse");
        let maximum_relative = raw
            .radiance
            .iter()
            .zip(&ris.radiance)
            .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs() / (1.0 + a[c])))
            .fold(0.0f32, f32::max);
        assert!(
            maximum_relative < 2e-5,
            "RIS K1 should preserve raw transport: {maximum_relative}"
        );
        assert!(ris.stats.frames.iter().all(|f| f.visibility_blocked == 0
            && f.maximum_reservoir_m == 1
            && f.initial_m_errors == 0));
        assert!(ris.stats.frames.iter().any(|f| f.zero_proposals > 0));
        options.frames = 8;
        options.reuse = RestirReuse::Both;
        let reused = lighting
            .render(&camera, &options, TraceMode::Restir)
            .expect("static reuse");
        assert_eq!(reused.stats.frames[0].temporal_sources_admitted, 0);
        assert!(
            reused
                .stats
                .frames
                .iter()
                .any(|f| f.temporal_sources_admitted > 0 && f.spatial_sources_admitted > 0)
        );
        assert!(
            reused
                .stats
                .frames
                .iter()
                .all(|f| f.initial_m_errors == 0 && f.maximum_reservoir_m <= 16)
        );
        let reset = lighting
            .render(&camera, &options, TraceMode::Restir)
            .expect("reset reused history");
        assert_eq!(reset.stats.frames[0].temporal_sources_admitted, 0);
        assert!(reset.stats.gpu_errors.is_empty());
    }
}

/// Reservoir source admission in the bounded static-camera prototype.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RestirReuse {
    None,
    Temporal,
    Spatial,
    Both,
}
impl RestirReuse {
    fn bits(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Temporal => 1,
            Self::Spatial => 2,
            Self::Both => 3,
        }
    }
}

/// Camera vectors are world-space, orthogonal unit directions.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct RayCamera {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub vertical_fov_degrees: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceOptions {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub frames: u32,
    /// Same finite path horizon for raw and cached rendering; included in the cache identity.
    pub bounces: u32,
    pub cell_size: f32,
    pub minimum_cache_samples: u32,
    pub seed: u32,
    pub candidates: u32,
    pub reuse: RestirReuse,
    pub history_m: u32,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            width: 320,
            height: 240,
            samples: 1,
            frames: 64,
            bounces: 6,
            cell_size: 0.25,
            minimum_cache_samples: 16,
            seed: 1,
            candidates: 1,
            reuse: RestirReuse::None,
            history_m: 64,
        }
    }
}

impl TraceOptions {
    fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > 2048
            || self.height > 2048
            || !(1..=64).contains(&self.samples)
            || !(1..=512).contains(&self.frames)
            || !(1..=8).contains(&self.bounces)
        {
            return Err(
                "trace dimensions must be 1..=2048; samples 1..=64, frames 1..=512, bounces 1..=8"
                    .into(),
            );
        }
        if !self.cell_size.is_finite()
            || !(0.01..=100.0).contains(&self.cell_size)
            || !(1..=1024).contains(&self.minimum_cache_samples)
        {
            return Err(
                "cell size must be finite in 0.01..=100; minimum cache samples 1..=1024".into(),
            );
        }
        if !(1..=16).contains(&self.candidates) || !(1..=1024).contains(&self.history_m) {
            return Err("RIS candidates must be 1..=16; reservoir history M cap 1..=1024".into());
        }
        let paths = u64::from(self.width)
            * u64::from(self.height)
            * u64::from(self.samples)
            * u64::from(self.frames);
        if paths > 250_000_000 {
            return Err(
                "trace exceeds 250 million camera paths; reduce resolution/samples/frames".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceFrameStats {
    pub frame: u32,
    pub wall_ms: f64,
    pub gpu_update_ms: Option<f64>,
    pub gpu_resolve_ms: Option<f64>,
    /// Raw/SHaRC render pass, or sum of all four ReSTIR passes.
    pub gpu_render_ms: Option<f64>,
    pub gpu_candidate_ms: Option<f64>,
    pub gpu_temporal_ms: Option<f64>,
    pub gpu_spatial_ms: Option<f64>,
    pub gpu_shade_ms: Option<f64>,
    pub update_paths: u32,
    pub update_trace_rays: u32,
    pub update_shadow_rays: u32,
    pub update_path_vertices: u32,
    pub camera_paths: u32,
    pub render_trace_rays: u32,
    pub render_shadow_rays: u32,
    pub render_path_vertices: u32,
    pub cache_queries: u32,
    pub cache_hits: u32,
    pub cache_deposits: u32,
    pub cache_dropped_deposits: u32,
    pub cache_hash_collisions: u32,
    pub cache_clipped_samples: u32,
    pub cache_occupied: u32,
    pub cache_confident: u32,
    pub candidate_proposals: u32,
    pub secondary_surface_proposals: u32,
    pub temporal_sources_admitted: u32,
    pub spatial_sources_admitted: u32,
    pub visibility_rays: u32,
    pub visibility_visible: u32,
    pub visibility_blocked: u32,
    pub history_rejections: u32,
    pub initial_selected_changes: u32,
    pub temporal_selected_changes: u32,
    pub spatial_selected_changes: u32,
    pub valid_final_reservoirs: u32,
    pub zero_proposals: u32,
    pub invalid_area_pdf: u32,
    pub maximum_reservoir_m: u32,
    pub initial_m_errors: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceStats {
    pub mode: TraceMode,
    pub adapter: String,
    pub backend: String,
    pub scene_signature: String,
    pub triangles: usize,
    pub emissive_triangles: usize,
    pub acceleration_structure_build_wall_ms: f64,
    pub trace_wall_ms: f64,
    pub readback_wall_ms: f64,
    pub cache_bytes: u64,
    pub reservoir_bytes: u64,
    pub camera: RayCamera,
    pub options: TraceOptions,
    pub frames: Vec<TraceFrameStats>,
    pub gpu_errors: Vec<String>,
    pub limitations: Vec<&'static str>,
}

pub struct TraceOutput {
    /// Linear scene-referred HDR, averaged across samples and frames. Preview conversion is separate.
    pub radiance: Vec<[f32; 4]>,
    pub stats: TraceStats,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TriangleGpu {
    positions: [[f32; 4]; 3],
    normals: [[f32; 4]; 3],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialGpu {
    albedo: [f32; 4],
    emission: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EmitterGpu {
    triangle: u32,
    area: f32,
    area_cdf: f32,
    padding: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Parameters {
    image: [u32; 4],
    limits: [u32; 4],
    grid: [f32; 4],
    camera_position: [f32; 4],
    camera_right: [f32; 4],
    camera_up: [f32; 4],
    camera_forward: [f32; 4],
    cache: [u32; 4],
    restir: [u32; 4],
}

fn buffer(
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
fn wait(device: &wgpu::Device, submission: wgpu::SubmissionIndex) -> Result<(), String> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .map(|_| ())
        .map_err(|error| format!("Metal GPU wait: {error}"))
}

/// Immutable geometry and material resources plus an explicitly resettable world-space cache.
pub struct RayLighting {
    device: wgpu::Device,
    queue: wgpu::Queue,
    _blas: wgpu::Blas,
    tlas: wgpu::Tlas,
    triangles: wgpu::Buffer,
    materials: wgpu::Buffer,
    emitters: wgpu::Buffer,
    hashes: wgpu::Buffer,
    accumulation: wgpu::Buffer,
    resolved: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    update_pipeline: wgpu::ComputePipeline,
    resolve_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::ComputePipeline,
    restir_pipelines: [wgpu::ComputePipeline; 4],
    timestamp_enabled: bool,
    errors: Arc<Mutex<Vec<String>>>,
    adapter: String,
    scene_signature: String,
    triangle_count: usize,
    emitter_count: usize,
    emitter_area: f32,
    build_wall_ms: f64,
}

impl RayLighting {
    pub fn new(gpu: &Gpu, scene: &RayScene) -> Result<Self, String> {
        if gpu.info.backend != wgpu::Backend::Metal {
            return Err("this isolated prototype requires the Metal backend".into());
        }
        let have = gpu.adapter.features();
        if !have.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
            return Err(format!(
                "{} does not expose EXPERIMENTAL_RAY_QUERY",
                gpu.info.name
            ));
        }
        if scene.environment_radiance().iter().any(|v| *v != 0.0)
            || scene.explicit_light_count() != 0
        {
            return Err("Metal trace prototype supports emissive triangle lights and a black environment only".into());
        }
        let world_materials: Vec<_> = scene.world_materials().collect();
        let mut materials = Vec::with_capacity(world_materials.len());
        for (index, surface) in world_materials.iter().enumerate() {
            let material = &surface.material;
            if material.base_color_texture.is_some()
                || material.metallic_roughness_texture.is_some()
                || material.normal_texture.is_some()
                || material.emissive_texture.is_some()
                || material.occlusion_texture.is_some()
            {
                return Err(format!(
                    "material {index}: textured transport is not implemented in the Metal prototype"
                ));
            }
            if material.alpha_mode != AlphaMode::Opaque
                || material.base_color[3] != 1.0
                || material.metallic != 0.0
                || !surface.casts_shadows
            {
                return Err(format!(
                    "material {index}: prototype requires opaque, nonmetallic, shadow-casting materials"
                ));
            }
            if material.base_color[..3]
                .iter()
                .any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
                || material.emissive.iter().any(|x| !x.is_finite() || *x < 0.0)
            {
                return Err(format!("material {index}: invalid reflectance or emission"));
            }
            materials.push(MaterialGpu {
                albedo: material.base_color,
                emission: [
                    material.emissive[0],
                    material.emissive[1],
                    material.emissive[2],
                    f32::from(material.double_sided),
                ],
            });
        }
        let world_triangles: Vec<_> = scene.world_triangles().collect();
        if world_triangles.is_empty() || world_triangles.len() > 1_000_000 {
            return Err("prototype needs 1..=1000000 triangles".into());
        }
        let mut positions = Vec::with_capacity(world_triangles.len() * 3);
        let mut triangles = Vec::with_capacity(world_triangles.len());
        let mut emitters = Vec::new();
        let mut emitter_area = 0.0;
        for (index, triangle) in world_triangles.iter().enumerate() {
            if triangle.material >= materials.len() {
                return Err("triangle material index out of range".into());
            }
            let p = triangle.positions.map(Vec3::from_array);
            let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            if !area.is_finite()
                || area <= 1e-12
                || triangle.normals.iter().any(|n| {
                    !Vec3::from_array(*n).is_finite() || Vec3::from_array(*n).length_squared() < 0.5
                })
            {
                return Err(format!(
                    "triangle {index}: degenerate geometry or invalid normals"
                ));
            }
            positions.extend(triangle.positions);
            triangles.push(TriangleGpu {
                positions: triangle.positions.map(|p| [p[0], p[1], p[2], 0.0]),
                normals: triangle
                    .normals
                    .map(|n| [n[0], n[1], n[2], triangle.material as f32]),
            });
            if materials[triangle.material].emission[..3]
                .iter()
                .any(|v| *v > 0.0)
            {
                emitter_area += area;
                emitters.push(EmitterGpu {
                    triangle: index as u32,
                    area,
                    area_cdf: emitter_area,
                    padding: 0,
                });
            }
        }
        let emitter_count = emitters.len();
        // A nonempty binding is still needed for scenes without emission.
        if emitters.is_empty() {
            emitters.push(EmitterGpu::zeroed());
        }
        let timestamp_enabled = have.contains(wgpu::Features::TIMESTAMP_QUERY);
        let features = wgpu::Features::EXPERIMENTAL_RAY_QUERY
            | if timestamp_enabled {
                wgpu::Features::TIMESTAMP_QUERY
            } else {
                wgpu::Features::empty()
            };
        let mut limits =
            wgpu::Limits::default().using_acceleration_structure_values(gpu.adapter.limits());
        if gpu.adapter.limits().max_storage_buffers_per_shader_stage < 14 {
            return Err("isolated native GI prototype requires 14 storage buffer bindings".into());
        }
        limits.max_storage_buffers_per_shader_stage = 14;
        let (device, queue) =
            pollster::block_on(gpu.adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("Metal GI prototype"),
                required_features: features,
                required_limits: limits,
                // SAFETY: this isolated tool deliberately opts into experimental ray-query APIs.
                experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                ..Default::default()
            }))
            .map_err(|e| format!("ray-query device: {e}"))?;
        let errors = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            eprintln!("Metal GI uncaptured GPU error: {error}");
            captured.lock().unwrap().push(error.to_string());
        }));
        let lost = errors.clone();
        device.set_device_lost_callback(move |reason, message| {
            eprintln!("Metal GI device lost ({reason:?}): {message}");
            lost.lock()
                .unwrap()
                .push(format!("device lost ({reason:?}): {message}"));
        });
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let triangle_buffer = upload(
            &device,
            "RT triangle shading data",
            &triangles,
            wgpu::BufferUsages::STORAGE,
        );
        let material_buffer = upload(
            &device,
            "RT constant materials",
            &materials,
            wgpu::BufferUsages::STORAGE,
        );
        let emitter_buffer = upload(
            &device,
            "RT emissive triangles",
            &emitters,
            wgpu::BufferUsages::STORAGE,
        );
        let vertices = upload(
            &device,
            "RT BLAS positions",
            &positions,
            wgpu::BufferUsages::BLAS_INPUT,
        );
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: positions.len() as u32,
            index_format: None,
            index_count: None,
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("static GI triangles"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles {
                descriptors: vec![size.clone()],
            },
        );
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("static GI world"),
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
            label: Some("GI BLAS/TLAS build"),
        });
        let build = wgpu::BlasBuildEntry {
            blas: &blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                size: &size,
                vertex_buffer: &vertices,
                first_vertex: 0,
                vertex_stride: 12,
                index_buffer: None,
                first_index: None,
                transform_buffer: None,
                transform_buffer_offset: None,
            }]),
        };
        let build_start = Instant::now();
        encoder.build_acceleration_structures([&build], [&tlas]);
        wait(&device, queue.submit([encoder.finish()]))?;
        let build_wall_ms = build_start.elapsed().as_secs_f64() * 1000.0;
        let storage_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let hashes = buffer(
            &device,
            "SHaRC exact spatial hash keys",
            u64::from(CACHE_SLOTS) * 32,
            storage_usage,
        );
        let accumulation = buffer(
            &device,
            "SHaRC frame sums and counts",
            u64::from(CACHE_SLOTS) * 16,
            storage_usage,
        );
        let resolved = buffer(
            &device,
            "SHaRC temporal radiance/confidence",
            u64::from(CACHE_SLOTS) * 16,
            storage_usage,
        );
        let entries: Vec<_> = (0..16)
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
                    _ => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage {
                            read_only: binding <= 4,
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GI trace resources"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GI trace layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GI path trace and SHaRC"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../../shaders/gi_trace.wgsl"),
                    include_str!("../../shaders/gi_reservoir.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    zero_initialize_workgroup_memory: false,
                    ..Default::default()
                },
                cache: None,
            })
        };
        let update_pipeline = pipeline("update");
        let resolve_pipeline = pipeline("resolve");
        let render_pipeline = pipeline("render");
        let restir_pipelines = [
            pipeline("restir_initial"),
            pipeline("restir_temporal"),
            pipeline("restir_spatial"),
            pipeline("restir_shade"),
        ];
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(format!("Metal GI setup validation: {error}"));
        }
        let pending = errors.lock().unwrap().clone();
        if !pending.is_empty() {
            return Err(pending.join("; "));
        }
        Ok(Self {
            device,
            queue,
            _blas: blas,
            tlas,
            triangles: triangle_buffer,
            materials: material_buffer,
            emitters: emitter_buffer,
            hashes,
            accumulation,
            resolved,
            layout,
            update_pipeline,
            resolve_pipeline,
            render_pipeline,
            restir_pipelines,
            timestamp_enabled,
            errors,
            adapter: gpu.info.name.clone(),
            scene_signature: scene.scene_signature().to_owned(),
            triangle_count: triangles.len(),
            emitter_count,
            emitter_area,
            build_wall_ms,
        })
    }

    /// Clear all derived state after changing a scene, camera/grid identity, or lighting.
    /// Geometry changes require a new RayLighting to rebuild its immutable BLAS/TLAS.
    pub fn reset_cache(&mut self) -> Result<(), String> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("reset GI cache"),
            });
        for buffer in [&self.hashes, &self.accumulation, &self.resolved] {
            encoder.clear_buffer(buffer, 0, None);
        }
        wait(&self.device, self.queue.submit([encoder.finish()]))
    }

    /// Run a reproducible static camera sequence, starting with empty cache/history every call.
    /// Sparse update paths are additional work, included in both frame timings and ray counters.
    pub fn render(
        &mut self,
        camera: &RayCamera,
        options: &TraceOptions,
        mode: TraceMode,
    ) -> Result<TraceOutput, String> {
        options.validate()?;
        if mode == TraceMode::Restir && options.samples != 1 {
            return Err("the ReSTIR prototype stores one receiver per pixel/frame: use --samples 1 and --candidates for RIS budget".into());
        }
        if mode == TraceMode::Restir
            && u64::from(options.width)
                * u64::from(options.height)
                * u64::from(options.frames)
                * u64::from(options.candidates)
                > 250_000_000
        {
            return Err("ReSTIR generation exceeds 250 million candidate paths".into());
        }
        let position = Vec3::from_array(camera.position);
        let forward = Vec3::from_array(camera.forward);
        let up = Vec3::from_array(camera.up);
        if !position.is_finite()
            || !forward.is_finite()
            || !up.is_finite()
            || (forward.length_squared() - 1.0).abs() > 1e-3
            || (up.length_squared() - 1.0).abs() > 1e-3
            || forward.dot(up).abs() > 1e-3
            || !camera.vertical_fov_degrees.is_finite()
            || !(1.0..=170.0).contains(&camera.vertical_fov_degrees)
        {
            return Err(
                "camera needs finite position, orthogonal unit forward/up and FOV 1..=170 degrees"
                    .into(),
            );
        }
        self.reset_cache()?;
        let right = forward.cross(up).normalize();
        let mut parameters = Parameters {
            image: [options.width, options.height, options.samples, 0],
            limits: [
                options.bounces,
                u32::from(mode == TraceMode::Sharc),
                CACHE_SLOTS,
                options.minimum_cache_samples,
            ],
            grid: [options.cell_size, 1e-4, self.emitter_area, 10000.0],
            camera_position: [position.x, position.y, position.z, 0.0],
            camera_right: [
                right.x,
                right.y,
                right.z,
                options.width as f32 / options.height as f32,
            ],
            camera_up: [up.x, up.y, up.z, 0.0],
            camera_forward: [
                forward.x,
                forward.y,
                forward.z,
                (camera.vertical_fov_degrees.to_radians() * 0.5).tan(),
            ],
            cache: [self.emitter_count as u32, 1024, 128, options.seed],
            restir: [
                options.candidates,
                options.reuse.bits(),
                options.history_m,
                1,
            ],
        };
        let pixels = u64::from(options.width) * u64::from(options.height);
        let image_size = pixels * 16;
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let image = buffer(
            &self.device,
            "linear GI output",
            image_size,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let parameters_buffer = upload(
            &self.device,
            "GI trace parameters",
            &[parameters],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let counters = buffer(
            &self.device,
            "GI per-frame counters",
            COUNTER_COUNT * 4,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let statistics_size = u64::from(options.frames) * COUNTER_COUNT * 4;
        let statistics = buffer(
            &self.device,
            "GI statistics history",
            statistics_size,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        );
        let reservoir_size = if mode == TraceMode::Restir {
            pixels * 80
        } else {
            80
        };
        if reservoir_size > u64::from(self.device.limits().max_storage_buffer_binding_size) {
            return Err(
                "ReSTIR receiver/reservoir exceeds storage binding limit; reduce resolution".into(),
            );
        }
        let restir_buffers: Vec<_> = (0..6)
            .map(|_| {
                buffer(
                    &self.device,
                    "ReSTIR receiver/reservoir",
                    reservoir_size,
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST,
                )
            })
            .collect();
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GI trace bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: parameters_buffer.as_entire_binding(),
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
                    resource: self.emitters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: image.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.hashes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: self.accumulation.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: self.resolved.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: counters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: restir_buffers[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: restir_buffers[1].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: restir_buffers[2].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: restir_buffers[3].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: restir_buffers[4].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: restir_buffers[5].as_entire_binding(),
                },
            ],
        });
        let queries = self.timestamp_enabled.then(|| {
            self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("GI pass timings"),
                ty: wgpu::QueryType::Timestamp,
                count: options.frames * TIMESTAMPS_PER_FRAME,
            })
        });
        let timestamp_size = u64::from(options.frames) * u64::from(TIMESTAMPS_PER_FRAME) * 8;
        let timestamp_resolve = queries.as_ref().map(|_| {
            buffer(
                &self.device,
                "GI timestamp resolve",
                timestamp_size,
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            )
        });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(format!("GI frame setup validation: {error}"));
        }
        let trace_start = Instant::now();
        let mut wall_times = Vec::with_capacity(options.frames as usize);
        for frame in 0..options.frames {
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let frame_start = Instant::now();
            parameters.image[3] = frame;
            self.queue
                .write_buffer(&parameters_buffer, 0, bytemuck::bytes_of(&parameters));
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("GI trace frame"),
                });
            encoder.clear_buffer(&counters, 0, None);
            if frame == 0 {
                encoder.clear_buffer(&image, 0, None);
                for buffer in &restir_buffers {
                    encoder.clear_buffer(buffer, 0, None);
                }
            }
            let dispatch = |encoder: &mut wgpu::CommandEncoder,
                            pipeline: &wgpu::ComputePipeline,
                            label: &'static str,
                            index: u32,
                            groups| {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: queries.as_ref().map(|query_set| {
                        wgpu::ComputePassTimestampWrites {
                            query_set,
                            beginning_of_pass_write_index: Some(
                                frame * TIMESTAMPS_PER_FRAME + index * 2,
                            ),
                            end_of_pass_write_index: Some(
                                frame * TIMESTAMPS_PER_FRAME + index * 2 + 1,
                            ),
                        }
                    }),
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                pass.dispatch_workgroups(groups, 1, 1);
            };
            if mode == TraceMode::Sharc {
                let sparse_paths = options.width.div_ceil(5) * options.height.div_ceil(5);
                dispatch(
                    &mut encoder,
                    &self.update_pipeline,
                    "SHaRC sparse update",
                    0,
                    sparse_paths.div_ceil(64),
                );
                dispatch(
                    &mut encoder,
                    &self.resolve_pipeline,
                    "SHaRC history resolve",
                    1,
                    CACHE_SLOTS.div_ceil(64),
                );
            }
            if mode == TraceMode::Restir {
                for (index, pipeline) in self.restir_pipelines.iter().enumerate() {
                    dispatch(
                        &mut encoder,
                        pipeline,
                        [
                            "RIS candidates",
                            "ReSTIR temporal",
                            "ReSTIR spatial",
                            "ReSTIR shade",
                        ][index],
                        index as u32,
                        (pixels as u32).div_ceil(64),
                    );
                }
                encoder.copy_buffer_to_buffer(
                    &restir_buffers[0],
                    0,
                    &restir_buffers[1],
                    0,
                    reservoir_size,
                );
                encoder.copy_buffer_to_buffer(
                    &restir_buffers[4],
                    0,
                    &restir_buffers[5],
                    0,
                    reservoir_size,
                );
            } else {
                dispatch(
                    &mut encoder,
                    &self.render_pipeline,
                    "GI full camera paths",
                    2,
                    (pixels as u32).div_ceil(64),
                );
            }
            encoder.copy_buffer_to_buffer(
                &counters,
                0,
                &statistics,
                u64::from(frame) * COUNTER_COUNT * 4,
                COUNTER_COUNT * 4,
            );
            wait(&self.device, self.queue.submit([encoder.finish()]))?;
            if let Some(error) = pollster::block_on(scope.pop()) {
                return Err(format!("GI frame {frame} validation: {error}"));
            }
            wall_times.push(frame_start.elapsed().as_secs_f64() * 1000.0);
        }
        let trace_wall_ms = trace_start.elapsed().as_secs_f64() * 1000.0;
        let readback_start = Instant::now();
        let image_read = buffer(
            &self.device,
            "GI HDR readback",
            image_size,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let statistics_read = buffer(
            &self.device,
            "GI counter readback",
            statistics_size,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let timestamps_read = queries.as_ref().map(|_| {
            buffer(
                &self.device,
                "GI timestamp readback",
                timestamp_size,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            )
        });
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("final GI readback"),
            });
        encoder.copy_buffer_to_buffer(&image, 0, &image_read, 0, image_size);
        encoder.copy_buffer_to_buffer(&statistics, 0, &statistics_read, 0, statistics_size);
        if let (Some(queries), Some(resolve), Some(read)) =
            (&queries, &timestamp_resolve, &timestamps_read)
        {
            encoder.resolve_query_set(
                queries,
                0..options.frames * TIMESTAMPS_PER_FRAME,
                resolve,
                0,
            );
            encoder.copy_buffer_to_buffer(resolve, 0, read, 0, timestamp_size);
        }
        let submission = self.queue.submit([encoder.finish()]);
        let read_buffers: Vec<_> = [&image_read, &statistics_read]
            .into_iter()
            .chain(timestamps_read.as_ref())
            .collect();
        let mut receivers = Vec::new();
        for read in &read_buffers {
            let (sender, receiver) = mpsc::channel();
            read.slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            receivers.push(receiver);
        }
        wait(&self.device, submission)?;
        for receiver in receivers {
            receiver
                .recv_timeout(Duration::from_secs(1))
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        }
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(format!("GI readback validation: {error}"));
        }
        let colors = image_read
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let radiance: Vec<[f32; 4]> = colors
            .chunks_exact(16)
            .map(|bytes| {
                let value: [f32; 4] = bytemuck::pod_read_unaligned(bytes);
                [
                    value[0] / options.frames as f32,
                    value[1] / options.frames as f32,
                    value[2] / options.frames as f32,
                    1.0,
                ]
            })
            .collect();
        if radiance
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("GPU produced nonfinite/negative radiance".into());
        }
        drop(colors);
        image_read.unmap();
        let data = statistics_read
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let counters: Vec<u32> = data
            .chunks_exact(4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        drop(data);
        statistics_read.unmap();
        let timestamp_values = if let Some(read) = &timestamps_read {
            let data = read
                .slice(..)
                .get_mapped_range()
                .map_err(|e| e.to_string())?;
            let values: Vec<u64> = data
                .chunks_exact(8)
                .map(|v| u64::from_le_bytes(v.try_into().unwrap()))
                .collect();
            drop(data);
            read.unmap();
            Some(values)
        } else {
            None
        };
        let period = f64::from(self.queue.get_timestamp_period()) / 1e6;
        let frames: Vec<TraceFrameStats> = counters
            .chunks_exact(COUNTER_COUNT as usize)
            .enumerate()
            .map(|(index, c)| {
                let timing = |pass: usize| {
                    timestamp_values.as_ref().map(|values| {
                        let base = index * TIMESTAMPS_PER_FRAME as usize + pass * 2;
                        values[base + 1].saturating_sub(values[base]) as f64 * period
                    })
                };
                TraceFrameStats {
                    frame: index as u32,
                    wall_ms: wall_times[index],
                    gpu_update_ms: if mode == TraceMode::Sharc {
                        timing(0)
                    } else {
                        None
                    },
                    gpu_resolve_ms: if mode == TraceMode::Sharc {
                        timing(1)
                    } else {
                        None
                    },
                    gpu_render_ms: if mode == TraceMode::Restir {
                        timing(0).map(|first| {
                            first
                                + timing(1).unwrap_or(0.0)
                                + timing(2).unwrap_or(0.0)
                                + timing(3).unwrap_or(0.0)
                        })
                    } else {
                        timing(2)
                    },
                    gpu_candidate_ms: if mode == TraceMode::Restir {
                        timing(0)
                    } else {
                        None
                    },
                    gpu_temporal_ms: if mode == TraceMode::Restir {
                        timing(1)
                    } else {
                        None
                    },
                    gpu_spatial_ms: if mode == TraceMode::Restir {
                        timing(2)
                    } else {
                        None
                    },
                    gpu_shade_ms: if mode == TraceMode::Restir {
                        timing(3)
                    } else {
                        None
                    },
                    update_paths: c[0],
                    update_trace_rays: c[1],
                    update_shadow_rays: c[2],
                    update_path_vertices: c[3],
                    camera_paths: c[4],
                    render_trace_rays: c[5],
                    render_shadow_rays: c[6],
                    render_path_vertices: c[7],
                    cache_queries: c[8],
                    cache_hits: c[9],
                    cache_deposits: c[10],
                    cache_dropped_deposits: c[11],
                    cache_hash_collisions: c[12],
                    cache_clipped_samples: c[13],
                    cache_occupied: c[14],
                    cache_confident: c[15],
                    candidate_proposals: c[16],
                    secondary_surface_proposals: c[17],
                    temporal_sources_admitted: c[18],
                    spatial_sources_admitted: c[19],
                    visibility_rays: c[20],
                    visibility_visible: c[21],
                    visibility_blocked: c[22],
                    history_rejections: c[23],
                    initial_selected_changes: c[24],
                    temporal_selected_changes: c[25],
                    spatial_selected_changes: c[26],
                    valid_final_reservoirs: c[27],
                    zero_proposals: c[28],
                    invalid_area_pdf: c[29],
                    maximum_reservoir_m: c[30],
                    initial_m_errors: c[31],
                }
            })
            .collect();
        if mode == TraceMode::Restir
            && frames
                .iter()
                .any(|f| f.initial_m_errors != 0 || f.maximum_reservoir_m > options.history_m)
        {
            return Err("GPU reservoir M invariant failed (all proposals must count; final M must respect cap)".into());
        }
        let errors = self.errors.lock().unwrap().clone();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(TraceOutput {
            radiance,
            stats: TraceStats {
                mode,
                adapter: self.adapter.clone(),
                backend: "Metal".into(),
                scene_signature: self.scene_signature.clone(),
                triangles: self.triangle_count,
                emissive_triangles: self.emitter_count,
                acceleration_structure_build_wall_ms: self.build_wall_ms,
                trace_wall_ms,
                readback_wall_ms: readback_start.elapsed().as_secs_f64() * 1000.0,
                cache_bytes: u64::from(CACHE_SLOTS) * 64,
                reservoir_bytes: reservoir_size * 6,
                camera: *camera,
                options: options.clone(),
                frames,
                gpu_errors: errors,
                limitations: vec![
                    "Isolated static diffuse prototype; not integrated into the interactive renderer",
                    "Constant opaque nonmetallic materials, emissive triangles, black sky; unsupported features are rejected",
                    "Lambertian transport, bounded path horizon; no GGX, transmission, denoising or temporal reprojection",
                    "SHaRC-style portable implementation, not NVIDIA SDK parity or a converged solution claim",
                    "Exact cell/normal/material/horizon keys; camera-distance LOD, nearest-cell reconstruction",
                    "Sparse update adds one camera path per 5x5 tile per sample; all costs and rays are included",
                    "New entries drop concurrent deposits during insertion frame; bounded sums clip radiance over 128",
                    "Confidence means accumulated sample count/age, not a statistical error bound",
                    "Every render call resets history; geometry changes require reconstruction and explicit cache reset",
                    "Prototype world units use ray epsilon 0.0001 and maximum ray distance 10000",
                    "Wall frame timings include CPU encoding, driver submission and GPU wait; timestamps measure passes",
                    "ReSTIR mode is a single-secondary-surface diffuse GI prototype; full ReSTIR PT is not implemented",
                    "Temporal/spatial reuse lacks complete support/visibility/M correction and can be biased",
                    "Only static scene/camera sequences; same primitive/material plus normal/depth compatibility, fresh selected-point visibility",
                    "ReSTIR supports one jittered receiver per pixel/frame; primary emission MIS is outside the indirect reservoir",
                ],
            },
        })
    }
}
