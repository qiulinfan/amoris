//! The fluid experiment (docs/spikes/fluid-box.md): a block of water released in a glass box, a dam
//! break, then sloshed side to side, simulated by MLS-MPM on the GPU (`amoris_render::fluid`).
//!
//! - `cargo run --release -p amoris-render --example fluid_box`: a window; the arrow keys push the
//!   water (tilt the box), R starts again, right drag looks around.
//! - `... --example fluid_box -- --shots out/fluid-box`: headless, a PNG every 30 frames for 10 s
//!   with a slosh from 4 s, and the solver's time per frame.
//! - `--cells N` scales the box (N cells across, 64 by default); `--substeps K` (10; 0 holds the
//!   water still, which times the drawing alone); `--spheres` draws the particles instead of the
//!   surface.

use amoris_assets::frame::{
    CameraView, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_render::app::{Host, RunOptions, run};
use amoris_render::fluid::{FluidSpec, FluidTuning};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The box: `n` cells across, three quarters as high and deep, 2.5 cm a cell; the water fills its
/// left third to two thirds of its height.
fn spec(n: u32, substeps: u32) -> FluidSpec {
    let grid = [n, n * 3 / 4, n * 3 / 4];
    let cell_m = 1.6 / n as f32;
    FluidSpec {
        origin: [
            -0.5 * grid[0] as f32 * cell_m,
            0.0,
            -0.5 * grid[2] as f32 * cell_m,
        ],
        cell_m,
        grid,
        block_lo: [3.0, 3.0, 3.0],
        block_hi: [
            grid[0] as f32 * 0.36,
            grid[1] as f32 * 0.7,
            grid[2] as f32 - 3.0,
        ],
        spacing: 0.5,
        substeps,
        tuning: FluidTuning::default(),
    }
}

fn model(id: u64, mesh: &str, pos: [f32; 3], scale: [f32; 3], color: [f32; 4]) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: pos,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }),
        look: Some(Look {
            mesh: mesh.into(),
            material: String::new(),
            color,
            metallic: 0.0,
            roughness: if color[3] < 1.0 { 0.05 } else { 0.8 },
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: color[3] >= 1.0,
            visible: true,
        }),
        anim: None,
    }
}

