//! Times the sailing scene (`probe::sailing_world`: the Sloop, six crates, the island sea, a gusty
//! breeze, the crew), natively: a tick split into the force systems and `physics.step`, the
//! stand-in snapshot and hash of `probe`, and a fork through it, as text and then as one JSON
//! line. For reading; the budgets are measured by the check's `perf` step (budgets.md).
//!
//! `cargo run -p amoris-physics --example physics_bench --release`
//!
//! With `level CELLS`, the level-scale scene instead (`probe::level`: ConvexVsMesh's bodies on a
//! terrain of `2 * CELLS^2` triangles; docs/bench/physics.md, "Static geometry and the cache"): the
//! first step (which builds every body), the warm ticks, the terrain's spec hash (what
//! `physics.step` computes for every body every tick), the physics cache's bytes and its encode
//! (into a new buffer, as a snapshot does) and decode, the stand-in snapshot, restore and fork,
//! whether a fork continues with its source's hashes, and a cold restore (the source world dropped
//! first). Each timed operation's result is freed after its timer stops. One JSON line per run.
//!
//! `cargo run -p amoris-physics --example physics_bench --release -- level 707 [--side 10]
//! [--warm 150] [--rounds 5] [--check 30]`
//!
//! With `trajectory`, the folds of `probe::layout::trajectories` as one JSON line: run on a build
//! of the cache's layout 1, they are `probe::layout::LAYOUT1_TRAJECTORIES`.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick

use std::time::Instant;

use amoris_physics::PhysicsCache;
use amoris_physics::probe::{fork, fresh, ledger, level, sailing_world};
use amoris_sim::persisted::PersistedCache;
use amoris_sim::{NoHooks, Sim, StepHooks, SystemKey, TickPhase};

/// Accumulates time per phase.
#[derive(Default)]
struct Phases {
    started: Option<Instant>,
    forces: f64,
    physics: f64,
}

impl StepHooks for Phases {
    fn phase(&mut self, phase: TickPhase, begin: bool) {
        if begin {
            self.started = Some(Instant::now());
        } else if let Some(t) = self.started.take() {
            let us = t.elapsed().as_secs_f64() * 1e6;
            match phase {
                TickPhase::Forces => self.forces += us,
                TickPhase::Physics => self.physics += us,
                _ => {}
            }
        }
    }

    fn system(&mut self, _key: &SystemKey, _begin: bool) {}
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("level") => level_bench(&args[1..]),
        Some("trajectory") => trajectory(),
        _ => sailing_bench(),
    }
}

fn trajectory() {
    let folds = amoris_physics::probe::layout::trajectories().unwrap();
    let runs = amoris_physics::probe::layout::LAYOUT1_TRAJECTORIES.map(|r| r.0);
    let folds: serde_json::Map<String, serde_json::Value> = runs
        .iter()
        .zip(folds)
        .map(|(name, f)| ((*name).to_owned(), format!("{f:016x}").into()))
        .collect();
    let identity = PhysicsCache::identity();
    println!(
        "{}",
        serde_json::json!({"identity": identity, "folds": folds})
    );
}

