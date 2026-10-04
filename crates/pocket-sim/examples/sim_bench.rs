//! First measurements of the simulation core's overheads (docs/spec/simulation.md 13; rng.md 11),
//! the slice 1 benches `sim.empty_tick`, `sim.sorted_iter` and `rng.entity_streams` until the
//! check's harness (budgets.md) runs them: `cargo run -p pocket-sim --example sim_bench --release`.
//! Prints the median and the range of five runs of each, in microseconds.

use std::hint::black_box;

use bevy_ecs::prelude::*;
use pocket_sim::order::by_id_mut;
use pocket_sim::{
    EntityId, NoHooks, RngTable, RunCondition, Sim, SimConfig, TickPhase, TickRate, entity,
};

#[derive(Component, Clone, Copy)]
struct Body([f64; 6]);

#[derive(Component, Clone, Copy)]
struct Marker;

/// Five runs of `f`, each timed over `n` calls: (median, min, max) microseconds per call.
#[allow(clippy::disallowed_methods)] // a bench outside the simulation times its own runs
fn time(n: u32, mut f: impl FnMut()) -> (f64, f64, f64) {
    let mut runs: Vec<f64> = (0..5)
        .map(|_| {
            let t = std::time::Instant::now();
            for _ in 0..n {
                f();
            }
            t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
        })
        .collect();
    runs.sort_by(f64::total_cmp);
    (runs[2], runs[0], runs[4])
}

fn report(name: &str, (med, lo, hi): (f64, f64, f64)) {
    println!("{name:40} {med:10.3} us  ({lo:.3} to {hi:.3})");
}

fn sim() -> Sim {
    Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap()
}

/// `n` entities with six doubles, half with a marker, fragmented: a tenth despawned and as many
/// spawned, the marker toggled on a tenth (simulation.md 13's probe).
fn crowd(n: u64) -> Sim {
    let mut s = sim();
    let mut b = s.boundary();
    for i in 0..n {
        let id = b.spawn(Body([1.0; 6])).unwrap();
        if i % 2 == 0 {
            let e = b.entity(id).unwrap();
            b.world_mut().entity_mut(e).insert(Marker);
        }
    }
    for i in (1..=n).step_by(10) {
        b.despawn(EntityId::new(i).unwrap()).unwrap();
        b.spawn(Body([2.0; 6])).unwrap();
    }
    for i in (3..=n).step_by(10) {
        if let Some(e) = b.entity(EntityId::new(i).unwrap()) {
            let mut w = b.world_mut().entity_mut(e);
            if w.contains::<Marker>() {
                w.remove::<Marker>();
            } else {
                w.insert(Marker);
            }
        }
    }
    s
}

fn main() {
    let mut s = sim();
    report(
        "sim.empty_tick (no systems)",
        time(20_000, || {
            s.step(&mut NoHooks).unwrap();
        }),
    );
    let mut s = sim();
    for (key, phase) in pocket_sim::schedule::SYSTEM_ORDER {
        if !key.starts_with("sim.") {
            s.add_system(key, phase, RunCondition::Always, || {})
                .unwrap();
        }
    }
    report(
        "sim.empty_tick (12 empty systems)",
        time(20_000, || {
            s.step(&mut NoHooks).unwrap();
        }),
    );
    for n in [1_000u64, 10_000] {
        let mut s = crowd(n);
        let mut q = s.world_mut().query::<(&EntityId, &mut Body)>();
        let w = s.world_mut();
        report(
            &format!("storage-order loop, {n} entities"),
            time(200, || {
                #[allow(clippy::disallowed_methods)] // the comparison the spec measures
                for (_, mut b) in q.iter_mut(w) {
                    b.0[0] += b.0[3] * 0.016;
                    black_box(&b.0);
                }
            }),
        );
        let mut s2 = crowd(n);
        let mut state =
            bevy_ecs::system::SystemState::<Query<(&EntityId, &mut Body)>>::new(s2.world_mut());
        let w2 = s2.world_mut();
        report(
            &format!("sim.sorted_iter (by_id_mut), {n} entities"),
            time(200, || {
                let mut q = state.get_mut(w2).unwrap();
                for (_, mut b) in by_id_mut(&mut q) {
                    b.0[0] += b.0[3] * 0.016;
                    black_box(&b.0);
                }
            }),
        );
    }
    let mut s = crowd(10_000);
    s.add_exclusive(
        "test.streams",
        TickPhase::Update,
        RunCondition::Always,
        |world: &mut World, _| {
            let ids: Vec<EntityId> = world.resource::<pocket_sim::EntityIndex>().ids().collect();
            let mut rng = world.resource_mut::<RngTable>();
            for id in ids {
                black_box(rng.entity("test.streams", id).unwrap().next_u32());
            }
        },
    )
    .unwrap();
    report(
        "rng.entity_streams, 10,000 entities, tick",
        time(50, || {
            s.step(&mut NoHooks).unwrap();
        }),
    );
    let mut s = sim();
    let _ = entity::spawn(s.world_mut(), ());
    report(
        "one step with one entity",
        time(20_000, || {
            s.step(&mut NoHooks).unwrap();
        }),
    );
}
