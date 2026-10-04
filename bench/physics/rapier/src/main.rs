//! The Rapier side of aipocket2's physics benchmark (docs/bench/physics.md): the scenes of Jolt's
//! PerformanceTest, stepped at 1/60 s, one JSON line of per-step timings per run, matching
//! bench/physics/jolt/jolt_bench.cpp's output.
//!
//!   physics-bench --scene <Pyramid|Pyramid30|ConvexVsMesh|Raycast|Ragdoll> [--steps N]
//!                 [--threads N] [--iters N] [--data DIR] [--csv FILE] [--hash-chain FILE] [--no-sleep]
//!                 [--fork-at N] [--multibody]
//!
//! Build configurations are features: `det` (what aipocket2 ships), none, `simd8`, `parallel`.
//! `--fork-at N` (feature `serde`) snapshots the world after N steps the way pocket-physics writes
//! its Cache (serde through bincode 1.3.3, fixed-width little-endian), restores the bytes into a
//! second world, steps both to the end and reports the fork's hash beside the original's.

mod ragdoll;
mod scenes;

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use rapier3d::prelude::*;

struct Args {
    scene: String,
    steps: usize,
    threads: usize,
    iters: Option<usize>,
    data: PathBuf,
    csv: Option<PathBuf>,
    hash_chain: Option<PathBuf>,
    no_sleep: bool,
    fork_at: Option<usize>,
    multibody: bool,
}

fn args() -> Args {
    let mut a = Args {
        scene: "Pyramid".into(),
        steps: 500,
        threads: 1,
        iters: None,
        data: PathBuf::from("data"),
        csv: None,
        hash_chain: None,
        no_sleep: false,
        fork_at: None,
        multibody: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| panic!("{k} needs a value"));
        match k.as_str() {
            "--scene" => a.scene = v(),
            "--steps" => a.steps = v().parse().expect("steps"),
            "--threads" => {
                let t = v();
                a.threads = if t == "max" {
                    std::thread::available_parallelism().map_or(1, |n| n.get())
                } else {
                    t.parse().expect("threads")
                };
            }
            "--iters" => a.iters = Some(v().parse().expect("iters")),
            "--data" => a.data = PathBuf::from(v()),
            "--csv" => a.csv = Some(PathBuf::from(v())),
            "--hash-chain" => a.hash_chain = Some(PathBuf::from(v())),
            "--no-sleep" => a.no_sleep = true,
            "--multibody" => a.multibody = true,
            "--fork-at" => a.fork_at = Some(v().parse().expect("fork-at")),
            _ => panic!("unknown argument {k}"),
        }
    }
    a
}

fn config() -> String {
    let mut parts = vec![];
    if cfg!(feature = "det") {
        parts.push("enhanced-determinism");
    }
    if cfg!(feature = "simd8") {
        parts.push("simd8");
    }
    if cfg!(feature = "parallel") {
        parts.push("parallel");
    }
    if parts.is_empty() {
        parts.push("default");
    }
    let mut s = parts.join("+");
    s.push_str(if cfg!(target_arch = "wasm32") { " wasm32" } else { " native" });
    s
}

/// FNV-1a over every body's position and rotation bits, in handle order.
fn body_hash(world: &PhysicsWorld) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: f32| {
        for b in x.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (_, b) in world.bodies.iter() {
        let p = b.translation();
        let q = b.rotation();
        for v in [p.x, p.y, p.z, q.x, q.y, q.z, q.w] {
            eat(v);
        }
    }
    h
}

/// CPU time of the calling thread in ms. With one thread the whole step runs on it, so this is the
/// step's cost without the time the scheduler gave other processes (the benchmark machine is shared).
#[cfg(not(target_arch = "wasm32"))]
fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes the timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 * 1e-6
}

#[cfg(target_arch = "wasm32")]
fn thread_cpu_ms() -> f64 {
    0.0
}

struct Summary {
    mean: f64,
    p50: f64,
    p95: f64,
    max: f64,
    total: f64,
}

fn summarize(ms: &[f64]) -> Summary {
    if ms.is_empty() {
        return Summary { mean: 0.0, p50: 0.0, p95: 0.0, max: 0.0, total: 0.0 };
    }
    let total: f64 = ms.iter().sum();
    let mut s = ms.to_vec();
    s.sort_by(f64::total_cmp);
    let pct = |p: f64| s[((p * (s.len() - 1) as f64 + 0.5) as usize).min(s.len() - 1)];
    Summary { mean: total / ms.len() as f64, p50: pct(0.5), p95: pct(0.95), max: s[s.len() - 1], total }
}