fn sailing_bench() {
    let ticks = 1200u32;
    let rounds = 5;
    let mut per_tick = Vec::new();
    let mut forces = Vec::new();
    let mut physics = Vec::new();
    for _ in 0..rounds {
        let mut sim = sailing_world();
        let mut hooks = Phases::default();
        let t = Instant::now();
        for _ in 0..ticks {
            sim.step(&mut hooks).unwrap();
        }
        let n = f64::from(ticks);
        per_tick.push(t.elapsed().as_secs_f64() * 1e6 / n);
        forces.push(hooks.forces / n);
        physics.push(hooks.physics / n);
    }
    let (tick_us, forces_us, physics_us) = (median(per_tick), median(forces), median(physics));
    println!(
        "tick (median of {rounds} runs of {ticks}): {tick_us:.1} us; forces {forces_us:.1} us; physics.step and contacts {physics_us:.1} us"
    );

    let l = ledger();
    let mut sim = sailing_world();
    for _ in 0..300 {
        sim.step(&mut NoHooks).unwrap();
    }
    let mut snap_us = Vec::new();
    let mut fork_us = Vec::new();
    let mut size = 0;
    for _ in 0..200 {
        let t = Instant::now();
        let s = l.snapshot(sim.world()).unwrap();
        std::hint::black_box(s.hash());
        snap_us.push(t.elapsed().as_secs_f64() * 1e6);
        size = s.sections.iter().map(|x| x.1.len()).sum::<usize>();
        let t = Instant::now();
        std::hint::black_box(fork(&l, &sim, fresh).unwrap());
        fork_us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let cache = l.snapshot(sim.world()).unwrap();
    let cache = cache.section("physics.rapier").map_or(0, <[u8]>::len);
    let (snapshot_us, fork_us) = (median(snap_us), median(fork_us));
    println!(
        "at tick 300: snapshot and FNV hash {snapshot_us:.1} us, fork (with a fresh world) {fork_us:.1} us, {size} bytes of which the physics cache {cache}"
    );
    let out = serde_json::json!({
        "scene": "sailing",
        "identity": PhysicsCache::identity(),
        "tick_us": tick_us,
        "forces_us": forces_us,
        "physics_us": physics_us,
        "snapshot_hash_us": snapshot_us,
        "fork_us": fork_us,
        "snapshot_bytes": size,
        "cache_bytes": cache,
    });
    println!("{out}");
}

fn flag(args: &[String], name: &str, default: u32) -> u32 {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map_or(default, |v| v.parse().expect("a number"))
}

fn step(sim: &mut Sim) {
    let r = sim.step(&mut NoHooks).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn level_bench(args: &[String]) {
    let cells: u32 = args
        .first()
        .and_then(|c| c.parse().ok())
        .expect("level CELLS");
    let side = i32::try_from(flag(args, "--side", 10)).unwrap();
    let warm = flag(args, "--warm", 150);
    let rounds = flag(args, "--rounds", 5);
    let check = flag(args, "--check", 30);
    let l = ledger();

    let t = Instant::now();
    let mut sim = level::level_world(cells, side);
    let setup_ms = ms(t);
    let t = Instant::now();
    step(&mut sim);
    let first_step_ms = ms(t);
    let mut ticks = Vec::new();
    for _ in 1..warm {
        let t = Instant::now();
        step(&mut sim);
        ticks.push(ms(t));
    }

    let (mut encode, mut decode, mut snapshot, mut restore, mut forked) =
        (vec![], vec![], vec![], vec![], vec![]);
    let mut spec = Vec::new();
    let mut bytes = Vec::new();
    for _ in 0..rounds {
        let t = Instant::now();
        std::hint::black_box(level::terrain_spec_hash(&sim));
        spec.push(ms(t));
        // Every result is freed after its timer stops; the encode writes into a new buffer, as a
        // snapshot does.
        drop(std::mem::take(&mut bytes));
        let t = Instant::now();
        let mut out = Vec::new();
        PhysicsCache::encode(sim.world(), &mut out).unwrap();
        encode.push(ms(t));
        bytes = out;
        let t = Instant::now();
        let staged = PhysicsCache::decode(&bytes).unwrap();
        decode.push(ms(t));
        drop(staged);
        let t = Instant::now();
        let snap = l.snapshot(sim.world()).unwrap();
        snapshot.push(ms(t));
        let t = Instant::now();
        let mut copy = fresh();
        l.restore(&mut copy, &snap).unwrap();
        restore.push(ms(t));
        drop(copy);
        drop(snap);
        let t = Instant::now();
        let f = fork(&l, &sim, fresh).unwrap();
        forked.push(ms(t));
        drop(f);
    }

    // A fork continues with its source's hashes, and a round trip keeps the bytes.
    let snap = l.snapshot(sim.world()).unwrap();
    let hash_at_warm = format!("{:016x}", snap.hash());
    let section = |name: &str| snap.section(name).map_or(0, <[u8]>::len);
    let (collider_bytes, snapshot_bytes) = (
        section("Collider"),
        snap.sections.iter().map(|s| s.1.len()).sum::<usize>(),
    );
    let mut f = fork(&l, &sim, fresh).unwrap();
    let mut continues = l.snapshot(f.world()).unwrap() == snap;
    for _ in 0..check {
        step(&mut sim);
        step(&mut f);
        continues &=
            l.snapshot(f.world()).unwrap().hash() == l.snapshot(sim.world()).unwrap().hash();
    }
    drop(f);

    // A cold restore: nothing alive shares the source's state any more.
    let snap = l.snapshot(sim.world()).unwrap();
    let hash = snap.hash();
    drop(sim);
    let t = Instant::now();
    let mut cold = fresh();
    l.restore(&mut cold, &snap).unwrap();
    let cold_ms = ms(t);
    let cold_same = l.snapshot(cold.world()).unwrap().hash() == hash;

    let out = serde_json::json!({
        "scene": "level",
        "cells": cells,
        "triangles": 2 * u64::from(cells) * u64::from(cells),
        "bodies": 4 * (2 * side + 1) * (2 * side + 1),
        "identity": PhysicsCache::identity(),
        "hash_at_warm": hash_at_warm,
        "setup_ms": setup_ms,
        "first_step_ms": first_step_ms,
        "tick_ms": median(ticks),
        "warm": warm,
        "cache_bytes": bytes.len(),
        "collider_bytes": collider_bytes,
        "snapshot_bytes": snapshot_bytes,
        "terrain_spec_hash_ms": median(spec),
        "cache_encode_ms": median(encode),
        "cache_decode_ms": median(decode),
        "snapshot_ms": median(snapshot),
        "restore_ms": median(restore),
        "fork_ms": median(forked),
        "rounds": rounds,
        "fork_continues": continues,
        "fork_checked_ticks": check,
        "cold_restore_ms": cold_ms,
        "cold_restore_same": cold_same,
    });
    println!("{out}");
}
