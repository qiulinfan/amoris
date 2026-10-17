//! The rule workload of `bench/rules` (docs/bench/script.md): the script world and its Rust twin
//! build the same world tick after tick (the bench's equality check, kept honest as the host
//! changes), and the phase clock a benchmark installs times each successful call once per phase;
//! timed natives change nothing and count every native call of `run`.
#![cfg(feature = "transpile")]

use std::sync::atomic::{AtomicU64, Ordering};

use amoris_script::{CompiledSet, ScriptSource, compile};

#[path = "../examples/rules_bench/workload.rs"]
mod workload;

/// A clock that advances one unit per reading, so each phase of a call reads exactly 1.
fn counter() -> f64 {
    static N: AtomicU64 = AtomicU64::new(0);
    #[allow(clippy::cast_precision_loss)] // far below 2^53 readings
    let n = N.fetch_add(1, Ordering::Relaxed) as f64;
    n
}

/// Another such clock, for a test that runs beside the one that reads `counter`.
fn counter2() -> f64 {
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
        .stack_size(amoris_script::host::THREAD_STACK_BYTES)
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

/// With its natives timed (`ScriptHost::time_natives`), the rule builds the same world; each
/// native call of `run` reads the clock twice with nothing between, so on a clock that advances one
/// unit per reading their time is their count: the `Math` calls of every living fighter (`wobble`,
/// `driftX`, `driftY`), the five events and the one `rng.fill`.
#[test]
fn timed_natives_change_nothing_and_count_every_native_call_of_run() {
    on_script_thread(|| {
        let set = rules();
        let mut timed =
            workload::Bench::script_timing_natives(&set, "damage", counter2, Some(counter2))
                .unwrap();
        let mut twin = workload::Bench::twin(counter2).unwrap();
        for t in 1..=10 {
            timed.tick().unwrap();
            twin.tick().unwrap();
            assert_eq!(timed.digest(), twin.digest(), "tick {t}");
            let p = timed.sample().phases;
            #[allow(clippy::cast_precision_loss)] // a few thousand
            let calls = p.native_calls as f64;
            assert_eq!(p.native, calls, "tick {t}");
            if t > 1 {
                // 1,000 `driftX` and `driftY` calls and one `wobble` per living hitter; four
                // `combat.damaged` events and one `combat.died`; one `rng.fill`.
                let [math, events, rng, world, other] = p.natives.map(|n| n.calls);
                assert!((2_000..=3_000).contains(&math), "tick {t}: {p:?}");
                assert_eq!((events, rng, world, other), (5, 1, 0, 0), "tick {t}");
                assert_eq!(p.native_calls, math + 6);
            }
        }
    });
}
