//! Static project rendering, including full glTF scenes, without a window or simulation.
//! `cargo run --release -p pocket-render --example showcase_bench -- PROJECT
//!  [--width 1920] [--height 1080] [--frames 300] [--warm 60] [--capture OUT.png]`
//! Reports submit-to-GPU-idle wall time, CPU encoding time, timestamp pass timings and source
//! geometry counts. This deliberately excludes game ticks, presentation and PNG readback.
//! `--capture` saves, after the measurement, a frame drawn at a fixed time (0 s), so captures of
//! the same project on two backends or adapters compare pixel for pixel (docs/bench/dx12.md).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    AnimView, CameraView, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose,
    RenderFrame,
};
use pocket_assets::mesh::ModelAsset;
use pocket_assets::visual::{Animator, Camera, Environment, Light, LightKind, Model, SkyKind};
use pocket_render::loader::{AssetSource, FileAssets};
use pocket_render::{BackendChoice, Gpu, Renderer};
use serde_json::{Value, json};

#[derive(Default)]
struct AssetInfo {
    triangles: usize,
    node_triangles: usize,
    mesh_triangles: Vec<usize>,
    mesh_names: HashMap<String, usize>,
    images: Vec<[u32; 2]>,
}

#[derive(Default)]
struct Assets {
    loaded: BTreeMap<String, AssetInfo>,
    errors: BTreeMap<String, String>,
}

struct TrackedSource {
    source: FileAssets,
    assets: Arc<Mutex<Assets>>,
}

impl AssetSource for TrackedSource {
    fn request(&mut self, path: &str) {
        self.source.request(path);
    }
    fn request_baked_gi(&mut self, path: &str) {
        self.source.request_baked_gi(path);
    }
    fn poll_baked_gi(&mut self) -> Vec<(String, Result<pocket_assets::gi::BakedGi, String>)> {
        self.source.poll_baked_gi()
    }
    fn request_neural_gi(&mut self, path: &str) {
        self.source.request_neural_gi(path);
    }
    fn poll_neural_gi(&mut self) -> Vec<(String, Result<pocket_assets::gi::NeuralGi, String>)> {
        self.source.poll_neural_gi()
    }

    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        let ready = self.source.poll();
        let mut state = self.assets.lock().expect("asset report lock");
        for (path, result) in &ready {
            match result {
                Ok(asset) => {
                    let triangles: Vec<usize> = asset
                        .meshes
                        .iter()
                        .map(|mesh| mesh.indices.len() / 3)
                        .collect();
                    state.loaded.insert(
                        path.clone(),
                        AssetInfo {
                            triangles: triangles.iter().sum(),
                            node_triangles: asset
                                .nodes
                                .iter()
                                .map(|node| triangles[node.mesh])
                                .sum(),
                            mesh_names: asset
                                .meshes
                                .iter()
                                .enumerate()
                                .map(|(index, mesh)| (mesh.name.clone(), index))
                                .collect(),
                            mesh_triangles: triangles,
                            images: asset
                                .images
                                .iter()
                                .map(|image| [image.width, image.height])
                                .collect(),
                        },
                    );
                }
                Err(error) => {
                    state.errors.insert(path.clone(), error.clone());
                }
            }
        }
        ready
    }
}

fn vector<const N: usize>(value: Option<&Value>, default: [f32; N]) -> Result<[f32; N], String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let array = value
        .as_array()
        .filter(|values| values.len() == N)
        .ok_or("invalid pose vector")?;
    let mut result = default;
    for (out, value) in result.iter_mut().zip(array) {
        *out = value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or("invalid pose number")? as f32;
    }
    Ok(result)
}

