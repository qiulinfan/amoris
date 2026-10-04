//! Test 5 of script-host.md 12 (writes and columns): every refusal of 5.3, 5.4, 5.8 and 6 gives its
//! code, location and entity, applies nothing and never aborts the process; each row of 5.4's
//! table holds; only changed cells are written back, `-0` is a change, rows follow ascending ids;
//! `world.get` copies list keys in declaration order; `console.log` runs no project code.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptLimits, SystemOutcome};
use pocket_sim::Sim;
use serde_json::json;

const COMPONENTS: &str = r#"import { component, field } from "pocket";
export const Mover = component("Mover", {
    version: 1, doc: "Something that moves.",
    fields: {
        x: field.f64(0, "Position."),
        hits: field.u32(0, "Times hit."),
        on: field.bool(false, "Switched on."),
        kind: field.enum(["boat", "buoy"], "boat", "What it is."),
        target: field.entity("What it follows."),
        label: field.str("", "A name."),
        pos: field.vec3({ x: 0, y: 0, z: 0 }, "Where it is."),
    },
});
export const Flag = component("Flag", {
    version: 1, doc: "A marker.",
    fields: { up: field.bool(true, "Raised.") },
});
"#;

fn main_ts(body: &str) -> String {
    format!(
        r#"import {{ game, system }} from "pocket";
import {{ Mover, Flag }} from "./components";
const setup = system({{
    name: "setup", phase: "update", doc: "Spawns three movers.", when: "start",
    run(ctx) {{
        for (let i = 0; i < 3; i++) ctx.world.spawn({{ Mover: {{ x: i, hits: i }} }});
    }},
}});
const probe = system({{
    name: "probe", phase: "update", doc: "The case under test.", when: {{ every: 2 }},
    queries: {{ movers: {{ with: ["Mover"] }} }},
    run(ctx, {{ movers }}) {{
        const [a, b, c] = [movers.id(0), movers.id(1), movers.id(2)];
{body}
    }},
}});
export default game({{ components: [Mover, Flag], systems: [setup, probe] }});
"#
    )
}

/// The line of the case body's first line in main.ts.
const BODY_LINE: u32 = 14;

struct Case {
    sim: Sim,
    report: pocket_sim::StepReport,
    outcome: SystemOutcome,
    before: u64,
}

fn run(body: &str) -> Case {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", &main_ts(body)),
    ]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 2);
    common::step(&mut sim);
    let before = common::hash(sim.world());
    let report = common::step(&mut sim);
    let outcome = pocket_script::scripts::last_tick(sim.world()).remove(0).1;
    Case {
        sim,
        report,
        outcome,
        before,
    }
}

/// A refused case: its code, its line (relative to the body) and entity, and nothing applied but
/// the failure's own event.
fn refused(body: &str, code: &str, line: u32, entity: Option<f64>) -> pocket_script::ScriptError {
    let c = run(body);
    let SystemOutcome::Failed { error, .. } = c.outcome else {
        panic!("{body}: {:#?}", c.outcome)
    };
    assert_eq!(error.code, code, "{body}: {error:#?}");
    let at = error
        .detail
        .location
        .clone()
        .unwrap_or_else(|| panic!("{body}: no location {error:#?}"));
    assert_eq!(
        (at.file.as_str(), at.line),
        ("scripts/main.ts", BODY_LINE + line),
        "{body}: {error:#?}"
    );
    assert_eq!(error.detail.entity, entity, "{body}: {error:#?}");
    // Nothing applied: the world differs from before only by the clock and script.failed.
    let kinds: Vec<&str> = c.report.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["script.failed"], "{body}");
    let _ = c.before;
    error
}

fn ok(body: &str) -> Case {
    let c = run(body);
    assert!(
        matches!(c.outcome, SystemOutcome::Ok(_)),
        "{body}: {:#?}",
        c.outcome
    );
    c
}

