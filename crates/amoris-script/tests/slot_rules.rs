//! Slot rules (script-host.md 7.2; test 5 of 12): an engine component whose Rust form its flat
//! slots cannot fully say registers a rule. A `set`, an `insert`, a `spawn` or a column write-back
//! the rule refuses fails its call with `script.write_not_held`, naming the component, the field,
//! the entity and the TypeScript line, and applies nothing; an accepted write reads back, later in
//! the same call and after the commit, as the rule settled it; a write that changes nothing
//! passes; the rule runs after its component's own field checks.
#![cfg(feature = "transpile")]
mod common;

use amoris_script::{
    EngineFns, ErrorPhase, ScriptError, ScriptLimits, SlotRefusal, SystemOutcome, register_engine,
    register_engine_rule,
};
use amoris_sim::registry::{
    ComponentOrigin, ComponentSchema, FieldType, FieldValue, ProjectValues,
};
use amoris_sim::{EntityId, PlainData, Sim, StepReport};

/// A reading that is low or high, never both: a tagged value, as a collider's shape is.
#[derive(
    Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
enum Level {
    Low(f64),
    High(f64),
}

/// An engine component whose flat form (`mode`, `low`, `high`) says more than it can hold.
#[derive(
    bevy_ecs::component::Component,
    Clone,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
struct Gauge {
    level: Level,
}

impl amoris_sim::Persisted for Gauge {
    const NAME: &'static str = "Gauge";
    const VERSION: u32 = 1;
}

fn read(g: &Gauge) -> ProjectValues {
    let nums = match g.level {
        Level::Low(x) => [0.0, x, 0.0],
        Level::High(x) => [1.0, 0.0, x],
    };
    ProjectValues {
        nums: nums.into(),
        strs: Box::new([]),
    }
}

/// In place: the level is replaced only when its slots changed.
fn write(g: &mut Gauge, v: &ProjectValues) {
    let now = read(g);
    let same = |(a, b): (&f64, &f64)| a.to_bits() == b.to_bits();
    if !v.nums.iter().zip(now.nums.iter()).all(same) {
        g.level = if v.nums[0] == 1.0 {
            Level::High(v.nums[2])
        } else {
            Level::Low(v.nums[1])
        };
    }
}

fn fns() -> EngineFns<Gauge> {
    EngineFns {
        read,
        write,
        make: |v| {
            let mut g = Gauge {
                level: Level::Low(0.0),
            };
            write(&mut g, v);
            g
        },
    }
}

/// `low` is held only in mode low and `high` only in mode high; the other one, left by a change of
/// mode, is settled to the 0 it reads.
fn rule(before: Option<&ProjectValues>, after: &mut ProjectValues) -> Result<(), SlotRefusal> {
    let (other, field, mode) = if after.nums[0] == 1.0 {
        (1, "low", "high")
    } else {
        (2, "high", "low")
    };
    let same = before.is_some_and(|b| b.nums[other].to_bits() == after.nums[other].to_bits());
    if after.nums[other] != 0.0 && !same {
        return Err(SlotRefusal {
            field,
            message: format!("Gauge.{field} is not held in mode {mode}."),
        });
    }
    after.nums[other] = 0.0;
    Ok(())
}

fn schema() -> ComponentSchema {
    ComponentSchema::new(
        "Gauge",
        ComponentOrigin::Engine,
        1,
        "A reading that is low or high.",
        vec![
            (
                "mode",
                FieldType::Enum(["low", "high"].map(Box::from).into()),
                FieldValue::Enum(0),
                "Which reading it holds.",
            ),
            (
                "low",
                FieldType::F64,
                FieldValue::F64(0.0),
                "The low reading.",
            ),
            (
                "high",
                FieldType::F64,
                FieldValue::F64(0.0),
                "The high reading.",
            ),
        ],
    )
    .unwrap()
}

fn main_ts(body: &str) -> String {
    format!(
        r#"import {{ game, system }} from "amoris";
const setup = system({{
    name: "setup", phase: "update", doc: "Spawns a low gauge and a high one.", when: "start",
    run(ctx) {{
        ctx.world.spawn({{ Gauge: {{ low: 3 }} }});
        ctx.world.spawn({{ Gauge: {{ mode: "high", high: 7 }} }});
    }},
}});
const probe = system({{
    name: "probe", phase: "update", doc: "The case under test.", when: {{ every: 2 }},
    queries: {{ gauges: {{ with: ["Gauge"] }} }},
    run(ctx, {{ gauges }}) {{ // RUN
        const [a, b] = [gauges.id(0), gauges.id(1)];
        const cols = gauges.cols.Gauge;
{body}
    }},
}});
export default game({{ systems: [setup, probe] }});
"#
    )
}

/// The 1-based line of main.ts that holds `mark`.
fn line_of(mark: &str) -> u32 {
    let text = main_ts("// BODY");
    let at = text.lines().position(|l| l.contains(mark)).unwrap();
    u32::try_from(at).unwrap() + 1
}

struct Case {
    sim: Sim,
    report: StepReport,
    outcome: SystemOutcome,
}

