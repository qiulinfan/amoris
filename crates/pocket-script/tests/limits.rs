//! What the host spends in Rust for a script is bounded before it spends it (script-sandbox.md 4.3;
//! script-host.md 5.7 and 5.8): query columns, ids and shuffle permutations are charged to the
//! call's `host_bytes_per_call` and copied into QuickJS-ng's memory; a script-sized length is
//! checked before the host allocates for it; a console line stops after a bounded walk; plain data
//! too large is refused while it is read.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptLimits, SystemOutcome};
use pocket_sim::Sim;

const COMPONENTS: &str = r#"import { component, field } from "pocket";
export const Big = component("Big", {
    version: 1, doc: "Four numbers.",
    fields: {
        a: field.f64(0, "A."), b: field.f64(0, "B."), c: field.f64(0, "C."), d: field.f64(0, "D."),
    },
});
"#;

/// A world with 10,000 `Big` entities spawned on tick 1 and `body` run on tick 2.
fn main_ts(body: &str) -> String {
    format!(
        r#"import {{ game, system }} from "pocket";
import {{ Big }} from "./components";
const setup = system({{
    name: "setup", phase: "update", doc: "Spawns many.", when: "start",
    run(ctx) {{
        for (let i = 0; i < 10000; i++) ctx.world.spawn({{ Big: {{ a: i }} }});
    }},
}});
const probe = system({{
    name: "probe", phase: "update", doc: "The case under test.", when: {{ every: 2 }},
    run(ctx) {{
{body}
    }},
}});
export default game({{ components: [Big], systems: [setup, probe] }});
"#
    )
}

/// The probe's outcome on tick 2, with the world after it.
fn run(body: &str, limits: ScriptLimits) -> (SystemOutcome, Sim) {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", &main_ts(body)),
    ]);
    let mut sim = common::sim_with(&set, limits, 1);
    common::step(&mut sim);
    common::step(&mut sim);
    let outcome = pocket_script::scripts::last_tick(sim.world()).remove(0).1;
    (outcome, sim)
}

fn failed(body: &str) -> pocket_script::ScriptError {
    match run(body, ScriptLimits::default()).0 {
        SystemOutcome::Failed { error, .. } => error,
        other => panic!("{body}: {other:#?}"),
    }
}

#[test]
fn query_buffers_are_charged_to_the_call() {
    // One query of 10,000 rows and four columns: 800,000 bytes of host buffers, well inside.
    let (outcome, _) = run(
        "const q = ctx.query({ with: [\"Big\"] }); if (q.len !== 10000) throw new Error(\"rows\");",
        ScriptLimits::default(),
    );
    assert!(matches!(outcome, SystemOutcome::Ok(_)), "{outcome:#?}");
    // Holding 200 of them (about 76 MB of columns) fails the call deterministically, by the host
    // byte budget, before the host or QuickJS-ng run out of memory.
    let hold = "const keep: any[] = []; for (let i = 0; i < 200; i++) keep.push(ctx.query({ with: [\"Big\"] }));";
    let e = failed(hold);
    assert_eq!(e.code, "script.budget_exceeded", "{e:#?}");
    assert_eq!(e.detail.extra["budget"], "host_bytes_per_call");
    // The columns live in QuickJS-ng's heap, where memory_bytes counts them: with the byte budget
    // lifted, holding them runs out of memory (a fault) instead of growing the host unseen.
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", &main_ts(hold)),
    ]);
    let limits = ScriptLimits {
        memory_bytes: 16 << 20,
        host_bytes_per_call: u32::MAX,
        ..ScriptLimits::default()
    };
    let mut sim = common::sim_with(&set, limits, 1);
    common::step(&mut sim);
    let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
    assert_eq!(p.code, "script.out_of_memory", "{p:#?}");
}

#[test]
fn shuffle_lengths_are_checked_before_allocating() {
    let ok = run(
        "const a = [1, 2, 3]; ctx.rng.shuffle(a); if (a.length !== 3) throw new Error(\"len\");",
        ScriptLimits::default(),
    );
    assert!(matches!(ok.0, SystemOutcome::Ok(_)), "{:#?}", ok.0);
    // 2^24 items fit the bound, but their permutation does not fit the call's host bytes.
    let e = failed("const a: any[] = []; a.length = 1 << 24; ctx.rng.shuffle(a);");
    assert_eq!(e.code, "script.budget_exceeded", "{e:#?}");
    // Past 2^24 the length itself is refused.
    let e = failed("const a: any[] = []; a.length = 4294967295; ctx.rng.shuffle(a);");
    assert_eq!(e.code, "rng.bound_invalid", "{e:#?}");
    let e = failed("ctx.rng.shuffle({ length: 4294967295 } as any);");
    assert_eq!(e.code, "rng.bound_invalid", "{e:#?}");
}

#[test]
fn console_lines_are_bounded() {
    // 1000 items at each of four levels: a trillion values if walked whole.
    let body = r#"        const l1 = new Array(1000).fill(0);
        const l2 = new Array(1000).fill(l1);
        const l3 = new Array(1000).fill(l2);
        const l4 = new Array(1000).fill(l3);
        console.log(l4);
        const wide: any = {};
        for (let i = 0; i < 20000; i++) wide["k" + i] = i;
        console.log("wide", wide);"#;
    let logged = |lines: u32| {
        let limits = ScriptLimits {
            log_lines_per_tick: lines,
            ..ScriptLimits::default()
        };
        let (outcome, sim) = run(body, limits);
        let SystemOutcome::Ok(stats) = outcome else {
            panic!("{outcome:#?}")
        };
        let log = sim
            .world()
            .get_non_send::<pocket_script::Scripts>()
            .unwrap()
            .host
            .take_log();
        (stats.steps, log)
    };
    let (steps, log) = logged(100);
    assert_eq!(log.len(), 2);
    for line in &log {
        assert!(line.text.ends_with('\u{2026}'), "{}", line.text);
        assert!(line.text.len() <= 4096 + 3, "{}", line.text.len());
    }
    assert!(
        log[0].text.starts_with("[[[[0, 0, 0"),
        "{}",
        &log[0].text[..40]
    );
    assert!(
        log[1].text.starts_with("wide { k0: 0, k1: 1"),
        "{}",
        &log[1].text[..40]
    );
    // The walk spends no steps: with no line written, the call takes the same steps.
    assert_eq!(logged(0).0, steps);
}

#[test]
fn plain_data_too_large_is_refused_while_it_is_read() {
    // Two million elements: refused by the array's length, before any element is read.
    let e = failed("ctx.emit(\"game.big\", { xs: new Array(2000000).fill(0) });");
    assert_eq!(e.code, "script.bad_data", "{e:#?}");
    // Two hundred thousand keys: refused by their count, before any key is listed.
    let e = failed(
        "const o: any = {}; for (let i = 0; i < 200000; i++) o[\"k\" + i] = i; ctx.emit(\"game.big\", o);",
    );
    assert_eq!(e.code, "script.bad_data", "{e:#?}");
    // Long strings count by their bytes.
    let e = failed("ctx.emit(\"game.big\", { s: \"x\".repeat(20000) });");
    assert_eq!(e.code, "script.bad_data", "{e:#?}");
    // Data at the limit's side still passes.
    let (outcome, _) = run(
        "ctx.emit(\"game.ok\", { xs: new Array(1000).fill(1) });",
        ScriptLimits::default(),
    );
    assert!(matches!(outcome, SystemOutcome::Ok(_)), "{outcome:#?}");
}
