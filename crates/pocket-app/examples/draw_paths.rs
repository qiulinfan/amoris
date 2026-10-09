//! Draws the same frame through both indirect draw paths of the renderer and compares them: a
//! device with every feature the adapter offers (one multi-draw per view and variant, batch bases
//! in `first_instance`) against one without what `--minimal` leaves out (by default WebGPU's
//! defaults: no optional features, default limits, no timestamps; the batch bases then go through
//! a dynamic-offset uniform). wgpu-core's indirect-call validation is on for the second device, so
//! a draw with a nonzero `first_instance` is dropped there as a browser drops it.
//!
//! ```sh
//! cargo run --release -p pocket-app --example draw_paths -- [--out DIR] [--width 960]
//!     [--height 540] [--ticks 120] [--minimal features,limits,timestamps] mixed cubes samples/anim
//! ```
//!
//! Scenes: `mixed` (demo::mixed: every pipeline variant, shadows), `cubes` (many_cubes' sphere
//! with shadows), or a project directory (the game runs `--ticks` ticks headless and its render
//! feed's merged frame is drawn from the scene's camera). Prints one JSON line per scene and writes
//! `<scene>-full.png`, `<scene>-baseline.png`, `<scene>-diff.png` and `draw_paths.json` to `--out`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_assets::Feed;
use pocket_assets::frame::RenderFrame;
use pocket_render::gpu::Minimal;
use pocket_render::loader::FileAssets;
use pocket_render::{BackendChoice, CameraState, Gpu, Renderer, demo};
use pocket_runtime::{Extractor, Game, Project};
use serde_json::{Value, json};

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// What to draw: a first frame, an optional model to register, the camera, the asset root.
struct Source {
    frame: RenderFrame,
    model: Option<(&'static str, pocket_assets::mesh::ModelAsset)>,
    camera: Option<CameraState>,
    root: Option<PathBuf>,
}

fn source(scene: &str, ticks: u64) -> Result<Source, String> {
    match scene {
        "mixed" => Ok(Source {
            frame: demo::mixed(8),
            model: Some((demo::MIXED_MODEL, demo::mixed_model())),
            camera: Some(demo::mixed_camera(8)),
            root: None,
        }),
        "cubes" => Ok(Source {
            frame: demo::many_cubes(100_000, false, true),
            model: None,
            camera: Some(demo::cubes_camera(240, false)),
            root: None,
        }),
        dir => {
            let root = Path::new(dir)
                .canonicalize()
                .map_err(|e| format!("{dir}: {e}"))?;
            let project = Project::load(&root).map_err(|p| p.message.clone())?;
            let setup = Arc::new(project.setup(false).map_err(|p| p.message.clone())?);
            let mut game =
                Game::new(setup, project.manifest.seed).map_err(|p| p.message.clone())?;
            let feed = Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::new();
            game.present(&mut extractor, &feed);
            for _ in 0..ticks {
                game.step().map_err(|p| p.message.clone())?;
                game.present(&mut extractor, &feed);
            }
            Ok(Source {
                frame: mailbox.take(),
                model: None,
                camera: None,
                root: Some(root),
            })
        }
    }
}

struct Shot {
    rgba: Vec<u8>,
    visible: Vec<(u64, f32)>,
    info: Value,
}

fn draw(src: &Source, minimal: Minimal, size: (u32, u32)) -> Result<Shot, String> {
    let gpu = Gpu::headless_with(BackendChoice::from_env(), minimal).map_err(|e| e.to_string())?;
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    if let Some(root) = &src.root {
        r.splats.set_root(root.clone());
        r.set_asset_source(Box::new(FileAssets::new(root.clone())));
    }
    if let Some((path, model)) = &src.model {
        r.add_model(path, model);
    }
    r.apply(src.frame.clone(), 0.0);
    r.set_camera_override(src.camera);
    // Load what the frame asks for (bounded), then draw at one fixed moment so both devices
    // interpolate and animate identically.
    let t = 1.0 / 120.0;
    let mut frames = 0;
    let _ = r.capture_rgba(t);
    while r.last.pending_assets > 0 && frames < 400 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = r.capture_rgba(t);
        frames += 1;
    }
    r.request_visible();
    let mut visible = None;
    for _ in 0..20 {
        let _ = r.capture_rgba(t);
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
    }
    let (_, _, rgba) = r.capture_rgba(t);
    let s = &r.last;
    Ok(Shot {
        rgba,
        visible: visible.unwrap_or_default(),
        info: json!({
            "adapter": gpu.info.name,
            "backend": s.backend,
            "indirect_first_instance": gpu.caps.indirect_first_instance,
            "multi_draw_indirect": gpu.caps.multi_draw_indirect,
            "timestamps": gpu.caps.timestamps,
            "draw_path": r.draw_path(),
            "instances": s.instances,
            "meshes": s.meshes,
            "materials": s.materials,
            "pending_assets": s.pending_assets,
            "load_frames": frames,
        }),
    })
}

