//! Applying a validated `act` call at a tick boundary (shared/contract/actions.md, When an action
//! takes effect, Channels and supersession): in action order, each control latched or pulse queued
//! after the intent holding its channel is superseded, each intent started after the live ones
//! sharing a channel with it are, each cancel ended. The answer's outcomes and the canonical form
//! the recorder keeps (What a replay records) come out together.

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::{PlainData, SimClock, Tick};
use serde_json::{Map, Value, json};

use super::catalog::{ActionCatalog, param_f64, queue_pulse, write_control};
use super::executor::{channels_of, deadline};
use super::lifecycle::{self, End};
use super::state::{IntentInstance, IntentStatus, IntentTable, ResolvedTarget, internal};
use super::validate::{Planned, StartPlan, Validated};
use super::view::Known;

/// What an applied call answers and records.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCall {
    /// One outcome per action, in order (actions.md, The answer).
    pub outcomes: Vec<Value>,
    /// The call as the recorder keeps it: the seat named, parameters canonical, references ids.
    pub canonical: Value,
    /// The first tick the actions act in.
    pub applied_at: Tick,
}

/// The live intents of `seat` occupying `channel`, in id order.
fn holding_channel(world: &World, catalog: &ActionCatalog, seat: &str, channel: &str) -> Vec<u64> {
    world
        .resource::<IntentTable>()
        .live_of(seat)
        .filter(|i| {
            channels_of(catalog, &i.intent, &i.params)
                .iter()
                .any(|c| c == channel)
        })
        .map(|i| i.id)
        .collect()
}

fn supersede(
    world: &mut World,
    catalog: &ActionCatalog,
    seat: &str,
    channels: &[String],
    by: Option<u64>,
) -> Vec<u64> {
    let mut ids: Vec<u64> = channels
        .iter()
        .flat_map(|c| holding_channel(world, catalog, seat, c))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    for &id in &ids {
        lifecycle::end(world, id, End::Superseded(by));
    }
    ids
}

fn entity_json(k: &Known) -> Value {
    serde_json::to_value(k.entity_name()).unwrap_or(Value::Null)
}

fn target_json(t: &ResolvedTarget, known: Option<&Known>) -> Value {
    match (t, known) {
        (_, Some(k)) => entity_json(k),
        (ResolvedTarget::Entity(e), None) => json!({"id": e.get()}),
        (ResolvedTarget::Point(p), None) => {
            json!({"x": pocket_contract::codes::num(p[0]), "y": pocket_contract::codes::num(p[1]),
                   "z": pocket_contract::codes::num(p[2])})
        }
    }
}

fn canonical_target(t: &ResolvedTarget) -> Value {
    match t {
        ResolvedTarget::Entity(e) => json!(e.get()),
        ResolvedTarget::Point(p) => json!({"x": p[0], "y": p[1], "z": p[2]}),
    }
}

/// Applies `v` (validated against this world at this boundary) for its seat. A failure here is an
/// engine bug (`internal.error`); the caller rolls the boundary back.
pub fn apply(
    world: &mut World,
    catalog: &ActionCatalog,
    v: &Validated,
    use_params: &[Option<Map<String, Value>>],
) -> Result<AppliedCall, Problem> {
    let clock = *world.resource::<SimClock>();
    let next = Tick(clock.tick.0 + 1);
    let mut outcomes = Vec::new();
    let mut canonical = Vec::new();
    for (i, step) in v.steps.iter().enumerate() {
        let given = use_params.get(i).cloned().flatten();
        let (o, c) = apply_one(world, catalog, v, step, next, clock.rate.0, given)?;
        outcomes.push(o);
        canonical.push(c);
    }
    Ok(AppliedCall {
        outcomes,
        canonical: json!({"seat": v.seat.id, "actions": canonical}),
        applied_at: next,
    })
}

