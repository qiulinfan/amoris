//! Actions: controls, intents and their executors, affordances, the act request and what a replay
//! records (shared/contract/actions.md; slice 2, its decisions in shared/contract/actions.md, Open
//! choices, "Slice 2").
//!
//! - **World state** ([`state`]): a seat's body's `Controls` (pulses waiting, unbound latched
//!   values) and the `IntentTable` resource; a seat's body is the entity whose observer names it.
//! - **Declarations** ([`defs`], [`catalog`]): the game's controls with their bindings, intents with
//!   their executors, affordances by kind and codes, installed by [`plugin`] as an Ignored resource.
//! - **The act request** ([`act`]): validated whole in three phases ([`validate`]) against the
//!   seat's view ([`view`]), then applied whole at the boundary ([`apply`]); the canonical call is
//!   what the recorder keeps.
//! - **Executors** ([`executor`]): `interface.intents` in the `Control` phase delivers pulses, fails
//!   intents past their deadline and runs the Rust executors ([`sailing`]'s helm, sail and
//!   waypoint intents); script executors run in `script.update` through [`lifecycle`].

pub mod affordance;
pub mod apply;
pub mod budget;
pub mod catalog;
pub mod defs;
pub mod executor;
pub mod lifecycle;
pub mod observe;
pub mod refs;
pub mod request;
pub mod sailing;
pub mod state;
pub mod validate;
pub mod view;
pub mod web;

use bevy_ecs::prelude::World;
use pocket_contract::{CheckOptions, Pointer, Problem, decode};
use pocket_sim::persisted::RegisterPersisted;
use pocket_sim::{RunCondition, Sim, TickPhase};
use serde_json::{Map, Value, json};

pub use catalog::{ActionCatalog, Bodies, FieldWriter, IntentEntry, SeatDecl, ViewFactory};
pub use request::{ActRequest, Action, AffordancesRequest, Caller, IntentsRequest};
pub use state::{Controls, IntentInstance, IntentStatus, IntentTable, SeatRow, seats};

/// Installs the action layer into a world: the intent table (when the world has none), the game's
/// catalog, and `interface.intents` at its row of the system order (simulation.md 4.4, row 2).
pub fn plugin(sim: &mut Sim, catalog: ActionCatalog) -> Result<(), Problem> {
    let w = sim.world_mut();
    if !w.contains_resource::<state::IntentTable>() {
        w.insert_resource(state::IntentTable::default());
    }
    w.insert_resource(catalog);
    pocket_sim::ComponentRegistry::register::<state::Controls>(w, None)?;
    sim.add_exclusive(
        "interface.intents",
        TickPhase::Control,
        RunCondition::Always,
        |world, ctx| {
            if let Err(p) = executor::run_intents(world) {
                ctx.fault(p);
            }
        },
    )
}

/// Declares the action layer's persistence classes (persistence.md 2): controls and the intent
/// table persisted; the catalog and the field writer ignored.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    state::declare(r);
    r.ignore::<ActionCatalog>("the game's action declarations, installed by the game builder")
        .ignore::<FieldWriter>("a function the runtime installs to write project fields");
}

/// What an `act` call did: the answer's parts that come from the world, and the canonical call the
/// recorder keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct ActDone {
    pub request: ActRequest,
    pub seat: String,
    pub applied_at: pocket_sim::Tick,
    pub outcomes: Vec<Value>,
    pub warnings: Vec<Problem>,
    pub canonical: Value,
}

/// What applying a call can change, as it stood before: put back when applying fails (an engine
/// bug), so a refused call leaves the world as it was. Controls bound to a component field other
/// than an engine one are written only by pulses, which apply queues in `Controls`.
struct Before {
    table: IntentTable,
    controls: Option<Controls>,
    engine: Vec<(String, Option<state::ControlValue>)>,
    inbox: pocket_sim::EventInbox,
    counter: pocket_sim::EventCounter,
    turn: Option<crate::time::turns::TurnState>,
}

