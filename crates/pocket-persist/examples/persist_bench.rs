//! First measurements of persistence (docs/spec/persistence.md 13; budgets.md `fork.churn`,
//! `publish.churn`, `restore.*`) until the check's harness runs them:
//! `cargo run -p pocket-persist --example persist_bench --release`. A crowd of 10,000 bodies (six
//! doubles and a name each, a tenth despawned and respawned, the churn fixture's shape) and a small
//! world of 30; prints the median and the range of five runs of each operation in microseconds.

use std::hint::black_box;

use bevy_ecs::prelude::*;
use pocket_persist::{
    RestoreOptions, fork_into, restore, section_digests, sim_registry, snapshot, world_hash,
};
use pocket_sim::{EntityId, Name, Persisted, RegisterPersisted, Sim, SimConfig, TickRate};
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Copy, Serialize, Deserialize)]
struct Body {
    p: [f64; 3],
    v: [f64; 3],
}

impl Persisted for Body {
    const NAME: &'static str = "Body";
    const VERSION: u32 = 1;
}

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

fn world(n: u64) -> Sim {
    let mut s = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap();
    let mut b = s.boundary();
    for i in 0..n {
        #[allow(clippy::cast_precision_loss)]
        let x = i as f64;
        b.spawn((
            Body {
                p: [x, -x, 0.5],
                v: [1.0, 0.0, -0.25],
            },
            Name::new(format!("b{i}")).unwrap(),
        ))
        .unwrap();
    }
    for i in (1..=n).step_by(10) {
        b.despawn(EntityId::new(i).unwrap()).unwrap();
        b.spawn(Body {
            p: [0.0; 3],
            v: [0.0; 3],
        })
        .unwrap();
    }
    s
}

fn main() {
    let mut reg = sim_registry();
    reg.component::<Body>();
    let reg = reg.build().unwrap();
    for (label, n, calls) in [("crowd 10,000", 10_000u64, 20u32), ("small 30", 30, 2000)] {
        let s = world(n);
        let snap = snapshot(s.world(), &reg).unwrap();
        let bytes = snap.to_bytes();
        println!(
            "{label}: {} sections, {} bytes",
            snap.sections().len(),
            bytes.len()
        );
        report(
            &format!("{label} world_hash"),
            time(calls, || {
                black_box(world_hash(s.world(), &reg).unwrap());
            }),
        );
        report(
            &format!("{label} section_digests"),
            time(calls, || {
                black_box(section_digests(s.world(), &reg).unwrap());
            }),
        );
        report(
            &format!("{label} snapshot"),
            time(calls, || {
                black_box(snapshot(s.world(), &reg).unwrap());
            }),
        );
        report(
            &format!("{label} to_bytes"),
            time(calls, || {
                black_box(snap.to_bytes());
            }),
        );
        report(
            &format!("{label} from_bytes"),
            time(calls, || {
                black_box(pocket_persist::Snapshot::from_bytes(&bytes).unwrap());
            }),
        );
        let mut target = world(0);
        report(
            &format!("{label} restore (verify)"),
            time(calls, || {
                black_box(
                    restore(target.world_mut(), &snap, &reg, RestoreOptions::default()).unwrap(),
                );
            }),
        );
        report(
            &format!("{label} restore (no verify)"),
            time(calls, || {
                black_box(
                    restore(
                        target.world_mut(),
                        &snap,
                        &reg,
                        RestoreOptions { verify: false },
                    )
                    .unwrap(),
                );
            }),
        );
        report(
            &format!("{label} fork_into"),
            time(calls, || {
                black_box(fork_into(s.world(), target.world_mut(), &reg).unwrap());
            }),
        );
    }
}