fn component<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn load_frame(root: &Path) -> Result<(RenderFrame, Vec<String>), String> {
    let path = root.join("scene.json");
    let scene: Value =
        serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if scene["format"] != "pocket-scene" || scene["version"] != 1 {
        return Err("expected pocket-scene version 1".into());
    }
    let mut frame = RenderFrame {
        reset: true,
        dt_s: 1.0 / 60.0,
        ..RenderFrame::default()
    };
    let mut models = Vec::new();
    let entities = scene["entities"]
        .as_array()
        .ok_or("scene.entities must be an array")?;
    for (index, entity) in entities.iter().enumerate() {
        if entity.get("prefab").is_some() {
            return Err(
                "static benchmark requires expanded entities; prefabs need the game runtime".into(),
            );
        }
        let id = index as u64 + 1;
        let components = &entity["components"];
        let transform = &components["Transform"];
        let position = vector(transform.get("position"), [0.0; 3])?;
        let rotation = vector(transform.get("rotation"), [0.0, 0.0, 0.0, 1.0])?;
        if let Some(value) = components.get("Model") {
            let model: Model = component(value)?;
            if model.visible {
                models.push(model.mesh.clone());
            }
            let animator: Option<Animator> =
                components.get("Animator").map(component).transpose()?;
            frame.instances.push(InstanceUpdate {
                id,
                pose: Some(Pose {
                    position,
                    rotation,
                    scale: model.scale.map(|value| value as f32),
                }),
                look: Some(Look {
                    mesh: model.mesh,
                    material: model.material,
                    color: model.color.map(|value| value as f32),
                    metallic: model.metallic as f32,
                    roughness: model.roughness as f32,
                    transmission: model.transmission.map(|value| value as f32),
                    ior: model.ior.map(|value| value as f32),
                    emissive: model.emissive.map(|value| value as f32),
                    cast_shadows: model.cast_shadows,
                    visible: model.visible,
                }),
                anim: animator.map(|animation| AnimView {
                    clip: animation.clip,
                    time: animation.time as f32,
                    rate: 0.0,
                    looped: animation.looped,
                }),
            });
        }
        if let Some(value) = components.get("Light") {
            let light: Light = component(value)?;
            let direction = Quat::from_array(rotation) * -Vec3::Z;
            frame.lights.get_or_insert_with(Vec::new).push(LightView {
                id,
                kind: match light.kind {
                    LightKind::Directional => LightKindView::Directional,
                    LightKind::Point => LightKindView::Point,
                    LightKind::Spot => LightKindView::Spot,
                },
                position,
                direction: direction.to_array(),
                color: light.color.map(|value| value as f32),
                intensity: light.intensity as f32,
                range: light.range as f32,
                inner_deg: light.inner_deg as f32,
                outer_deg: light.outer_deg as f32,
                shadows: light.shadows,
            });
        }
        if let Some(value) = components.get("Camera") {
            let camera: Camera = component(value)?;
            frame.cameras.get_or_insert_with(Vec::new).push(CameraView {
                id,
                position,
                rotation,
                fov_deg: camera.fov_deg as f32,
                near: camera.near as f32,
                far: camera.far as f32,
                active: camera.active,
                exposure_ev: camera.exposure_ev as f32,
            });
        }
        if frame.environment.is_none()
            && let Some(value) = components.get("Environment")
        {
            let environment: Environment = component(value)?;
            frame.environment = Some(EnvironmentView {
                sky: match environment.sky {
                    SkyKind::Atmosphere => 0,
                    SkyKind::Color => 1,
                },
                sky_color: environment.sky_color.map(|value| value as f32),
                ambient: environment.ambient as f32,
                baked_gi: environment.baked_gi,
                neural_gi: environment.neural_gi,
                gi_intensity: environment.gi_intensity as f32,
                fog_density: environment.fog_density as f32,
                fog_color: environment.fog_color.map(|value| value as f32),
                exposure_ev: environment.exposure_ev as f32,
                bloom: environment.bloom as f32,
            });
        }
        for unsupported in [
            "Splat",
            "Ocean",
            "Sea",
            "ParticleEmitter",
            "UiText",
            "UiBar",
        ] {
            if components.get(unsupported).is_some() {
                return Err(format!(
                    "unsupported static benchmark component: {unsupported}"
                ));
            }
        }
    }
    Ok((frame, models))
}

