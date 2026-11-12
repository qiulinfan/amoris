//! Script intents (docs/spec/script-host.md 5.5; checks table, 10): `intent(name, def)`
//! declarations installed in the action catalog at the swap, an executor carrying out its
//! instances through its seat's controls, its commands applied at the commit with their lifecycle
//! events and not at all when the call fails, its restricted context, and `ctx.intent`.
#![cfg(feature = "transpile")]

mod common;

use std::sync::Arc;

use amoris_interface::action::catalog::{ActionCatalog, SeatDecl};
use amoris_interface::action::defs::{ControlBinding, ControlDef, ControlKind};
use amoris_interface::action::state::ControlValue;
use amoris_interface::action::view::{ActorView, Known, KnownEvent, Visibility};
use amoris_interface::action::{Caller, Controls, IntentStatus, IntentTable};
use amoris_script::{CompiledSet, ScriptLimits, SystemOutcome};
use amoris_sim::{EntityId, Name, NoHooks, PlainData, Sim, SimConfig, Tick, TickRate};
use bevy_ecs::prelude::World;
use serde_json::{Value, json};

/// A view that knows the entity named `Buoy`, 5 m north of the body, and reads one instrument.
struct Sight<'w> {
    world: &'w World,
    body: EntityId,
}

impl ActorView for Sight<'_> {
    fn tick(&self) -> Tick {
        self.world.resource::<amoris_sim::SimClock>().tick
    }
    fn body(&self) -> EntityId {
        self.body
    }
    fn instrument(&self, name: &str) -> Option<Value> {
        (name == "depth_m").then(|| json!(4.5))
    }
    fn known(&self, id: EntityId) -> Option<Known> {
        let e = amoris_sim::entity::entity(self.world, id)?;
        (self.world.get::<Name>(e)?.as_str() == "Buoy").then(|| Known {
            id,
            name: Some("Buoy".into()),
            kind: "buoy".into(),
            visibility: Visibility::Seen,
            pos_m: [0.0, 0.0, -5.0],
            range_m: 5.0,
            bearing_deg: 0.0,
            facts: vec![("lit".into(), json!(true))],
        })
    }
    fn all_known(&self) -> Vec<Known> {
        let index = self.world.resource::<amoris_sim::EntityIndex>();
        index.iter().filter_map(|(id, _)| self.known(id)).collect()
    }
    fn events_since(&self, _: u64) -> Vec<KnownEvent> {
        Vec::new()
    }
}

fn control(name: &str, kind: ControlKind, channel: &str) -> ControlDef {
    ControlDef {
        name: name.into(),
        doc: format!("The test's {name}."),
        kind,
        channel: channel.into(),
        binding: ControlBinding::Unbound,
    }
}

/// `Body` is the seat `hand`'s.
fn bodies(w: &World) -> Vec<(String, EntityId)> {
    let index = w.resource::<amoris_sim::EntityIndex>();
    index
        .iter()
        .filter(|(_, e)| w.get::<Name>(*e).is_some_and(|n| n.as_str() == "Body"))
        .map(|(id, _)| ("hand".to_owned(), id))
        .collect()
}

fn catalog() -> ActionCatalog {
    let mut c = ActionCatalog::empty(
        "test",
        bodies,
        Arc::new(|w: &World, row: &amoris_interface::action::SeatRow| {
            Box::new(Sight {
                world: w,
                body: row.body,
            }) as Box<dyn ActorView>
        }),
    );
    c.seats = vec![SeatDecl::any("hand")];
    c.controls = vec![
        control(
            "lever",
            ControlKind::Axis {
                min: 0.0,
                max: 100.0,
                default: 0.0,
            },
            "arm",
        ),
        control("button", ControlKind::Pulse { target_kinds: None }, "arm"),
        control("lamp", ControlKind::Toggle { default: false }, "dial"),
    ];
    c
}

/// The game's intents and the executor that carries them out: `push_lever` pushes the lever a
/// notch a tick until `to`, keeps its count in the instance's state and reports it; with `jam` it
/// rejects at the second notch; with `throw` it throws after staging progress.
const INTENTS: &str = r#"
import { executor, intent, param } from "amoris";

