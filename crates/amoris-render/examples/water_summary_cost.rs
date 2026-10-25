//! Isolated summary encoding and whole GPU timestamp span; not a Game/frame benchmark.
//! Every returned digest is checked against fixed Rust BLAKE3 input outside timed GPU work.
use amoris_render::{
    Gpu, profiler::GpuProfiler, water_compute::GpuWater, water_summary::WaterSummary,
};
use serde_json::{Value, json};
use std::time::Instant;

fn stats(values: impl Iterator<Item = f64>) -> Value {
    let mut values: Vec<_> = values.collect();
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    json!({"n": values.len(), "median_ms": values[(values.len()-1)/2],
           "p95_ms": values[(values.len()-1)*95/100], "max_ms": values[values.len()-1]})
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let value = |name: &str, fallback: u32| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map_or(fallback, |pair| pair[1].parse().unwrap())
    };
    let side = value("--side", 512);
    let warmup = value("--warmup", 64);
    let ticks = value("--ticks", 128);
    assert!((2..=512).contains(&side) && ticks > 0);
    let gpu = Gpu::headless(amoris_render::host_backend()).expect("summary cost requires a GPU");
    assert!(gpu.caps.timestamps, "summary cost requires GPU timestamps");
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("summary cost validation: {e}")
    }));
    gpu.device.set_device_lost_callback(|reason, message| {
        if reason != wgpu::DeviceLostReason::Destroyed {
            panic!("summary cost device lost: {reason:?}: {message}");
        }
    });
    let columns: Vec<_> = (0..side * side)
        .map(|n| {
            [
                (n % 13) as f32 * 0.125 - 0.25,
                0.25 + (n % 7) as f32 * 0.125,
                if n % side == 0 {
                    0.0
                } else {
                    (n % 9) as f32 * 0.125
                },
                if n < side {
                    0.0
                } else {
                    (n % 5) as f32 * -0.125
                },
            ]
        })
        .collect();
    let mut expected = blake3::Hasher::new();
    expected.update(bytemuck::cast_slice(&columns));
    expected.update(&vec![0; (side * 2).div_ceil(4) as usize * 16]);
    let expected = *expected.finalize().as_bytes();
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, [side, side]).unwrap();
    field.upload(&columns, &vec![1; columns.len()]).unwrap();
    let mut summary = WaterSummary::new(&gpu.device, &gpu.queue);
    let mut profiler = GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
    profiler.set_tracing(&gpu.device, true);
    let mut previous_end = 0;
    let mut rows = Vec::with_capacity((warmup + ticks) as usize);
    for tick in 1..=warmup + ticks {
        profiler.poll();
        let frame = profiler.frame();
        assert!(profiler.times_frame());
        let start = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        summary
            .encode_profiled(&mut encoder, &field, Some(&mut profiler))
            .unwrap();
        let encode_ms = start.elapsed().as_secs_f64() * 1e3;
        // The profiler retains its existing deferred resolve lifecycle on Metal/MoltenVK.
        profiler.resolve(&mut encoder);
        let finish = Instant::now();
        let commands = encoder.finish();
        let finish_ms = finish.elapsed().as_secs_f64() * 1e3;
        let submit = Instant::now();
        gpu.queue.submit([commands]);
        let submit_ms = submit.elapsed().as_secs_f64() * 1e3;
        profiler.after_submit();
        let read = Instant::now();
        let got = summary.read().unwrap();
        let read_ms = read.elapsed().as_secs_f64() * 1e3;
        let complete_ms = start.elapsed().as_secs_f64() * 1e3;
        assert_eq!(got.digest, expected);
        // A diagnostic drain happens outside the measured CPU interval, once per iteration.
        profiler.drain(&gpu.device);
        let raw = profiler.take_raw();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].frame, frame);
        let scopes = &raw[0].scopes;
        let first = scopes.first().unwrap().begin;
        let last = scopes.last().unwrap().end;
        assert!(first > previous_end, "stale summary timestamp frame");
        assert!(last >= first);
        previous_end = last;
        let sum: u64 = scopes
            .iter()
            .map(|scope| scope.end.checked_sub(scope.begin).unwrap())
            .sum();
        let scale = f64::from(profiler.period_ns()) * 1e-6;
        rows.push(json!({"tick":tick,"frame":frame,"encode_ms":encode_ms,
            "finish_ms":finish_ms,"submit_ms":submit_ms,"read_ms":read_ms,
            "complete_ms":complete_ms,"scope_count":scopes.len(),
            "gpu_span_ms":(last-first) as f64*scale,"gpu_sum_ms":sum as f64*scale,
            "scopes":scopes.iter().map(|s|json!({"label":s.label,"begin":s.begin,
                "end":s.end})).collect::<Vec<_>>()}));
    }
    let mut result = json!({"adapter":gpu.info.name,"backend":format!("{:?}",gpu.info.backend),
        "side":side,"warmup":warmup,"ticks":ticks,"resolution_ns":profiler.resolution_ns(),
        "kernel":amoris_render::water_resident::kernel_key(),"rows":rows,
        "scope":"fixed uploaded field, summary only, timestamps enabled; no Game/solver/drawing"});
    for name in [
        "encode_ms",
        "finish_ms",
        "submit_ms",
        "read_ms",
        "complete_ms",
        "gpu_span_ms",
        "gpu_sum_ms",
    ] {
        result[name] = stats(
            rows.iter()
                .skip(warmup as usize)
                .map(|r| r[name].as_f64().unwrap()),
        );
    }
    if let Some(pair) = args.windows(2).find(|pair| pair[0] == "--out") {
        let path = std::path::Path::new(&pair[1]);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
    result.as_object_mut().unwrap().remove("rows");
    println!("{result}");
}
