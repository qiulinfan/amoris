//! Controlled geometry cost, not fluid-solving cost. Render complete matching inputs with
//! culling off/on; explicit counter/capture readbacks occur after the timed frames.
use amoris_render::{Antialiasing, BackendChoice, Gpu, Gtao, Renderer, demo::water};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn summary(mut samples: Vec<f64>) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::Value::Null;
    }
    samples.sort_by(f64::total_cmp);
    json!({"n":samples.len(),"median_ms":samples[samples.len()/2],"p95_ms":samples[samples.len()*95/100]})
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |key: &str| args.windows(2).find(|p| p[0] == key).map(|p| p[1].clone());
    let out = PathBuf::from(value("--out").unwrap_or_else(|| "out/water-mesh/measure".into()));
    let compare_lod = args.iter().any(|a| a == "--compare-lod");
    let sweep = args.iter().any(|a| a == "--sweep");
    let trace = args.iter().any(|a| a == "--trace");
    let frames: usize = value("--frames").map(|v| v.parse().unwrap()).unwrap_or(120);
    let side: u32 = value("--side").map(|v| v.parse().unwrap()).unwrap_or(512);
    let field = value("--field").unwrap_or_else(|| "full".into());
    let camera = value("--camera").unwrap_or_else(|| "near".into());
    let repeats: usize = value("--repeats").map(|v| v.parse().unwrap()).unwrap_or(3);
    let settle: f64 = value("--settle").map(|v| v.parse().unwrap()).unwrap_or(0.0);
    assert!(settle.is_finite() && (0.0..=60.0).contains(&settle));
    std::fs::create_dir_all(&out).unwrap();
    let setup_start = Instant::now();
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("hardware GPU");
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("water mesh GPU validation: {e}")
    }));
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("water geometry benchmark"),
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_water_foam(false);
    r.set_prepass(amoris_render::PrepassMode::Off);
    r.set_occlusion(amoris_render::OcclusionMode::Off);
    r.set_lod(amoris_render::LodMode::Off);
    let pose = water::mesh_camera(side, &camera);
    r.set_camera_override(Some(pose));
    r.apply(water::mesh_scene(side, &field), 0.0);
    if let Some(count) = value("--motion").map(|n| n.parse::<usize>().unwrap()) {
        assert!(count >= 3);
        let mut rows = Vec::new();
        r.set_water_culling(true);
        for on in [false, true] {
            r.set_lod(if on {
                amoris_render::LodMode::On
            } else {
                amoris_render::LodMode::Off
            });
            r.apply(water::mesh_scene(side, &field), 0.0);
            for frame in 0..count {
                let phase = frame as f32 / (count - 1) as f32 * std::f32::consts::TAU;
                let factor = 0.59 + 0.41 * phase.cos();
                let centre = glam::Vec3::new(side as f32 * 0.125, 0.0, side as f32 * 0.125);
                let mut camera = water::mesh_camera(side, "full");
                camera.position = centre + (camera.position - centre) * factor;
                r.set_camera_override(Some(camera));
                let (_, _, pixels) = r.capture_rgba(frame as f64 / 60.0);
                image::save_buffer(
                    out.join(format!("motion-{on}-{frame:03}.png")),
                    &pixels,
                    1280,
                    720,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                let counts = r.water_mesh_stats().unwrap();
                assert!(
                    counts
                        .iter()
                        .all(|(_, s)| s.max_error_pixels <= r.lod().pixels * 1.0001)
                );
                rows.push(
                    json!({"lod":on,"frame":frame,"eye":camera.position.to_array(),"mesh":counts}),
                );
            }
        }
        std::fs::write(
            out.join("motion.json"),
            format!("{}\n", serde_json::to_string_pretty(&rows).unwrap()),
        )
        .unwrap();
        return;
    }
    let setup_ms = setup_start.elapsed().as_secs_f64() * 1000.0;
    let mut result = Vec::new();
    for repeat in 0..repeats {
        for on in if repeat % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            if compare_lod {
                r.set_water_culling(true);
                r.set_lod(if on {
                    amoris_render::LodMode::On
                } else {
                    amoris_render::LodMode::Off
                });
            } else {
                r.set_water_culling(on);
            }
            let mut wall = Vec::new();
            let mut cpu = Vec::new();
            let mut warmup = Vec::new();
            let mut motion = Vec::new();
            let mut rows = Vec::new();
            let trace_file = trace.then(|| out.join(format!("trace-{repeat}-{on}.json")));
            if let Some(path) = &trace_file {
                r.start_trace(amoris_render::trace::TraceSpec {
                    frames: frames.try_into().expect("trace frame count fits u32"),
                    skip: 30,
                    out: Some(path.clone()),
                });
            }
            for frame in 0..frames + 30 {
                if sweep {
                    let phase =
                        frame.saturating_sub(30) as f32 / frames as f32 * std::f32::consts::TAU;
                    let factor = 0.59 + 0.41 * phase.cos();
                    let centre = glam::Vec3::new(side as f32 * 0.125, 0.0, side as f32 * 0.125);
                    let mut c = pose;
                    c.position = centre + (pose.position - centre) * factor;
                    r.set_camera_override(Some(c));
                }
                // Clock calibration and trace export stay outside the render/wait interval.
                if trace {
                    r.trace_begin_frame();
                }
                let start = Instant::now();
                let stats = r.render(&target, if sweep { frame as f64 / 60.0 } else { 0.0 });
                gpu.device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(std::time::Duration::from_secs(30)),
                    })
                    .unwrap();
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                if trace {
                    r.trace_end_frame();
                }
                if frame >= 30 {
                    if sweep {
                        // Diagnostic mapping follows the measured render/wait interval.
                        motion.push(json!({"frame":frame-30,"wall_ms":elapsed_ms,"mesh":r.water_mesh_stats().unwrap()}));
                    }
                    wall.push(elapsed_ms);
                    cpu.push(f64::from(stats.cpu_ms));
                    rows.push(json!({"frame":frame-30,"wall_ms":elapsed_ms,"cpu_ms":stats.cpu_ms}));
                } else {
                    warmup.push(json!({"frame":frame,"wall_ms":elapsed_ms,"cpu_ms":stats.cpu_ms}));
                }
                if frame == 0 && settle > 0.0 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(settle));
                }
            }
            if trace {
                assert_eq!(r.finish_trace().as_ref(), trace_file.as_ref());
            }
            let counts = if on || compare_lod {
                Some(r.water_mesh_stats().unwrap())
            } else {
                None
            };
            let (_, _, pixels) = r.capture_still(0.0);
            let png = out.join(format!("{field}-{camera}-{side}-{repeat}-{on}.png"));
            image::save_buffer(png, &pixels, 1280, 720, image::ColorType::Rgba8).unwrap();
            // FrameStats exposes the latest asynchronous GPU snapshot, which may repeat.
            // A trace retains actual source frame IDs; never treat snapshots as fresh samples.
            let row = json!({"side":side,"field":field,"camera":camera,"view":{"position":pose.position.to_array(),"rotation":pose.rotation.to_array(),"fov_y":pose.fov_y,"near":pose.near},"adapter":gpu.info.name,"backend":gpu.backend_name(),"timestamps":gpu.caps.timestamps,"setup_ms":setup_ms,"settle_seconds":settle,"warmup":warmup,"repeat":repeat,"comparison":if compare_lod{"lod"}else{"culling"},"lod":compare_lod && on,"culling":on || compare_lod,"timing_scope":"render plus explicit GPU wait; excludes solver/readback/capture", "wall":summary(wall),"cpu":summary(cpu),"gpu":null,"passes":{},"trace_file":trace_file,"rows":rows,"mesh":counts,"motion":motion});
            println!("{row}");
            result.push(row);
        }
    }
    std::fs::write(
        out.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&result).unwrap()),
    )
    .unwrap();
}
