//! Compare complete physics.water ticks (upload, GPU synchronization/readback, drying and sleep
//! included) with CPU ticks.
//! cargo run --release -p amoris-app --example gpu_water -- --ticks 120
#[path = "../src/water_compute.rs"]
mod water_compute;

use amoris_runtime::{GameSetup, Project};
use amoris_sim::{StepHooks, SystemKey, TickPhase};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
struct Timer {
    start: Option<Instant>,
    ms: f64,
}
impl StepHooks for Timer {
    fn system(&mut self, key: &SystemKey, begin: bool) {
        if key.as_str() != "physics.water" {
            return;
        }
        if begin {
            self.start = Some(Instant::now());
        } else {
            self.ms = self.start.take().unwrap().elapsed().as_secs_f64() * 1e3;
        }
    }
    fn phase(&mut self, _: TickPhase, _: bool) {}
}
fn setup(side: u32, gpu: bool, source: bool) -> Arc<GameSetup> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-dam");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    let r = &mut setup.scene.entities[1].components["WaterRegion"];
    r["solver"] = json!(if gpu { "gpu" } else { "cpu" });
    r["size"] = json!([side, side]);
    let tiles: Vec<_> = (0..side / 16)
        .flat_map(|z| {
            (0..side / 16).map(move |x| {
                let depth: Vec<_> = (0..256)
                    .map(|n| if x * 16 + n % 16 < side / 2 { 2.0 } else { 0.0 })
                    .collect();
                json!({"key": [x,z], "bed": vec![0.0;256], "depth": depth,
            "u": vec![0.0;256], "w": vec![0.0;256], "still": 0})
            })
        })
        .collect();
    r["tiles"] = json!(tiles);
    if source {
        setup.scene.entities.push(serde_json::from_value(json!({"name":"Spring","components":{
            "WaterSource":{"region":2,"at":[f64::from(side)*0.25+0.5,f64::from(side)*0.5+0.5],"rate_m3s":1.0}
        }})).unwrap());
    }
    Arc::new(setup)
}
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}
fn resident(gpu: &amoris_render::Gpu, side: u32, ticks: u32) -> Option<f64> {
    if !gpu.caps.timestamps {
        return None;
    }
    let mut field =
        amoris_render::water_compute::GpuWater::new(&gpu.device, &gpu.queue, [side, side]).unwrap();
    let columns: Vec<_> = (0..side * side)
        .map(|n| [0.0, if n % side < side / 2 { 2.0 } else { 0.0 }, 0.0, 0.0])
        .collect();
    field.upload(&columns, &vec![1; columns.len()]).unwrap();
    let mut profiler =
        amoris_render::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
    let mut samples = Vec::new();
    for tick in 0..ticks + 10 {
        profiler.poll();
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        field
            .encode_profiled(&mut encoder, 1.0, 1.0 / 120.0, 2, 0.02, Some(&mut profiler))
            .unwrap();
        profiler.resolve(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
        profiler.after_submit();
        // Outside the GPU timestamped work, including Metal's separate resolve submission.
        profiler.drain(&gpu.device);
        if tick >= 10 {
            samples.push(f64::from(profiler.total_ms()));
        }
    }
    Some(median(&mut samples))
}
fn run() {
    let args: Vec<_> = std::env::args().collect();
    let ticks: u32 = args
        .windows(2)
        .find(|a| a[0] == "--ticks")
        .map(|a| a[1].parse().unwrap())
        .unwrap_or(120);
    assert!(ticks > 0);
    let source = args.iter().any(|a| a == "--source");
    let scenario = if source { "continuous-source" } else { "dam" };
    let device = amoris_render::Gpu::headless(amoris_render::host_backend()).unwrap();
    eprintln!(
        "resident device: {} ({:?})",
        device.info.name, device.info.backend
    );
    println!("scenario,side,cpu_water_ms,gpu_water_ms,resident_gpu_pass_ms");
    for side in [128, 256, 512] {
        let mut cpu = water_compute::builder(setup(side, false, source), 1)
            .build()
            .unwrap();
        let mut gpu = water_compute::builder(setup(side, true, source), 1)
            .build()
            .unwrap();
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut timer = Timer::default();
        for tick in 0..ticks + 10 {
            // Alternate order to avoid giving either path all the cold/hot machine state.
            if tick % 2 == 0 {
                cpu.step_with(&mut timer).unwrap();
                if tick >= 10 {
                    a.push(timer.ms);
                }
                gpu.step_with(&mut timer).unwrap();
                if tick >= 10 {
                    b.push(timer.ms);
                }
            } else {
                gpu.step_with(&mut timer).unwrap();
                if tick >= 10 {
                    b.push(timer.ms);
                }
                cpu.step_with(&mut timer).unwrap();
                if tick >= 10 {
                    a.push(timer.ms);
                }
            }
        }
        let resident = resident(&device, side, ticks)
            .map(|v| format!("{v:.4}"))
            .unwrap_or_else(|| "not_checked_no_timestamps".into());
        println!(
            "{scenario},{side},{:.4},{:.4},{resident}",
            median(&mut a),
            median(&mut b)
        );
    }
}
fn main() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
