//! Wet/dry accuracy study of the unchanged production MAC kernel, not a performance harness.
//! Damped translating parabola, independently substituted into the shallow-water PDE.
#[path = "common/depth_oracle.rs"]
mod depth_oracle;
#[path = "common/depth_transport.rs"]
mod depth_transport;
use amoris_assets::water_field::FieldCheckpoint;
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater};
use serde_json::json;
const GRAVITY: f64 = 9.81;
const DEPTH: f64 = 0.5;
const RADIUS: f64 = 8.0;
const DRAG: f64 = 0.2;
const SHIFT: f64 = 0.75;
const LENGTH: f64 = 32.0;
const WIDTH: f64 = 8.0;
fn motion(t: f64, shift: f64) -> (f64, f64) {
    let omega2 = 2.0 * GRAVITY * DEPTH / (RADIUS * RADIUS);
    let omega = (omega2 - DRAG * DRAG * 0.25).sqrt();
    let (s, c) = (omega * t).sin_cos();
    let decay = (-DRAG * t * 0.5).exp();
    (
        shift * decay * (c + DRAG * 0.5 / omega * s),
        -shift * decay * omega2 / omega * s,
    )
}
fn bed(x: f64) -> f64 {
    DEPTH * x * x / (RADIUS * RADIUS)
}
fn height(x: f64, t: f64, shift: f64) -> f64 {
    let q = motion(t, shift).0;
    (DEPTH - bed(x - q)).max(0.0)
}
// Independently verify the analytic family, including moving wet support, before GPU comparison.
fn check_reference() {
    let eps = 1e-4;
    for t in [0.25, 1.0, 4.0, 12.0] {
        let (q, u) = motion(t, SHIFT);
        let ut = (motion(t + eps, SHIFT).1 - motion(t - eps, SHIFT).1) / (2.0 * eps);
        assert!((ut + 2.0 * GRAVITY * DEPTH * q / (RADIUS * RADIUS) + DRAG * u).abs() < 1e-9);
        for x in [-4.0, 0.0, 4.0] {
            let ht = (height(x, t + eps, SHIFT) - height(x, t - eps, SHIFT)) / (2.0 * eps);
            let hx = (height(x + eps, t, SHIFT) - height(x - eps, t, SHIFT)) / (2.0 * eps);
            assert!((ht + u * hx).abs() < 1e-9);
        }
        assert!(height(q - RADIUS - eps, t, SHIFT) == 0.0);
        assert!(height(q + RADIUS + eps, t, SHIFT) == 0.0);
    }
    // Maximum possible reference speed remains inside the kernel's constant-calm branch.
    assert!(SHIFT * (2.0 * GRAVITY * DEPTH).sqrt() / RADIUS < 0.5);
}
fn front(depths: &[f64], dx: f64, threshold: f64) -> [f64; 2] {
    let first = depths.iter().position(|h| *h > threshold).unwrap();
    let last = depths.iter().rposition(|h| *h > threshold).unwrap();
    let position = |i: usize| -LENGTH * 0.5 + (i as f64 + 0.5) * dx;
    let left = if first > 0 {
        let weight = (threshold - depths[first - 1]) / (depths[first] - depths[first - 1]);
        position(first - 1) + dx * weight
    } else {
        position(first)
    };
    let right = if last + 1 < depths.len() {
        let weight = (depths[last] - threshold) / (depths[last] - depths[last + 1]);
        position(last) + dx * weight
    } else {
        position(last)
    };
    [left, right]
}
fn study(gpu: &Gpu, nx: u32, dt: f64, shift: f64, mode: &str) -> serde_json::Value {
    let dx = LENGTH / f64::from(nx);
    let nz = (WIDTH / dx) as u32;
    let initial = FieldCheckpoint::new(
        [nx, nz],
        0.0,
        (0..nx * nz)
            .map(|n| {
                let x = -LENGTH * 0.5 + (f64::from(n % nx) + 0.5) * dx;
                [
                    (bed(x) as f32).to_bits(),
                    (height(x, 0.0, shift) as f32).to_bits(),
                    0,
                    0,
                ]
            })
            .collect(),
    );
    let volume = |cp: &FieldCheckpoint| {
        cp.columns
            .iter()
            .map(|c| f64::from(f32::from_bits(c[1])))
            .sum::<f64>()
            * dx
            * dx
    };
    let initial_volume = volume(&initial);
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
    field.upload_checkpoint(&initial).unwrap();
    let mut research =
        (mode != "production").then(|| depth_transport::Field::new(gpu, &initial, mode));
    let count = (0.25 / dt).round() as u32;
    assert!((f64::from(count) * dt - 0.25).abs() < 1e-10);
    let mut trajectory = Vec::new();
    let mut profiles = Vec::new();
    let mut max_mass_error = 0.0f64;
    let mut max_speed = 0.0f64;
    let mut max_l1 = 0.0f64;
    let mut max_linf = 0.0f64;
    let mut max_front_error = [0.0f64; 2];
    for sample in 0..=128 {
        if sample > 0 {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let mut remaining = count;
            while remaining > 0 {
                let batch = remaining.min(8);
                if let Some(r) = &mut research {
                    r.encode(&mut encoder, dx as f32, dt as f32, batch);
                } else {
                    field
                        .encode_maccormack(&mut encoder, dx as f32, dt as f32, batch, 0.0)
                        .unwrap();
                }
                remaining -= batch;
            }
            gpu.queue.submit([encoder.finish()]);
        }
        let t = f64::from(sample) * 0.25;
        let cp = research.as_ref().map_or_else(
            || field.checkpoint_complete(0.0).unwrap(),
            |r| r.checkpoint(),
        );
        assert!(cp.columns.iter().all(
            |c| c.iter().all(|w| f32::from_bits(*w).is_finite()) && f32::from_bits(c[1]) >= 0.0
        ));
        let xs = (0..nx)
            .map(|x| -LENGTH * 0.5 + (f64::from(x) + 0.5) * dx)
            .collect::<Vec<_>>();
        let depths = (0..nx)
            .map(|x| f64::from(f32::from_bits(cp.columns[(x + nx * (nz / 2)) as usize][1])))
            .collect::<Vec<_>>();
        let expected = xs.iter().map(|x| height(*x, t, shift)).collect::<Vec<_>>();
        let l1 = depths
            .iter()
            .zip(&expected)
            .map(|(h, e)| (h - e).abs())
            .sum::<f64>()
            * dx
            / LENGTH;
        let linf = depths
            .iter()
            .zip(&expected)
            .map(|(h, e)| (h - e).abs())
            .fold(0.0f64, f64::max);
        let mass_error = (volume(&cp) - initial_volume).abs();
        let row_start = nx * (nz / 2);
        let cell_velocity = (0..nx)
            .map(|i| {
                let left = f64::from(f32::from_bits(cp.columns[(row_start + i) as usize][2]));
                let right = if i + 1 < nx {
                    f64::from(f32::from_bits(cp.columns[(row_start + i + 1) as usize][2]))
                } else {
                    0.0
                };
                0.5 * (left + right)
            })
            .collect::<Vec<_>>();
        let reference_u = motion(t, shift).1;
        let velocity_l1 = cell_velocity
            .iter()
            .zip(&expected)
            .map(|(u, h)| (u - reference_u).abs() * h)
            .sum::<f64>()
            / expected.iter().sum::<f64>();
        let energy = depths
            .iter()
            .zip(&xs)
            .zip(&cell_velocity)
            .map(|((h, x), u)| GRAVITY * (h * bed(*x) + 0.5 * h * h) + 0.5 * h * u * u)
            .sum::<f64>()
            * dx
            * WIDTH;

        let speed = cp
            .columns
            .iter()
            .map(|c| f64::from(f32::from_bits(c[2])).abs())
            .fold(0.0f64, f64::max);
        let mut fronts = Vec::new();
        for (j, threshold) in [1e-4f64, 1e-3].iter().enumerate() {
            let actual = front(&depths, dx, *threshold);
            let q = motion(t, shift).0;
            let r = RADIUS * (1.0 - threshold / DEPTH).sqrt();
            let exact = [q - r, q + r];
            let error = (actual[0] - exact[0])
                .abs()
                .max((actual[1] - exact[1]).abs());
            max_front_error[j] = max_front_error[j].max(error);
            fronts.push(
                json!({"threshold":threshold,"actual":actual,"reference":exact,"error":error}),
            );
        }
        let dry_volume = depths
            .iter()
            .zip(&expected)
            .filter(|(_, e)| **e == 0.0)
            .map(|(h, _)| h)
            .sum::<f64>()
            * dx
            * WIDTH;
        let weight = depths.iter().sum::<f64>();
        let centroid = depths.iter().zip(&xs).map(|(h, x)| h * x).sum::<f64>() / weight;
        max_mass_error = max_mass_error.max(mass_error);
        max_speed = max_speed.max(speed);
        max_l1 = max_l1.max(l1);
        max_linf = max_linf.max(linf);
        trajectory.push(json!({"time":t,"l1_depth":l1,"linf_depth":linf,"centroid":centroid,"reference_centroid":motion(t,shift).0,"mass_error":mass_error,"energy_proxy":energy,"velocity_l1":velocity_l1,"dry_reference_volume":dry_volume,"max_speed":speed,"fronts":fronts}));
        if [0, 16, 32, 48, 64, 96, 128].contains(&sample) {
            profiles.push(json!({"time":t,"x":xs,"bed":xs.iter().map(|x| bed(*x)).collect::<Vec<_>>(),"depth":depths,"velocity":cell_velocity,"reference_depth":expected}));
        }
    }
    json!({"nx":nx,"nz":nz,"dx":dx,"dt":dt,"shift":shift,"initial_volume":initial_volume,"continuum_volume":4.0/3.0*DEPTH*RADIUS*WIDTH,"max_mass_error":max_mass_error,"max_speed":max_speed,"constant_drag_branch_preserved":max_speed<=0.5,"max_l1_depth":max_l1,"max_linf_depth":max_linf,"max_front_error":max_front_error,"trajectory":trajectory,"profiles":profiles})
}
fn cost(gpu: &Gpu) -> serde_json::Value {
    use amoris_render::profiler::GpuProfiler;
    let summary = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        json!({"median_ms":v[v.len()/2],"p95_ms":v[v.len()*95/100]})
    };
    let mut rows = Vec::new();
    for side in [128u32, 256, 512] {
        for repeat in 0..3 {
            for muscl in if repeat % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let mode = if muscl { "muscl" } else { "research-baseline" };
                let initial = FieldCheckpoint::new(
                    [side; 2],
                    0.0,
                    (0..side * side)
                        .map(|i| {
                            [
                                0,
                                if i % side < side / 2 {
                                    2.0f32.to_bits()
                                } else {
                                    0
                                },
                                0,
                                0,
                            ]
                        })
                        .collect(),
                );
                let mut field = depth_transport::Field::new(gpu, &initial, mode);
                let mut profiler = GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
                let mut wall = Vec::new();
                let mut total = Vec::new();
                let mut depth = Vec::new();
                for frame in 0..150 {
                    profiler.poll();
                    let id = profiler.frame();
                    let start = std::time::Instant::now();
                    let mut e = gpu.device.create_command_encoder(&Default::default());
                    field.encode_profiled(
                        &mut e,
                        32.0 / side as f32,
                        1.0 / 240.0,
                        1,
                        Some(&mut profiler),
                    );
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
                                .expect("same-frame timings");
                            assert_eq!(t.passes.len(), 5);
                            total.push(t.passes.iter().map(|p| f64::from(p.1)).sum());
                            depth.push(
                                t.passes
                                    .iter()
                                    .filter(|p| p.0 == "water donors" || p.0 == "water transport")
                                    .map(|p| f64::from(p.1))
                                    .sum(),
                            );
                        }
                    }
                }
                let actual = field.checkpoint();
                assert!(actual.columns.iter().all(|c| f32::from_bits(c[1]) >= 0.0
                    && c.iter().all(|v| f32::from_bits(*v).is_finite())));
                let mass = (actual
                    .columns
                    .iter()
                    .map(|c| f64::from(f32::from_bits(c[1])))
                    .sum::<f64>()
                    - f64::from(side * side))
                    * (32.0 / f64::from(side)).powi(2);
                rows.push(json!({"side":side,"mode":mode,"repeat":repeat,"wall":summary(wall),"gpu":if total.is_empty(){None}else{Some(summary(total))},"depth_stages":if depth.is_empty(){None}else{Some(summary(depth))},"mass_error":mass}));
            }
        }
    }
    json!({"scope":"standalone matched dam, five MAC passes, 120 samples after 30; alternating order; excludes world/hash/probes/checkpoints/publication/render and profiler drain; wall includes device wait","backend":gpu.backend_name(),"adapter":gpu.info.name,"rows":rows})
}

