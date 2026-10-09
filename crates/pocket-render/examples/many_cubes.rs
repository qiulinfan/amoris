//! The Bevy `many_cubes` stress test, rebuilt on this renderer for a like-for-like comparison
//! (docs/bench/bevy-baseline.md): 1,600,000 cubes on a sphere (or a dense grid), one mesh, one
//! material, a directional light, a camera turning at a fixed rate per frame.
//!
//! `cargo run --release -p pocket-render --example many_cubes -- [--dense] [--orbit] [--count N]
//!  [--bench FRAMES] [--shadows] [--vsync] [--occlusion off|on|auto]`
//!
//! `--orbit` lays the cubes out densely and circles them from outside (occlusion culling's
//! benchmark, docs/bench/occlusion.md); `--occlusion` overrides `POCKET_OCCLUSION`. With
//! `--headless-bench`, `--splats N` adds a cloud of N Gaussian splats on a sphere just behind the
//! cubes' (with TAA the renderer then draws the unjittered depth they test against:
//! docs/bench/taa-gtao.md).

use glam::{Quat, Vec3};
use pocket_assets::frame::RenderFrame;
use pocket_render::app::{Host, RunOptions, run};
use pocket_render::{BackendChoice, OcclusionMode, Renderer};

struct Cubes {
    frame: Option<RenderFrame>,
    rot: Quat,
    dense: bool,
    /// Circle the dense layout from outside (`--orbit`), at this frame.
    orbit: Option<u32>,
    count: usize,
    occlusion: Option<OcclusionMode>,
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

impl Host for Cubes {
    fn update(&mut self, r: &mut Renderer, now: f64) {
        if let Some(f) = self.frame.take() {
            r.apply(f, now);
            if let Some(m) = self.occlusion {
                r.set_occlusion(m);
            }
        }
        if let Some(frame) = &mut self.orbit {
            r.set_camera_override(Some(pocket_render::demo::orbit_camera(*frame, self.count)));
            *frame += 1;
            return;
        }
        // Bevy's move_camera with --benchmark: rotate about local z then x by 0.15/60 each frame;
        // the dense layout's camera stands still.
        let cam = if self.dense {
            pocket_render::CameraState::look_at(
                Vec3::new(100.0, 90.0, 100.0),
                Vec3::new(0.0, -10.0, 0.0),
            )
        } else {
            // Transform::rotate_z then rotate_x: both rotate about the world axes.
            let d = 0.15 / 60.0;
            self.rot = (Quat::from_rotation_x(d) * Quat::from_rotation_z(d) * self.rot).normalize();
            let mut c = pocket_render::CameraState::look_at(Vec3::ZERO, -Vec3::Z);
            c.rotation = self.rot;
            c
        };
        r.set_camera_override(Some(cam));
    }
}

fn main() {
    env_logger_init();
    let count: usize = arg("--count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_600_000);
    let orbit = flag("--orbit");
    let dense = flag("--dense") || orbit;
    let occlusion = arg("--occlusion").and_then(|s| OcclusionMode::parse(&s));
    let shadows = flag("--shadows");
    let bench: Option<u32> = arg("--bench").and_then(|s| s.parse().ok());
    let t = std::time::Instant::now();
    let frame = pocket_render::demo::many_cubes(count, dense, shadows);
    println!(
        "built {} cubes in {:.0} ms",
        count,
        t.elapsed().as_secs_f64() * 1000.0
    );
    if let Some(path) = arg("--capture") {
        // Headless: draw a few frames offscreen and save the last.
        let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1600, 900);
        let mut host = Cubes {
            frame: Some(frame),
            rot: Quat::IDENTITY,
            dense,
            orbit: orbit.then_some(0),
            count,
            occlusion,
        };
        for i in 0..5 {
            host.update(&mut r, i as f64 / 60.0);
            let _ = r.capture_rgba(i as f64 / 60.0);
        }
        let (w, h, px) = r.capture_rgba(0.1);
        image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).expect("png");
        println!("saved {path} ({w}x{h}); {:?}", r.last);
        return;
    }
    if let Some(frames) = arg("--headless-bench").and_then(|s| s.parse::<u32>().ok()) {
        let size = arg("--size")
            .and_then(|s| {
                s.split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            })
            .unwrap_or((1280u32, 720u32));
        let splats = arg("--splats").and_then(|s| s.parse::<usize>().ok());
        let mut host = Cubes {
            frame: Some(frame),
            rot: Quat::IDENTITY,
            dense,
            orbit: orbit.then_some(0),
            count,
            occlusion,
        };
        if let (Some(_), Some(f)) = (splats, &mut host.frame) {
            f.splats = Some(vec![pocket_assets::frame::SplatView {
                id: count as u64 + 2,
                asset: "shell".into(),
                pose: Default::default(),
                visible: true,
            }]);
        }
        headless_bench(host, size, frames, splats.map(shell));
        return;
    }
    let options = RunOptions {
        title: format!(
            "many_cubes ({count}{})",
            if orbit {
                ", orbit"
            } else if dense {
                ", dense"
            } else {
                ""
            }
        ),
        width: 1280,
        height: 720,
        backend: BackendChoice::from_env(),
        vsync: flag("--vsync"),
        fly_camera: None,
        walk: false,
        bench: bench.map(|n| (60, n)),
    };
    match run(
        Cubes {
            frame: Some(frame),
            rot: Quat::IDENTITY,
            dense,
            orbit: orbit.then_some(0),
            count,
            occlusion,
        },
        options,
    ) {
        Ok(Some(r)) => println!("{r:#?}"),
        Ok(None) => {}
        Err(e) => eprintln!("error: {e}"),
    }
}