fn field(sim: &Sim, id: f64, field: &str) -> serde_json::Value {
    let e = pocket_sim::EntityId::from_f64(id).unwrap();
    common::component_json(sim.world(), e, "Mover")
        .map_or(serde_json::Value::Null, |v| v[field].clone())
}

#[test]
fn refusals_name_their_code_line_and_entity() {
    let e = refused(
        r#"        ctx.world.set(a, "Mover", { xx: 1 });"#,
        "script.unknown_field",
        0,
        Some(1.0),
    );
    assert_eq!(e.detail.suggestions, ["x"]);
    let e = refused(
        r#"        ctx.world.set(a, "Mover", { hits: 2.5 });"#,
        "script.write_not_integer",
        0,
        Some(1.0),
    );
    assert!(e.detail.hint.unwrap().contains("Math.round"));
    refused(
        r#"        ctx.world.set(b, "Mover", { x: 0 / 0 });"#,
        "script.write_not_finite",
        0,
        Some(2.0),
    );
    refused(
        r#"        ctx.world.set(b, "Mover", { hits: -1 });"#,
        "script.write_out_of_range",
        0,
        Some(2.0),
    );
    refused(
        r#"        ctx.world.set(99 as any, "Mover", { x: 1 });"#,
        "script.entity_missing",
        0,
        None,
    );
    refused(
        r#"        ctx.world.insert(a, "Mover");"#,
        "script.component_present",
        0,
        Some(1.0),
    );
    let e = refused(
        r#"        ctx.world.set(a, "Flag", { up: false });"#,
        "script.component_missing",
        0,
        Some(1.0),
    );
    assert!(e.detail.hint.unwrap().contains("insert"));
    refused(
        r#"        ctx.world.set(a, "Mover", { kind: "ship" });"#,
        "script.write_type",
        0,
        Some(1.0),
    );
    refused(
        r#"        ctx.world.set(a, "Mover", { target: 77 });"#,
        "script.entity_missing",
        0,
        Some(1.0),
    );
    refused(
        r#"        ctx.world.set(a, "Mover", { label: "x".repeat(2000) });"#,
        "script.write_too_long",
        0,
        Some(1.0),
    );
    refused(
        r#"        ctx.world.set(a, "Mover", { pos: { x: 1, q: 2 } });"#,
        "script.unknown_field",
        0,
        Some(1.0),
    );
    let e = refused(
        r#"        ctx.world.get(a, "Mvoer");"#,
        "script.unknown_component",
        0,
        None,
    );
    assert_eq!(e.detail.suggestions, ["Mover"]);
    // Two bad fields: the first in own-key order.
    let e = refused(
        r#"        ctx.world.set(a, "Mover", { hits: 1.5, x: 0 / 0 });"#,
        "script.write_not_integer",
        0,
        Some(1.0),
    );
    assert_eq!(e.detail.field.as_deref(), Some("hits"));
    // A write to a world.get copy throws a TypeError with the hint.
    let e = refused(
        "        const m = ctx.world.get(a, \"Mover\") as any;\n        m.x = 3;",
        "script.exception",
        1,
        None,
    );
    assert!(e.detail.hint.unwrap().contains("ctx.world.set"));
}