fn main() {
    check_reference();
    let args = std::env::args().collect::<Vec<_>>();
    let out = args
        .windows(2)
        .find(|a| a[0] == "--out")
        .map_or("out/water-shoreline/quality", |a| a[1].as_str());
    std::fs::create_dir_all(out).unwrap();
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("shoreline study needs hardware GPU");
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
        panic!("shoreline GPU validation: {e}")
    }));
    if args.iter().any(|a| a == "--bench") {
        std::fs::write(
            std::path::Path::new(out).join("cost.json"),
            serde_json::to_vec_pretty(&cost(&gpu)).unwrap(),
        )
        .unwrap();
        return;
    }
    if args.iter().any(|a| a == "--fixtures") {
        let data = depth_oracle::fixtures(&gpu);
        std::fs::write(
            std::path::Path::new(out).join("fixture.json"),
            serde_json::to_vec_pretty(&data).unwrap(),
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(out).join("depth.wgsl"),
            format!(
                "{}\n{}",
                include_str!("../shaders/water_compute.wgsl"),
                include_str!("../shaders/water_depth_first_order.wgsl")
            ),
        )
        .unwrap();
        return;
    }
    let mut mode = if args.iter().any(|a| a == "--muscl") {
        "muscl"
    } else if args
        .iter()
        .any(|a| a == "--research-baseline" || a == "--raw-pressure" || a == "--thin-pressure")
    {
        "research-baseline"
    } else {
        "production"
    }
    .to_owned();
    if args.iter().any(|a| a == "--raw-pressure") {
        mode.push_str("-raw");
    }
    if args.iter().any(|a| a == "--thin-pressure") {
        mode.push_str("-thin");
    }
    let mut runs = Vec::new();
    for nx in [32, 64, 128] {
        for dt in [0.01, 0.005] {
            let row = study(&gpu, nx, dt, SHIFT, &mode);
            println!(
                "nx={nx} dt={dt}: L1={} front={} mass={} speed={}",
                row["max_l1_depth"],
                row["max_front_error"],
                row["max_mass_error"],
                row["max_speed"]
            );
            runs.push(row);
        }
        runs.push(study(&gpu, nx, 0.01, 0.0, &mode));
    }
    let data = json!({"scope":"selected production or isolated experimental depth/pressure kernel; point-sampled damped translating parabola, not official SWASHES parameters; maps are diagnostics, not timings","mode":mode,"reference":"https://arxiv.org/html/1110.0288#S4.SS2.SSS3","adapter":gpu.info.name,"backend":gpu.backend_name(),"H":DEPTH,"radius":RADIUS,"drag":DRAG,"initial_shift":SHIFT,"runs":runs});
    std::fs::write(
        std::path::Path::new(out).join("shoreline.json"),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
}