impl Before {
    fn take(world: &World, catalog: &ActionCatalog, v: &validate::Validated) -> Before {
        let body = v.seat.body;
        let engine = catalog
            .controls
            .iter()
            .filter(|c| matches!(c.binding, defs::ControlBinding::Engine { .. }))
            .map(|c| (c.name.clone(), catalog::read_control(world, c, body)))
            .collect();
        Before {
            table: world.resource::<IntentTable>().clone(),
            controls: pocket_sim::entity::entity(world, body)
                .and_then(|e| world.get::<Controls>(e).cloned()),
            engine,
            inbox: world.resource::<pocket_sim::EventInbox>().clone(),
            counter: *world.resource::<pocket_sim::EventCounter>(),
            turn: world
                .get_resource::<crate::time::turns::TurnState>()
                .cloned(),
        }
    }

    fn restore(self, world: &mut World, catalog: &ActionCatalog, v: &validate::Validated) {
        let body = v.seat.body;
        world.insert_resource(self.table);
        world.insert_resource(self.inbox);
        world.insert_resource(self.counter);
        if let Some(t) = self.turn {
            world.insert_resource(t);
        }
        if let Some(e) = pocket_sim::entity::entity(world, body) {
            match self.controls {
                Some(c) => {
                    world.entity_mut(e).insert(c);
                }
                None => {
                    world.entity_mut(e).remove::<Controls>();
                }
            }
        }
        for (name, value) in self.engine {
            if let (Some(def), Some(value)) = (catalog.control(&name), value) {
                let _ = catalog::write_control(world, def, body, &value);
            }
        }
    }
}

/// Validates `raw` for `caller` and applies it at the current boundary. On `Err` nothing was
/// applied: a call that fails while applying (an engine bug, `internal.error`) is undone whole,
/// its intents, controls, events and turn put back.
pub fn act(world: &mut World, caller: &Caller, raw: &Value) -> Result<ActDone, Problem> {
    let catalog = world
        .get_resource::<ActionCatalog>()
        .cloned()
        .ok_or_else(|| state::internal("act", "the game declares no actions"))?;
    crate::time::turns::check_episode(world)?;
    let (req, v) = validate::validate(world, &catalog, caller, raw)?;
    crate::time::turns::check_act(world, &v.seat.id, &req)?;
    let use_params: Vec<Option<Map<String, Value>>> = req
        .actions
        .iter()
        .map(|a| match a {
            Action::Use { params, .. } => Some(params.clone()),
            _ => None,
        })
        .collect();
    let before = Before::take(world, &catalog, &v);
    let done = match apply::apply(world, &catalog, &v, &use_params) {
        Ok(d) => d,
        Err(p) => {
            before.restore(world, &catalog, &v);
            return Err(p);
        }
    };
    Ok(ActDone {
        seat: v.seat.id.clone(),
        request: req,
        applied_at: done.applied_at,
        outcomes: done.outcomes,
        warnings: v.warnings,
        canonical: done.canonical,
    })
}

/// The seat's view of the world (the catalog's: its perception), or `None` when the seat has no
/// body.
fn seat_view<'w>(world: &'w World, seat: &str) -> Option<Box<dyn view::ActorView + 'w>> {
    let catalog = world.get_resource::<ActionCatalog>()?;
    let row = seats(world).ok()?.into_iter().find(|r| r.id == seat)?;
    Some((catalog.view)(world, &row))
}

/// An instance as callers see it (actions.md, `IntentView`): its target named as its seat knows
/// it, a failure whole. The name comes from the seat's view, never the world's `Name`, so the
/// answer for a target the seat no longer knows is the same whether it still exists or not.
pub fn intent_view(world: &World, i: &IntentInstance) -> Value {
    let view = seat_view(world, &i.seat);
    intent_view_in(world, view.as_deref(), i)
}

/// [`intent_view`] through a view already built for the instance's seat.
pub fn intent_view_in(
    world: &World,
    view: Option<&dyn view::ActorView>,
    i: &IntentInstance,
) -> Value {
    let mut v = json!({
        "id": i.id,
        "intent": i.intent,
        "status": i.status.name(),
        "params": i.params.to_json(),
        "started_tick": i.started_tick.0,
        "progress": state::readings_json(&i.progress),
    });
    if let Some(t) = &i.tag {
        v["tag"] = json!(t);
    }
    if let Some(t) = &i.target {
        v["target"] = match t {
            state::ResolvedTarget::Entity(e) => {
                let name = view.and_then(|v| v.known(*e)).and_then(|k| k.name);
                match name {
                    Some(n) => json!({"id": e.get(), "name": n}),
                    None => json!({"id": e.get()}),
                }
            }
            state::ResolvedTarget::Point(p) => json!({"x": p[0], "y": p[1], "z": p[2]}),
        };
    }
    if let Some(t) = i.finished_tick {
        v["finished_tick"] = json!(t.0);
    }
    if let Some(p) = lifecycle::failure(world, i.id) {
        v["failure"] = serde_json::to_value(p).unwrap_or(Value::Null);
    }
    if let Some(b) = i.superseded_by {
        v["superseded_by"] = json!(b);
    }
    v
}

