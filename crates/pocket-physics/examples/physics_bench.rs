//! Times the sailing scene (`probe::sailing_world`: the Sloop, six crates, the island sea, a gusty
//! breeze, the crew), natively: a tick split into the force systems and `physics.step`, the
//! stand-in snapshot and hash of `probe`, and a fork through it. For reading; the budgets are
//! measured by the check's `perf` step (budgets.md).
//!
//! `cargo run -p pocket-physics --example physics_bench --release`

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick

use std::time::Instant;

use pocket_physics::probe::{fork, fresh, ledger, sailing_world};
use pocket_sim::{NoHooks, StepHooks, SystemKey, TickPhase};

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

fn main() {
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
    println!(
        "tick (median of {rounds} runs of {ticks}): {:.1} us; forces {:.1} us; physics.step and contacts {:.1} us",
        median(per_tick),
        median(forces),
        median(physics)
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
    println!(
        "at tick 300: snapshot and FNV hash {:.1} us, fork (with a fresh world) {:.1} us, {size} bytes of which the physics cache {cache}",
        median(snap_us),
        median(fork_us)
    );
}
