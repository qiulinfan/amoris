//! Small-amplitude characteristic boundary study. Raw output stays under ignored out/.
#[path = "../tests/common/water_radiation.rs"]
mod experiment;
use amoris_assets::water_field::FieldBoundary;
use amoris_render::{BackendChoice, Gpu};
fn cost(gpu: &Gpu) -> serde_json::Value {
    use amoris_assets::water_field::FieldCheckpoint;
    use amoris_render::{profiler::GpuProfiler, water_compute::GpuWater};
    let summary = |mut a: Vec<f64>| {
        if a.is_empty() {
            return serde_json::Value::Null;
        }
        a.sort_by(f64::total_cmp);
        serde_json::json!({"median_ms":a[a.len()/2],"p95_ms":a[a.len()*95/100]})
    };
    let mut rows = Vec::new();
    for side in [128u32, 256, 512] {
        for radiation in [false, true] {
            for repeat in 0..3 {
                let cell = 128.0 / side as f32;
                let initial = FieldCheckpoint::new(
                    [side; 2],
                    0.0,
                    vec![[0, 1.0f32.to_bits(), 0, 0]; (side * side) as usize],
                );
                let edge = if radiation {
                    FieldBoundary::Radiation {
                        surface_relative_m: 1.0,
                        outward_velocity_mps: 0.0,
                    }
                } else {
                    FieldBoundary::Stage {
                        surface_relative_m: 1.0,
                    }
                };
                let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
                field.upload_checkpoint(&initial).unwrap();
                let mut profiler = GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
                let mut wall = Vec::new();
                let mut total = Vec::new();
                let mut boundary = Vec::new();
                for frame in 0..150 {
                    profiler.poll();
                    let id = profiler.frame();
                    let start = std::time::Instant::now();
                    let mut e = gpu.device.create_command_encoder(&Default::default());
                    field
                        .encode_boundaries_profiled(
                            &mut e,
                            cell,
                            0.01,
                            1,
                            0.0,
                            &[edge; 4],
                            Some(&mut profiler),
                        )
                        .unwrap();
                    profiler.resolve(&mut e);
                    gpu.queue.submit([e.finish()]);
                    profiler.after_submit();
                    gpu.device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: Some(std::time::Duration::from_secs(30)),
                        })
                        .unwrap();
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    profiler.drain(&gpu.device);
                    if frame >= 30 {
                        wall.push(elapsed);
                        if profiler.enabled() {
                            let t = profiler
                                .arrived
                                .iter()
                                .find(|t| t.frame == id)
                                .expect("current boundary timing");
                            assert_eq!(t.passes.len(), 8);
                            total.push(t.passes.iter().map(|p| f64::from(p.1)).sum());
                            boundary.push(
                                t.passes
                                    .iter()
                                    .filter(|p| p.0.starts_with("water boundary"))
                                    .map(|p| f64::from(p.1))
                                    .sum(),
                            );
                        }
                    }
                }
                assert_eq!(field.checkpoint_complete(0.0).unwrap(), initial);
                assert_eq!(field.read_boundary_report().unwrap(), Default::default());
                rows.push(serde_json::json!({"side":side,"cell_m":cell,"radiation":radiation,"repeat":repeat,"wall":summary(wall),"gpu_pass_sum":summary(total),"boundary_stages":summary(boundary)}));
            }
        }
    }
    serde_json::json!({"scope":"same exact still field on both modes; all four edges open; MAC plus prediction/limiting/receipt, 8 timed passes, 120 after 30; excludes game/hash/probes/publication/render/diagnostic maps and profiler drain from wall span","adapter":gpu.info.name,"backend":gpu.backend_name(),"rows":rows})
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let out = args
        .windows(2)
        .find(|a| a[0] == "--out")
        .map_or("out/water-radiation/quality", |a| a[1].as_str());
    std::fs::create_dir_all(out).unwrap();
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("radiation needs GPU");
    gpu.device
        .on_uncaptured_error(std::sync::Arc::new(|e| panic!("radiation validation: {e}")));
    if args.iter().any(|a| a == "--bench") {
        let value = cost(&gpu);
        std::fs::write(
            std::path::Path::new(out).join("cost.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        return;
    }
    if args.iter().any(|a| a == "--fixtures") {
        let value = experiment::browser_fixtures(&gpu);
        for (name, source) in [
            (
                "base.wgsl",
                include_str!("../shaders/water_compute.wgsl").to_string(),
            ),
            (
                "mac.wgsl",
                format!(
                    "{}\n{}",
                    include_str!("../shaders/water_compute.wgsl"),
                    include_str!("../shaders/water_maccormack.wgsl")
                ),
            ),
            (
                "boundary.wgsl",
                format!(
                    "{}\n{}",
                    include_str!("../shaders/water_compute.wgsl"),
                    include_str!("../shaders/water_boundaries.wgsl")
                ),
            ),
        ] {
            std::fs::write(std::path::Path::new(out).join(name), source).unwrap();
        }
        std::fs::write(
            std::path::Path::new(out).join("fixture.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        return;
    }
    let mut rows = Vec::new();
    let mut comparisons = Vec::new();
    for cells in [32, 64, 128] {
        let control = experiment::pulse_domain(&gpu, 1, cells * 2, 64.0, FieldBoundary::Wall);
        let mut errors = Vec::new();
        for edge in [
            FieldBoundary::Wall,
            FieldBoundary::Outflow,
            FieldBoundary::Stage {
                surface_relative_m: 1.0,
            },
            FieldBoundary::Radiation {
                surface_relative_m: 1.0,
                outward_velocity_mps: 0.0,
            },
        ] {
            let r = experiment::pulse(&gpu, 1, cells, edge);
            println!(
                "cells={cells} edge={edge:?} R={} mass={}",
                r.apparent_amplitude_ratio, r.max_mass_error_m3
            );
            errors.push(experiment::return_difference(&r, &control));
            rows.push(r);
        }
        comparisons.push(serde_json::json!({"cells":cells,"double_domain_background_amplitude_ratio":control.apparent_amplitude_ratio,"wall_relative_amplitude":[1.0,(errors[1]/errors[0]).sqrt(),(errors[2]/errors[0]).sqrt(),(errors[3]/errors[0]).sqrt()]}));
        rows.push(control);
    }
    let value = serde_json::json!({"scope":"actual GPU normal wave-packet exit; maps are diagnostics, not performance","adapter":gpu.info.name,"backend":gpu.backend_name(),"rows":rows,"comparisons":comparisons});
    std::fs::write(
        std::path::Path::new(out).join("pulse.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}
