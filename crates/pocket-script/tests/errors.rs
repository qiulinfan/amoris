//! Test 6 of script-host.md 12 (module state and errors, the runtime half; the lint's fixtures are
//! tests/lint.rs): errors name the TypeScript line and column through nested calls and the
//! prelude, with the entity when thrown inside `each`; a failed system's `script.failed` event
//! data is exactly `{system, code}`; `harden` refuses a top-level Map; mutating another module's
//! frozen export throws at the mutation, with the hint; a script cannot forge a host code or a
//! fault, and a top-level binding named `__proto__` is hardened like any other.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptHost, ScriptLimits, SystemOutcome};
use pocket_sim::PlainData;
use serde_json::json;

const COMPONENTS: &str = r#"import { component, field } from "pocket";
export const Armor = component("Armor", {
    version: 1, doc: "Armor.",
    fields: { value: field.f64(0, "How much.") },
});
"#;

const UTIL: &str = r#"export function checkArmor(value: number, limit: number): number {
    if (value > limit) {
        throw new RangeError(`armor ${value} over ${limit as number}`);
    }
    return value;
}
"#;

const MAIN: &str = r#"import { game, system } from "pocket";
import { Armor } from "./components";
import { checkArmor } from "./util";
const setup = system({
    name: "setup", phase: "update", doc: "Spawns armor.", when: "start",
    run(ctx) {
        for (let i = 0; i < 4; i++) ctx.world.spawn({ Armor: { value: i * 10 } });
    },
});
const inspect = system({
    name: "inspect", phase: "update", doc: "Checks armor.", when: { every: 2 },
    queries: { all: { with: ["Armor"] } },
    run(ctx, { all }) {
        all.each((row, e) => {
            checkArmor(all.cols.Armor.value[row], 15);
        });
    },
});
const refuse = system({
    name: "refuse", phase: "update", doc: "Writes to a missing entity.", when: { every: 3 },
    run(ctx) {
        ctx.world.set(999 as any, "Armor", { value: 1 });
    },
});
export default game({ components: [Armor], systems: [setup, inspect, refuse] });
"#;

#[test]
fn errors_name_the_typescript_line_and_the_entity() {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/util.ts", UTIL),
        ("scripts/main.ts", MAIN),
    ]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    common::step(&mut sim); // tick 1: setup
    let r = common::step(&mut sim); // tick 2: inspect fails at entity 3 (value 20)
    assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
    let outcomes = pocket_script::scripts::last_tick(sim.world());
    let SystemOutcome::Failed { error, .. } = &outcomes[0].1 else {
        panic!("{outcomes:#?}")
    };
    assert_eq!(error.code, "script.exception");
    assert_eq!(error.message, "armor 20 over 15");
    let at = error.detail.location.as_ref().unwrap();
    assert_eq!(
        (at.file.as_str(), at.line),
        ("scripts/util.ts", 3),
        "{error:#?}"
    );
    assert!(at.column >= 9, "{at:?}");
    assert_eq!(error.detail.entity, Some(3.0));
    assert_eq!(error.detail.tick, Some(2));
    assert_eq!(error.detail.system.as_deref(), Some("inspect"));
    assert_eq!(error.detail.js_error.as_ref().unwrap().name, "RangeError");
    let files: Vec<&str> = error
        .detail
        .stack
        .iter()
        .map(|f| f.at.file.as_str())
        .collect();
    assert_eq!(
        files,
        [
            "scripts/util.ts",
            "scripts/main.ts",
            "pocket",
            "scripts/main.ts"
        ],
        "{:#?}",
        error.detail.stack
    );
    assert_eq!(error.detail.stack[1].at.line, 15);
    // The failure: sim.system_failed in the report, and script.failed {system, code} in the next
    // tick's inbox, with no subject.
    assert_eq!(r.errors[0].code, "sim.system_failed");
    assert_eq!(
        r.errors[0].detail["cause"]["code"],
        json!("script.exception")
    );
    let failed: Vec<_> = r
        .events
        .iter()
        .filter(|e| e.kind.as_str() == "script.failed")
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].subject, None);
    assert_eq!(
        failed[0].data,
        PlainData::object(vec![
            ("system".into(), PlainData::String("inspect".into())),
            ("code".into(), PlainData::String("script.exception".into())),
        ])
        .unwrap()
    );
    // Tick 3: the host refusal names the calling line.
    let r = common::step(&mut sim);
    assert_eq!(r.errors.len(), 1);
    let outcomes = pocket_script::scripts::last_tick(sim.world());
    let SystemOutcome::Failed { error, .. } = &outcomes[0].1 else {
        panic!("{outcomes:#?}")
    };
    assert_eq!(error.code, "script.entity_missing");
    let at = error.detail.location.as_ref().unwrap();
    assert_eq!(
        (at.file.as_str(), at.line),
        ("scripts/main.ts", 22),
        "{error:#?}"
    );
}

