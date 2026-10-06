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
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
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
            pocket_render::CameraState::look_at(Vec3::new(100.0, 90.0, 100.0), Vec3::new(0.0, -10.0, 0.0))
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
    let count: usize = arg("--count").and_then(|s| s.parse().ok()).unwrap_or(1_600_000);
    let dense = flag("--dense");
    let shadows = flag("--shadows");
    let bench: Option<u32> = arg("--bench").and_then(|s| s.parse().ok());
    let t = std::time::Instant::now();
    let frame = pocket_render::demo::many_cubes(count, dense, shadows);
    println!("built {} cubes in {:.0} ms", count, t.elapsed().as_secs_f64() * 1000.0);
    if let Some(path) = arg("--capture") {
        // Headless: draw a few frames offscreen and save the last.
        let gpu = pocket_render::Gpu::headless(BackendChoice::from_env()).expect("gpu");
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1600, 900);
        let mut host = Cubes { frame: Some(frame), rot: Quat::IDENTITY, dense };
        for i in 0..5 {
            host.update(&mut r, i as f64 / 60.0);
            let _ = r.capture_rgba(i as f64 / 60.0);
        }
        let (w, h, px) = r.capture_rgba(0.1);
        image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8).expect("png");
        println!("saved {path} ({w}x{h}); {:?}", r.last);
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
    match run(Cubes { frame: Some(frame), rot: Quat::IDENTITY, dense }, options) {
        Ok(Some(r)) => println!("{r:#?}"),
        Ok(None) => {}
        Err(e) => eprintln!("error: {e}"),
    }
}

fn env_logger_init() {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info && !m.target().starts_with("wgpu") && !m.target().starts_with("naga")
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