/// The table, the box's base and its glass walls, a sun and the sky.
fn scene(s: &FluidSpec) -> RenderFrame {
    let size = [0, 1, 2].map(|a| s.grid[a] as f32 * s.cell_m);
    // The walls stand where the solver's walls are, three cells in.
    let inset = 3.0 * s.cell_m;
    let (w, h, d) = (size[0] - 2.0 * inset, size[1], size[2] - 2.0 * inset);
    let glass = [0.75, 0.85, 0.9, 0.12];
    let t = 0.01;
    let instances = vec![
        model(
            1,
            "plane",
            [0.0, -0.001, 0.0],
            [8.0, 1.0, 8.0],
            [0.42, 0.38, 0.33, 1.0],
        ),
        model(
            2,
            "cube",
            [0.0, inset - 0.01, 0.0],
            [w + 2.0 * t, 0.02, d + 2.0 * t],
            [0.2, 0.2, 0.22, 1.0],
        ),
        model(
            3,
            "cube",
            [-w / 2.0 - t / 2.0, h / 2.0, 0.0],
            [t, h, d],
            glass,
        ),
        model(
            4,
            "cube",
            [w / 2.0 + t / 2.0, h / 2.0, 0.0],
            [t, h, d],
            glass,
        ),
        model(
            5,
            "cube",
            [0.0, h / 2.0, -d / 2.0 - t / 2.0],
            [w, h, t],
            glass,
        ),
        model(
            6,
            "cube",
            [0.0, h / 2.0, d / 2.0 + t / 2.0],
            [w, h, t],
            glass,
        ),
    ];
    RenderFrame {
        tick: 1,
        t_s: 0.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        lights: Some(vec![LightView {
            id: 10,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(-0.35, -1.0, -0.45).normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 8.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]),
        cameras: Some(vec![CameraView {
            id: 11,
            position: [1.9, 1.5, 2.2],
            rotation: glam::Quat::from_rotation_y(0.7)
                .mul_quat(glam::Quat::from_rotation_x(-0.42))
                .to_array(),
            fov_deg: 45.0,
            near: 0.05,
            far: 0.0,
            active: true,
            exposure_ev: 0.0,
        }]),
        environment: Some(EnvironmentView {
            sky: 0,
            sky_color: [0.35, 0.5, 0.75],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.5,
            bloom: 0.05,
        }),
        ..RenderFrame::default()
    }
}

/// The slosh: a push to and fro along x from `start` frames on, for `frames` frames.
fn slosh(frame: u64, start: u64, frames: u64) -> [f32; 2] {
    if frame < start || frame >= start + frames {
        return [0.0; 2];
    }
    let t = (frame - start) as f32 / 60.0;
    [3.0 * (t * std::f32::consts::TAU * 0.55).sin(), 0.0]
}

struct Box3 {
    spec: FluidSpec,
    started: bool,
    keys: std::collections::HashSet<String>,
}

impl Host for Box3 {
    fn update(&mut self, r: &mut Renderer, now: f64) {
        if !self.started {
            r.apply(scene(&self.spec), now);
            r.set_fluid(Some(self.spec));
            self.started = true;
        }
        if self.keys.remove("KeyR") {
            r.set_fluid(Some(self.spec));
        }
        if let Some(f) = r.fluid.as_mut() {
            let k = |name: &str| if self.keys.contains(name) { 1.0 } else { 0.0 };
            f.push = [
                4.0 * (k("ArrowRight") - k("ArrowLeft")),
                4.0 * (k("ArrowDown") - k("ArrowUp")),
            ];
        }
    }

    fn key(&mut self, key: &str, pressed: bool) {
        if pressed {
            self.keys.insert(key.to_owned());
        } else if key != "KeyR" {
            self.keys.remove(key);
        }
    }

    fn status(&self) -> Option<&str> {
        Some("arrows push the water, R restarts, right drag looks")
    }
}

fn shots(dir: &str, spec: FluidSpec, spheres: bool) {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("a GPU");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.apply(scene(&spec), 0.0);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(1.55, 1.25, 1.9),
        Vec3::new(0.0, 0.3, 0.0),
    )));
    r.set_fluid(Some(spec));
    if let Some(f) = r.fluid.as_mut() {
        f.spheres = spheres;
    }
    println!(
        "{} particles, grid {:?}, {} substeps a frame",
        r.fluid.as_ref().map_or(0, |f| f.count()),
        spec.grid,
        spec.substeps
    );
    std::fs::create_dir_all(dir).expect("the output directory");
    let mut times = Vec::new();
    for frame in 0..600u64 {
        if let Some(f) = r.fluid.as_mut() {
            f.push = slosh(frame, 240, 240);
        }
        let t = std::time::Instant::now();
        let (w, h, rgba) = r.capture_rgba(frame as f64 / 60.0);
        times.push(t.elapsed().as_secs_f64() * 1e3);
        if frame % 30 == 0 {
            let path = format!("{dir}/frame-{frame:03}.png");
            image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("a PNG");
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "frame with readback: median {:.2} ms, p95 {:.2} ms",
        times[times.len() / 2],
        times[times.len() * 95 / 100]
    );
}

/// Warnings and errors (wgpu's validation errors among them) to stderr.
fn log_init() {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Warn
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
    log::set_max_level(log::LevelFilter::Warn);
}

fn main() {
    log_init();
    let n: u32 = arg("--cells").and_then(|s| s.parse().ok()).unwrap_or(64);
    let substeps: u32 = arg("--substeps").and_then(|s| s.parse().ok()).unwrap_or(10);
    let spec = spec(n, substeps);
    if let Some(dir) = arg("--shots") {
        shots(&dir, spec, std::env::args().any(|a| a == "--spheres"));
        return;
    }
    let host = Box3 {
        spec,
        started: false,
        keys: Default::default(),
    };
    let options = RunOptions {
        title: "Amoris — fluid box".into(),
        fly_camera: Some(CameraState::look_at(
            Vec3::new(1.55, 1.25, 1.9),
            Vec3::new(0.0, 0.3, 0.0),
        )),
        ..RunOptions::default()
    };
    if let Err(e) = run(host, options) {
        eprintln!("{e}");
    }
}