fn summary(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({
        "mean": sorted.iter().sum::<f64>() / sorted.len() as f64,
        "p50": sorted[sorted.len() / 2],
        "p95": sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)],
    })
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let root =
        PathBuf::from(arguments.first().ok_or(
            "usage: showcase_bench PROJECT [--width N] [--height N] [--frames N] [--warm N]",
        )?);
    let number = |name: &str, default: u32| -> Result<u32, String> {
        match arguments.iter().position(|value| value == name) {
            Some(index) => arguments
                .get(index + 1)
                .ok_or_else(|| format!("missing {name} value"))?
                .parse::<u32>()
                .map_err(|error| error.to_string()),
            None => Ok(default),
        }
    };
    let width = number("--width", 1920)?.clamp(16, 4096);
    let height = number("--height", 1080)?.clamp(16, 4096);
    let frames = number("--frames", 300)?.clamp(1, 100_000);
    let warm = number("--warm", 60)?.max(3);
    let (frame, models) = load_frame(&root)?;
    let assets = Arc::new(Mutex::new(Assets::default()));
    let gpu = Gpu::headless(BackendChoice::from_env()).map_err(|error| error.to_string())?;
    let render_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let errors = render_errors.clone();
    gpu.device
        .on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let message = format!("{error:?}");
            eprintln!("renderer validation: {message}");
            errors.lock().expect("render error lock").push(message);
        }));
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::new(&gpu, format, width, height);
    renderer.set_asset_source(Box::new(TrackedSource {
        source: FileAssets::new(root.clone()),
        assets: assets.clone(),
    }));
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("showcase benchmark"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    renderer.apply(frame, 0.0);
    let started = Instant::now();
    let mut complete = 0;
    let mut total_warm = 0;
    while complete < warm {
        if started.elapsed() > Duration::from_secs(120) {
            return Err("asset loading/warmup exceeded 120 seconds".into());
        }
        let stats = renderer.render(&view, started.elapsed().as_secs_f64());
        if let Some(error) = renderer.gi_error() {
            return Err(format!("GI asset failed during warmup: {error}"));
        }
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        complete = if stats.pending_assets == 0 {
            complete + 1
        } else {
            0
        };
        total_warm += 1;
        if stats.pending_assets > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        let state = assets.lock().expect("asset report lock");
        if !state.errors.is_empty() {
            return Err(format!("asset imports failed: {:?}", state.errors));
        }
        if !render_errors.lock().expect("render error lock").is_empty() {
            return Err("renderer validation failed; see stderr".into());
        }
    }
    let mut wall = Vec::with_capacity(frames as usize);
    let mut cpu = Vec::with_capacity(frames as usize);
    let mut gpu_ms = Vec::with_capacity(frames as usize);
    let mut passes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let measured_at = Instant::now();
    for _ in 0..frames {
        if measured_at.elapsed() > Duration::from_secs(120) {
            return Err("measurement exceeded 120 seconds; reduce --frames".into());
        }
        let time = Instant::now();
        let stats = renderer.render(&view, started.elapsed().as_secs_f64());
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        wall.push(time.elapsed().as_secs_f64() * 1000.0);
        cpu.push(f64::from(stats.cpu_ms));
        if gpu.caps.timestamps && !stats.passes.is_empty() {
            gpu_ms.push(f64::from(stats.gpu_ms));
            for (name, milliseconds) in stats.passes {
                passes
                    .entry(name)
                    .or_default()
                    .push(f64::from(milliseconds));
            }
        }
    }
    // A fixed-time frame for cross-backend comparison: time-driven state (interpolation, particles,
    // the sea) is the same at the same time, so only the backend and the adapter differ.
    if let Some(index) = arguments.iter().position(|value| value == "--capture") {
        let path = PathBuf::from(arguments.get(index + 1).ok_or("missing --capture path")?);
        for _ in 0..8 {
            renderer.render(&view, 0.0);
            gpu.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| e.to_string())?;
        }
        let (w, h, pixels) = renderer.capture_rgba(0.0);
        image::save_buffer(&path, &pixels, w, h, image::ColorType::Rgba8)
            .map_err(|e| e.to_string())?;
        eprintln!("saved {} ({w}x{h})", path.display());
    }
    // A same-camera GI comparison is untimed and reuses the resident model and probe field.
    if let Some(index) = arguments.iter().position(|value| value == "--gi-compare") {
        let directory = PathBuf::from(
            arguments
                .get(index + 1)
                .ok_or("missing --gi-compare directory")?,
        );
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        if !renderer.gi_loaded() {
            return Err("GI comparison needs a successfully loaded probe or neural asset".into());
        }
        let intensity = renderer
            .scene
            .environment
            .as_ref()
            .ok_or("GI comparison needs Environment")?
            .gi_intensity;
        for (name, value) in [("on", intensity), ("off", 0.0), ("on-repeat", intensity)] {
            renderer.scene.environment.as_mut().unwrap().gi_intensity = value;
            for _ in 0..32 {
                renderer.render(&view, 0.0);
                gpu.device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .map_err(|e| e.to_string())?;
            }
            let (w, h, pixels) = renderer.capture_rgba(0.0);
            image::save_buffer(
                directory.join(format!("{name}.png")),
                &pixels,
                w,
                h,
                image::ColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
        }
        std::fs::write(
            directory.join("capture.json"),
            serde_json::to_vec_pretty(&json!({
                "adapter":gpu.info.name,"backend":gpu.backend_name(),"loaded":renderer.gi_loaded(),
                "gi_error":renderer.gi_error(),"pending_assets":renderer.last.pending_assets,
                "gi_asset":renderer.scene.environment.as_ref().map(|e|e.baked_gi.as_str()),
                "intensity_on":intensity,"intensity_off":0.0,"settle_frames":32,
                "same_camera_material_lights_exposure":true,"capture_time":0.0
            }))
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    // Pixel coverage is an extra ID pass after timing, so its readback cannot affect the sample.
    renderer.request_visible();
    renderer.render(&view, started.elapsed().as_secs_f64());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| error.to_string())?;
    let coverage = renderer.take_visible();
    if !render_errors.lock().expect("render error lock").is_empty() {
        return Err("renderer validation failed; see stderr".into());
    }
    let state = assets.lock().expect("asset report lock");
    let scene_triangles: Option<usize> = models
        .iter()
        .map(|mesh| {
            if let Some(primitive) = pocket_assets::primitives::primitive(mesh) {
                return Some(primitive.triangles());
            }
            let (path, selector) = mesh
                .split_once('#')
                .map_or((mesh.as_str(), None), |(path, selector)| {
                    (path, Some(selector))
                });
            let asset = state.loaded.get(path)?;
            match selector {
                None => Some(asset.node_triangles),
                Some(selector) => {
                    let index = asset
                        .mesh_names
                        .get(selector)
                        .copied()
                        .or_else(|| selector.parse().ok())?;
                    asset.mesh_triangles.get(index).copied()
                }
            }
        })
        .sum();
    let report = json!({
        "measurement": "static renderer, submit-to-GPU-idle; no simulation, presentation or PNG readback",
        "project": root.display().to_string(),
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "timestamp_queries": gpu.caps.timestamps,
        "indirect_first_instance": gpu.caps.indirect_first_instance,
        "width": width,
        "height": height,
        "warm_frames": total_warm,
        "complete_warm_frames": complete,
        "frames": frames,
        "submit_to_idle_ms": summary(&wall),
        "cpu_encode_ms": summary(&cpu),
        "profiled_passes_gpu_ms": summary(&gpu_ms),
        "gpu_timing_samples": gpu_ms.len(),
        "passes_ms": passes.iter().map(|(name, samples)| (*name, summary(samples))).collect::<BTreeMap<_, _>>(),
        "instances": renderer.last.instances,
        "entities": renderer.last.entities,
        "meshes": renderer.last.meshes,
        "materials": renderer.last.materials,
        "punctual_lights": renderer.last.lights,
        "pending_assets": renderer.last.pending_assets,
        "pixel_visible_entities": coverage.as_ref().map(Vec::len),
        "pixel_coverage": coverage,
        "source_unique_triangles": state.loaded.values().map(|asset| asset.triangles).sum::<usize>(),
        "scene_triangles_before_culling": scene_triangles,
        "source_images": state.loaded.values().map(|asset| asset.images.len()).sum::<usize>(),
        "source_image_sizes": state.loaded.iter().map(|(path, asset)| (path, &asset.images)).collect::<BTreeMap<_, _>>(),
        "texture_layer_size": pocket_render::materials::TEX_SIZE,
        "notes": "Instance count is submitted primitive instances, not post-culling visibility. Scene triangle count includes primitives and repeated scene nodes/entities, excludes shadow-pass multiplication; null for unknown mesh selectors. GPU pass queries are delayed by a few frames and cover only instrumented passes (postprocessing is excluded). Metal pass boundaries can omit tile-rendering work, so the pass sum is not whole-frame GPU cost. Submit-to-idle measures the complete static rendering workload. All reported frames follow completed asset warmup; pixel coverage is an untimed extra ID pass.",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("showcase benchmark failed: {error}");
        std::process::exit(1);
    }
}