fn main() {
    if let Err(e) = run() {
        eprintln!("draw_paths: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(value(&args, "--out").unwrap_or_else(|| "draw_paths".into()));
    let w: u32 = value(&args, "--width").map_or(Ok(960), |s| s.parse().map_err(|_| "--width"))?;
    let h: u32 = value(&args, "--height").map_or(Ok(540), |s| s.parse().map_err(|_| "--height"))?;
    let ticks: u64 =
        value(&args, "--ticks").map_or(Ok(120), |s| s.parse().map_err(|_| "--ticks"))?;
    let minimal = Minimal::parse(
        &value(&args, "--minimal").unwrap_or_else(|| "features,limits,timestamps".into()),
    );
    let mut scenes = Vec::new();
    let mut skip = false;
    for a in &args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = true;
        } else {
            scenes.push(a.clone());
        }
    }
    if scenes.is_empty() {
        scenes = vec!["mixed".into(), "cubes".into()];
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut report = Vec::new();
    for scene in &scenes {
        let src = source(scene, ticks)?;
        let full = draw(&src, Minimal::default(), (w, h))?;
        let base = draw(&src, minimal, (w, h))?;
        let name = Path::new(scene)
            .file_name()
            .map_or(scene.clone(), |n| n.to_string_lossy().into_owned());
        let cmp = compare(&full, &base, (w, h), &out, &name)?;
        let line = json!({"scene": scene, "size": [w, h], "full": full.info,
            "baseline": base.info, "compare": cmp});
        println!("{line}");
        report.push(line);
    }
    std::fs::write(
        out.join("draw_paths.json"),
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Pixel and id-pass coverage differences; writes both images and an amplified difference.
fn compare(
    a: &Shot,
    b: &Shot,
    (w, h): (u32, u32),
    out: &Path,
    name: &str,
) -> Result<Value, String> {
    let mut differing = 0u64;
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut diff = Vec::with_capacity(a.rgba.len());
    for (pa, pb) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        let d = (0..3).map(|i| pa[i].abs_diff(pb[i])).max().unwrap_or(0);
        sum += u64::from(d);
        max = max.max(d);
        if d > 8 {
            differing += 1;
        }
        let v = d.saturating_mul(8);
        diff.extend_from_slice(&[v, v, v, 255]);
    }
    let pixels = u64::from(w) * u64::from(h);
    let save = |file: String, px: &[u8]| {
        image::save_buffer(out.join(file), px, w, h, image::ColorType::Rgba8)
            .map_err(|e| e.to_string())
    };
    save(format!("{name}-full.png"), &a.rgba)?;
    save(format!("{name}-baseline.png"), &b.rgba)?;
    save(format!("{name}-diff.png"), &diff)?;
    let share = |v: &[(u64, f32)], e: u64| v.iter().find(|x| x.0 == e).map_or(0.0, |x| x.1);
    let mut ids: Vec<u64> = a.visible.iter().chain(&b.visible).map(|x| x.0).collect();
    ids.sort_unstable();
    ids.dedup();
    let only_full = ids.iter().filter(|e| share(&b.visible, **e) == 0.0).count();
    let only_baseline = ids.iter().filter(|e| share(&a.visible, **e) == 0.0).count();
    let coverage_l1: f32 = ids
        .iter()
        .map(|e| (share(&a.visible, *e) - share(&b.visible, *e)).abs())
        .sum();
    Ok(json!({
        "pixels_differing_gt8": differing,
        "pixels_differing_share": differing as f64 / pixels.max(1) as f64,
        "mean_abs_diff": sum as f64 / pixels.max(1) as f64,
        "max_abs_diff": max,
        "entities_visible_full": a.visible.len(),
        "entities_visible_baseline": b.visible.len(),
        "entities_only_full": only_full,
        "entities_only_baseline": only_baseline,
        "coverage_l1": coverage_l1,
    }))
}
