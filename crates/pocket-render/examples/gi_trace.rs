//! Static ray-query diffuse tracing / SHaRC / bounded RIS-ReSTIR GI experiments, on any adapter with
//! ray queries (Metal, Vulkan, Direct3D 12; `POCKET_BACKEND` and `POCKET_ADAPTER` choose).
//! `cargo run --release -p pocket-render --example gi_trace -- PROJECT --mode raw|sharc|restir
//!    --output image.png --width 320 --height 240 --samples 1 --frames 64`
//! Writes a preview PNG, a JSON report, and linear float32 RGBA for unbiased image comparisons.

use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use pocket_render::gi::bake::RayScene;
use pocket_render::gi::rt::{RayCamera, RayLighting, RestirReuse, TraceMode, TraceOptions};
use pocket_render::{BackendChoice, Gpu};
use serde_json::{Value, json};

fn vector<const N: usize>(value: Option<&Value>, default: [f32; N]) -> Result<[f32; N], String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let array = value
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or("invalid camera vector")?;
    let mut result = default;
    for (out, number) in result.iter_mut().zip(array) {
        *out = number
            .as_f64()
            .filter(|x| x.is_finite())
            .ok_or("invalid camera number")? as f32;
    }
    Ok(result)
}

fn camera(root: &Path) -> Result<RayCamera, String> {
    let scene: Value =
        serde_json::from_slice(&std::fs::read(root.join("scene.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let entities = scene["entities"]
        .as_array()
        .ok_or("missing scene entities")?;
    let entity = entities
        .iter()
        .find(|e| e["components"].get("Camera").is_some())
        .ok_or("scene needs a Camera")?;
    let transform = &entity["components"]["Transform"];
    let position = vector(transform.get("position"), [0.0; 3])?;
    let rotation = Quat::from_array(vector(transform.get("rotation"), [0.0, 0.0, 0.0, 1.0])?);
    if !rotation.is_finite() || (rotation.length_squared() - 1.0).abs() > 1e-3 {
        return Err("camera quaternion must be normalized".into());
    }
    let fov = entity["components"]["Camera"]["fov_deg"]
        .as_f64()
        .unwrap_or(60.0) as f32;
    Ok(RayCamera {
        position,
        forward: (rotation * -Vec3::Z).to_array(),
        up: (rotation * Vec3::Y).to_array(),
        vertical_fov_degrees: fov,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gi_trace: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("usage: gi_trace PROJECT --mode raw|sharc|restir --output PNG [--width W --height H --samples N --frames N --bounces N --cell-size M --min-cache-samples N --seed N --candidates K --reuse none|temporal|spatial|both --history-m N --lean-shaders true|false]")?);
    let mut options = TraceOptions::default();
    let mut mode = TraceMode::Raw;
    let mut output = PathBuf::from("gi-trace.png");
    let mut lean_shaders = false;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let integer = |value: &str| {
            value
                .parse::<u32>()
                .map_err(|_| format!("{flag} needs an integer"))
        };
        match flag.as_str() {
            "--mode" => {
                mode = match value.as_str() {
                    "raw" => TraceMode::Raw,
                    "sharc" => TraceMode::Sharc,
                    "restir" => TraceMode::Restir,
                    _ => return Err("mode must be raw, sharc or restir".into()),
                }
            }
            "--output" => output = PathBuf::from(value),
            "--width" => options.width = integer(&value)?,
            "--height" => options.height = integer(&value)?,
            "--samples" => options.samples = integer(&value)?,
            "--frames" => options.frames = integer(&value)?,
            "--bounces" => options.bounces = integer(&value)?,
            "--min-cache-samples" => options.minimum_cache_samples = integer(&value)?,
            "--seed" => options.seed = integer(&value)?,
            "--candidates" => options.candidates = integer(&value)?,
            "--lean-shaders" => {
                lean_shaders = value
                    .parse()
                    .map_err(|_| "--lean-shaders needs true or false")?
            }
            "--history-m" => options.history_m = integer(&value)?,
            "--reuse" => {
                options.reuse = match value.as_str() {
                    "none" => RestirReuse::None,
                    "temporal" => RestirReuse::Temporal,
                    "spatial" => RestirReuse::Spatial,
                    "both" => RestirReuse::Both,
                    _ => return Err("reuse must be none, temporal, spatial or both".into()),
                }
            }
            "--cell-size" => {
                options.cell_size = value.parse().map_err(|_| "--cell-size needs a number")?
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if output.extension().and_then(|x| x.to_str()) != Some("png") {
        return Err("output must end in .png".into());
    }
    let scene = RayScene::from_project(&root)?;
    let camera = camera(&root)?;
    let gpu = Gpu::headless(BackendChoice::from_env()).map_err(|e| e.to_string())?;
    let mut lighting = RayLighting::with_shaders(&gpu, &scene, lean_shaders)?;
    let traced = lighting.render(&camera, &options, mode)?;
    let mut preview = image::RgbImage::new(options.width, options.height);
    let srgb = |linear: f32| {
        let mapped = linear.max(0.0) / (1.0 + linear.max(0.0));
        let encoded = if mapped <= 0.0031308 {
            12.92 * mapped
        } else {
            1.055 * mapped.powf(1.0 / 2.4) - 0.055
        };
        (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    for (pixel, color) in preview.pixels_mut().zip(&traced.radiance) {
        *pixel = image::Rgb([srgb(color[0]), srgb(color[1]), srgb(color[2])]);
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    preview.save(&output).map_err(|e| e.to_string())?;
    let linear = output.with_extension("linear.f32");
    let mut bytes = Vec::with_capacity(traced.radiance.len() * 16);
    for pixel in &traced.radiance {
        for value in pixel {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(&linear, bytes).map_err(|e| e.to_string())?;
    let report = output.with_extension("json");
    std::fs::write(
        &report,
        serde_json::to_vec_pretty(&traced.stats).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let frames = &traced.stats.frames;
    let count = frames.len() as f64;
    let total_rays: u64 = frames
        .iter()
        .map(|f| {
            u64::from(f.update_trace_rays)
                + u64::from(f.update_shadow_rays)
                + u64::from(f.render_trace_rays)
                + u64::from(f.render_shadow_rays)
        })
        .sum();
    let cache_hits: u64 = frames.iter().map(|f| u64::from(f.cache_hits)).sum();
    let mean_gpu_ms = frames.iter().all(|f| f.gpu_render_ms.is_some()).then(|| {
        frames
            .iter()
            .map(|f| {
                f.gpu_render_ms.unwrap_or(0.0)
                    + f.gpu_update_ms.unwrap_or(0.0)
                    + f.gpu_resolve_ms.unwrap_or(0.0)
            })
            .sum::<f64>()
            / count
    });
    println!(
        "{}",
        json!({"adapter": traced.stats.adapter, "mode": mode, "preview": output, "preview_transform": "Reinhard then sRGB",
        "linear_rgba_f32_le": linear, "report": report, "frames": frames.len(), "mean_frame_wall_ms": frames.iter().map(|f| f.wall_ms).sum::<f64>() / count,
        "mean_frame_gpu_ms": mean_gpu_ms, "total_rays_including_update_and_shadows": total_rays, "cache_hits": cache_hits,
        "last_frame": frames.last(), "gpu_errors": traced.stats.gpu_errors,
        "quality_note": if mode == TraceMode::Restir {
            "Fresh RIS baseline; temporal/spatial reuse is a biased static prototype, full ReSTIR PT is not implemented"
        } else { "A finite static diffuse comparison; cache confidence does not imply convergence" }})
    );
    Ok(())
}