/// A world with `Gauge` registered (schema, slot functions and rule) before the scripts, stepped
/// through the setup's tick and the probe's.
fn run(body: &str) -> Case {
    let set = common::compiled(&[("scripts/main.ts", &main_ts(body))]);
    let mut sim = Sim::new(amoris_sim::SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 2,
    })
    .unwrap();
    amoris_script::install(&mut sim, ScriptLimits::default()).unwrap();
    amoris_sim::ComponentRegistry::register::<Gauge>(sim.world_mut(), Some(schema())).unwrap();
    register_engine::<Gauge>(sim.world_mut(), fns()).unwrap();
    register_engine_rule::<Gauge>(sim.world_mut(), rule).unwrap();
    amoris_script::swap(sim.world_mut(), &set, false).unwrap_or_else(|e| panic!("{e:#?}"));
    let first = common::step(&mut sim);
    assert!(first.errors.is_empty(), "{:#?}", first.errors);
    let report = common::step(&mut sim);
    let outcome = amoris_script::scripts::last_tick(sim.world()).remove(0).1;
    Case {
        sim,
        report,
        outcome,
    }
}

/// The gauge's slots, or `None` when the entity is not there.
fn gauge(sim: &Sim, id: u64) -> Option<Vec<f64>> {
    let id = EntityId::new(id).unwrap();
    common::component_values(sim.world(), id, "Gauge").map(|v| v.nums.to_vec())
}

/// A refused case: `script.write_not_held` for `Gauge.<field>` in `phase`, at `line` and for
/// `entity`, and nothing applied but the failure's own event.
fn refused(body: &str, field: &str, phase: ErrorPhase, line: u32, entity: Option<f64>) -> Case {
    let c = run(body);
    let SystemOutcome::Failed { error, .. } = &c.outcome else {
        panic!("{body}: {:#?}", c.outcome)
    };
    let error: &ScriptError = error;
    assert_eq!(error.code, "script.write_not_held", "{body}: {error:#?}");
    assert_eq!(error.detail.phase, Some(phase), "{body}: {error:#?}");
    assert_eq!(error.detail.component.as_deref(), Some("Gauge"), "{body}");
    assert_eq!(error.detail.field.as_deref(), Some(field), "{body}");
    assert_eq!(error.detail.entity, entity, "{body}: {error:#?}");
    let at = error.detail.location.clone().expect("a location");
    assert_eq!(
        (at.file.as_str(), at.line),
        ("scripts/main.ts", line),
        "{body}: {error:#?}"
    );
    let kinds: Vec<&str> = c.report.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["script.failed"], "{body}");
    // Neither gauge changed, and nothing was spawned.
    assert_eq!(gauge(&c.sim, 1), Some(vec![0.0, 3.0, 0.0]), "{body}");
    assert_eq!(gauge(&c.sim, 2), Some(vec![1.0, 0.0, 7.0]), "{body}");
    assert_eq!(gauge(&c.sim, 3), None, "{body}");
    // The report names the system, the entity and the cause.
    let p = &c.report.errors[0];
    assert_eq!(p.code, "sim.system_failed", "{p:#?}");
    assert_eq!(p.detail["system"], "script:probe", "{p:#?}");
    assert_eq!(p.detail["cause"]["code"], "script.write_not_held", "{p:#?}");
    assert_eq!(p.detail["cause"]["detail"]["field"], field, "{p:#?}");
    c
}

fn ok(body: &str) -> Case {
    let c = run(body);
    assert!(
        matches!(c.outcome, SystemOutcome::Ok(_)),
        "{body}: {:#?}",
        c.outcome
    );
    assert!(c.report.errors.is_empty(), "{body}: {:#?}", c.report.errors);
    c
}

#[test]
fn a_refused_set_insert_or_spawn_fails_at_its_line_and_applies_nothing() {
    let body = line_of("// BODY");
    // A set: the call's earlier, accepted write is discarded with it.
    let c = refused(
        "        ctx.world.set(a, \"Gauge\", { low: 5 });\n        ctx.world.set(a, \"Gauge\", { high: 1 });",
        "high",
        ErrorPhase::Run,
        body + 1,
        Some(1.0),
    );
    let SystemOutcome::Failed { error, .. } = &c.outcome else {
        unreachable!()
    };
    assert_eq!(error.message, "Gauge.high is not held in mode low.");
    // An insert, onto an entity the call spawned.
    refused(
        "        const e = ctx.world.spawn();\n        ctx.world.insert(e, \"Gauge\", { high: 2 });",
        "high",
        ErrorPhase::Run,
        body + 1,
        Some(3.0),
    );
    // A spawn: no entity yet.
    refused(
        r#"        ctx.world.spawn({ Gauge: { mode: "high", low: 1 } });"#,
        "low",
        ErrorPhase::Run,
        body,
        None,
    );
}

