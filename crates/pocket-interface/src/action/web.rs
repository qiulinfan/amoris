//! The action layer's web test (checks.md 7.2, `tests`): the sailing intents carried out in a world
//! with physics, perception and the sailing catalog, the world's hash at every tick and each
//! intent's end, as a report the WebAssembly build must reproduce byte for byte. The hash is FNV-1a
//! over the persisted sections `pocket_physics::probe`'s ledger encodes, with the action, time
//! and perception declarations added (`pocket-persist`, the engine's hash, is not linkable here).

use pocket_physics::Transform;
use pocket_physics::probe::Ledger;
use pocket_sim::{Name, NoHooks, Sim, SimConfig, TickRate};
use serde_json::{Value, json};

use super::request::Caller;
use super::state::IntentTable;
use crate::perception::{Observer, Perceivable, Vec3};

/// The ticks the report runs.
pub const TICKS: u64 = 3600;

/// The declarations the report's hash covers.
pub fn ledger() -> Ledger {
    let mut l = pocket_physics::probe::ledger();
    super::declare(&mut l);
    crate::time::turns::declare(&mut l);
    crate::perception::declare(&mut l);
    l
}

fn noop_writer(
    _: &mut bevy_ecs::prelude::World,
    _: pocket_sim::EntityId,
    _: &str,
    _: &str,
    _: &Value,
) -> Result<(), pocket_contract::Problem> {
    Ok(())
}

/// The sailing game's world: the Sloop on a beam reach in a 6 m/s westerly, the seat `skipper`'s
/// body, and a mark 80 m north of it.
pub fn world() -> Result<Sim, pocket_contract::Problem> {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })?;
    pocket_physics::plugin(&mut sim)?;
    let defs = crate::perception::load(crate::perception::probe::SAILING)?;
    let catalog = super::sailing::game_catalog(&defs)?;
    crate::perception::plugin(&mut sim, defs)?;
    super::plugin(&mut sim, catalog)?;
    crate::time::turns::plugin(&mut sim, crate::time::turns::TimeRules::default())?;
    sim.world_mut()
        .insert_resource(super::FieldWriter(noop_writer));
    let name = |n: &str| Name::new(n);
    let mut b = sim.boundary();
    b.spawn((name("Sea")?, pocket_physics::sailing::calm_sea()))?;
    b.spawn((name("Breeze")?, pocket_physics::sailing::breeze(270.0, 6.0)))?;
    let mut sloop = pocket_physics::sailing::sloop([0.0; 3], 0.0);
    sloop.6 = pocket_physics::Boat::sail_set();
    let id = b.spawn((name("Sloop")?, sloop))?;
    let e = b
        .entity(id)
        .ok_or_else(|| super::state::internal("action.web", "the Sloop is not live"))?;
    b.world_mut().entity_mut(e).insert((
        Observer {
            profile: "skipper".into(),
            seat: Some("skipper".into()),
            omniscient: false,
            team: None,
        },
        Perceivable {
            kind: "boat".into(),
            detect_m: 1500.0,
            height_m: 6.0,
            priority: 70,
            chart_m: None,
        },
    ));
    let at = [0.0, 0.0, -80.0];
    b.spawn((
        name("Mark1")?,
        Transform::at(at),
        Perceivable {
            kind: "mark".into(),
            detect_m: 400.0,
            height_m: 2.0,
            priority: 80,
            chart_m: Some(Vec3::of(at)),
        },
    ))?;
    Ok(sim)
}

/// The run: trim the sail and keep it trimmed, then at tick 600 sail to Mark1, then at its arrival
/// come to heading 90 and hold it. Every tick's hash and each intent's end.
pub fn run(ticks: u64) -> Result<(Vec<u64>, Vec<String>), String> {
    let mut sim = world().map_err(|p| p.message)?;
    let player = Caller::Player {
        seat: "skipper".into(),
    };
    let ledger = ledger();
    let act = |sim: &mut Sim, v: Value| {
        super::act(sim.world_mut(), &player, &v)
            .map(|_| ())
            .map_err(|p| format!("{v}: {} {}", p.code, p.message))
    };
    act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
    )?;
    let mut hashes = Vec::new();
    let mut turned = false;
    for t in 1..=ticks {
        if t == 600 {
            act(
                &mut sim,
                json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                                    "params": {"trim": "manual"}}]}),
            )?;
        }
        let arrived = sim
            .world()
            .resource::<IntentTable>()
            .by_id
            .get(&2)
            .is_some_and(|i| i.status == super::IntentStatus::Succeeded);
        if arrived && !turned {
            turned = true;
            act(
                &mut sim,
                json!({"actions": [{"do": "start", "intent": "come_to_heading",
                                    "params": {"heading_deg": 90, "keep": true}}]}),
            )?;
        }
        sim.step(&mut NoHooks).map_err(|p| p.message)?;
        hashes.push(ledger.snapshot(sim.world())?.hash());
    }
    let ends = sim
        .world()
        .resource::<IntentTable>()
        .by_id
        .values()
        .map(|i| {
            format!(
                "intent {} {} {} finished {:?}",
                i.id,
                i.intent,
                i.status.name(),
                i.finished_tick.map(|t| t.0)
            )
        })
        .collect();
    Ok((hashes, ends))
}

/// Whether the intents ended as they must (trim holding, `sail_to` succeeded, the heading
/// holding), natively and in WebAssembly alike.
pub fn check_ends(ends: &[String]) -> Result<(), String> {
    let want = [
        "intent 1 trim_sail holding",
        "intent 2 sail_to succeeded",
        "intent 3 come_to_heading holding",
    ];
    for (w, got) in want.iter().zip(ends) {
        if !got.starts_with(w) {
            return Err(format!("expected {w}, got {got}"));
        }
    }
    if ends.len() != want.len() {
        return Err(format!("{} intents: {ends:?}", ends.len()));
    }
    Ok(())
}

/// The report: `ok`/`FAIL` for the test, each intent's end, then `tick hash` for every tick.
pub fn web_report() -> String {
    let mut out = String::new();
    match run(TICKS) {
        Ok((hashes, ends)) => {
            match check_ends(&ends) {
                Ok(()) => out.push_str("ok actions.intents\n"),
                Err(e) => out.push_str(&format!("FAIL actions.intents: {e}\n")),
            }
            for e in ends {
                out.push_str(&e);
                out.push('\n');
            }
            for (i, h) in hashes.iter().enumerate() {
                out.push_str(&format!("{} {h:016x}\n", i + 1));
            }
        }
        Err(e) => out.push_str(&format!("FAIL actions.run: {e}\n")),
    }
    out
}
