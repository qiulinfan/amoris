//! Reload equivalence (charter 3.7; hot-update.md 9; script-host.md 12, test 8): reloading the
//! unchanged bundle at any boundary, or giving the world a new host, leaves every later tick's
//! hash equal to the run without it; the `module-state` negative control (a module-level counter
//! compiled with the lint off) diverges on the tick after the reload, and with the lint on it does
//! not compile (`module-state-lint`).
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptLimits, ScriptSource, SwapOutcome, compile};
use pocket_sim::Sim;

fn workload() -> pocket_script::CompiledSet {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/web/workload");
    compile(&ScriptSource::read_dir(&root).unwrap(), &Default::default()).unwrap()
}

fn run(
    set: &pocket_script::CompiledSet,
    ticks: u64,
    mut at: impl FnMut(u64, &mut Sim),
) -> Vec<u64> {
    let mut sim = common::sim_with(set, ScriptLimits::default(), 77);
    let mut out = Vec::new();
    for t in 1..=ticks {
        let r = common::step(&mut sim);
        assert!(r.errors.is_empty(), "tick {t}: {:#?}", r.errors);
        out.push(common::hash(sim.world()));
        at(t, &mut sim);
    }
    out
}

fn first_divergence(a: &[u64], b: &[u64]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y).map(|i| i + 1)
}

#[test]
fn reloading_unchanged_scripts_changes_nothing() {
    let set = workload();
    let plain = run(&set, 120, |_, _| {});
    let reloaded = run(&set, 120, |t, sim| {
        if [30, 31, 77].contains(&t) {
            let r = pocket_script::swap(sim.world_mut(), &set, true).unwrap();
            assert_eq!(
                r.outcome,
                SwapOutcome::Applied,
                "a forced reload must happen (reload.not_performed)"
            );
        }
    });
    assert_eq!(first_divergence(&plain, &reloaded), None);
    // A new host for the same world on the same thread changes nothing either.
    let rehosted = run(&set, 120, |t, sim| {
        if t == 50 {
            pocket_script::scripts::rehost(sim.world_mut(), ScriptLimits::default()).unwrap();
        }
    });
    assert_eq!(first_divergence(&plain, &rehosted), None);
}

const STATEFUL: &str = r#"import { game, system } from "pocket";
let calls = 0;
const count = system({
    name: "count", phase: "update", doc: "Counts its calls in a module variable.",
    run(ctx) {
        calls += 1;
        ctx.emit("game.count", { calls });
    },
});
export default game({ systems: [count] });
"#;

#[test]
fn module_state_is_caught_by_the_reload_check() {
    // module-state-lint: the lint refuses it.
    let src = ScriptSource::new().with("scripts/main.ts", STATEFUL);
    let e = compile(&src, &Default::default()).unwrap_err();
    assert!(e.iter().any(|e| e.code == "lint.module_let"), "{e:#?}");
    // module-state: with the lint off it runs, and the reload check names the tick after the reload.
    let set = common::compiled_lint_off(&[("scripts/main.ts", STATEFUL)]);
    let plain = run(&set, 40, |_, _| {});
    let reloaded = run(&set, 40, |t, sim| {
        if t == 20 {
            pocket_script::swap(sim.world_mut(), &set, true).unwrap();
        }
    });
    assert_eq!(first_divergence(&plain, &reloaded), Some(21));
}

#[test]
fn a_fork_is_a_fresh_sim_with_the_same_scripts() {
    // `Scripts` is non-send data: bevy_ecs refuses it on any other thread, and persistence leaves
    // it out of a snapshot. So a fork or a restore is a new Sim on its own thread with `install`
    // and `swap` of the same compiled set; replaying the same inputs (none here) from the same seed
    // gives the same digest at every tick as the world it forks.
    let set = workload();
    let plain = run(&set, 120, |_, _| {});
    let json = set.to_json();
    let forked = std::thread::Builder::new()
        .stack_size(pocket_script::host::THREAD_STACK_BYTES)
        .spawn(move || {
            let set = pocket_script::CompiledSet::from_json(&json).unwrap();
            run(&set, 120, |_, _| {})
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(first_divergence(&plain, &forked), None);
}