export const push = intent("push_lever", {
    doc: "Push the lever a notch a tick up to `to`.",
    params: {
        to: param.number("Where to stop.", { default: 3, min: 1, max: 50 }),
        jam: param.bool("Jam at the second notch.", { default: false }),
        throw: param.bool("Throw after the progress.", { default: false }),
        probe: param.choice(["none", "world", "emit", "lamp", "intent", "code", "reading"], "What to misuse.", { default: "none" }),
    },
    channels: ["arm"],
    canHold: true,
    timeoutS: 5,
    failures: [{ code: "lever.jammed", when: "Asked to jam.", detail: ["at"], template: "The lever jammed at {at}." }],
    progress: [{ name: "count", doc: "Notches so far.", unit: { unit: "count" }, precision: 0 }],
});

export const look = intent("look_at", {
    doc: "Note what the seat sees of the target, then succeed.",
    target: { entity: ["buoy"] },
    channels: ["dial"],
    timeoutS: 2,
});

export const pusher = executor({
    name: "pusher",
    phase: "update",
    doc: "Pushes levers and looks at buoys.",
    intents: [push, look],
    run(ctx) {
        for (const it of ctx.intents) {
            if (it.intent === "look_at") {
                const v = ctx.view(it);
                const t = it.target as { id: number };
                const p = v.percept(t.id as never);
                const near = v.nearby({ kinds: ["buoy"] });
                ctx.setState(it, { depth: v.instrument("depth_m") ?? null, name: p?.name ?? null, lit: p?.facts.lit ?? null, near: near.length });
                ctx.controls(it).set("lamp", true);
                ctx.succeed(it);
                continue;
            }
            const probe = it.params.probe;
            if (probe === "world") (ctx as unknown as { world: unknown }).world;
            if (probe === "emit") (ctx as unknown as { emit(k: string): void }).emit("game.x");
            if (probe === "intent") (ctx as unknown as { intent(a: number, k: string): void }).intent(1, "push_lever");
            if (probe === "lamp") ctx.controls(it).set("lamp", true);
            if (probe === "code") ctx.reject(it, "lever.unknown");
            if (probe === "reading") ctx.progress(it, { counted: 1 });
            const n = ((it.state as number | null) ?? 0) + 1;
            ctx.controls(it).set("lever", n);
            ctx.setState(it, n);
            ctx.progress(it, { count: n });
            if (it.params.throw) throw new Error("after the progress");
            if (it.params.jam && n === 2) ctx.reject(it, "lever.jammed", { at: n });
            else if (n >= (it.params.to as number)) ctx.reached(it);
        }
    },
});
"#;

const MAIN: &str = r#"
import { game } from "amoris";
import { pusher } from "./intents";
export default game({ systems: [pusher] });
"#;

fn set(main: &str) -> CompiledSet {
    common::compiled(&[("scripts/main.ts", main), ("scripts/intents.ts", INTENTS)])
}

/// A world with the test's catalog, `Body` and `Buoy`, and `main` installed.
fn world(main: &str) -> (Sim, EntityId) {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 5,
    })
    .unwrap();
    amoris_interface::action::plugin(&mut sim, catalog()).unwrap();
    amoris_script::install(&mut sim, ScriptLimits::default()).unwrap();
    let body = sim.boundary().spawn((Name::new("Body").unwrap(),)).unwrap();
    sim.boundary().spawn((Name::new("Buoy").unwrap(),)).unwrap();
    amoris_script::swap(sim.world_mut(), &set(main), false).unwrap_or_else(|e| panic!("{e:#?}"));
    (sim, body)
}

fn act(sim: &mut Sim, req: Value) -> u64 {
    let caller = Caller::Player {
        seat: "hand".into(),
    };
    let done = amoris_interface::action::act(sim.world_mut(), &caller, &req)
        .unwrap_or_else(|p| panic!("{p:#?}"));
    done.outcomes[0]["intent_id"].as_u64().unwrap()
}

