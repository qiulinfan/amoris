//! The lookahead bench (docs/bench/player.md, "Fork lookahead"; charter 2 and 6: the benefit of
//! fork lookahead; docs/spec/budgets.md 5.1, `fork.sail`, `branch.sail`, `fork.sail.memory`):
//!
//! - **course**: on the sailing course's seeded variants (`course.rs`), under the seat's
//!   perception and bound to the omniscient profile, three decision models: the reference skipper
//!   (`greedy`), the fork lookahead around it (`lookahead`, the benchmark's fork condition: 4
//!   branches, 600 branch ticks per decision) and the same lookahead with forks off (`nofork`: the
//!   same candidates valued from the chart alone), the control that isolates what forking adds.
//!   Each episode's ticks to finish, crates aboard, decisions, acts, branch ticks, wall time and
//!   every weighed decision; `tools/runtime/lookahead_bench.py table` makes the tables. Every number but
//!   wall time is exact (deterministic).
//! - **fork**: on the course at tick 300 (the greedy skipper sailing it), the wall time of a
//!   snapshot, of a restore into a kept branch, of a fork into a kept branch (`Game::fork_into`:
//!   `fork.sail`), of a fresh fork with its own script host (`Game::fork`: `branch.sail`) and of
//!   dropping one, with the bytes each allocates and holds (a counting global allocator, this
//!   binary only) and the script heap a fresh branch holds. What a timed call returns is dropped
//!   after the timing.
//!
//! `cargo run --release -p amoris-runtime --features transpile --example lookahead_bench --
//! course|fork|probe [out.json] [--seeds A-B] [--models greedy,lookahead,nofork]`. Timings
//! depend on the machine and its load.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use amoris_link::Source;
use amoris_runtime::lookahead::{self, CRATE_TICKS, Episode, Limits, Lookahead, Model, Seat};
use amoris_runtime::skipper::Skipper;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

mod course;

/// Counts the bytes this process allocates and frees through Rust's allocator.
struct Counting;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static FREED: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards every call to the system allocator unchanged; the counters are atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's contract is System's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
        FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn counters() -> (u64, u64) {
    (
        ALLOCATED.load(Ordering::Relaxed),
        FREED.load(Ordering::Relaxed),
    )
}

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn course() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            let p = Project::load(&repo().join("samples/sailing-course"))
                .unwrap_or_else(|e| panic!("{e:#?}"));
            Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
        })
        .clone()
}

