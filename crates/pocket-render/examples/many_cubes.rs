//! The Bevy `many_cubes` stress test, rebuilt on this renderer for a like-for-like comparison
//! (docs/bench/bevy-baseline.md): 1,600,000 cubes on a sphere (or a dense grid), one mesh, one
//! material, a directional light, a camera turning at a fixed rate per frame.
//!
//! `cargo run --release -p pocket-render --example many_cubes -- [--dense] [--count N]
//!  [--bench FRAMES] [--shadows] [--vsync]`

use glam::{Quat, Vec3};
use pocket_assets::frame::RenderFrame;
use pocket_render::app::{Host, RunOptions, run};
use pocket_render::{BackendChoice, Renderer};

struct Cubes {
    frame: Option<RenderFrame>,
    rot: Quat,
    dense: bool,
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
    let dense = flag("--dense");
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
        headless_bench(
            Cubes {
                frame: Some(frame),
                rot: Quat::IDENTITY,
                dense,
            },
            size,
            frames,
        );
        return;
    }
    let options = RunOptions {
        title: format!("many_cubes ({count}{})", if dense { ", dense" } else { "" }),
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
        },
        options,
    ) {
        Ok(Some(r)) => println!("{r:#?}"),
        Ok(None) => {}
        Err(e) => eprintln!("error: {e}"),
    }
}

/// Offscreen at `size`, no presentation: 60 warm-up frames, then `frames` measured ones, each
/// submitted and waited for (submit to GPU idle). Prints one JSON object: start-up costs, frame
/// wall time, CPU encoding time, timestamped GPU time and its passes (docs/bench/dx12.md).
fn headless_bench(mut host: Cubes, size: (u32, u32), frames: u32) {
    let t = std::time::Instant::now();
    let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let device_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&gpu, format, size.0, size.1);
    let renderer_ms = t.elapsed().as_secs_f64() * 1000.0;
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
    let report = serde_json::json!({
        "bench": if host.dense { "many_cubes dense" } else { "many_cubes sphere" },
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "driver": format!("{} {}", gpu.info.driver, gpu.info.driver_info),
        "size": [size.0, size.1],
        "instances": r.last.instances,
        "frames": frames,
        "device_ms": (device_ms * 10.0).round() / 10.0,
        "renderer_new_ms": (renderer_ms * 10.0).round() / 10.0,
        "first_frame_ms": (first_frame_ms * 10.0).round() / 10.0,
        "submit_to_idle_ms": summary(&mut wall),
        "cpu_encode_ms": summary(&mut cpu),
        "gpu_ms": summary(&mut gpu_ms),
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