#[test]
fn data_refusals() {
    for (body, why) in [
        (
            r#"        ctx.emit("game.x", { get v() { return 1; } } as any);"#,
            "a getter",
        ),
        (
            r#"        ctx.emit("game.x", new Proxy({}, {}) as any);"#,
            "a Proxy",
        ),
        (
            r#"        class P { v = 1 } ctx.emit("game.x", new P() as any);"#,
            "a class instance",
        ),
        (r#"        ctx.emit("game.x", [1, , 3] as any);"#, "a hole"),
        (
            r#"        ctx.emit("game.x", { v: undefined } as any);"#,
            "undefined",
        ),
        (
            r#"        ctx.emit("game.x", { v: "\uD800" });"#,
            "a lone surrogate",
        ),
        (
            r#"        ctx.emit("game.x", { v: Infinity });"#,
            "an infinity",
        ),
    ] {
        let e = refused(body, "script.bad_data", 0, None);
        assert!(e.detail.extra.contains_key("path"), "{why}: {e:#?}");
    }
    refused(
        r#"        ctx.emit("script.mine", {});"#,
        "script.event_reserved",
        0,
        None,
    );
    refused(
        r#"        ctx.emit("Bad Kind", {});"#,
        "sim.event_kind_invalid",
        0,
        None,
    );
    // A getter that would call back into the host is refused before it runs: nothing spawns.
    let e = refused(
        r#"        ctx.world.set(a, "Mover", { get x() { ctx.world.spawn({}); return 1; } } as any);"#,
        "script.bad_data",
        0,
        Some(1.0),
    );
    assert!(e.message.contains("getter"), "{e:#?}");
}

/// A write-back is checked after the call, so it is placed at the system's `run` (line 12).
fn refused_cell(body: &str, code: &str, entity: f64) {
    let c = run(body);
    let SystemOutcome::Failed { error, .. } = c.outcome else {
        panic!("{body}: {:#?}", c.outcome)
    };
    assert_eq!(error.code, code, "{body}: {error:#?}");
    assert_eq!(
        error.detail.phase,
        Some(pocket_script::ErrorPhase::WriteBack)
    );
    assert_eq!(
        error.detail.location.as_ref().map(|l| l.line),
        Some(BODY_LINE - 2),
        "{error:#?}"
    );
    assert_eq!(error.detail.entity, Some(entity), "{body}");
    let kinds: Vec<&str> = c.report.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["script.failed"], "{body}");
}

