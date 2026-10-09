//! The levels-of-detail benchmark (docs/spec/lod.md, docs/bench/lod.md): a field of dense
//! procedural meshes (`demo::lod_field`: bumpy rocks of 81,920 triangles and torus knots of 49,152
//! at the default detail) seen low over a corner, drawn offscreen with levels of detail on or off.
//!
//! `cargo run --release -p pocket-render --example lod_field -- [--n 48] [--spacing 6]
//!  [--detail 6] [--size 1280x720] [--t 0] [--fly] [--frames 120] [--warm 30] [--lod on|off]
//!  [--pixels 1] [--occlusion off|on|auto] [--capture OUT.png]`
//!
//! Prints one JSON object: how long making the meshes and their levels took, the levels, frame wall
//! time (submit to GPU idle), CPU encoding and timestamped GPU time with its passes, and what the
//! last frame drew per level (read back from the indirect arguments). `--fly` moves the camera from
//! the corner toward the middle over the measured frames; `--capture` saves the frame drawn after
//! the measurement at camera position `--t`.

use std::time::Instant;

use pocket_render::{BackendChoice, Gpu, LodMode, OcclusionMode, Renderer, demo};
use serde_json::json;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
    arg(name).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn summary(v: &mut [f64]) -> serde_json::Value {
    v.sort_by(f64::total_cmp);
    let n = v.len().max(1);
    json!({
        "mean": round(v.iter().sum::<f64>() / n as f64),
        "p50": round(v.get(v.len() / 2).copied().unwrap_or(0.0)),
        "p95": round(v.get((v.len() * 95 / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0)),
    })
}

fn main() {
    let n: u32 = number("--n", 48);
    let spacing: f32 = number("--spacing", 6.0);
    let detail: u32 = number("--detail", 6);
    let (w, h) = arg("--size")
        .and_then(|s| {
            s.split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        })
        .unwrap_or((1280u32, 720u32));
    let frames: u32 = number("--frames", 120);
    let warm: u32 = number("--warm", 30).max(3);
    let t: f32 = number("--t", 0.0);
    let fly = flag("--fly");

    let start = Instant::now();
    let mut raw = demo::lod_meshes(detail);
    let model_ms = start.elapsed().as_secs_f64() * 1000.0;
    // What the importer adds to a mesh: its levels and vertex order (pocket_assets::lod).
    let mut build_ms = Vec::new();
    for m in &mut raw {
        let start = Instant::now();
        pocket_assets::lod::build(m, &pocket_assets::lod::LodOptions::default());
        build_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let model = demo::lod_asset(raw);
    let meshes: Vec<serde_json::Value> = model
        .meshes
        .iter()
        .zip(&build_ms)
        .map(|(m, ms)| {
            json!({
                "name": m.name,
                "vertices": m.vertices.len(),
                "radius": m.bounds.radius,
                "triangles": std::iter::once(m.triangles())
                    .chain(m.lods.iter().map(|l| l.indices.len() / 3))
                    .collect::<Vec<_>>(),
                "errors": m.lods.iter().map(|l| (f64::from(l.error) * 1e5).round() / 1e5).collect::<Vec<_>>(),
                "lod_build_ms": round(*ms),
            })
        })
        .collect();

    let gpu = Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&gpu, format, w, h);
    let mut lod = pocket_render::LodSettings::from_env();
    if let Some(m) = arg("--lod").and_then(|s| LodMode::parse(&s)) {
        lod.mode = m;
    }
    if let Some(p) = arg("--pixels").and_then(|s| s.parse().ok()) {
        lod.pixels = p;
    }
    r.set_lod_settings(lod);
    if let Some(m) = arg("--occlusion").and_then(|s| OcclusionMode::parse(&s)) {
        r.set_occlusion(m);
    }
    let start = Instant::now();
    r.add_model(demo::LOD_MODEL, &model);
    r.apply(demo::lod_field(n, spacing), 0.0);
    let upload_ms = start.elapsed().as_secs_f64() * 1000.0;

    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let mut wall = Vec::new();
    let mut cpu = Vec::new();
    let mut gpu_ms = Vec::new();
    let mut passes: Vec<(&'static str, f64, u32)> = Vec::new();
    let mut first_frame_ms = 0.0;
    // The same moment every frame (no interpolation in flight): only the camera moves.
    let now = 1.0;
    for i in 0..warm + frames {
        let ct = if fly && i >= warm {
            (i - warm) as f32 / frames.max(2).saturating_sub(1) as f32
        } else if fly {
            0.0
        } else {
            t
        };
        r.set_camera_override(Some(demo::lod_field_camera(n, spacing, ct)));
        let start = Instant::now();
        let stats = r.render(&view, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if i == 0 {
            first_frame_ms = ms;
        }
        if i < warm {
            continue;
        }
        wall.push(ms);
        cpu.push(f64::from(stats.cpu_ms));
        if !stats.passes.is_empty() {
            gpu_ms.push(f64::from(stats.gpu_ms));
        }
        for (l, ms) in &stats.passes {
            match passes.iter_mut().find(|(k, _, _)| k == l) {
                Some(e) => {
                    e.1 += f64::from(*ms);
                    e.2 += 1;
                }
                None => passes.push((l, f64::from(*ms), 1)),
            }
        }
    }
    let counts = r.draw_counts();
    let set = |s: &pocket_render::lod::SetCounts| {
        json!({
            "instances": s.instances,
            "triangles": s.triangles,
            "total_triangles": s.triangles(),
        })
    };
    let report = json!({
        "bench": "lod_field",
        "backend": r.last.backend,
        "adapter": gpu.info.name,
        "draw_path": r.draw_path(),
        "size": [w, h],
        "n": n,
        "instances": n * n,
        "spacing": spacing,
        "detail": detail,
        "lod": r.last.lod,
        "pixels": lod.pixels,
        "occlusion": r.last.occlusion,
        "fly": fly,
        "t": t,
        "model_ms": round(model_ms),
        "upload_ms": round(upload_ms),
        "first_frame_ms": round(first_frame_ms),
        "meshes": meshes,
        "frames": frames,
        "wall_ms": summary(&mut wall),
        "cpu_ms": summary(&mut cpu),
        "gpu_ms": summary(&mut gpu_ms),
        "passes_ms": passes
            .iter()
            .map(|(l, s, c)| (l.to_string(), json!(round(s / f64::from(*c)))))
            .collect::<serde_json::Map<_, _>>(),
        "draw_calls": r.last.draw_calls,
        "last_frame": {
            "camera": set(&counts.camera),
            "late": set(&counts.late),
            "shadow_triangles": counts.shadow_triangles(),
            "cascades": counts.cascades.iter().map(set).collect::<Vec<_>>(),
        },
        "list_bytes": r.list_bytes(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if let Some(path) = arg("--capture") {
        r.set_camera_override(Some(demo::lod_field_camera(n, spacing, t)));
        let mut last = Vec::new();
        for _ in 0..3 {
            last = r.capture_rgba(now).2;
        }
        image::save_buffer(&path, &last, w, h, image::ColorType::Rgba8).expect("png");
        eprintln!("saved {path}");
    }
}