fn start(sim: &mut Sim, params: Value) -> u64 {
    act(
        sim,
        json!({"actions": [{"do": "start", "intent": "push_lever", "params": params}]}),
    )
}

fn step(sim: &mut Sim) {
    sim.step(&mut NoHooks).unwrap_or_else(|p| panic!("{p:#?}"));
}

fn status(sim: &Sim, id: u64) -> IntentStatus {
    sim.world().resource::<IntentTable>().by_id[&id].status
}

fn lever(sim: &Sim, body: EntityId) -> Option<f64> {
    let e = amoris_sim::entity::require(sim.world(), body).unwrap();
    match sim.world().get::<Controls>(e)?.values.get("lever")? {
        ControlValue::Number(x) => Some(*x),
        _ => None,
    }
}

/// The kinds of the `intent.*` events the last tick appended.
fn intent_events(sim: &Sim) -> Vec<String> {
    sim.world()
        .resource::<amoris_sim::EventInbox>()
        .events()
        .iter()
        .map(|e| e.kind.as_str().to_owned())
        .filter(|k| k.starts_with("intent."))
        .collect()
}

/// The code the executor's last call failed with, if it failed.
fn failed(sim: &Sim) -> Option<String> {
    amoris_script::scripts::last_tick(sim.world())
        .into_iter()
        .find_map(|(_, o)| match o {
            SystemOutcome::Failed { error, .. } => Some(error.code),
            _ => None,
        })
}

#[test]
fn the_swap_declares_the_intents_in_the_catalog() {
    let (sim, _) = world(MAIN);
    let c = sim.world().resource::<ActionCatalog>();
    let push = c.intent("push_lever").expect("push_lever is declared");
    assert!(push.executor.is_none());
    assert_eq!(push.def.channels, ["arm"]);
    assert!(push.def.can_hold);
    assert_eq!(push.def.failures, ["lever.jammed", "intent.timeout"]);
    assert_eq!(
        push.def.defaults,
        *json!({"to": 3, "jam": false, "throw": false, "probe": "none", "timeout_s": 5,
                "keep": false})
        .as_object()
        .unwrap()
    );
    let schema = push.def.params.schema();
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(schema["properties"]["to"]["maximum"], json!(50));
    assert!(c.code("lever.jammed").is_some());
    assert_eq!(c.script_codes, ["lever.jammed"]);
    let look = c.intent("look_at").unwrap();
    assert_eq!(look.def.target.kinds(), ["buoy"]);
}

#[test]
fn an_executor_carries_out_its_intent_through_the_seats_controls() {
    let (mut sim, body) = world(MAIN);
    let id = start(&mut sim, json!({"to": 3}));
    assert_eq!(status(&sim, id), IntentStatus::Active);
    let mut levers = Vec::new();
    for _ in 0..3 {
        step(&mut sim);
        levers.push(lever(&sim, body));
    }
    assert_eq!(levers, [Some(1.0), Some(2.0), Some(3.0)]);
    assert_eq!(status(&sim, id), IntentStatus::Succeeded);
    let i = sim.world().resource::<IntentTable>().by_id[&id].clone();
    assert_eq!(i.state, PlainData::Number(3.0));
    assert_eq!(i.progress, [("count".to_owned(), PlainData::Number(3.0))]);
    assert_eq!(i.finished_tick, Some(Tick(3)));
    assert_eq!(intent_events(&sim), ["intent.succeeded"]);
    // Finished, it is handed to no executor: the next tick calls none.
    step(&mut sim);
    assert!(amoris_script::scripts::last_tick(sim.world()).is_empty());
    assert_eq!(lever(&sim, body), Some(3.0));
}