fn apply_one(
    world: &mut World,
    catalog: &ActionCatalog,
    v: &Validated,
    step: &Planned,
    next: Tick,
    rate: u32,
    use_params: Option<Map<String, Value>>,
) -> Result<(Value, Value), Problem> {
    let seat = &v.seat;
    match step {
        Planned::Set { controls } => {
            let mut superseded = Vec::new();
            let mut shown = Map::new();
            for (name, value) in controls {
                let def = catalog
                    .control(name)
                    .ok_or_else(|| internal("act", "a validated control vanished"))?;
                superseded.extend(supersede(
                    world,
                    catalog,
                    &seat.id,
                    std::slice::from_ref(&def.channel),
                    None,
                ));
                write_control(world, def, seat.body, value)?;
                shown.insert(name.clone(), value.to_json());
            }
            Ok((
                json!({"did": "set", "controls": shown, "superseded": superseded}),
                json!({"do": "set", "controls": shown}),
            ))
        }
        Planned::Pulse { control, target } => {
            let def = catalog
                .control(control)
                .ok_or_else(|| internal("act", "a validated control vanished"))?;
            let superseded = supersede(
                world,
                catalog,
                &seat.id,
                std::slice::from_ref(&def.channel),
                None,
            );
            queue_pulse(world, def, seat.body, target.as_ref().map(|k| k.id))?;
            let mut o = json!({"did": "pulse", "control": control, "superseded": superseded});
            let mut c = json!({"do": "pulse", "control": control});
            if let Some(k) = target {
                o["target"] = entity_json(k);
                c["target"] = json!(k.id.get());
            }
            Ok((o, c))
        }
        Planned::Start(plan) => start(world, catalog, &seat.id, seat.body, plan, next, rate),
        Planned::Use { entity, verb, then } => {
            let (inner, _) = apply_one(world, catalog, v, then, next, rate, None)?;
            let mut c = json!({"do": "use", "entity": entity.id.get(), "verb": verb});
            if let Some(p) = use_params.filter(|p| !p.is_empty()) {
                c["params"] = Value::Object(p);
            }
            if let Planned::Start(StartPlan { tag: Some(t), .. }) = &**then {
                c["tag"] = json!(t);
            }
            Ok((
                json!({"did": "use", "entity": entity_json(entity), "verb": verb, "then": inner}),
                c,
            ))
        }
        Planned::Cancel { intent_id } => {
            let live = world
                .resource::<IntentTable>()
                .by_id
                .get(intent_id)
                .is_some_and(|i| i.status.is_live());
            if live {
                lifecycle::end(world, *intent_id, End::Cancelled);
            }
            let status = world
                .resource::<IntentTable>()
                .by_id
                .get(intent_id)
                .map_or(IntentStatus::Cancelled, |i| i.status);
            Ok((
                json!({"did": "cancel", "intent_id": intent_id, "status": status.name()}),
                json!({"do": "cancel", "intent_id": intent_id}),
            ))
        }
        Planned::EndTurn => {
            let turn = crate::time::turns::end_turn(world, &seat.id)?;
            Ok((
                json!({"did": "end_turn", "turn": turn}),
                json!({"do": "end_turn"}),
            ))
        }
    }
}

fn start(
    world: &mut World,
    catalog: &ActionCatalog,
    seat: &str,
    body: pocket_sim::EntityId,
    plan: &StartPlan,
    next: Tick,
    rate: u32,
) -> Result<(Value, Value), Problem> {
    let entry = catalog
        .intent(&plan.intent)
        .ok_or_else(|| internal("act", "a validated intent vanished"))?;
    let id = {
        let mut t = world.resource_mut::<IntentTable>();
        let id = t.next_id;
        t.next_id += 1;
        id
    };
    let superseded = supersede(world, catalog, seat, &plan.channels, Some(id));
    let params = PlainData::from_json(&Value::Object(plan.params.clone()));
    let timeout_s = param_f64(&params, "timeout_s").unwrap_or(entry.def.default_timeout_s);
    let progress = lifecycle::round_progress(&entry.def.progress, plan.accepted.progress.clone());
    let instance = IntentInstance {
        id,
        seat: seat.to_owned(),
        actor: body,
        intent: plan.intent.clone(),
        tag: plan.tag.clone(),
        params,
        target: plan.target,
        started_tick: next,
        deadline_tick: Some(deadline(next, timeout_s, rate)),
        status: IntentStatus::Active,
        finished_tick: None,
        failure: None,
        superseded_by: None,
        progress: progress.clone(),
        state: plan.accepted.state.clone(),
    };
    world
        .resource_mut::<IntentTable>()
        .by_id
        .insert(id, instance);
    lifecycle::started(world, id);
    let mut o = json!({
        "did": "start", "intent_id": id, "intent": plan.intent, "params": plan.params,
        "superseded": superseded,
        "progress": super::state::readings_json(&progress),
    });
    let mut c = json!({"do": "start", "intent": plan.intent, "params": plan.params});
    if let Some(t) = &plan.tag {
        o["tag"] = json!(t);
        c["tag"] = json!(t);
    }
    if let Some(t) = &plan.target {
        o["target"] = target_json(t, plan.target_known.as_ref());
        c["target"] = canonical_target(t);
    }
    Ok((o, c))
}
