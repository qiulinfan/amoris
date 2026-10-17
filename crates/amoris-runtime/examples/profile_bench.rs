//! The tick profiler's bench (docs/bench/profiler.md): what the step hooks cost and what they
//! measure, on a project's game outside the game thread.
//!
//! - **overhead**: `ticks` ticks without hooks (`Game::step`) and `ticks` with the game thread's
//!   profiler as the loop runs it (`profile::TickTimer` on a monotonic clock, the loop's own four
//!   readings around the streams, the extraction, the snapshot and the work after the tick, and
//!   the push into a shared amoris-link ring that never waits), in alternating blocks
//!   (A B B A ...) of one world, so drift and the world's own evolution fall on both sides alike;
//!   the mean and the median block per tick of each, and the overhead of the profiled ticks.
//! - **pacing** (`--pace-ms 16.667`): a tick every that many milliseconds, the thread sleeping in
//!   slices of at most 1 ms between them as the game thread does in real time; each tick is timed
//!   by itself and the sleeps are left out. Against an unpaced run it shows what real-time pacing
//!   alone does to a tick's cost, and the hooks' overhead under it.
//! - **clock**: what one reading of the loop's clock costs through the timer (the hooks read it
//!   twice per system), and the systems run per tick.
//! - **profile**: the profiled ticks summarized as `profile.frame` answers (amoris-link's
//!   `profile`, read from the ring after each block and merged): each system's own time per
//!   tick, the phases, the tick.
//!
//! `cargo run --release -p amoris-runtime --features transpile --example profile_bench -- \
//!  [--ticks 3000] [--block 250] [--warmup 120] [--pace-ms 0] [--out out.json] <project>..`
//! Timings depend on the machine and its load: the bench doc labels them provisional.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use amoris_link::profile::push_ready;
use amoris_link::{ProfileRing, TickProfile};
use amoris_runtime::profile::TickTimer;
use amoris_runtime::{Game, GameBuilder, Project};
use serde_json::{Value, json};

struct Options {
    ticks: u64,
    block: u64,
    warmup: u64,
    /// 0: back to back; else a tick every this many milliseconds.
    pace_ms: f64,
    out: Option<PathBuf>,
    projects: Vec<PathBuf>,
}

fn options() -> Options {
    let mut o = Options {
        ticks: 3000,
        block: 250,
        warmup: 120,
        pace_ms: 0.0,
        out: None,
        projects: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = |name: &str| -> u64 {
            args.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} takes a number"))
        };
        match a.as_str() {
            "--ticks" => o.ticks = num("--ticks"),
            "--block" => o.block = num("--block").max(1),
            "--warmup" => o.warmup = num("--warmup"),
            "--pace-ms" => {
                o.pace_ms = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| panic!("--pace-ms takes a number"));
            }
            "--out" => o.out = args.next().map(PathBuf::from),
            p => o.projects.push(PathBuf::from(p)),
        }
    }
    if o.projects.is_empty() {
        o.projects
            .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing"));
    }
    o
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// `v` to `1 / k` (k = 10, 100, ...).
fn round(v: f64, k: f64) -> f64 {
    (v * k).round() / k
}

/// The summaries of consecutive blocks as one (weighted by their ticks; maxima the largest).
fn merge(parts: &[Value]) -> Value {
    let ticks: u64 = parts.iter().map(|p| p["ticks"].as_u64().unwrap_or(0)).sum();
    let n = ticks.max(1) as f64;
    let w = |p: &Value| p["ticks"].as_u64().unwrap_or(0) as f64;
    let rows = |key: &str| -> Vec<Value> {
        let mut out: Vec<(String, f64, f64)> = Vec::new();
        for p in parts {
            for r in p[key].as_array().into_iter().flatten() {
                let name = r["name"].as_str().unwrap_or("").to_owned();
                let ms = r["ms"].as_f64().unwrap_or(0.0) * w(p);
                let max = r["max_ms"].as_f64().unwrap_or(0.0);
                match out.iter_mut().find(|o| o.0 == name) {
                    Some(o) => {
                        o.1 += ms;
                        o.2 = o.2.max(max);
                    }
                    None => out.push((name, ms, max)),
                }
            }
        }
        out.into_iter()
            .map(|(name, ms, max)| json!({"name": name, "ms": round(ms / n, 1e4), "max_ms": max}))
            .collect()
    };
    let avg = |key: &str| {
        parts
            .iter()
            .map(|p| p[key].as_f64().unwrap_or(0.0) * w(p))
            .sum::<f64>()
            / n
    };
    let max = |key: &str| {
        parts
            .iter()
            .map(|p| p[key].as_f64().unwrap_or(0.0))
            .fold(0.0, f64::max)
    };
    json!({
        "ticks": ticks,
        "tick_ms": round(avg("tick_ms"), 1e4),
        "tick_max_ms": max("tick_max_ms"),
        "busy_ms": round(avg("busy_ms"), 1e4),
        "systems": rows("systems"),
        "phases": rows("phases"),
    })
}