#[test]
fn reached_holds_with_keep_and_reject_fails_with_its_code() {
    let (mut sim, _) = world(MAIN);
    let held = start(&mut sim, json!({"to": 1, "keep": true}));
    step(&mut sim);
    assert_eq!(status(&sim, held), IntentStatus::Holding);
    assert_eq!(intent_events(&sim), ["intent.reached"]);
    // Holding, it is handed on; a new start on its channel supersedes it.
    let jam = start(&mut sim, json!({"jam": true, "to": 9}));
    assert_eq!(status(&sim, held), IntentStatus::Superseded);
    step(&mut sim);
    assert_eq!(status(&sim, jam), IntentStatus::Active);
    step(&mut sim);
    assert_eq!(status(&sim, jam), IntentStatus::Failed);
    let failure = amoris_interface::action::lifecycle::failure(sim.world(), jam).unwrap();
    assert_eq!(failure.code, "lever.jammed");
    assert_eq!(failure.message, "The lever jammed at 2.");
    assert_eq!(failure.detail["at"], json!(2));
    let failed: Vec<_> = sim
        .world()
        .resource::<amoris_sim::EventInbox>()
        .events()
        .iter()
        .filter(|e| e.kind.as_str() == "intent.failed")
        .map(|e| e.data.to_json())
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["code"], json!("lever.jammed"));
    assert_eq!(failed[0]["count"], json!(2));
}

#[test]
fn a_failed_call_changes_no_instance_and_deadlines_still_apply() {
    let (mut sim, body) = world(MAIN);
    let id = start(&mut sim, json!({"throw": true, "timeout_s": 0.05}));
    let before = sim.world().resource::<IntentTable>().by_id[&id].clone();
    step(&mut sim);
    assert_eq!(failed(&sim).as_deref(), Some("script.exception"));
    assert_eq!(sim.world().resource::<IntentTable>().by_id[&id], before);
    assert_eq!(lever(&sim, body), None);
    // 0.05 s at 60 ticks a second is three ticks from its start: interface.intents fails it.
    for _ in 0..3 {
        step(&mut sim);
    }
    assert_eq!(status(&sim, id), IntentStatus::Failed);
    let failure = amoris_interface::action::lifecycle::failure(sim.world(), id).unwrap();
    assert_eq!(failure.code, "intent.timeout");
}

#[test]
fn an_executors_context_is_restricted() {
    for (probe, code) in [
        ("world", "script.restricted"),
        ("emit", "script.restricted"),
        ("intent", "script.restricted"),
        ("lamp", "script.restricted"),
        ("code", "script.undeclared_code"),
        ("reading", "script.unknown_key"),
    ] {
        let (mut sim, body) = world(MAIN);
        let id = start(&mut sim, json!({"probe": probe}));
        step(&mut sim);
        assert_eq!(failed(&sim).as_deref(), Some(code), "{probe}");
        assert_eq!(status(&sim, id), IntentStatus::Active, "{probe}");
        assert_eq!(lever(&sim, body), None, "{probe}");
    }
}

#[test]
fn an_executor_sees_its_seats_perception() {
    let (mut sim, body) = world(MAIN);
    let id = act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "look_at", "target": "Buoy"}]}),
    );
    step(&mut sim);
    assert_eq!(failed(&sim), None);
    let i = sim.world().resource::<IntentTable>().by_id[&id].clone();
    assert_eq!(i.status, IntentStatus::Succeeded);
    assert_eq!(
        i.state.to_json(),
        json!({"depth": 4.5, "lit": true, "name": "Buoy", "near": 1})
    );
    let e = amoris_sim::entity::require(sim.world(), body).unwrap();
    assert_eq!(
        sim.world().get::<Controls>(e).unwrap().values.get("lamp"),
        Some(&ControlValue::Bool(true))
    );
}

/// A rule that starts `push_lever` for the body at the first tick with `ctx.intent`, then tries
/// the act each later tick names.
const ACTING: &str = r#"
import { game, system } from "amoris";
import { pusher } from "./intents";

