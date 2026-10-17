//! The rule workload of `bench/rules` (docs/bench/script.md): the script world and its Rust twin
//! build the same world tick after tick (the bench's equality check, kept honest as the host
//! changes), and the phase clock a benchmark installs times each successful call once per phase.
#![cfg(feature = "transpile")]

use std::sync::atomic::{AtomicU64, Ordering};

use pocket_script::{CompiledSet, ScriptSource, compile};

#[path = "../examples/rules_bench/workload.rs"]
mod workload;

/// A clock that advances one unit per reading, so each phase of a call reads exactly 1.
fn counter() -> f64 {
    static N: AtomicU64 = AtomicU64::new(0);
    #[allow(clippy::cast_precision_loss)] // far below 2^53 readings
    let n = N.fetch_add(1, Ordering::Relaxed) as f64;
    n
}

fn rules() -> CompiledSet {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/rules");
    let src = ScriptSource::read_dir(&root).unwrap();
    compile(&src, &Default::default()).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn on_script_thread(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(pocket_script::host::THREAD_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_rule_and_its_twin_build_the_same_world() {
    on_script_thread(|| {
        let set = rules();
        let mut script = workload::Bench::script(&set, "damage", counter).unwrap();
        let mut twin = workload::Bench::twin(counter).unwrap();
        let mut combat = [0u64; 3];
        for t in 1..=20 {
            script.tick().unwrap();
            twin.tick().unwrap();
            assert_eq!(script.digest(), twin.digest(), "tick {t}");
            let c = script.combat();
            assert_eq!(c, twin.combat(), "tick {t}");
            for (sum, x) in combat.iter_mut().zip(c) {
                *sum += x;
            }
            let s = script.sample();
            // Tick 1 runs populate and damage, later ticks damage alone.
            let calls = if t == 1 { 2.0 } else { 1.0 };
            let p = s.phases;
            assert_eq!(
                (p.prepare, p.run, p.diff, p.apply, f64::from(p.calls)),
                (calls, calls, calls, calls, calls),
                "tick {t}"
            );
            assert!(s.steps > 0);
        }
        // Four `combat.damaged` events a tick (250 hits each), nearly every fighter hitting, and
        // kills.
        assert_eq!(combat[0], 80, "{combat:?}");
        assert!(combat[1] > 19_000, "{combat:?}");
        assert!(combat[2] > 0, "{combat:?}");
    });
}