fn wall() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn dev(g: &mut Game, name: &str, params: Value) -> Value {
    g.apply(&Command::new(Source::Developer(0), 1, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// The course's variant `seed` at tick 0, under perception or omniscient.
fn variant(seed: u64, omniscient: bool) -> Game {
    course::game(course(), seed, omniscient)
}

/// The median of sorted values: the middle one, or the mean of the two middle ones.
fn median_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn stats(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / v.len() as f64;
    let r = |x: f64| (x * 100.0).round() / 100.0;
    json!({"n": v.len(), "min": r(v[0]), "median": r(median_sorted(&v)),
           "max": r(v[v.len() - 1]), "mean": r(mean), "sd": r(var.sqrt())})
}

static MARGIN: OnceLock<f64> = OnceLock::new();

fn margin() -> Option<f64> {
    MARGIN.get().copied()
}

/// The most main ticks an episode runs: ten minutes of game time.
const MAX_TICKS: u64 = 36_000;

/// The decision models the course compares.
const MODELS: [&str; 3] = ["greedy", "lookahead", "nofork"];

/// One episode: `model` is one of [`MODELS`].
fn episode(seed: u64, omniscient: bool, model: &str, limits: Limits) -> (Episode, f64) {
    let mut g = variant(seed, omniscient);
    let mut m = match model {
        "greedy" => Model::Greedy(Skipper::new()),
        _ => {
            let mut l = Lookahead::new(limits, 0);
            if let Some(m) = margin() {
                l = l.switch_margin(m);
            }
            if model == "nofork" {
                l = l.without_forks();
            }
            Model::Lookahead(Box::new(l))
        }
    };
    let t0 = Instant::now();
    let ep = lookahead::play(&mut g, &mut m, 0, MAX_TICKS, Some(wall))
        .unwrap_or_else(|e| panic!("seed {seed} {model}: {e:#?}"));
    (ep, t0.elapsed().as_secs_f64())
}

/// The score the comparison ranks by, in ticks (lower is better): ticks to finish less
/// `CRATE_TICKS` per crate aboard; an unfinished course counts its limit.
fn score(ep: &Episode) -> f64 {
    let ticks = if ep.finished { ep.ticks } else { MAX_TICKS };
    ticks as f64 - CRATE_TICKS * ep.taken
}

fn course_bench(seeds: std::ops::RangeInclusive<u64>, limits: Limits, models: &[String]) -> Value {
    let mut runs = Vec::new();
    for condition in ["perception", "omniscient"] {
        let omniscient = condition == "omniscient";
        for seed in seeds.clone() {
            for model in models {
                let (ep, wall_s) = episode(seed, omniscient, model, limits);
                eprintln!(
                    "{condition} seed {seed} {model}: finished {} ticks {} taken {} decisions {} \
                     acts {} branch ticks {} wall {wall_s:.2}s",
                    ep.finished,
                    ep.ticks,
                    ep.taken,
                    ep.decisions,
                    ep.acts,
                    ep.lookahead.as_ref().map_or(0, |s| s.branch_ticks)
                );
                runs.push(json!({"condition": condition, "seed": seed, "model": model,
                    "finished": ep.finished, "ticks": ep.ticks, "taken": ep.taken,
                    "next_mark": ep.next_mark, "decisions": ep.decisions, "acts": ep.acts,
                    "score": score(&ep), "lookahead": ep.lookahead,
                    "wall_s": (wall_s * 1000.0).round() / 1000.0,
                    "decide_s": (ep.decide_s * 1000.0).round() / 1000.0,
                    "world_hash": ep.world_hash, "weighed": ep.weighed}));
            }
        }
    }
    json!({"provisional": true, "limits": limits, "crate_ticks": CRATE_TICKS,
           "switch_m": margin().unwrap_or(lookahead::SWITCH_M),
           "max_ticks": MAX_TICKS, "seeds": [seeds.start(), seeds.end()], "models": models,
           "runs": runs})
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    median_sorted(&v)
}

/// Times `f` `n` times after `warm` unmeasured calls: microseconds, the bytes the median call
/// allocated, and the median of what a call left allocated (allocated less freed). What each call
/// returns is kept until every call is timed, so dropping it is not timed.
fn timed<T>(warm: usize, n: usize, mut f: impl FnMut() -> T) -> Value {
    let mut keep = Vec::with_capacity(warm + n);
    for _ in 0..warm {
        keep.push(f());
    }
    let mut us = Vec::with_capacity(n);
    let mut bytes = Vec::with_capacity(n);
    let mut held = Vec::with_capacity(n);
    for _ in 0..n {
        let (a0, f0) = counters();
        let t0 = Instant::now();
        let r = f();
        us.push(t0.elapsed().as_secs_f64() * 1e6);
        let (a1, f1) = counters();
        bytes.push((a1 - a0) as f64);
        held.push((a1 - a0) as f64 - (f1 - f0) as f64);
        keep.push(r);
    }
    drop(keep);
    json!({"us": stats(us), "allocated_bytes_median": median(bytes),
           "held_bytes_median": median(held)})
}

fn fork_bench() -> Value {
    // The course at tick 300 with the greedy skipper sailing it.
    let mut main = variant(0, false);
    let mut seat = Seat::new(0);
    let mut k = Skipper::new();
    while main.tick().0 < 300 {
        let obs = seat
            .call(&mut main, "player.observe", lookahead::observe_request())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        if let Some(a) = k.decide(&obs) {
            seat.call(&mut main, "player.act", a)
                .unwrap_or_else(|e| panic!("{e:#?}"));
        }
        let left = 300 - main.tick().0;
        seat.call(&mut main, "player.wait", json!({"ticks": left}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
    }
    let tick = main.tick().0;
    let snap = main.snapshot().unwrap_or_else(|e| panic!("{e:#?}"));
    let snapshot_bytes: usize = snap.sections().iter().map(|s| s.bytes.len()).sum();
    let entities = main
        .world()
        .resource::<amoris_sim::EntityAllocator>()
        .next()
        .saturating_sub(1);
    let mut kept = main.fork().unwrap_or_else(|e| panic!("{e:#?}"));
    let snapshot = timed(10, 100, || {
        std::hint::black_box(main.snapshot().unwrap_or_else(|e| panic!("{e:#?}")))
    });
    let restore = timed(10, 100, || {
        kept.restore(&snap).unwrap_or_else(|e| panic!("{e:#?}"));
    });
    let fork_into = timed(10, 100, || {
        main.fork_into(&mut kept)
            .unwrap_or_else(|e| panic!("{e:#?}"));
    });
    let fresh = timed(3, 20, || {
        std::hint::black_box(main.fork().unwrap_or_else(|e| panic!("{e:#?}")))
    });
    // Dropping a fresh branch: its world and its script host.
    let mut drop_us = Vec::new();
    for _ in 0..20 {
        let b = main.fork().unwrap_or_else(|e| panic!("{e:#?}"));
        let t0 = Instant::now();
        drop(std::hint::black_box(b));
        drop_us.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    // What a live fresh branch holds: Rust bytes allocated and not freed while it stays alive,
    // and its script host's heap (QuickJS-ng allocates through the C runtime, not counted above).
    let mut held = Vec::new();
    let mut script_heap = Vec::new();
    let mut branches = Vec::new();
    for _ in 0..16 {
        let (a0, f0) = counters();
        let b = main.fork().unwrap_or_else(|e| panic!("{e:#?}"));
        let (a1, f1) = counters();
        held.push(((a1 - a0) as f64) - ((f1 - f0) as f64));
        script_heap.push(
            b.world()
                .non_send::<amoris_script::scripts::Scripts>()
                .host
                .heap_bytes() as f64,
        );
        branches.push(b);
    }
    let main_heap = main
        .world()
        .non_send::<amoris_script::scripts::Scripts>()
        .host
        .heap_bytes();
    // A branch steps as main does: 600 ticks in a kept branch.
    main.fork_into(&mut kept)
        .unwrap_or_else(|e| panic!("{e:#?}"));
    let t0 = Instant::now();
    kept.run(600).unwrap_or_else(|e| panic!("{e:#?}"));
    let branch_tick_us = t0.elapsed().as_secs_f64() * 1e6 / 600.0;
    drop(branches);
    json!({"provisional": true, "world": "samples/sailing-course, seed 0, greedy skipper",
        "tick": tick, "entities": entities, "snapshot_bytes": snapshot_bytes,
        "snapshot": snapshot, "restore_into_kept_branch": restore,
        "fork_into_kept_branch": fork_into, "fork_fresh_branch": fresh,
        "drop_fresh_branch_us": stats(drop_us),
        "fresh_branch_rust_bytes_held": stats(held),
        "fresh_branch_script_heap_bytes": stats(script_heap),
        "main_script_heap_bytes": main_heap,
        "branch_tick_us": (branch_tick_us * 10.0).round() / 10.0})
}

/// What the model reads, for a look at the course's observations and the variants' placements.
fn probe() -> Value {
    let mut out = Vec::new();
    for seed in [0u64, 1, 2] {
        let mut g = variant(seed, false);
        let mut seat = Seat::new(0);
        seat.call(&mut g, "player.wait", json!({"ticks": 1}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let obs = seat
            .call(&mut g, "player.observe", lookahead::observe_request())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        g.run(120).unwrap_or_else(|e| panic!("{e:#?}"));
        let crates: Vec<Value> = ["Crate1", "Crate2", "Crate3", "Crate4"]
            .iter()
            .map(|c| {
                dev(
                    &mut g,
                    "world.get",
                    json!({"entity": c, "components": ["Transform"]}),
                )["components"]["Transform"]["position"]
                    .clone()
            })
            .collect();
        let wind = dev(
            &mut g,
            "world.get",
            json!({"entity": "Breeze", "components": ["Wind"]}),
        );
        let edit = course::Course::read(&mut variant(0, false)).variant(seed, false);
        out.push(json!({"seed": seed, "edit": edit,
            "observation_tick_1": obs, "crates_tick_121": crates,
            "wind": wind["components"]["Wind"]}));
    }
    Value::Array(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_else(|| "course".into());
    let out = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let mut seeds = 0..=20u64;
    let mut limits = Limits::default();
    let mut models: Vec<String> = MODELS.iter().map(|m| (*m).to_owned()).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seeds" => {
                let s = it.next().expect("--seeds A-B");
                let (a, b) = s.split_once('-').expect("--seeds A-B");
                seeds = a.parse().expect("a seed")..=b.parse().expect("a seed");
            }
            "--models" => {
                let list = it.next().expect("--models a,b");
                models = list.split(',').map(str::to_owned).collect();
                assert!(
                    models.iter().all(|m| MODELS.contains(&m.as_str())),
                    "models are {MODELS:?}"
                );
            }
            "--margin" => {
                let m: f64 = it.next().and_then(|s| s.parse().ok()).expect("metres");
                MARGIN.set(m).expect("one margin");
            }
            "--branches" => limits.branches = it.next().and_then(|s| s.parse().ok()).expect("n"),
            "--ticks" => {
                limits.ticks_per_decision = it.next().and_then(|s| s.parse().ok()).expect("n");
            }
            _ => {}
        }
    }
    let run = move || match mode.as_str() {
        "probe" => probe(),
        "fork" => fork_bench(),
        _ => course_bench(seeds, limits, &models),
    };
    // Scripts need the script host's stack (script-sandbox.md 4.3).
    let report = std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench ran");
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    if let Some(path) = out {
        std::fs::write(&path, text + "\n").unwrap_or_else(|e| panic!("{path}: {e}"));
    } else {
        println!("{text}");
    }
}