const start = system({
    name: "start",
    phase: "update",
    doc: "Starts push_lever for entity 1, or tries what the tick asks.",
    run(ctx) {
        if (ctx.tick === 1) ctx.intent(1 as never, "push_lever", { to: 2 }, { tag: "npc" });
        if (ctx.tick === 5) ctx.intent(1 as never, "pull_lever");
        if (ctx.tick === 6) ctx.intent(2 as never, "push_lever");
        if (ctx.tick === 7) ctx.intent(99 as never, "push_lever");
        if (ctx.tick === 8) ctx.intent(1 as never, "look_at", {}, { target: 77 as never });
    },
});

export default game({ systems: [start, pusher] });
"#;

/// The line of `ACTING` that tick `tick`'s act is on.
fn acting_line(tick: u64) -> u32 {
    let needle = format!("ctx.tick === {tick})");
    let at = ACTING.lines().position(|l| l.contains(&needle)).unwrap();
    u32::try_from(at + 1).unwrap()
}

#[test]
fn ctx_intent_starts_an_intent_at_the_commit_and_throws_a_refusal_at_its_line() {
    let (mut sim, body) = world(ACTING);
    step(&mut sim);
    assert_eq!(failed(&sim), None);
    let table = sim.world().resource::<IntentTable>();
    let (&id, i) = table
        .by_id
        .iter()
        .next()
        .expect("ctx.intent started an intent");
    assert_eq!(
        (i.intent.as_str(), i.tag.as_deref()),
        ("push_lever", Some("npc"))
    );
    // Applied at the commit of tick 1, it starts in tick 2, so the executor did not run it yet.
    assert_eq!(i.started_tick, Tick(2));
    assert_eq!(intent_events(&sim), ["intent.started"]);
    assert_eq!(lever(&sim, body), None);
    step(&mut sim);
    step(&mut sim);
    assert_eq!(status(&sim, id), IntentStatus::Succeeded);
    assert_eq!(lever(&sim, body), Some(2.0));
    step(&mut sim);
    for (tick, code) in [
        (5, "action.unknown_intent"),
        (6, "script.no_seat"),
        (7, "script.entity_missing"),
        (8, "perception.unknown_entity"),
    ] {
        step(&mut sim);
        assert_eq!(
            sim.world().resource::<amoris_sim::SimClock>().tick,
            Tick(tick)
        );
        let error = amoris_script::scripts::last_tick(sim.world())
            .into_iter()
            .find_map(|(_, o)| match o {
                SystemOutcome::Failed { error, .. } => Some(error),
                _ => None,
            })
            .unwrap_or_else(|| panic!("tick {tick} refused nothing"));
        assert_eq!(error.code, code, "tick {tick}");
        let at = error.detail.location.expect("the refusal has its line");
        assert_eq!(
            (at.file.as_str(), at.line),
            ("scripts/main.ts", acting_line(tick))
        );
    }
    assert_eq!(sim.world().resource::<IntentTable>().by_id.len(), 1);
}