/// `intents` (actions.md, Intent instances): the seat's intents as views, newest first, within
/// `budget_tokens` (the newest kept, the count of the older ones left out in `omitted`).
pub fn intents(world: &World, caller: &Caller, raw: &Value) -> Result<Value, Problem> {
    let decoded = decode::<IntentsRequest>(raw, &CheckOptions::new("the intents request"))?;
    let (req, warnings) = (decoded.value, decoded.warnings);
    let seat = validate::seat_for(world, caller, req.seat.as_deref(), "intents")?;
    let table = world.resource::<IntentTable>();
    let mut ids: Option<Vec<u64>> = None;
    if let Some(list) = &req.ids {
        let mut out = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let path = Pointer::root().key("ids").index(i);
            match r.id() {
                Some(id) if table.by_id.get(&id).is_some_and(|x| x.seat == seat.id) => out.push(id),
                Some(id) => return Err(pocket_contract::codes::unknown_intent_id(&path, id)),
                None => {
                    return Err(pocket_contract::codes::wrong_type(
                        &path,
                        "an intent id: a number or \"#n\"",
                        "string",
                    ));
                }
            }
        }
        ids = Some(out);
    }
    // Targets are named as the seat knows them (its view), never from the world.
    let view = world
        .get_resource::<ActionCatalog>()
        .map(|c| (c.view)(world, &seat));
    let views: Vec<Value> = table
        .by_id
        .values()
        .rev()
        .filter(|i| i.seat == seat.id)
        .filter(|i| ids.as_ref().is_none_or(|l| l.contains(&i.id)))
        .filter(|i| req.active_only != Some(true) || i.status.is_live())
        .map(|i| intent_view_in(world, view.as_deref(), i))
        .collect();
    let default = budget::default_for(world, &seat.id);
    budget::fit(
        head(world, &warnings),
        "intents",
        views,
        req.budget_tokens,
        default,
        true,
    )
}

/// The members every `intents` and `affordances` answer starts with.
fn head(world: &World, warnings: &[Problem]) -> Map<String, Value> {
    let tick = world.resource::<pocket_sim::SimClock>().tick.0;
    let mut m = Map::new();
    m.insert("tick".into(), json!(tick));
    m.insert("omniscient".into(), json!(false));
    if !warnings.is_empty() {
        m.insert(
            "warnings".into(),
            serde_json::to_value(warnings).unwrap_or(Value::Null),
        );
    }
    m
}

/// `affordances` (actions.md, Affordances): ranked as perception ranks percepts, within
/// `budget_tokens` (`omitted` counts what was left out, when anything was).
pub fn affordances(world: &World, caller: &Caller, raw: &Value) -> Result<Value, Problem> {
    let decoded = decode::<AffordancesRequest>(raw, &CheckOptions::new("the affordances request"))?;
    let (req, warnings) = (decoded.value, decoded.warnings);
    let seat = validate::seat_for(world, caller, req.seat.as_deref(), "affordances")?;
    let catalog = world
        .get_resource::<ActionCatalog>()
        .ok_or_else(|| state::internal("affordances", "the game declares no actions"))?;
    let view = (catalog.view)(world, &seat);
    let only = match &req.entity {
        Some(r) => Some(validate::resolve(
            &*view,
            r,
            &Pointer::root().key("entity"),
        )?),
        None => None,
    };
    let list = affordance::list(catalog, &*view, &seat, &req, only.as_ref());
    let default = budget::default_for(world, &seat.id);
    budget::fit(
        head(world, &warnings),
        "affordances",
        list,
        req.budget_tokens,
        default,
        false,
    )
}