/// `n` splats 1 to 3 m wide scattered on a sphere of radius 520 m (just behind the cubes' sphere
/// of 500 m, before the box around them), deterministic.
fn shell(n: usize) -> pocket_render::splat::SplatCloud {
    let mut x = 0x2545_f491u32;
    let mut f = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        (x >> 8) as f32 / (1u32 << 24) as f32
    };
    let raw: Vec<pocket_render::splat::RawSplat> = (0..n)
        .map(|_| {
            let d = Vec3::new(f() - 0.5, f() - 0.5, f() - 0.5).normalize_or(Vec3::Z);
            let s = 0.5 + f();
            pocket_render::splat::RawSplat {
                position: (d * 520.0).to_array(),
                scale: [s, s, s * 0.2],
                rotation: Quat::from_rotation_arc(Vec3::Z, d).to_array(),
                color: [f(), f(), f()],
                opacity: 0.3 + 0.7 * f(),
            }
        })
        .collect();
    pocket_render::splat::SplatCloud::from_raw(&raw, 0, &[])
}

/// Offscreen at `size`, no presentation: 60 warm-up frames, then `frames` measured ones, each
/// submitted and waited for (submit to GPU idle). Prints one JSON object: start-up costs, frame
/// wall time, CPU encoding time, timestamped GPU time and its passes (docs/bench/dx12.md).
/// `splats`: a cloud registered as `shell` (the host's frame draws it).
fn headless_bench(
    mut host: Cubes,
    size: (u32, u32),
    frames: u32,
    splats: Option<pocket_render::splat::SplatCloud>,
) {
    let t = std::time::Instant::now();
    let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let device_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&gpu, format, size.0, size.1);
    let renderer_ms = t.elapsed().as_secs_f64() * 1000.0;
    let splat_count = splats.as_ref().map_or(0, |c| c.splats.len());
    if let Some(c) = &splats {
        r.splats.insert("shell", c);
    }
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
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
    let mut first_frame_ms = 0.0;
    let mut wall = Vec::new();
    let mut cpu = Vec::new();
    let mut gpu_ms = Vec::new();
    let mut passes: Vec<(&'static str, f64, u32)> = Vec::new();
    // Ray-traced shadows (POCKET_RT_SHADOWS=1): measured frames that rebuilt the casters' TLAS.
    let mut rt_rebuilds = 0;
    let mut modes: Vec<(&'static str, u32)> = Vec::new();
    let mut occluded_share = Vec::new();
    let warm = 60;
    for i in 0..warm + frames {
        let now = f64::from(i) / 60.0;
        let t = std::time::Instant::now();
        host.update(&mut r, now);
        let stats = r.render(&view, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if i == 0 {
            first_frame_ms = ms;
        }
        if i < warm {
            continue;
        }
        wall.push(ms);
        cpu.push(f64::from(stats.cpu_ms));
        rt_rebuilds += u32::from(r.rt_shadow_stats().is_some_and(|s| s.rebuilt));
        match modes.iter_mut().find(|(m, _)| *m == stats.occlusion) {
            Some(e) => e.1 += 1,
            None => modes.push((stats.occlusion, 1)),
        }
        if let Some(o) = &stats.occlusion_stats {
            occluded_share.push(o.occluded_share());
        }
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
    let summary = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let n = v.len().max(1);
        let round = |x: f64| (x * 1000.0).round() / 1000.0;
        serde_json::json!({
            "mean": round(v.iter().sum::<f64>() / n as f64),
            "p50": round(v.get(v.len() / 2).copied().unwrap_or(0.0)),
            "p95": round(v.get((v.len() * 95 / 100).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0)),
        })
    };
    let occlusion = r.last.occlusion_stats.map(|o| {
        serde_json::json!({
            "frustum": o.frustum,
            "occluded": o.occluded,
            "early": o.early,
            "late": o.late,
            "frustum_triangles": o.frustum_triangles,
            "occluded_triangles": o.occluded_triangles,
        })
    });
    let report = serde_json::json!({
        "bench": if host.orbit.is_some() {
            "many_cubes orbit"
        } else if host.dense {
            "many_cubes dense"
        } else {
            "many_cubes sphere"
        },
        "occlusion_mode": r.occlusion().name(),
        "occlusion_frames": modes
            .iter()
            .map(|(m, n)| ((*m).to_owned(), serde_json::json!(n)))
            .collect::<serde_json::Map<_, _>>(),
        "occluded_triangle_share": summary(&mut occluded_share),
        "occlusion_last": occlusion,
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "driver": format!("{} {}", gpu.info.driver, gpu.info.driver_info),
        "size": [size.0, size.1],
        "antialiasing": r.antialiasing().name(),
        "gtao": r.gtao().name(),
        "splats": splat_count,
        "instances": r.last.instances,
        "frames": frames,
        "device_ms": (device_ms * 10.0).round() / 10.0,
        "renderer_new_ms": (renderer_ms * 10.0).round() / 10.0,
        "first_frame_ms": (first_frame_ms * 10.0).round() / 10.0,
        "submit_to_idle_ms": summary(&mut wall),
        "cpu_encode_ms": summary(&mut cpu),
        "gpu_ms": summary(&mut gpu_ms),
        "rt_shadows": r.rt_shadow_stats().map(|s| serde_json::json!({
            "instances": s.instances, "meshes": s.meshes, "rebuilt_frames": rt_rebuilds,
        })),
        "passes_ms": passes
            .iter()
            .map(|(l, s, n)| ((*l).to_owned(), serde_json::json!((s / f64::from(*n) * 1000.0).round() / 1000.0)))
            .collect::<serde_json::Map<_, _>>(),
    });
    println!("{report}");
}

fn env_logger_init() {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info
                && !m.target().starts_with("wgpu")
                && !m.target().starts_with("naga")
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: L = L;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}