#[test]
fn a_program_whose_intents_do_not_hold_together_is_refused() {
    let bad = |intents: &str| {
        let main = format!(
            "import {{ executor, game, intent, param }} from \"amoris\";\nexport const p = param;\n{intents}\n\
             const x = executor({{ name: \"x\", phase: \"update\", doc: \"\", intents: [i], run() {{}} }});\n\
             export default game({{ systems: [x] }});\n"
        );
        let (mut sim, _) = world(MAIN);
        let set = common::compiled(&[("scripts/main.ts", &main)]);
        let errors = amoris_script::swap(sim.world_mut(), &set, false).unwrap_err();
        // The world keeps its program and its catalog.
        assert!(
            sim.world()
                .resource::<ActionCatalog>()
                .intent("push_lever")
                .is_some()
        );
        errors[0].message.clone()
    };
    let m = bad(r#"const i = intent("steer", { doc: "", channels: ["helm"], timeoutS: 1 });"#);
    assert!(m.contains("channel helm"), "{m}");
    let m = bad(
        r#"const i = intent("steer", { doc: "", channels: ["arm"], timeoutS: 1, failures: ["steer.lost"] });"#,
    );
    assert!(m.contains("steer.lost"), "{m}");
    let m = bad(
        r#"const i = intent("steer", { doc: "", channels: ["arm"], timeoutS: 1, failures: ["action.conflict"] });"#,
    );
    assert!(m.contains("engine's family"), "{m}");
    let m = bad(
        r#"const i = intent("steer", { doc: "", channels: ["arm"], timeoutS: 1, params: { keep: param.bool("") } });"#,
    );
    assert!(m.contains("parameter keep"), "{m}");
    let m =
        bad(r#"const i = intent("steer", { doc: "", channels: ["arm"], timeoutS: 1, speed: 3 });"#);
    assert!(m.contains("speed"), "{m}");
}

/// A world without a player layer (as a replay's starts) takes the program; its intents join the
/// catalog when the layer's declarations come back (`scripts::declare_intents`). The runtime
/// refuses a game without one whose scripts declare intents.
#[test]
fn a_world_without_a_catalog_takes_the_intents_when_one_comes() {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 5,
    })
    .unwrap();
    amoris_script::install(&mut sim, ScriptLimits::default()).unwrap();
    amoris_script::swap(sim.world_mut(), &set(MAIN), false).unwrap();
    assert!(!sim.world().contains_resource::<ActionCatalog>());
    assert_eq!(
        amoris_script::scripts::live_intents(sim.world()),
        ["push_lever", "look_at"]
    );
    sim.world_mut().insert_resource(catalog());
    amoris_script::scripts::declare_intents(sim.world_mut()).unwrap();
    let c = sim.world().resource::<ActionCatalog>();
    assert!(c.intent("push_lever").is_some() && c.code("lever.jammed").is_some());
}

/// The executor of version 2 counts by tens: a hot update mid-intent hands it the state.
const PUSHER_BY_TENS: &str = r#"
import { executor, game } from "amoris";
import { push, look } from "./intents";
const pusher = executor({
    name: "pusher",
    phase: "update",
    doc: "Pushes by tens.",
    intents: [push, look],
    run(ctx) {
        for (const it of ctx.intents) {
            const n = ((it.state as number | null) ?? 0) + 10;
            ctx.controls(it).set("lever", n);
            ctx.setState(it, n);
        }
    },
});
export default game({ systems: [pusher] });
"#;

#[test]
fn a_hot_update_hands_live_instances_to_the_new_executor() {
    let (mut sim, body) = world(MAIN);
    let id = start(&mut sim, json!({"to": 50}));
    step(&mut sim);
    step(&mut sim);
    assert_eq!(lever(&sim, body), Some(2.0));
    amoris_script::swap(sim.world_mut(), &set(PUSHER_BY_TENS), false).unwrap();
    step(&mut sim);
    assert_eq!(lever(&sim, body), Some(12.0));
    assert_eq!(status(&sim, id), IntentStatus::Active);
    // A program that declares no intents takes them out of the catalog; the instance waits for
    // its deadline, carried out by no one.
    let none = common::compiled(&[(
        "scripts/main.ts",
        "import { game } from \"amoris\";\nexport default game({ systems: [] });\n",
    )]);
    amoris_script::swap(sim.world_mut(), &none, false).unwrap();
    assert!(
        sim.world()
            .resource::<ActionCatalog>()
            .intent("push_lever")
            .is_none()
    );
    assert!(
        sim.world()
            .resource::<ActionCatalog>()
            .script_codes
            .is_empty()
    );
    step(&mut sim);
    assert_eq!(lever(&sim, body), Some(12.0));
    assert_eq!(status(&sim, id), IntentStatus::Active);
}

#[test]
fn the_same_starts_give_the_same_world() {
    let run = || {
        let (mut sim, body) = world(MAIN);
        let mut seen = Vec::new();
        for t in 0..12u64 {
            if t % 4 == 0 {
                start(&mut sim, json!({"to": 2.0 + (t / 4) as f64, "jam": t == 8}));
            }
            step(&mut sim);
            seen.push((common::hash(sim.world()), lever(&sim, body)));
        }
        let table = sim.world().resource::<IntentTable>().clone();
        (seen, table)
    };
    assert_eq!(run(), run());
}
