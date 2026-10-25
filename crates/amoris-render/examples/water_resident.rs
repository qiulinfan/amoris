//! A resident compute-loop benchmark, not yet the game tick: no full field readback, 64 point
//! queries per frame, 60 Hz submission. Prints submission cost, observed sample age and backpressure.
//! cargo run --release -p amoris-render --example water_resident -- --frames 180
use amoris_render::{
    Gpu,
    water_compute::GpuWater,
    water_samples::{SampleStamp, WaterSamples},
};
use std::time::{Duration, Instant};

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p) as usize]
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let frames = args
        .windows(2)
        .find(|a| a[0] == "--frames")
        .map(|a| a[1].parse::<u64>().unwrap())
        .unwrap_or(180);
    assert!(frames > 10);
    let gpu = Gpu::headless(amoris_render::host_backend()).unwrap();
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("resident water validation: {e}")
    }));
    eprintln!(
        "resident core only: {} ({:?}), 2 substeps, 64 samples, paced at 60 Hz",
        gpu.info.name, gpu.info.backend
    );
    println!(
        "side,submit_p50_ms,submit_p95_ms,age_p50_ticks,age_p95_ticks,max_age_ticks,completed,backpressure,readback_bytes_per_batch"
    );
    for side in [128, 256, 512] {
        let mut water = GpuWater::new(&gpu.device, &gpu.queue, [side, side]).unwrap();
        let columns: Vec<_> = (0..side * side)
            .map(|n| [0.0, if n % side < side / 2 { 2.0 } else { 0.0 }, 0.0, 0.0])
            .collect();
        water.upload(&columns, &vec![1; columns.len()]).unwrap();
        let mut samples = WaterSamples::new(&gpu.device, &gpu.queue, 64).unwrap();
        let points: Vec<_> = (0..64)
            .map(|i| {
                [
                    (i % 8) as f32 * side as f32 / 8.0 + 0.5,
                    (i / 8) as f32 * side as f32 / 8.0 + 0.5,
                ]
            })
            .collect();
        let (mut cpu, mut age, mut stalled) = (Vec::new(), Vec::new(), 0);
        let mut deadline = Instant::now();
        for tick in 1..=frames {
            gpu.device.poll(wgpu::PollType::Poll).unwrap();
            for batch in samples.take_ready() {
                batch.columns.unwrap();
                age.push((tick - batch.stamp.tick) as f64);
            }
            let start = Instant::now();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            water
                .encode(&mut encoder, 1.0, 1.0 / 120.0, 2, 0.02)
                .unwrap();
            if samples
                .request(
                    &mut encoder,
                    &water,
                    &points,
                    SampleStamp { epoch: 1, tick },
                )
                .unwrap()
                .is_none()
            {
                stalled += 1;
            }
            gpu.queue.submit([encoder.finish()]);
            samples.after_submit();
            if tick > 10 {
                cpu.push(start.elapsed().as_secs_f64() * 1e3);
            }
            deadline += Duration::from_secs_f64(1.0 / 60.0);
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
        // Only the final drain waits. It is outside submission timings and occurs once per run.
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        for batch in samples.take_ready() {
            batch.columns.unwrap();
            age.push((frames + 1 - batch.stamp.tick) as f64);
        }
        assert!(!age.is_empty(), "no GPU sample completed");
        let completed = age.len();
        let maximum = age.iter().copied().fold(0.0, f64::max);
        let submit50 = percentile(&mut cpu, 0.5);
        let submit95 = percentile(&mut cpu, 0.95);
        let age50 = percentile(&mut age, 0.5);
        let age95 = percentile(&mut age, 0.95);
        println!(
            "{side},{submit50:.4},{submit95:.4},{age50:.0},{age95:.0},{maximum:.0},{completed},{stalled},1024"
        );
    }
}
