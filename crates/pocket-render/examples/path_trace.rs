//! Complete static surface path tracing on Metal; writes linear HDR, preview PNG and a report.
use glam::{Quat, Vec3};
use pocket_render::gi::bake::RayScene;
use pocket_render::gi::nrc::NrcConfig;
use pocket_render::gi::pt::{PathTracer, PtOptions, RayCamera};
use pocket_render::{BackendChoice, Gpu};
use serde_json::Value;
use std::path::{Path, PathBuf};

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

fn camera(root: &Path, scene_file: &str) -> Result<RayCamera, String> {
    let scene: Value =
        serde_json::from_slice(&std::fs::read(root.join(scene_file)).map_err(|e| e.to_string())?)
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
        eprintln!("path_trace: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or("usage: path_trace PROJECT [--scene scene.json --output PNG --width W --height H --samples N --frames N --bounces N --rr-start N --nee true|false --rr true|false --readback-every-frame true|false --synchronize true|false --seed N --emission-scale N --punctual-scale N --environment-scale N]")?);
    let mut options = PtOptions::default();
    let mut nrc = NrcConfig::default();
    let mut use_nrc = false;
    let mut scene_file = "scene.json".to_owned();
    let mut output = PathBuf::from("path-trace.png");
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let integer = || {
            value
                .parse::<u32>()
                .map_err(|_| format!("{flag} needs an integer"))
        };
        let boolean = || {
            value
                .parse::<bool>()
                .map_err(|_| format!("{flag} needs true or false"))
        };
        let number = || {
            value
                .parse::<f32>()
                .map_err(|_| format!("{flag} needs a number"))
        };
        match flag.as_str() {
            "--output" => output = PathBuf::from(value),
            "--scene" => scene_file = value,
            "--width" => options.width = integer()?,
            "--height" => options.height = integer()?,
            "--samples" => options.samples = integer()?,
            "--frames" => options.frames = integer()?,
            "--bounces" => options.bounces = integer()?,
            "--rr-start" => options.rr_start = integer()?,
            "--seed" => options.seed = integer()?,
            "--workgroup" => options.workgroup = integer()?,
            "--shadow-any-hit" => options.shadow_any_hit = boolean()?,
            "--nrc" => use_nrc = boolean()?,
            "--nrc-query" => nrc.querying = boolean()?,
            "--nrc-train" => nrc.training = boolean()?,
            "--nrc-batch" => nrc.batch_size = integer()?,
            "--nrc-warmup" => nrc.warmup_updates = integer()?,
            "--nrc-depth" => nrc.min_query_depth = integer()?,
            "--nrc-rate" => nrc.learning_rate = number()?,
            "--specialize-nrc" => options.specialize_nrc = boolean()?,
            "--light-change-frame" => options.light_change_frame = Some(integer()?),
            "--light-change-factor" => options.light_change_factor = number()?,
            "--nee" => options.nee = boolean()?,
            "--rr" => options.russian_roulette = boolean()?,
            "--synchronize" => options.synchronize_frames = boolean()?,
            "--readback-every-frame" => options.readback_every_frame = boolean()?,
            "--emission-scale" => options.lighting_scale[0] = number()?,
            "--punctual-scale" => options.lighting_scale[1] = number()?,
            "--environment-scale" => options.lighting_scale[2] = number()?,
            "--lighting-scale" => options.lighting_scale[3] = number()?,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if use_nrc {
        options.online_nrc = Some(nrc);
    }
    if output.extension().and_then(|x| x.to_str()) != Some("png") {
        return Err("output must end in .png".into());
    }
    let scene = RayScene::for_path_tracing_file(&root, &scene_file)?;
    let camera = camera(&root, &scene_file)?;
    let gpu = Gpu::headless(BackendChoice::Metal).map_err(|e| e.to_string())?;
    let mut tracer = PathTracer::new(&gpu, &scene)?;
    let traced = tracer.render(&camera, &options)?;
    let mut preview = image::RgbImage::new(options.width, options.height);
    let encode = |v: f32| {
        let mapped = v.max(0.0) / (1.0 + v.max(0.0));
        let srgb = if mapped <= 0.0031308 {
            12.92 * mapped
        } else {
            1.055 * mapped.powf(1.0 / 2.4) - 0.055
        };
        (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    for (p, c) in preview.pixels_mut().zip(&traced.radiance) {
        *p = image::Rgb([encode(c[0]), encode(c[1]), encode(c[2])]);
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    preview.save(&output).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(traced.radiance.len() * 16);
    for pixel in &traced.radiance {
        for value in pixel {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(output.with_extension("linear.f32"), bytes).map_err(|e| e.to_string())?;
    std::fs::write(
        output.with_extension("json"),
        serde_json::to_vec_pretty(&traced.stats).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "{}; {} camera paths; trace {:.3} ms; mean RGB {:?}",
        output.display(),
        u64::from(options.width)
            * u64::from(options.height)
            * u64::from(options.samples)
            * u64::from(options.frames),
        traced.stats.trace_wall_ms,
        traced.stats.mean_radiance
    );
    Ok(())
}
