//! Hot update (hot-update.md 14, the tests that need only the simulation and the script host): the
//! swap lands between two ticks; a candidate failing at any stage keeps the old scripts and the
//! world; project components are added, or refused when they change without a migration; a hot
//! update fixes a failing system; an unchanged bundle is a no-op unless forced.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::scripts::SchemaChange;
use pocket_script::{ScriptLimits, ScriptSource, SwapOutcome, compile};
use pocket_sim::Sim;

const COMPONENTS_V1: &str = r#"import { component, field } from "pocket";
export const Tally = component("Tally", {
    version: 1, doc: "A count.",
    fields: { n: field.u32(0, "The count.") },
});
"#;

fn main_ts(step: &str, extra: &str) -> String {
    format!(
        r#"import {{ game, system }} from "pocket";
import {{ Tally }} from "./components";
const setup = system({{
    name: "setup", phase: "update", doc: "One tally.", when: "start",
    run(ctx) {{ ctx.world.spawn({{ Tally: {{}} }}); ctx.emit("game.setup"); }},
}});
const add = system({{
    name: "add", phase: "update", doc: "Adds to the tally.",
    run(ctx) {{
        const e = ctx.single("Tally");
        const t = ctx.world.get(e, "Tally") as any;
        {step}
    }},
}});
{extra}
export default game({{ components: [Tally], systems: [setup, add] }});
"#
    )
}

fn set(
    components: &str,
    main: &str,
) -> Result<pocket_script::CompiledSet, Vec<pocket_script::ScriptError>> {
    compile(
        &ScriptSource::new()
            .with("scripts/components.ts", components)
            .with("scripts/main.ts", main),
        &Default::default(),
    )
}

fn tally(sim: &Sim) -> serde_json::Value {
    let e = pocket_sim::EntityId::from_f64(1.0).unwrap();
    common::component_json(sim.world(), e, "Tally").unwrap()["n"].clone()
}

fn bundle(sim: &Sim) -> pocket_sim::ContentHash {
    sim.world()
        .get_non_send::<pocket_script::Scripts>()
        .unwrap()
        .program
        .as_ref()
        .unwrap()
        .bundle()
        .hash
}

#[test]
fn a_swap_lands_between_two_ticks() {
    let one = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + 1 });", ""),
    )
    .unwrap();
    let ten = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + 10 });", ""),
    )
    .unwrap();
    let mut sim = common::sim_with(&one, ScriptLimits::default(), 1);
    for _ in 0..3 {
        common::step(&mut sim);
    }
    assert_eq!(tally(&sim), serde_json::json!(3));
    let r = pocket_script::swap(sim.world_mut(), &ten, false).unwrap();
    assert_eq!(r.outcome, SwapOutcome::Applied);
    assert_eq!(r.systems.kept, ["setup", "add"]);
    assert_eq!(
        r.components,
        [SchemaChange::Unchanged {
            name: "Tally".into()
        }]
    );
    let report = common::step(&mut sim);
    assert_eq!(tally(&sim), serde_json::json!(13));
    // The start system did not run again.
    assert!(
        report
            .events
            .iter()
            .all(|e| e.kind.as_str() != "game.setup")
    );
    assert_eq!(bundle(&sim), ten.bundle.hash);
    let back = sim
        .world()
        .get_non_send::<pocket_script::Scripts>()
        .unwrap()
        .previous
        .clone()
        .unwrap();
    assert_eq!(back.bundle.hash, one.bundle.hash);
}

#[test]
fn a_failing_candidate_keeps_the_old_scripts_and_the_world() {
    let good = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + 1 });", ""),
    )
    .unwrap();
    let mut sim = common::sim_with(&good, ScriptLimits::default(), 1);
    common::step(&mut sim);
    let before = (common::hash(sim.world()), bundle(&sim));
    // Stage 2, compile: a syntax error and a lint error, each with its TypeScript location.
    let e = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + };", ""),
    )
    .unwrap_err();
    assert_eq!(e[0].code, "script.syntax");
    assert_eq!(e[0].detail.location.as_ref().unwrap().line, 12);
    let e = set(COMPONENTS_V1, &main_ts("", "let leak = 0;")).unwrap_err();
    assert_eq!(e[0].code, "lint.module_let");
    // Stage 5, prepare: a load exception, a definition error and a schema refusal.
    let bad_load = set(
        COMPONENTS_V1,
        &main_ts("", "const x = (Math as any).nope.y;"),
    )
    .unwrap();
    let not_game = set(COMPONENTS_V1, "export default 1;\n").unwrap();
    let unbumped = set(
        &COMPONENTS_V1.replace(
            "n: field.u32(0, \"The count.\")",
            "n: field.u32(0, \"The count.\"), m: field.f64(0, \"More.\")",
        ),
        &main_ts("", ""),
    )
    .unwrap();
    for (candidate, code) in [
        (&bad_load, "script.load_exception"),
        (&not_game, "script.entry_not_game"),
        (&unbumped, "version.unbumped"),
    ] {
        let e = pocket_script::swap(sim.world_mut(), candidate, false).unwrap_err();
        assert_eq!(e[0].code, code, "{e:#?}");
        assert_eq!((common::hash(sim.world()), bundle(&sim)), before);
    }
    let e = pocket_script::swap(sim.world_mut(), &bad_load, false).unwrap_err();
    assert_eq!(e[0].detail.location.as_ref().unwrap().line, 15, "{e:#?}");
    // The old scripts run on.
    common::step(&mut sim);
    assert_eq!(tally(&sim), serde_json::json!(2));
}