/// script-sandbox.md 5.1: a component's rule runs after that component's own fields and before
/// the next component of a spawn.
#[test]
fn the_rule_runs_after_its_components_fields() {
    let body = line_of("// BODY");
    // An unknown field after the value the rule would refuse: the field check comes first.
    let c = run(r#"        ctx.world.set(a, "Gauge", { high: 1, hihg: 2 });"#);
    let SystemOutcome::Failed { error, .. } = &c.outcome else {
        panic!("{:#?}", c.outcome)
    };
    assert_eq!(error.code, "script.unknown_field", "{error:#?}");
    // The rule's refusal comes before the next component of the spawn is looked at.
    refused(
        r#"        ctx.world.spawn({ Gauge: { high: 1 }, Nothing: {} });"#,
        "high",
        ErrorPhase::Run,
        body,
        None,
    );
    let c = run(r#"        ctx.world.spawn({ Nothing: {}, Gauge: { high: 1 } });"#);
    let SystemOutcome::Failed { error, .. } = &c.outcome else {
        panic!("{:#?}", c.outcome)
    };
    assert_eq!(error.code, "script.unknown_component", "{error:#?}");
}

/// A write-back is checked after the call, so it is placed at the system's `run`, with the row's
/// entity.
#[test]
fn a_refused_column_write_names_the_rows_entity() {
    let run_line = line_of("// RUN");
    refused(
        "        cols.high[0] = 4;",
        "high",
        ErrorPhase::WriteBack,
        run_line,
        Some(1.0),
    );
    refused(
        "        cols.low[0] = 4;\n        cols.low[1] = 4;",
        "low",
        ErrorPhase::WriteBack,
        run_line,
        Some(2.0),
    );
    // The cells' own checks come first, a later slot's too: `low` is not held in mode high, and
    // `high`, the slot after it, is not a number the world takes.
    let c = run("        cols.low[1] = 4;\n        cols.high[1] = 0 / 0;");
    let SystemOutcome::Failed { error, .. } = &c.outcome else {
        panic!("{:#?}", c.outcome)
    };
    assert_eq!(error.code, "script.write_not_finite", "{error:#?}");
}

#[test]
fn an_accepted_write_reads_back_as_the_rule_settled_it() {
    // A change of mode by `set`: the stale reading is 0 for the call's own later read and after
    // the commit.
    let c = ok(
        r#"        ctx.world.set(a, "Gauge", { mode: "high", high: 4 });
        const g = ctx.world.get(a, "Gauge")!;
        ctx.emit("game.saw", { mode: g.mode, low: g.low, high: g.high });"#,
    );
    assert_eq!(gauge(&c.sim, 1), Some(vec![1.0, 0.0, 4.0]));
    let saw = c
        .report
        .events
        .iter()
        .find(|e| e.kind.as_str() == "game.saw");
    let data = &saw.expect("game.saw").data;
    assert_eq!(data.get("mode"), Some(&PlainData::String("high".into())));
    assert_eq!(data.get("low"), Some(&PlainData::Number(0.0)));
    assert_eq!(data.get("high"), Some(&PlainData::Number(4.0)));
    // The same through columns, cell by cell, with the stale cell left as it was handed over.
    let c = ok("        cols.mode[1] = 0;\n        cols.low[1] = 2;");
    assert_eq!(gauge(&c.sim, 2), Some(vec![0.0, 2.0, 0.0]));
    assert_eq!(gauge(&c.sim, 1), Some(vec![0.0, 3.0, 0.0]));
    // An insert and a spawn hold what they were given.
    let c = ok(r#"        const e = ctx.world.spawn();
        ctx.world.insert(e, "Gauge", { mode: "high", high: 9 });
        ctx.world.spawn({ Gauge: { low: -1 } });"#);
    assert_eq!(gauge(&c.sim, 3), Some(vec![1.0, 0.0, 9.0]));
    assert_eq!(gauge(&c.sim, 4), Some(vec![0.0, -1.0, 0.0]));
}

#[test]
fn a_write_that_changes_nothing_passes() {
    let c = ok(
        r#"        ctx.world.set(a, "Gauge", { mode: "low", low: 3 });
        ctx.world.set(b, "Gauge", { low: 0 });
        ctx.world.set(b, "Gauge", {});
        cols.high[1] = 7;"#,
    );
    assert_eq!(gauge(&c.sim, 1), Some(vec![0.0, 3.0, 0.0]));
    assert_eq!(gauge(&c.sim, 2), Some(vec![1.0, 0.0, 7.0]));
}

/// A rule is registered for a component scripts can already name.
#[test]
fn a_rule_needs_its_components_accessor() {
    let mut sim = Sim::new(amoris_sim::SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 2,
    })
    .unwrap();
    amoris_script::install(&mut sim, ScriptLimits::default()).unwrap();
    let e = register_engine_rule::<Gauge>(sim.world_mut(), rule).unwrap_err();
    assert_eq!(e.code, "script.component_unregistered", "{e:#?}");
    amoris_sim::ComponentRegistry::register::<Gauge>(sim.world_mut(), Some(schema())).unwrap();
    let e = register_engine_rule::<Gauge>(sim.world_mut(), rule).unwrap_err();
    assert_eq!(e.code, "script.component_unregistered", "{e:#?}");
    register_engine::<Gauge>(sim.world_mut(), fns()).unwrap();
    register_engine_rule::<Gauge>(sim.world_mut(), rule).unwrap();
}
