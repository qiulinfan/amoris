//! Export the renderer's raw atmosphere for a matching CPU probe bake.
//! cargo run --release -p pocket-render --example sky_radiance_cube -- PROJECT
//!   [--scene sky-source.json] --output sky.json

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn main() {
    if let Err(error) = run() {
        eprintln!("sky_radiance_cube: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "import")))]
fn main() {
    eprintln!("sky_radiance_cube requires native target and the import feature");
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn run() -> Result<(), String> {
    use pocket_render::gi::bake::RayScene;
    use pocket_render::gi::sky_radiance::{MAX_SKY_JSON_BYTES, SkyRadianceCube};
    use pocket_render::{BackendChoice, Gpu};
    use std::path::PathBuf;

    const USAGE: &str =
        "usage: sky_radiance_cube PROJECT [--scene sky-source.json] --output sky.json";
    let mut args = std::env::args().skip(1);
    let project = args.next().ok_or(USAGE)?;
    if project == "--help" || project == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    if project.starts_with('-') {
        return Err(USAGE.into());
    }
    let mut scene_file = "scene.json".to_string();
    let mut output = None;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err(format!("duplicate option {flag}"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match flag.as_str() {
            "--scene" => scene_file = value,
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {flag}; {USAGE}")),
        }
    }
    let output = output.ok_or("--output FILE is required")?;
    let start = std::time::Instant::now();
    let scene = RayScene::for_path_tracing_file(&PathBuf::from(project), &scene_file)?;
    let gpu = Gpu::headless(BackendChoice::Metal).map_err(|e| e.to_string())?;
    let cube = SkyRadianceCube::export_scene(&gpu, &scene)?;
    let bytes = serde_json::to_vec(&cube).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_SKY_JSON_BYTES {
        return Err("serialized sky exceeds 32 MiB".into());
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&output, &bytes).map_err(|e| format!("{}: {e}", output.display()))?;
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [0.0_f32; 3];
    for pixel in &cube.texels {
        for channel in 0..3 {
            minimum[channel] = minimum[channel].min(pixel[channel]);
            maximum[channel] = maximum[channel].max(pixel[channel]);
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "output": output.display().to_string(),
            "scene": scene_file,
            "format": cube.format,
            "adapter": gpu.info.name,
            "backend": gpu.backend_name(),
            "size": cube.size,
            "texels": cube.texels.len(),
            "bytes": bytes.len(),
            "ambient": cube.ambient,
            "source": cube.source,
            "radiance_min": minimum,
            "radiance_max": maximum,
            "elapsed_seconds": start.elapsed().as_secs_f64(),
            "gpu_validation": "passed",
        })
    );
    Ok(())
}