/// The world's bytes as pocket-physics' Cache writes them (crates/pocket-physics/src/cache.rs).
#[cfg(feature = "serde")]
fn snapshot_options() -> impl bincode::Options {
    use bincode::Options;
    bincode::DefaultOptions::new().with_fixint_encoding().with_little_endian().reject_trailing_bytes()
}

/// Snapshot and restore: (the restored world, its byte count, save ms, restore ms).
#[cfg(feature = "serde")]
fn fork(world: &PhysicsWorld) -> (PhysicsWorld, usize, f64, f64) {
    use bincode::Options;
    let t0 = Instant::now();
    let bytes = snapshot_options().serialize(world).expect("serialize the world");
    let t1 = Instant::now();
    let restored: PhysicsWorld = snapshot_options().deserialize(&bytes).expect("deserialize the world");
    let t2 = Instant::now();
    (restored, bytes.len(), (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3)
}

#[cfg(not(feature = "serde"))]
fn fork(_: &PhysicsWorld) -> (PhysicsWorld, usize, f64, f64) {
    panic!("--fork-at needs the `serde` feature")
}

fn main() {
    let a = args();

    #[cfg(feature = "parallel")]
    rayon::ThreadPoolBuilder::new().num_threads(a.threads).build_global().expect("rayon pool");
    #[cfg(not(feature = "parallel"))]
    assert_eq!(a.threads, 1, "--threads needs the `parallel` feature");

    let mut world = PhysicsWorld::new();
    world.integration_parameters.dt = 1.0 / 60.0;
    if let Some(n) = a.iters {
        world.integration_parameters.num_solver_iterations = n;
    }
    let setup = Instant::now();
    let mut rays = Vec::new();
    let mut filter = None;
    let mut extra = String::new();
    match a.scene.as_str() {
        "Pyramid" => scenes::pyramid(&mut world, 15),
        "Pyramid30" => scenes::pyramid(&mut world, 30),
        "ConvexVsMesh" => scenes::convex_vs_mesh(&mut world),
        "Raycast" => {
            scenes::convex_vs_mesh(&mut world);
            rays = scenes::ray_grid();
        }
        "Ragdoll" => {
            let (f, st) = ragdoll::build(&mut world, &a.data.join("ragdoll.txt"), a.no_sleep, a.multibody);
            extra = format!(
                ",\"statics\":{},\"triangles\":{},\"ragdoll_bodies\":{},\"joints\":{},\"joint_kind\":\"{}\"",
                st.statics,
                st.triangles,
                st.bodies,
                st.joints,
                if a.multibody { "multibody" } else { "impulse" }
            );
            filter = Some(f);
        }
        s => panic!("unknown scene {s}"),
    }
    // Like jolt_bench's --no-sleep: every body stays awake; the scene reports as <Scene>NoSleep.
    let scene_name = if a.no_sleep {
        assert_eq!(a.scene, "Ragdoll", "--no-sleep is for the Ragdoll scene (the others set sleeping as Jolt's do)");
        format!("{}NoSleep", a.scene)
    } else {
        a.scene.clone()
    };
    let setup_ms = setup.elapsed().as_secs_f64() * 1e3;
    let hooks: &dyn PhysicsHooks = match &filter {
        Some(f) => f,
        None => &(),
    };

    let mut step_ms = Vec::with_capacity(a.steps);
    let mut cpu_ms = Vec::with_capacity(a.steps);
    let mut ray_ms = Vec::new();
    let mut hits = 0u64;
    let mut fraction_sum = 0f64;
    let mut chain: Option<(u64, File)> =
        a.hash_chain.as_ref().map(|p| (0xcbf2_9ce4_8422_2325, File::create(p).expect("hash chain file")));
    let mut forked: Option<(PhysicsWorld, usize, f64, f64)> = None;
    for i in 0..a.steps {
        if a.fork_at == Some(i) {
            forked = Some(fork(&world));
        }
        let c0 = thread_cpu_ms();
        let t0 = Instant::now();
        world.step_with_events(hooks, &());
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        cpu_ms.push(thread_cpu_ms() - c0);
        if let Some((f, ..)) = forked.as_mut() {
            f.step_with_events(hooks, &()); // not timed
        }

        if !rays.is_empty() {
            let r0 = Instant::now();
            let qp = world.query_pipeline();
            for ray in &rays {
                if let Some((_, toi)) = qp.cast_ray(ray, 1.0, true) {
                    hits += 1;
                    fraction_sum += f64::from(toi);
                }
            }
            ray_ms.push(r0.elapsed().as_secs_f64() * 1e3);
        }

        if let Some((h, f)) = chain.as_mut() {
            let s = body_hash(&world);
            *h = (*h ^ s).wrapping_mul(0x0000_0100_0000_01b3);
            writeln!(f, "{s:016x}").expect("hash chain write");
        }
    }

    if let Some(p) = &a.csv {
        let mut f = File::create(p).expect("csv");
        writeln!(f, "step,step_ms{}", if rays.is_empty() { "" } else { ",ray_ms" }).unwrap();
        for (i, s) in step_ms.iter().enumerate() {
            match ray_ms.get(i) {
                Some(r) => writeln!(f, "{i},{s},{r}").unwrap(),
                None => writeln!(f, "{i},{s}").unwrap(),
            }
        }
    }

    let s = summarize(&step_ms);
    let warm = summarize(step_ms.get(1..).unwrap_or(&[]));
    let active = world.islands.active_bodies().count();
    // Where the dynamic bodies ended, to compare outcomes across engines (a pyramid that stands keeps
    // its top box near the initial stack height minus the 0.5 m gaps).
    let (mut top_y, mut sum_y, mut n_dyn) = (f32::MIN, 0f64, 0usize);
    for (_, b) in world.bodies.iter().filter(|(_, b)| b.is_dynamic()) {
        top_y = top_y.max(b.translation().y);
        sum_y += f64::from(b.translation().y);
        n_dyn += 1;
    }
    let mean_y = sum_y / n_dyn.max(1) as f64;
    let mut out = format!(
        "{{\"engine\":\"rapier\",\"version\":\"0.36.0\",\"deterministic\":{},\"config\":\"{}\",\"scene\":\"{}\",\"threads\":{},\"steps\":{},\
\"solver_iterations\":{},\"bodies\":{},\"colliders\":{},\"active_at_end\":{},\"setup_ms\":{:.1},\"mean_ms\":{:.4},\"p50_ms\":{:.4},\"p95_ms\":{:.4},\
\"max_ms\":{:.4},\"total_ms\":{:.2},\"first_step_ms\":{:.4},\"mean_ms_after_first\":{:.4},\"top_y\":{:.3},\"mean_y\":{:.3}{}",
        cfg!(feature = "det"),
        config(),
        scene_name,
        a.threads,
        a.steps,
        world.integration_parameters.num_solver_iterations,
        world.bodies.len(),
        world.colliders.len(),
        active,
        setup_ms,
        s.mean,
        s.p50,
        s.p95,
        s.max,
        s.total,
        step_ms.first().copied().unwrap_or(0.0),
        warm.mean,
        top_y,
        mean_y,
        extra
    );
    if a.threads == 1 && !cfg!(feature = "parallel") && !cfg!(target_arch = "wasm32") {
        let c = summarize(&cpu_ms);
        out.push_str(&format!(",\"cpu_mean_ms\":{:.4},\"cpu_p50_ms\":{:.4}", c.mean, c.p50));
    }
    if !rays.is_empty() {
        let r = summarize(&ray_ms);
        out.push_str(&format!(
            ",\"rays_per_step\":{},\"ray_mean_ms\":{:.4},\"ray_p95_ms\":{:.4},\"ray_total_ms\":{:.2},\"ray_hits\":{},\"ray_fraction_sum\":{:.3}",
            rays.len(),
            r.mean,
            r.p95,
            r.total,
            hits,
            fraction_sum
        ));
    }
    if let Some((h, _)) = &chain {
        out.push_str(&format!(",\"hash_chain\":\"0x{h:016x}\""));
    }
    if let (Some(at), Some((f, bytes, save, restore))) = (a.fork_at, &forked) {
        out.push_str(&format!(
            ",\"fork_at\":{at},\"state_bytes\":{bytes},\"save_ms\":{save:.3},\"restore_ms\":{restore:.3},\"fork_hash\":\"0x{:016x}\"",
            body_hash(f)
        ));
    }
    out.push_str(&format!(",\"hash\":\"0x{:016x}\"}}", body_hash(&world)));
    println!("{out}");
}