#[test]
fn project_components_across_a_swap() {
    let v1 = set(COMPONENTS_V1, &main_ts("", "")).unwrap();
    let mut sim = common::sim_with(&v1, ScriptLimits::default(), 1);
    common::step(&mut sim);
    // Added.
    let added = COMPONENTS_V1.to_owned()
        + "export const Score = component(\"Score\", { version: 1, doc: \"Points.\", fields: { p: field.f64(0, \"Points.\") } });\n";
    let with_score = set(
        &added,
        &main_ts("", "")
            .replace("import { Tally }", "import { Tally, Score }")
            .replace("components: [Tally]", "components: [Tally, Score]"),
    )
    .unwrap();
    let r = pocket_script::swap(sim.world_mut(), &with_score, false).unwrap();
    assert_eq!(
        r.components[1],
        SchemaChange::Added {
            name: "Score".into(),
            version: 1
        }
    );
    // A docs-only change needs no version.
    let docs = set(
        &added.replace("\"Points.\") }", "\"Points scored.\") }"),
        &main_ts("", "")
            .replace("import { Tally }", "import { Tally, Score }")
            .replace("components: [Tally]", "components: [Tally, Score]"),
    )
    .unwrap();
    pocket_script::swap(sim.world_mut(), &docs, false).unwrap();
    // Removed without retiring, bumped without a live migration, downgraded: refused.
    let e = pocket_script::swap(sim.world_mut(), &v1, false).unwrap_err();
    assert_eq!(e[0].code, "migrate.missing_removal");
    let bumped = added.replace("version: 1, doc: \"Points.\", fields: { p: field.f64(0, \"Points.\") }", "version: 2, doc: \"Points.\", fields: { p: field.f64(0, \"Points.\"), q: field.f64(0, \"More.\") }");
    let bumped = set(
        &bumped,
        &main_ts("", "")
            .replace("import { Tally }", "import { Tally, Score }")
            .replace("components: [Tally]", "components: [Tally, Score]"),
    )
    .unwrap();
    assert_eq!(
        pocket_script::swap(sim.world_mut(), &bumped, false).unwrap_err()[0].code,
        "migrate.missing_step"
    );
    // A downgrade: a world whose Tally is at version 2 offered version 1.
    let v2 = set(
        &COMPONENTS_V1.replace("version: 1", "version: 2"),
        &main_ts("", ""),
    )
    .unwrap();
    let mut later = common::sim_with(&v2, ScriptLimits::default(), 1);
    common::step(&mut later);
    let e = pocket_script::swap(later.world_mut(), &v1, false).unwrap_err();
    assert_eq!(e[0].code, "version.downgrade");
}

#[test]
fn a_hot_update_fixes_a_failing_system() {
    let broken = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + 0.5 });", ""),
    )
    .unwrap();
    let fixed = set(
        COMPONENTS_V1,
        &main_ts("ctx.world.set(e, \"Tally\", { n: t.n + 1 });", ""),
    )
    .unwrap();
    let mut sim = common::sim_with(&broken, ScriptLimits::default(), 1);
    common::step(&mut sim);
    let r = common::step(&mut sim);
    assert_eq!(r.errors.len(), 1);
    pocket_script::swap(sim.world_mut(), &fixed, false).unwrap();
    for _ in 0..3 {
        let r = common::step(&mut sim);
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
    }
    assert_eq!(tally(&sim), serde_json::json!(3));
}

#[test]
fn an_unchanged_bundle_is_a_no_op_unless_forced() {
    let s = set(COMPONENTS_V1, &main_ts("", "")).unwrap();
    let mut sim = common::sim_with(&s, ScriptLimits::default(), 1);
    common::step(&mut sim);
    assert_eq!(
        pocket_script::swap(sim.world_mut(), &s, false)
            .unwrap()
            .outcome,
        SwapOutcome::Unchanged
    );
    assert_eq!(
        pocket_script::swap(sim.world_mut(), &s, true)
            .unwrap()
            .outcome,
        SwapOutcome::Applied
    );
}

#[test]
fn new_defaults_apply_to_components_inserted_after_the_swap() {
    // Docs and defaults are not part of the fingerprint (hot-update.md 6): the same version with a
    // new default is Unchanged, and a Tally inserted after the swap gets the new default.
    let step = "if (ctx.tick === 3) ctx.world.spawn({ Tally: {} });";
    let before = set(COMPONENTS_V1, &main_ts(step, "")).unwrap();
    let five = COMPONENTS_V1
        .replace(
            "field.u32(0, \"The count.\")",
            "field.u32(5, \"The count, from five.\")",
        )
        .replace("doc: \"A count.\"", "doc: \"A count of things.\"");
    let after = set(&five, &main_ts(step, "")).unwrap();
    let mut sim = common::sim_with(&before, ScriptLimits::default(), 1);
    common::step(&mut sim);
    common::step(&mut sim);
    let r = pocket_script::swap(sim.world_mut(), &after, false).unwrap();
    assert_eq!(
        r.components,
        [SchemaChange::Unchanged {
            name: "Tally".into()
        }]
    );
    let r = common::step(&mut sim);
    assert!(r.errors.is_empty(), "{:#?}", r.errors);
    let spawned = pocket_sim::EntityId::from_f64(2.0).unwrap();
    assert_eq!(
        common::component_json(sim.world(), spawned, "Tally").unwrap()["n"],
        serde_json::json!(5)
    );
    // The tally inserted before the swap keeps its value.
    assert_eq!(tally(&sim), serde_json::json!(0));
}
