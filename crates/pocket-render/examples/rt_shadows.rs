//! Cascaded against ray-traced sun shadows (docs/spec/rt-shadows.md): the same scene drawn
//! offscreen by a renderer on a device without ray queries and by one on a device with them
//! (`Gpu::headless_ray_query`, what `POCKET_RT_SHADOWS=1` asks). Prints one JSON object with each
//! path's frame timings (submit to GPU idle, CPU encoding, timestamped GPU passes) and saves a
//! capture of each, for `image_diff`.
//!
//! `cargo run --release -p pocket-render --example rt_shadows -- [--scene mixed|cubes] [--n 6]
//!  [--heroes 0] [--count 10000] [--size 1920x1080] [--frames 240] [--moving] [--capture PREFIX]`
//!
//! `mixed`: the mixed demo scene (every pipeline variant on a ground, n x n cells) under a slanted
//! sun, with `--heroes` skinned characters of samples/anim in a row in front (their bottom levels
//! are rebuilt every frame); `cubes`: many_cubes' sphere of `count` cubes with shadows. `--moving`
//! sends every frame a new tick that turns every instance, so the ray-traced path rebuilds its top
//! level each frame. `POCKET_BACKEND` and `POCKET_ADAPTER` choose the GPU.

use glam::Quat;
use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::{BackendChoice, CameraState, Gpu, Renderer, demo};
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

const HERO: &str = "models/hero.glb";

struct Scene {
    first: RenderFrame,
    camera: CameraState,
    mixed: bool,
}

fn scene() -> Scene {
    if arg("--scene").as_deref() == Some("cubes") {
        let count = arg("--count")
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_000);
        Scene {
            first: demo::many_cubes(count, false, true),
            camera: demo::cubes_camera(0, false),
            mixed: false,
        }
    } else {
        let n = arg("--n").and_then(|s| s.parse().ok()).unwrap_or(6);
        let heroes: u64 = arg("--heroes").and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut first = demo::mixed(n);
        let row = n as f32 * 1.5 + 2.0;
        for i in 0..heroes {
            first.instances.push(InstanceUpdate {
                id: 100_000 + i,
                pose: Some(Pose {
                    position: [(i as f32 - (heroes as f32 - 1.0) * 0.5) * 1.5, 0.0, row],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.6; 3],
                }),
                look: Some(Look {
                    mesh: HERO.into(),
                    material: String::new(),
                    color: [1.0; 4],
                    metallic: 0.0,
                    roughness: 0.6,
                    transmission: None,
                    ior: None,
                    emissive: [0.0; 3],
                    cast_shadows: true,
                    visible: true,
                }),
                anim: Some(AnimView {
                    clip: "idle".into(),
                    time: 0.37 * i as f32,
                    rate: 1.0,
                    looped: true,
                }),
            });
        }
        Scene {
            first,
            camera: demo::mixed_camera(n),
            mixed: true,
        }
    }
}

/// Tick `tick`: every instance of the first frame turned about y by a step per tick.
fn moved(first: &RenderFrame, tick: u64) -> RenderFrame {
    let turn = Quat::from_rotation_y(tick as f32 * 0.01);
    RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        instances: first
            .instances
            .iter()
            .filter_map(|u| {
                let pose = u.pose?;
                Some(InstanceUpdate {
                    id: u.id,
                    pose: Some(Pose {
                        rotation: (turn * Quat::from_array(pose.rotation))
                            .normalize()
                            .to_array(),
                        ..pose
                    }),
                    look: None,
                    anim: None,
                })
            })
            .collect(),
        ..RenderFrame::default()
    }
}

fn run(ray_traced: bool, size: (u32, u32), frames: u32, moving: bool) -> serde_json::Value {
    let s = scene();
    let gpu = Gpu::headless_ray_query(BackendChoice::from_env(), ray_traced).expect("gpu");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    if s.mixed {
        r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
        let hero = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/anim/models/hero.glb"
        ))
        .expect("samples/anim/models/hero.glb");
        let hero = pocket_assets::import::import_glb_bytes(&hero).expect("hero.glb imports");
        r.add_model(HERO, &hero);
    }
    r.apply(s.first.clone(), 0.0);
    r.set_camera_override(Some(s.camera));
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rt_shadows target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let warm = 30;
    let (mut wall, mut cpu, mut gpu_ms) = (Vec::new(), Vec::new(), Vec::new());
    let mut passes: Vec<(&'static str, f64, u32)> = Vec::new();
    let mut rebuilds = 0;
    for i in 0..warm + frames {
        let now = f64::from(i) / 60.0;
        let t = std::time::Instant::now();
        if moving && i > 0 {
            r.apply(moved(&s.first, u64::from(i) + 1), now);
        }
        let stats = r.render(&view, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        if i < warm {
            continue;
        }
        wall.push(t.elapsed().as_secs_f64() * 1000.0);
        cpu.push(f64::from(stats.cpu_ms));
        if !stats.passes.is_empty() {
            gpu_ms.push(f64::from(stats.gpu_ms));
        }
        rebuilds += u32::from(r.rt_shadow_stats().is_some_and(|s| s.rebuilt));
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
    let capture = arg("--capture").map(|prefix| {
        let path = format!("{prefix}-{}.png", if ray_traced { "rt" } else { "csm" });
        let (w, h, px) = r.capture_rgba(f64::from(warm + frames) / 60.0);
        image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).expect("png");
        path
    });
    let summary = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let round = |x: f64| (x * 1000.0).round() / 1000.0;
        json!({
            "mean": round(v.iter().sum::<f64>() / v.len().max(1) as f64),
            "p50": round(v.get(v.len() / 2).copied().unwrap_or(0.0)),
            "p95": round(v.get((v.len() * 95 / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0)),
        })
    };
    json!({
        "shadows": if r.rt_shadow_stats().is_some() { "ray traced" } else { "cascaded" },
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "instances": r.last.instances,
        "rt": r.rt_shadow_stats().map(|s| json!({
            "instances": s.instances, "masked": s.masked, "meshes": s.meshes, "rebuilt_frames": rebuilds,
        })),
        "submit_to_idle_ms": summary(&mut wall),
        "cpu_encode_ms": summary(&mut cpu),
        "gpu_ms": summary(&mut gpu_ms),
        "passes_ms": passes
            .iter()
            .map(|(l, s, n)| ((*l).to_owned(), json!((s / f64::from(*n) * 1000.0).round() / 1000.0)))
            .collect::<serde_json::Map<_, _>>(),
        "capture": capture,
    })
}

fn main() {
    let size = arg("--size")
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1920, 1080));
    let frames = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(240);
    let moving = flag("--moving");
    let report = json!({
        "scene": arg("--scene").unwrap_or_else(|| "mixed".into()),
        "heroes": arg("--heroes").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0),
        "size": [size.0, size.1],
        "frames": frames,
        "moving": moving,
        "cascaded": run(false, size, frames, moving),
        "ray_traced": run(true, size, frames, moving),
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
