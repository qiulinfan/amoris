//! Controlled water-shading and motion lab. Fields here are analytic inputs, not solver evidence.
//! cargo run --release -p amoris-render --example water_lab -- --out out/water-lab --frames 180
use amoris_assets::frame::RenderFrame;
use amoris_render::demo::water::scene;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;
use std::path::PathBuf;

const W: u32 = 1280;
const H: u32 = 720;
fn bench_foam(gpu: &Gpu, side: u32) {
    if !gpu.caps.timestamps {
        println!("{side},not_checked_no_timestamps");
        return;
    }
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("foam benchmark water"),
        size: u64::from(side) * u64::from(side) * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(
        &buffer,
        0,
        bytemuck::cast_slice(&vec![[1.0f32, 1.0, 5.0, 0.0]; (side * side) as usize]),
    );
    let mut foam =
        amoris_render::water_foam::WaterFoam::new(&gpu.device, &buffer, [side, side], 0.5, false);
    let mut profiler =
        amoris_render::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
    let mut samples = Vec::new();
    for tick in 0..100 {
        profiler.poll();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let scope = profiler.compute_scope("water foam");
        foam.encode(&gpu.queue, &mut enc, 1.0 / 60.0, false, scope);
        profiler.resolve(&mut enc);
        gpu.queue.submit([enc.finish()]);
        profiler.after_submit();
        profiler.drain(&gpu.device);
        if tick >= 20 {
            samples.push(profiler.total_ms());
        }
    }
    samples.sort_by(f32::total_cmp);
    println!("{side},{:.5}", samples[samples.len() / 2]);
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(pair) = args.windows(3).find(|a| a[0] == "--compare") {
        let a = image::open(&pair[1]).unwrap().to_rgba8();
        let b = image::open(&pair[2]).unwrap().to_rgba8();
        assert_eq!(
            a.dimensions(),
            b.dimensions(),
            "comparison requires equal capture sizes"
        );
        let mut changed = 0u64;
        let mut sum = 0u64;
        let mut maximum = 0u8;
        for (a, b) in a.pixels().zip(b.pixels()) {
            let delta = [
                a[0].abs_diff(b[0]),
                a[1].abs_diff(b[1]),
                a[2].abs_diff(b[2]),
            ];
            changed += u64::from(delta.iter().any(|&v| v > 8));
            sum += delta.iter().map(|&v| u64::from(v)).sum::<u64>();
            maximum = maximum.max(*delta.iter().max().unwrap());
        }
        println!(
            "{}",
            serde_json::json!({"width":a.width(),"height":a.height(),"changed_gt8":changed,
            "mean_abs_rgb":sum as f64 / (u64::from(a.width())*u64::from(a.height())*3) as f64,"max_abs_rgb":maximum})
        );
        return;
    }

    let tracing = args.iter().any(|a| a == "--trace");
    assert!(
        !(tracing && args.iter().any(|a| a == "--motion")),
        "run trace and motion capture separately"
    );
    let out = args
        .windows(2)
        .find(|p| p[0] == "--out")
        .map(|p| PathBuf::from(&p[1]))
        .unwrap_or_else(|| "out/water-lab".into());
    let frames: u64 = args
        .windows(2)
        .find(|p| p[0] == "--frames")
        .map(|p| p[1].parse().unwrap())
        .unwrap_or(180);
    std::fs::create_dir_all(&out).unwrap();
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
    if args.iter().any(|a| a == "--bench") {
        println!("side,foam_gpu_ms");
        for side in [128, 256, 512] {
            bench_foam(&gpu, side);
        }
        return;
    }
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("water lab"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    for case in ["pool", "rapids", "fall"] {
        let selected = args
            .windows(2)
            .find(|p| p[0] == "--case")
            .map(|p| p[1].as_str());
        if selected.is_some_and(|c| c != case) {
            continue;
        }
        for foam in if args.iter().any(|a| a == "--reverse") {
            [true, false]
        } else {
            [false, true]
        } {
            let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
            r.set_antialiasing(Antialiasing::Msaa);
            r.set_gtao(Gtao::Off);
            r.set_prepass(amoris_render::PrepassMode::Off);
            r.set_occlusion(amoris_render::OcclusionMode::Off);
            r.set_water_foam(foam);
            r.set_water_optics(
                !args.iter().any(|a| a == "--no-refraction"),
                !args.iter().any(|a| a == "--no-reflection"),
                !args.iter().any(|a| a == "--no-detail"),
            );
            let medium_view = args
                .windows(2)
                .find(|a| a[0] == "--view")
                .map(|a| a[1].as_str());
            let (eye, at) = match medium_view {
                Some("under") => (Vec3::new(12.0, 0.5, 12.0), Vec3::new(20.0, 0.6, 20.0)),
                Some("line") => (Vec3::new(12.0, 1.2, 12.0), Vec3::new(20.0, 1.3, 20.0)),
                _ => (Vec3::new(25.0, 7.0, 25.0), Vec3::new(14.0, 1.4, 16.0)),
            };
            r.set_camera_override(Some(CameraState::look_at(eye, at)));
            r.apply(scene(case), 0.0);
            if tracing {
                r.start_trace(amoris_render::trace::TraceSpec {
                    skip: frames.min(60) as u32,
                    frames: frames.saturating_sub(60).max(1) as u32,
                    out: Some(out.join(format!(
                        "{case}-{}-trace.json",
                        if foam { "foam" } else { "bare" }
                    ))),
                });
            }
            let mut previous_motion: Option<Vec<u8>> = None;
            let mut motion_deltas = Vec::new();
            let mut next_frame = std::time::Instant::now();
            for tick in 0..=frames {
                r.trace_begin_frame();
                let t = tick as f64 / 60.0;
                r.apply(
                    RenderFrame {
                        tick: tick + 2,
                        t_s: t,
                        dt_s: 1.0 / 60.0,
                        ..RenderFrame::default()
                    },
                    t,
                );
                if args.iter().any(|a| a == "--orbit") {
                    let angle = 0.7 + tick as f32 * 0.003;
                    r.set_camera_override(Some(CameraState::look_at(
                        Vec3::new(16.0 + angle.cos() * 16.0, 5.0, 16.0 + angle.sin() * 16.0),
                        Vec3::new(16.0, 1.4, 16.0),
                    )));
                }
                if args.iter().any(|a| a == "--dive") {
                    let phase = tick as f32 / frames.max(1) as f32 * std::f32::consts::TAU;
                    let y = 1.2 + 0.9 * phase.cos();
                    r.set_camera_override(Some(CameraState::look_at(
                        Vec3::new(12.0, y, 12.0),
                        Vec3::new(20.0, y + 0.1, 20.0),
                    )));
                }
                r.render(&view, t);
                if r.trace_end_frame() {
                    r.finish_trace();
                }
                if tracing {
                    let period = std::time::Duration::from_secs_f64(1.0 / 60.0);
                    next_frame += period;
                    // Clock calibration or a scheduler pause can miss a deadline. Do not catch
                    // up with an unpaced burst that exhausts timestamp readback slots.
                    let now = std::time::Instant::now();
                    if next_frame < now {
                        next_frame = now + period;
                    }
                    std::thread::sleep(
                        next_frame.saturating_duration_since(std::time::Instant::now()),
                    );
                }
                if args.iter().any(|a| a == "--motion")
                    && foam
                    && ((case == "rapids" && tick >= 60 && tick % 10 == 0)
                        || args.iter().any(|a| a == "--dive"))
                {
                    let motion = out.join("motion");
                    std::fs::create_dir_all(&motion).unwrap();
                    let (_, _, rgba) = r.capture_rgba(t);
                    if let Some(previous) = &previous_motion {
                        let sum: u64 = rgba
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .zip(previous.as_chunks::<4>().0)
                            .map(|(a, b)| {
                                (0..3).map(|c| u64::from(a[c].abs_diff(b[c]))).sum::<u64>()
                            })
                            .sum();
                        motion_deltas
                            .push(format!("{tick},{:.6}", sum as f64 / (W * H * 3) as f64));
                    }
                    image::save_buffer(
                        motion.join(format!("{tick:05}.png")),
                        &rgba,
                        W,
                        H,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    previous_motion = Some(rgba);
                }
            }
            if !motion_deltas.is_empty() {
                std::fs::write(
                    out.join("motion-deltas.csv"),
                    format!("to_frame,mean_rgb_delta\n{}\n", motion_deltas.join("\n")),
                )
                .unwrap();
            }
            for (name, eye, at) in [
                (
                    "close",
                    Vec3::new(25.0, 5.0, 24.0),
                    Vec3::new(14.0, 1.4, 16.0),
                ),
                (
                    "far",
                    Vec3::new(45.0, 30.0, 43.0),
                    Vec3::new(16.0, 1.0, 16.0),
                ),
                (
                    "grazing",
                    Vec3::new(29.0, 2.8, 25.0),
                    Vec3::new(12.0, 1.3, 15.0),
                ),
                (
                    "under",
                    Vec3::new(12.0, 0.5, 12.0),
                    Vec3::new(20.0, 0.6, 20.0),
                ),
            ] {
                r.set_camera_override(Some(CameraState::look_at(eye, at)));
                let (_, _, rgba) = r.capture_rgba(frames as f64 / 60.0);
                let path = out.join(format!(
                    "{case}-{}-{name}.png",
                    if foam { "foam" } else { "bare" }
                ));
                image::RgbaImage::from_raw(W, H, rgba)
                    .unwrap()
                    .save(&path)
                    .unwrap();
                println!("{}", path.display());
            }
        }
    }
}