const STATEFUL: &str = r#"import { game } from "pocket";
const keep = 1;
const cache = new Map();
export default game({ systems: [] });
"#;

#[test]
fn harden_refuses_a_top_level_map() {
    let set = common::compiled_lint_off(&[("scripts/main.ts", STATEFUL)]);
    let host = ScriptHost::new(ScriptLimits::default()).unwrap();
    let sim = pocket_sim::Sim::new(pocket_sim::SimConfig {
        rate: pocket_sim::TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap();
    let Err(e) = host.instantiate(&set, sim.world()) else {
        panic!("loaded")
    };
    assert_eq!(e[0].code, "script.module_state", "{e:#?}");
    assert_eq!(e[0].detail.extra["binding"], json!("cache"));
    let at = e[0].detail.location.as_ref().unwrap();
    assert_eq!((at.file.as_str(), at.line), ("scripts/main.ts", 3));
    assert!(e[0].message.contains("holds a Map"), "{}", e[0].message);
}

const CONFIG: &str = r#"export default { count: 0, speeds: [1, 2, 3] };
"#;

const MUTATE: &str = r#"import { game, system } from "pocket";
import cfg from "./config";
const bump = system({
    name: "bump", phase: "update", doc: "Mutates a frozen export.",
    run(ctx) {
        cfg.speeds.reverse();
        (cfg as any).count++;
    },
});
const sorter = system({
    name: "sorter", phase: "update", doc: "Copies first.",
    run(ctx) {
        const s = cfg.speeds.slice().reverse();
        ctx.emit("game.sorted", { first: s[0] });
    },
});
export default game({ systems: [bump, sorter] });
"#;

#[test]
fn mutating_a_frozen_module_value_throws_at_the_mutation() {
    let set = common::compiled(&[("scripts/config.ts", CONFIG), ("scripts/main.ts", MUTATE)]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    let r = common::step(&mut sim);
    assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
    let outcomes = pocket_script::scripts::last_tick(sim.world());
    let SystemOutcome::Failed { error, .. } = &outcomes[0].1 else {
        panic!()
    };
    assert_eq!(
        error.detail.js_error.as_ref().unwrap().name,
        "TypeError",
        "{error:#?}"
    );
    // `reverse()` on the frozen array, in place: the first mutation.
    assert_eq!(
        error.detail.location.as_ref().unwrap().line,
        6,
        "{error:#?}"
    );
    assert!(
        error
            .detail
            .hint
            .as_deref()
            .unwrap_or("")
            .contains("component"),
        "{error:#?}"
    );
    assert!(matches!(outcomes[1].1, SystemOutcome::Ok(_)));
}

const DATE: &str = r#"import { game, system } from "pocket";
const clock = system({
    name: "clock", phase: "update", doc: "Reads a clock.",
    run(ctx) {
        ctx.emit("game.time", { t: Date.now() });
    },
});
export default game({ systems: [clock] });
"#;

#[test]
fn a_removed_global_has_a_hint() {
    let set = common::compiled_lint_off(&[("scripts/main.ts", DATE)]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    common::step(&mut sim);
    let outcomes = pocket_script::scripts::last_tick(sim.world());
    let SystemOutcome::Failed { error, .. } = &outcomes[0].1 else {
        panic!()
    };
    assert_eq!(
        error.detail.js_error.as_ref().unwrap().name,
        "ReferenceError"
    );
    assert_eq!(
        error.detail.hint.as_deref(),
        Some("Date is not available in game scripts: use ctx.time or ctx.tick")
    );
    assert_eq!(error.detail.location.as_ref().unwrap().line, 5);
}

/// A system that throws `THROW` in a try/catch around a host refusal, so it can rethrow the host's
/// own error or a copy of it.
const FORGE: &str = r#"import { game, system } from "pocket";
const forge = system({
    name: "forge", phase: "update", doc: "Throws what it is given.",
    run(ctx) {
        let caught: any = null;
        try { ctx.world.get(999 as any, "Nothing"); } catch (e) { caught = e; }
        void caught;
        THROW
    },
});
export default game({ systems: [forge] });
"#;

/// The failure `throw` gives, or the fault's code when it poisons the world.
fn forged(throw: &str) -> Result<pocket_script::ScriptError, String> {
    let set = common::compiled(&[("scripts/main.ts", &FORGE.replace("THROW", throw))]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    match sim.step(&mut pocket_sim::NoHooks) {
        Err(p) => Err(p.code),
        Ok(_) => match &pocket_script::scripts::last_tick(sim.world())[0].1 {
            SystemOutcome::Failed { error, .. } => Ok(error.clone()),
            other => panic!("{throw}: {other:#?}"),
        },
    }
}

#[test]
fn scripts_cannot_forge_host_codes_or_faults() {
    for throw in [
        "throw new (globalThis as any).InternalError(\"out of memory\");",
        "throw new RangeError(\"Maximum call stack size exceeded\");",
        "throw new (globalThis as any).InternalError(\"call depth limit exceeded\");",
        "throw { __pocketCode: \"sim.internal\" };",
        "throw { __pocketCode: \"script.stack_overflow\" };",
        "throw { __pocketCode: \"script.budget_exceeded\" };",
        "throw { message: caught.message, name: caught.name, stack: caught.stack };",
    ] {
        let e = forged(throw).unwrap_or_else(|code| panic!("{throw} faulted with {code}"));
        assert_eq!(e.code, "script.exception", "{throw}: {e:#?}");
    }
    // The host's own error, rethrown, keeps its code and detail.
    let e = forged("throw caught;").unwrap();
    assert_eq!(e.code, "script.unknown_component", "{e:#?}");
    assert_eq!(e.detail.component.as_deref(), Some("Nothing"));
}

const PROTO: &str = r#"import { game, system } from "pocket";
const __proto__ = { n: 0 };
const count = system({
    name: "count", phase: "update", doc: "Counts in a binding named __proto__.",
    run(ctx) {
        __proto__.n += 1;
        ctx.emit("game.count", { n: __proto__.n });
    },
});
export default game({ systems: [count] });
"#;

#[test]
fn a_binding_named_proto_is_hardened() {
    // The epilogue passes bindings under computed keys, so `__proto__` is a key like any other and
    // its value is frozen: the increment throws instead of counting across calls.
    let set = common::compiled(&[("scripts/main.ts", PROTO)]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    for _ in 0..2 {
        let r = common::step(&mut sim);
        assert_eq!(r.errors.len(), 1, "{:#?}", r.events);
        let outcomes = pocket_script::scripts::last_tick(sim.world());
        let SystemOutcome::Failed { error, .. } = &outcomes[0].1 else {
            panic!("{outcomes:#?}")
        };
        assert_eq!(error.detail.js_error.as_ref().unwrap().name, "TypeError");
    }
}