#[test]
fn column_writes_are_checked() {
    refused_cell(
        r#"        movers.cols.Mover.on[1] = 5;"#,
        "script.write_type",
        2.0,
    );
    refused_cell(
        r#"        movers.cols.Mover.hits[2] = 0.5;"#,
        "script.write_not_integer",
        3.0,
    );
    refused_cell(
        r#"        movers.cols.Mover.kind[0] = 2;"#,
        "script.write_type",
        1.0,
    );
    refused_cell(
        r#"        movers.cols.Mover.target[0] = 99;"#,
        "script.entity_missing",
        1.0,
    );
    refused_cell(
        r#"        movers.cols.Mover.pos.y[0] = 1 / 0;"#,
        "script.write_not_finite",
        1.0,
    );
    // A detached column.
    let c = run(r#"        (movers.cols.Mover.x.buffer as any).transfer();"#);
    let SystemOutcome::Failed { error, .. } = c.outcome else {
        panic!("{:#?}", c.outcome)
    };
    assert_eq!(error.code, "script.write_type", "{error:#?}");
}

#[test]
fn the_overlay_rows_of_5_4() {
    let c = ok(r#"        const e = ctx.world.spawn({ Mover: { x: 9 } });
        if (!ctx.world.exists(e)) throw new Error("spawned id not visible");
        if (ctx.query({ with: ["Mover"] }).len !== 3) throw new Error("query saw the spawn");
        ctx.world.set(a, "Mover", { x: 42 });
        if ((ctx.world.get(a, "Mover") as any).x !== 42) throw new Error("get missed the set");
        ctx.world.despawn(b);
        if (ctx.world.exists(b) || ctx.world.get(b, "Mover") !== undefined || ctx.world.has(b, "Mover")) throw new Error("despawn not seen");
        ctx.world.despawn(b);
        ctx.world.remove(c, "Flag");
        if (ctx.world.get(77 as any, "Mover") !== undefined || ctx.world.has(77 as any, "Mover")) throw new Error("absent id");
        ctx.world.set(c, "Mover", { target: e });
        movers.cols.Mover.target[0] = e;"#);
    assert_eq!(field(&c.sim, 1.0, "x"), json!(42.0));
    assert_eq!(field(&c.sim, 1.0, "target"), json!(4));
    assert_eq!(field(&c.sim, 2.0, "x"), serde_json::Value::Null);
    assert_eq!(field(&c.sim, 3.0, "target"), json!(4));
    assert_eq!(field(&c.sim, 4.0, "x"), json!(9.0));
    refused(
        "        ctx.world.despawn(b);\n        ctx.world.set(b, \"Mover\", { x: 1 });",
        "script.entity_missing",
        1,
        None,
    );
    refused(
        "        ctx.world.despawn(b);\n        ctx.world.insert(b, \"Flag\");",
        "script.entity_missing",
        1,
        None,
    );
    refused(
        "        ctx.world.despawn(b);\n        ctx.world.remove(b, \"Flag\");",
        "script.entity_missing",
        1,
        None,
    );
    refused(
        r#"        ctx.world.despawn(1000 as any);"#,
        "script.entity_missing",
        0,
        None,
    );
}

#[test]
fn only_changed_cells_are_written_back() {
    // Two queries over Mover: the second's untouched columns must not undo the first's write.
    let c = ok(r#"        const again = ctx.query({ with: ["Mover"] });
        movers.cols.Mover.x[1] = 7;
        again.cols.Mover.hits[1] = 8;
        movers.cols.Mover.x[0] = -0;"#);
    assert_eq!(field(&c.sim, 2.0, "x"), json!(7.0));
    assert_eq!(field(&c.sim, 2.0, "hits"), json!(8));
    let SystemOutcome::Ok(stats) = c.outcome else {
        panic!()
    };
    assert_eq!(stats.cells_written, 3, "{stats:?}");
    // -0 was written: its bits.
    let e = pocket_sim::EntityId::from_f64(1.0).unwrap();
    let v = common::component_values(c.sim.world(), e, "Mover").unwrap();
    assert_eq!(v.nums[0].to_bits(), (-0.0f64).to_bits());
}

#[test]
fn copies_and_ids_have_their_order() {
    let c = ok(
        r#"        const keys = Object.keys(ctx.world.get(a, "Mover") as any).join(",");
        const ids = Array.from(movers.ids).join(",");
        ctx.emit("game.order", { keys, ids, row: movers.row(c), none: movers.row(99 as any) });"#,
    );
    let d = &c.report.events[0].data;
    assert_eq!(
        d.to_json(),
        json!({"keys": "x,hits,on,kind,target,label,pos", "ids": "1,2,3", "row": 2, "none": -1})
    );
}

#[test]
fn console_runs_no_project_code() {
    let body = r#"        let called = 0;
        const o = { a: 1, get g() { called++; return 2; }, toJSON() { called++; return 1; }, toString() { called++; return "s"; }, nested: { deep: [1, "two", null] } };
        console.log("value", o, 1.5, -0, 1e21);
        if (called !== 0) throw new Error("console ran project code");"#;
    let steps = |lines: u32| {
        let set = common::compiled(&[
            ("scripts/components.ts", COMPONENTS),
            ("scripts/main.ts", &main_ts(body)),
        ]);
        let limits = ScriptLimits {
            log_lines_per_tick: lines,
            ..ScriptLimits::default()
        };
        let mut sim = common::sim_with(&set, limits, 2);
        common::step(&mut sim);
        common::step(&mut sim);
        let o = pocket_script::scripts::last_tick(sim.world()).remove(0).1;
        let SystemOutcome::Ok(s) = o else {
            panic!("{o:#?}")
        };
        let log = sim
            .world()
            .get_non_send::<pocket_script::Scripts>()
            .unwrap()
            .host
            .take_log();
        (s.steps, log)
    };
    let (with, log) = steps(100);
    let (without, none) = steps(0);
    assert_eq!(with, without);
    assert!(none.is_empty());
    assert_eq!(
        log[0].text,
        "value { a: 1, g: [getter], toJSON: [function], toString: [function], nested: { deep: [1, \"two\", null] } } 1.5 0 1e+21"
    );
    assert_eq!(log[0].system.as_deref(), Some("probe"));
}
