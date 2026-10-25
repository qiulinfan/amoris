//! The drawing cost of shallow water (spike, docs/spikes/shallow-water.md): a synthetic field of
//! `N` by `N` columns of 0.5 m, waves running over a bed that leaves about a third of it dry, seen
//! from above so it fills the frame, drawn headless at 1280 x 720 with MSAA.
//!
//! ```sh
//! AMORIS_TRACE=frames=240,skip=60,out=out/profiler/shallow-256.json \
//!     cargo run --release -p amoris-render --example shallow_draw -- --n 256 [--off]
//! ```
//!
//! `--off` hands no water to the renderer (the same frames otherwise), `--away` hands it over but
//! looks at the sky (the upload without the draw); compare the traces with
//! `python tools/frame_trace.py diff`. Prints the median capture time and the host's time to build
//! and hand over each frame's columns.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_render::shallow_water::ShallowWaterView;
use amoris_render::{Antialiasing, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn scene(half: f32) -> RenderFrame {
    RenderFrame {
        tick: 1,
        t_s: 0.0,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances: vec![InstanceUpdate {
            id: 1,
            pose: Some(Pose {
                position: [0.0, -0.3, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [2.0 * half, 1.0, 2.0 * half],
            }),
            look: Some(Look {
                mesh: "plane".into(),
                material: String::new(),
                color: [0.42, 0.38, 0.33, 1.0],
                metallic: 0.0,
                roughness: 0.8,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: false,
                visible: true,
            }),
            anim: None,
        }],
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

/// The field at `t` seconds: a bed rising toward the x edges, a surface of two travelling waves.
fn field(n: u32, t: f32) -> ShallowWaterView {
    let cell = 0.5;
    let half = n as f32 * cell * 0.5;
    let mut columns = Vec::with_capacity((n * n) as usize);
    for k in 0..n {
        for i in 0..n {
            let x = (i as f32 + 0.5) * cell - half;
            let z = (k as f32 + 0.5) * cell - half;
            let bed = 1.6 * (x / half) * (x / half) - 0.3;
            let surface = 0.6 + 0.15 * (0.4 * x - 1.3 * t).sin() + 0.1 * (0.3 * z + 0.9 * t).sin();
            let depth = (surface - bed).max(0.0);
            let speed = 1.0 + 4.0 * (0.05 * z + t).sin().abs();
            columns.push([bed + depth, depth, speed, 0.3]);
        }
    }
    ShallowWaterView {
        origin: [-half, -half],
        cell_m: cell,
        nx: n,
        nz: n,
        columns,
    }
}

fn main() {
    amoris_render::stderr_log::init();
    let n: u32 = arg("--n").and_then(|s| s.parse().ok()).unwrap_or(256);
    let off = std::env::args().any(|a| a == "--off");
    let frames: u64 = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(300);
    let gpu = Gpu::headless(amoris_render::host_backend()).expect("a GPU");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    let half = n as f32 * 0.25;
    r.apply(scene(half), 0.0);
    // `--away`: the camera looks up, so the columns are handed over and uploaded but not drawn.
    let target = if std::env::args().any(|a| a == "--away") {
        Vec3::new(0.0, half * 3.0, half * 0.9 + 1.0)
    } else {
        Vec3::new(0.0, 0.0, -half * 0.1)
    };
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(0.0, half * 0.9, half * 0.9),
        target,
    )));
    let (mut capture, mut host) = (Vec::new(), Vec::new());
    for frame in 0..frames {
        let t = frame as f32 / 60.0;
        let start = std::time::Instant::now();
        if !off {
            r.set_shallow_water(Some(field(n, t)));
        }
        host.push(start.elapsed().as_secs_f64() * 1e3);
        let start = std::time::Instant::now();
        let (w, h, rgba) = r.capture_rgba(f64::from(t));
        capture.push(start.elapsed().as_secs_f64() * 1e3);
        if frame == frames - 1
            && let Some(path) = arg("--shot")
        {
            image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("a PNG");
        }
        if r.trace_end_frame() {
            r.finish_trace();
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    println!(
        "{n} x {n} columns{}: capture median {:.3} ms; host build and hand-over {:.3} ms",
        if off { " (not drawn)" } else { "" },
        median(&mut capture),
        median(&mut host)
    );
}
