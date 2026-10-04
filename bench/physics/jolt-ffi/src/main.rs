//! Rust stepping Jolt through joltc: Jolt's Pyramid scene (1,240 boxes) built and stepped across the
//! C ABI, timed like bench/physics/jolt/jolt_bench.cpp so the two can be compared, plus the two
//! per-step costs an engine adds on top of the step: reading every body's pose back (`--sync`, as
//! pocket-physics writes Transforms after each step) and 10,000 single-ray queries (`--rays`).
//!
//!   jolt-ffi-proto [--steps N] [--threads N] [--height H] [--sync] [--rays]
//!
//! The end-of-run hash is jolt_bench's (FNV-1a over positions and rotations), so equal hashes show
//! the FFI build simulates exactly what the C++ build does. The FFI itself is in src/lib.rs.

use std::time::Instant;

use jolt_ffi_proto::{Pyramid, Quat, Vec3};

/// CPU time of the calling thread in ms (single-threaded runs: the step without time given to other
/// processes).
fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes the timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 * 1e-6
}

struct Summary {
    mean: f64,
    p95: f64,
    total: f64,
}

fn summarize(ms: &[f64]) -> Summary {
    if ms.is_empty() {
        return Summary { mean: 0.0, p95: 0.0, total: 0.0 };
    }
    let total: f64 = ms.iter().sum();
    let mut s = ms.to_vec();
    s.sort_by(f64::total_cmp);
    let p95 = s[((0.95 * (s.len() - 1) as f64 + 0.5) as usize).min(s.len() - 1)];
    Summary { mean: total / ms.len() as f64, p95, total }
}

fn main() {
    let mut steps = 500usize;
    let mut threads = 1i32;
    let mut height = 15i32;
    let mut sync = false;
    let mut rays_on = false;
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--steps" => steps = it.next().unwrap().parse().unwrap(),
            "--threads" => threads = it.next().unwrap().parse().unwrap(),
            "--height" => height = it.next().unwrap().parse().unwrap(),
            "--sync" => sync = true,
            "--rays" => rays_on = true,
            _ => panic!("unknown argument {k}"),
        }
    }

    let mut scene = Pyramid::new(height, threads);
    let mut poses = vec![(Vec3::default(), Quat::default()); scene.ids.len()];
    let (mut step_ms, mut sync_ms, mut ray_ms, mut cpu_ms) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut hits, mut fraction_sum) = (0u64, 0f64);
    for _ in 0..steps {
        let c0 = thread_cpu_ms();
        let t0 = Instant::now();
        scene.step();
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        cpu_ms.push(thread_cpu_ms() - c0);
        if sync {
            let t1 = Instant::now();
            scene.read_poses(&mut poses);
            sync_ms.push(t1.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(&poses);
        }
        if rays_on {
            let t2 = Instant::now();
            let (h, f) = scene.cast_rays();
            ray_ms.push(t2.elapsed().as_secs_f64() * 1e3);
            hits += h;
            fraction_sum += f;
        }
    }
    let hash = scene.hash();
    let bodies = scene.ids.len();
    drop(scene);

    let s = summarize(&step_ms);
    let warm = summarize(&step_ms[1.min(step_ms.len())..]);
    let mut out = format!(
        "{{\"engine\":\"jolt-via-joltc\",\"deterministic\":{},\"scene\":\"Pyramid{}\",\"threads\":{},\"steps\":{},\"bodies\":{},\"mean_ms\":{:.4},\"p95_ms\":{:.4},\"total_ms\":{:.2},\"mean_ms_after_first\":{:.4}",
        cfg!(feature = "det"),
        if height == 15 { String::new() } else { height.to_string() },
        threads,
        steps,
        bodies,
        s.mean,
        s.p95,
        s.total,
        warm.mean
    );
    if threads == 1 {
        out.push_str(&format!(",\"cpu_mean_ms\":{:.4}", summarize(&cpu_ms).mean));
    }
    if sync {
        let y = summarize(&sync_ms);
        out.push_str(&format!(",\"sync_mean_ms\":{:.4},\"sync_ns_per_body\":{:.1}", y.mean, y.mean * 1e6 / bodies as f64));
    }
    if rays_on {
        let r = summarize(&ray_ms);
        out.push_str(&format!(
            ",\"ray_mean_ms\":{:.4},\"ray_p95_ms\":{:.4},\"ray_hits\":{},\"ray_fraction_sum\":{:.3}",
            r.mean, r.p95, hits, fraction_sum
        ));
    }
    out.push_str(&format!(",\"hash\":\"0x{hash:x}\"}}"));
    println!("{out}");
}
