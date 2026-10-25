//! Native numerical quality evidence; raw JSON/profiles belong under ignored out/.
#[path = "../tests/common/water_advection.rs"]
mod experiment;
use amoris_render::Gpu;
fn gpu_cost(gpu: &Gpu, side: u32, modern: bool) -> serde_json::Value {
    use amoris_render::{profiler::GpuProfiler, water_compute::GpuWater};
    let mut profiler = GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
    let cell_m = 128.0 / side as f32;
    if !profiler.enabled() {
        return serde_json::json!({"side":side,"maccormack":modern,"skipped":"GPU timestamps unavailable"});
    }
    let initial: Vec<_> = (0..side * side)
        .map(|n| {
            [
                0.0,
                2.0,
                if n % side == 0 {
                    0.0
                } else {
                    ((f64::from(n / side) + 0.5) * f64::from(cell_m) * std::f64::consts::TAU / 8.0)
                        .sin() as f32
                        * 0.4
                },
                if n / side == 0 { 0.0 } else { 1.0 },
            ]
        })
        .collect();
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, [side, side]).unwrap();
    field.upload(&initial, &vec![1; initial.len()]).unwrap();
    let mut totals = vec![];
    let mut passes = std::collections::BTreeMap::<String, Vec<f32>>::new();
    for n in 0..80 {
        profiler.poll();
        let frame = profiler.frame();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if modern {
            field
                .encode_maccormack_profiled(&mut encoder, cell_m, 0.01, 1, 0.0, Some(&mut profiler))
                .unwrap();
        } else {
            field
                .encode_profiled(&mut encoder, cell_m, 0.01, 1, 0.0, Some(&mut profiler))
                .unwrap();
        }
        profiler.resolve(&mut encoder);
        gpu.queue.submit([encoder.finish()]);
        profiler.after_submit();
        profiler.drain(&gpu.device);
        let timing = profiler
            .arrived
            .iter()
            .find(|t| t.frame == frame)
            .expect("current frame GPU timings must arrive");
        assert_eq!(timing.passes.len(), if modern { 5 } else { 3 });
        if n >= 20 {
            let mut stages = std::collections::BTreeMap::<String, f32>::new();
            for (name, ms) in &timing.passes {
                assert!(ms.is_finite() && *ms >= 0.0);
                *stages.entry((*name).into()).or_default() += ms;
            }
            totals.push(stages.values().sum::<f32>());
            for (name, ms) in stages {
                passes.entry(name).or_default().push(ms);
            }
        }
    }
    let summary = |mut values: Vec<f32>| {
        values.sort_by(f32::total_cmp);
        serde_json::json!({"median_ms":values[values.len()/2],"p95_ms":values[values.len()*95/100]})
    };
    let stages: std::collections::BTreeMap<_, _> = passes
        .into_iter()
        .map(|(name, values)| (name, summary(values)))
        .collect();
    serde_json::json!({"side":side,"cell_m":cell_m,"dt":0.01,"maccormack":modern,"substeps":1,"samples":totals.len(),
        "total":summary(totals),"stages":stages,"resolution_ns":profiler.resolution_ns()})
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let out = args
        .windows(2)
        .find(|a| a[0] == "--out")
        .map(|a| std::path::PathBuf::from(&a[1]));
    let gpu =
        Gpu::headless(amoris_render::host_backend()).expect("advection requires a hardware GPU");
    gpu.device
        .on_uncaptured_error(std::sync::Arc::new(|e| panic!("advection validation: {e}")));
    if args.iter().any(|a| a == "--cost") {
        let costs: Vec<_> = [128, 256, 512]
            .into_iter()
            .flat_map(|side| [false, true].map(|modern| gpu_cost(&gpu, side, modern)))
            .collect();
        let result = serde_json::json!({"adapter":gpu.info.name,"backend":format!("{:?}",gpu.info.backend),"costs":costs});
        if let Some(out) = out {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(
                out.join("cost.json"),
                serde_json::to_vec_pretty(&result).unwrap(),
            )
            .unwrap();
        }
        println!("{result}");
        return;
    }
    let first = experiment::shear(&gpu, 128, 1.0, false, 120).unwrap();
    let modern = experiment::shear(&gpu, 128, 1.0, true, 120).unwrap();
    let fine = experiment::shear(&gpu, 256, 0.5, false, 120).unwrap();
    let finest = experiment::shear(&gpu, 512, 0.25, false, 120).unwrap();
    let result = serde_json::json!({"adapter":gpu.info.name,"backend":format!("{:?}",gpu.info.backend),"first_order":first,"maccormack":modern,"first_order_256":fine,"first_order_512":finest});
    if let Some(out) = out {
        std::fs::create_dir_all(&out).unwrap();
        let mut profile = String::from("z,reference,first_order,maccormack\n");
        for (a, b) in first.profile.iter().zip(&modern.profile) {
            profile.push_str(&format!("{},{},{},{}\n", a[0], a[1], a[2], b[2]));
        }
        std::fs::write(out.join("shear.csv"), profile).unwrap();
        std::fs::write(
            out.join("summary.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }
    println!("{result}");
}