fn bench(dir: &Path, o: &Options) -> Value {
    let project = Project::load(dir).unwrap_or_else(|e| panic!("{}: {e:#?}", dir.display()));
    let seed = project.manifest.seed;
    let name = project.manifest.name.clone();
    let setup = Arc::new(project.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut g: Game = GameBuilder::new(setup)
        .seed(seed)
        .project(dir.to_path_buf())
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"));
    g.run(o.warmup).unwrap_or_else(|e| panic!("{e:#?}"));
    let start = Instant::now();
    let clock: amoris_runtime::profile::ProfileClock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let mut timer = TickTimer::new(clock);
    // What one reading of the loop's clock costs, through the timer.
    let reads = 1_000_000u32;
    let t0 = Instant::now();
    let mut sink = 0.0;
    for _ in 0..reads {
        sink += timer.now();
    }
    let clock_ns = t0.elapsed().as_secs_f64() * 1e9 / f64::from(reads);
    std::hint::black_box(sink);
    let ring = Mutex::new(ProfileRing::default());
    let mut ready: Vec<TickProfile> = Vec::new();
    let mut systems_run = 0usize;
    let mut reuse: Option<TickProfile> = None;
    let mut after = 0.0;
    let mut cursor = 1;
    let mut parts: Vec<Value> = Vec::new();
    let (mut plain, mut hooked) = (Vec::new(), Vec::new());
    let blocks = o.ticks.div_ceil(o.block);
    let paced = o.pace_ms > 0.0;
    let mut due = Instant::now();
    for b in 0..2 * blocks {
        // A B B A A B B A ...: each variant leads half the pairs.
        let with_hooks = matches!(b % 4, 1 | 2);
        let mut busy = Duration::ZERO;
        let t0 = Instant::now();
        for _ in 0..o.block {
            if paced {
                due += Duration::from_secs_f64(o.pace_ms / 1000.0);
                wait_until(due);
            }
            let tick0 = Instant::now();
            if with_hooks {
                timer.begin(reuse.take());
                let began = timer.now();
                g.step_with(&mut timer).unwrap_or_else(|e| panic!("{e:#?}"));
                let ended = timer.now();
                let t = timer.tick_mut();
                t.tick = g.tick().0;
                t.tick_ms = ended - began;
                systems_run += t.systems.len();
                // The loop's readings after the step (streams, extraction, snapshot: nothing
                // runs between them here) and its push.
                let streamed = timer.now();
                let presented = timer.now();
                let published = timer.now();
                let t = timer.tick_mut();
                t.present_ms = presented - streamed;
                t.publish_ms = (streamed - ended) + (published - presented);
                t.after_ms = after;
                ready.push(timer.take());
                if let Some(old) = push_ready(&ring, &mut ready) {
                    reuse = Some(old);
                }
                after = timer.now() - published;
            } else {
                g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            }
            if paced {
                busy += tick0.elapsed();
            }
        }
        let spent = if paced { busy } else { t0.elapsed() };
        let us = spent.as_secs_f64() * 1e6 / o.block as f64;
        if with_hooks {
            hooked.push(us);
            // Outside the timed block: the block's ticks, before the ring drops them.
            let r = ring
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            parts.push(r.since(&mut cursor, 0));
        } else {
            plain.push(us);
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let (a, b) = (mean(&plain), mean(&hooked));
    let (ma, mb) = (median(plain.clone()), median(hooked.clone()));
    let systems = merge(&parts);
    let per_tick = systems_run as f64 / (blocks * o.block) as f64;
    json!({
        "project": name,
        "ticks_each": blocks * o.block,
        "block": o.block,
        "warmup": o.warmup,
        "pace_ms": o.pace_ms,
        "plain_us_per_tick": round(a, 100.0),
        "hooked_us_per_tick": round(b, 100.0),
        "overhead_pct": round((b - a) / a * 100.0, 100.0),
        "plain_block_median_us": round(ma, 100.0),
        "hooked_block_median_us": round(mb, 100.0),
        "median_overhead_pct": round((mb - ma) / ma * 100.0, 100.0),
        "clock_ns": round(clock_ns, 10.0),
        "systems_per_tick": round(per_tick, 100.0),
        // Two readings per system, two around the step and the loop's four after it.
        "readings_per_tick": round(2.0 * per_tick + 6.0, 100.0),
        "plain_blocks_us": plain.iter().map(|v| round(*v, 10.0)).collect::<Vec<_>>(),
        "hooked_blocks_us": hooked.iter().map(|v| round(*v, 10.0)).collect::<Vec<_>>(),
        "profile": systems,
    })
}

/// Waits for `due` in slices of at most 1 ms, as the game thread's `wait_until` does.
fn wait_until(due: Instant) {
    loop {
        let now = Instant::now();
        if now >= due {
            return;
        }
        std::thread::sleep((due - now).min(Duration::from_millis(1)));
    }
}

fn main() {
    let o = options();
    let run = move || {
        let runs: Vec<Value> = o.projects.iter().map(|p| bench(p, &o)).collect();
        let report = json!({"provisional": true, "runs": runs});
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        println!("{text}");
        if let Some(path) = &o.out {
            std::fs::write(path, text + "\n").unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    };
    // Scripts need the script host's stack (script-sandbox.md 4.3).
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench ran");
}
