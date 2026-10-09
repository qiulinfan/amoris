//! The game's action declarations with the code behind them: every control with its binding, every
//! intent with its executor (Rust, or none for an intent a script executor carries out), the
//! affordances of each kind, the game's codes, and how an actor's view is built. A resource of
//! class Ignored: the game builder installs it in every world, its fork and its replay, so it is
//! never world state (persistence.md 2).

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::{Resource, World};
use pocket_contract::{Candidate, Problem};
use pocket_sim::{EntityId, PlainData};
use serde_json::Value;

use super::defs::{AffordanceDef, CodeDef, ControlBinding, ControlDef, IntentDef};
use super::executor::IntentExecutor;
use super::state::{ControlValue, Controls, SeatRow, StoredProblem, shown};
use super::view::ActorView;

/// Builds an actor's view of the world: the seat's perception (actions.md, Executors, rule 1).
pub type ViewFactory =
    Arc<dyn for<'w> Fn(&'w World, &SeatRow) -> Box<dyn ActorView + 'w> + Send + Sync>;

/// The seats' bodies, `(seat, body)` in `EntityId` order: the entities whose observer names a seat
/// (perception.md, Observers).
pub type Bodies = fn(&World) -> Vec<(String, EntityId)>;

/// A seat the game declares (README, Seats and callers): its id, and the controls and intents it
/// may use (`None`: all the game declares). Declared order is seat order.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatDecl {
    pub id: String,
    pub controls: Option<Vec<String>>,
    pub intents: Option<Vec<String>>,
}

impl SeatDecl {
    /// A seat that may use every control and intent.
    pub fn any(id: &str) -> SeatDecl {
        SeatDecl {
            id: id.to_owned(),
            controls: None,
            intents: None,
        }
    }
}

/// One declared intent: its declaration, and its Rust executor, or `None` when a script executor
/// carries it out (script-host.md 5.5): `interface.intents` checks its deadline and leaves the rest
/// to `script.update`.
#[derive(Clone)]
pub struct IntentEntry {
    pub def: IntentDef,
    pub executor: Option<Arc<dyn IntentExecutor>>,
}

/// Writes a component's field by name: a project component the action layer cannot name in Rust
/// (`Crew.take`). The runtime, which links the script host, installs it (architecture.md 4.5: the
/// interface never calls into the script host). Class Ignored.
#[derive(Resource, Clone, Copy)]
pub struct FieldWriter(pub fn(&mut World, EntityId, &str, &str, &Value) -> Result<(), Problem>);

/// The action declarations of the game a world runs. Class Ignored.
#[derive(Resource, Clone)]
pub struct ActionCatalog {
    pub game: String,
    /// In seat order.
    pub seats: Vec<SeatDecl>,
    pub bodies: Bodies,
    /// In declared order.
    pub controls: Vec<ControlDef>,
    /// In declared order.
    pub intents: Vec<IntentEntry>,
    /// By kind, each kind's verbs in declared order.
    pub affordances: BTreeMap<String, Vec<AffordanceDef>>,
    pub codes: Vec<CodeDef>,
    pub view: ViewFactory,
}

/// The contract's own failure templates (errors.md), for failures kept in the world.
const CONTRACT_TEMPLATES: [(&str, &str); 4] = [
    (
        "intent.timeout",
        "{intent} did not finish within {timeout_s} s.",
    ),
    (
        "perception.target_lost",
        "{target} is no longer known to you.",
    ),
    (
        "internal.error",
        "The engine failed in {where}; this is a bug, reported as {report}.",
    ),
    (
        "perception.unknown_entity",
        "No entity '{ref}' is known to you; did you mean {suggestions}?",
    ),
];

impl ActionCatalog {
    /// A catalog with nothing declared, finding bodies with `bodies` and building views with
    /// `view`.
    pub fn empty(game: &str, bodies: Bodies, view: ViewFactory) -> ActionCatalog {
        ActionCatalog {
            game: game.to_owned(),
            seats: Vec::new(),
            bodies,
            controls: Vec::new(),
            intents: Vec::new(),
            affordances: BTreeMap::new(),
            codes: Vec::new(),
            view,
        }
    }

    /// A catalog that declares nothing and has no seats.
    pub fn none() -> ActionCatalog {
        ActionCatalog::empty(
            "",
            |_| Vec::new(),
            Arc::new(|w: &World, row: &SeatRow| -> Box<dyn ActorView + '_> {
                Box::new(super::view::Nothing::new(w, row.body))
            }),
        )
    }

    pub fn control(&self, name: &str) -> Option<&ControlDef> {
        self.controls.iter().find(|c| c.name == name)
    }

    pub fn intent(&self, name: &str) -> Option<&IntentEntry> {
        self.intents.iter().find(|i| i.def.name == name)
    }

    pub fn code(&self, code: &str) -> Option<&CodeDef> {
        self.codes.iter().find(|c| c.code == code)
    }

    /// The affordances of `kind`.
    pub fn affordances_of(&self, kind: &str) -> &[AffordanceDef] {
        self.affordances.get(kind).map_or(&[], Vec::as_slice)
    }

    /// The affordance of some kind whose effect pulses `control`, with its kind.
    pub fn pulse_affordance(&self, kind: &str, control: &str) -> Option<&AffordanceDef> {
        self.affordances_of(kind).iter().find(
            |a| matches!(&a.effect, super::defs::Effect::Pulse { control: c } if c == control),
        )
    }

    /// Declares an intent a script executor carries out (script-host.md 5.5); a later declaration
    /// of the same name replaces it (a hot update).
    pub fn declare_script_intent(&mut self, def: IntentDef) {
        self.intents.retain(|i| i.def.name != def.name);
        self.intents.push(IntentEntry {
            def,
            executor: None,
        });
    }

    /// The control names a seat may set, for suggestions.
    pub fn controls_for<'a>(&'a self, seat: &SeatRow) -> Vec<Candidate<'a>> {
        self.controls
            .iter()
            .filter(|c| seat.may_control(&c.name))
            .map(|c| Candidate::new(&c.name))
            .collect()
    }

    /// The intent names a seat may start, for suggestions.
    pub fn intents_for<'a>(&'a self, seat: &SeatRow) -> Vec<Candidate<'a>> {
        self.intents
            .iter()
            .filter(|i| seat.may_intend(&i.def.name))
            .map(|i| Candidate::new(&i.def.name))
            .collect()
    }

    /// The control `name` when the seat may use it and its body can take it (a sailing control on
    /// a body with a `Boat`): the seat's controls are those its body takes.
    pub fn seat_control(&self, world: &World, seat: &SeatRow, name: &str) -> Option<&ControlDef> {
        self.control(name)
            .filter(|c| seat.may_control(name) && c.binding.writable(world, seat.body))
    }

    /// The control names of [`ActionCatalog::seat_control`], for suggestions.
    pub fn seat_controls<'a>(&'a self, world: &World, seat: &SeatRow) -> Vec<Candidate<'a>> {
        self.controls
            .iter()
            .filter(|c| self.seat_control(world, seat, &c.name).is_some())
            .map(|c| Candidate::new(&c.name))
            .collect()
    }

    /// The intent `name` when the seat may start it and its body takes every control on the
    /// intent's channels, which its executor writes (`trim_sail` needs a body with a sail).
    pub fn seat_intent(&self, world: &World, seat: &SeatRow, name: &str) -> Option<&IntentEntry> {
        let entry = self.intent(name).filter(|_| seat.may_intend(name))?;
        let takes = self
            .controls
            .iter()
            .filter(|c| entry.def.channels.contains(&c.channel))
            .all(|c| c.binding.writable(world, seat.body));
        takes.then_some(entry)
    }

    /// The intent names of [`ActionCatalog::seat_intent`], for suggestions.
    pub fn seat_intents<'a>(&'a self, world: &World, seat: &SeatRow) -> Vec<Candidate<'a>> {
        self.intents
            .iter()
            .filter(|i| self.seat_intent(world, seat, &i.def.name).is_some())
            .map(|i| Candidate::new(&i.def.name))
            .collect()
    }

    /// A stored problem shown: its message rendered from the game's template for its code, or the
    /// contract's.
    pub fn render(&self, stored: &StoredProblem) -> Problem {
        let detail = match stored.detail.to_json() {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let template = self
            .code(&stored.code)
            .map(|c| c.template.clone())
            .or_else(|| {
                CONTRACT_TEMPLATES
                    .iter()
                    .find(|(c, _)| *c == stored.code)
                    .map(|(_, t)| (*t).to_owned())
            });
        match template {
            Some(t) => Problem::from_template(&stored.code, &t, detail),
            None => shown(stored, format!("The intent failed with {}.", stored.code)),
        }
    }

    /// A game code's problem over `detail` (`internal.error` for a code the game never declared, a
    /// bug in its executor).
    pub fn problem(&self, code: &str, detail: pocket_contract::Detail) -> Problem {
        match self.code(code) {
            Some(c) => c.problem(detail),
            None => super::state::internal("action.catalog", &format!("undeclared code {code}")),
        }
    }
}

/// The latched value of a control on `body`: its bound field's, or the one kept in `Controls`, or
/// its default.
pub fn read_control(world: &World, def: &ControlDef, body: EntityId) -> Option<ControlValue> {
    match def.binding {
        ControlBinding::Engine { read, .. } => read(world, body),
        _ => {
            let e = pocket_sim::entity::entity(world, body)?;
            world
                .get::<Controls>(e)
                .and_then(|c| c.values.get(&def.name).cloned())
                .or_else(|| def.kind.default_value())
        }
    }
}

fn controls_mut<'w>(
    world: &'w mut World,
    body: EntityId,
) -> Result<bevy_ecs::world::Mut<'w, Controls>, Problem> {
    let e = pocket_sim::entity::require(world, body)?;
    if world.get::<Controls>(e).is_none() {
        world.entity_mut(e).insert(Controls::default());
    }
    world
        .get_mut::<Controls>(e)
        .ok_or_else(|| super::state::internal("action.controls", "no Controls on the body"))
}

/// Latches `value` on `body`'s control (checked by the caller to fit it).
pub fn write_control(
    world: &mut World,
    def: &ControlDef,
    body: EntityId,
    value: &ControlValue,
) -> Result<(), Problem> {
    match def.binding {
        ControlBinding::Engine { write, .. } => write(world, body, Some(value)),
        ControlBinding::Field {
            component, field, ..
        } => {
            let writer = *world.get_resource::<FieldWriter>().ok_or_else(|| {
                super::state::internal("action.controls", "no FieldWriter installed")
            })?;
            (writer.0)(world, body, component, field, &value.to_json())
        }
        ControlBinding::Unbound => {
            controls_mut(world, body)?
                .values
                .insert(def.name.clone(), value.clone());
            Ok(())
        }
    }
}

/// Queues one pulse of `def` on `body`.
pub fn queue_pulse(
    world: &mut World,
    def: &ControlDef,
    body: EntityId,
    target: Option<EntityId>,
) -> Result<(), Problem> {
    controls_mut(world, body)?
        .pulses
        .entry(def.name.clone())
        .or_default()
        .push_back(super::state::Pulse { target });
    Ok(())
}

/// Delivers the front pulse of each control of every body, in `EntityId` order and declared
/// control order, into its binding, and removes it: the pulse acts in this tick.
pub fn deliver_pulses(world: &mut World, catalog: &ActionCatalog) -> Result<(), Problem> {
    let bodies: Vec<EntityId> = {
        let index = world.resource::<pocket_sim::EntityIndex>();
        index
            .iter()
            .filter(|(_, e)| {
                world
                    .get::<Controls>(*e)
                    .is_some_and(|c| !c.pulses.is_empty())
            })
            .map(|(id, _)| id)
            .collect()
    };
    for body in bodies {
        for def in catalog.controls.iter().filter(|c| c.kind.is_pulse()) {
            let pulse = {
                let mut c = controls_mut(world, body)?;
                let p = c.pulses.get_mut(&def.name).and_then(|q| q.pop_front());
                if c.pulses.get(&def.name).is_some_and(|q| q.is_empty()) {
                    c.pulses.remove(&def.name);
                }
                p
            };
            let Some(pulse) = pulse else { continue };
            if !def.binding.writable(world, body) {
                // The body lost what took the control since the pulse was queued: it acts on
                // nothing, as an unbound pulse no rule reads.
                continue;
            }
            let target = pulse.target.map(|t| ControlValue::Number(t.to_f64()));
            match def.binding {
                ControlBinding::Engine { write, .. } => write(world, body, target.as_ref())?,
                ControlBinding::Field {
                    component, field, ..
                } => {
                    let writer = *world.get_resource::<FieldWriter>().ok_or_else(|| {
                        super::state::internal("action.controls", "no FieldWriter installed")
                    })?;
                    let v = target.as_ref().map_or(Value::Null, ControlValue::to_json);
                    (writer.0)(world, body, component, field, &v)?;
                }
                ControlBinding::Unbound => {}
            }
        }
    }
    Ok(())
}

/// A parameter of canonical parameters, as a number.
pub fn param_f64(params: &PlainData, key: &str) -> Option<f64> {
    match params.get(key)? {
        PlainData::Number(x) => Some(*x),
        _ => None,
    }
}

/// A parameter as a string.
pub fn param_str<'a>(params: &'a PlainData, key: &str) -> Option<&'a str> {
    match params.get(key)? {
        PlainData::String(s) => Some(s),
        _ => None,
    }
}

/// A parameter as a boolean.
pub fn param_bool(params: &PlainData, key: &str) -> Option<bool> {
    match params.get(key)? {
        PlainData::Bool(b) => Some(*b),
        _ => None,
    }
}

/// The affordances the perception declarations carry on their kinds (perception.md,
/// `KindDef.affordances`; actions.md, Affordances), decoded strictly: `definition.invalid` at
/// `/kinds/<k>/affordances/<i>` for one that is not an `AffordanceDef`.
pub fn affordances_from(
    defs: &crate::perception::PerceptionDefs,
) -> Result<BTreeMap<String, Vec<AffordanceDef>>, Problem> {
    let mut out = BTreeMap::new();
    for (k, kind) in defs.decl.kinds.iter().enumerate() {
        let mut list = Vec::new();
        for (i, a) in kind.affordances.iter().enumerate() {
            let at = pocket_contract::Pointer::root()
                .key("kinds")
                .index(k)
                .key("affordances")
                .index(i);
            let def: AffordanceDef = serde_json::from_value(a.clone())
                .map_err(|e| pocket_contract::codes::definition_invalid(&at, &e.to_string()))?;
            list.push(def);
        }
        if !list.is_empty() {
            out.insert(kind.kind.clone(), list);
        }
    }
    Ok(out)
}

impl ActionCatalog {
    /// Checks the declarations at load (README, The game definition): names unique, channels
    /// named, every affordance's effect a declared intent taking the kind as its target or a
    /// declared pulse targeting the kind, and every fact requirement on what a player may know (a
    /// `Hidden` or `Owner` fact of another entity is refused). `definition.invalid` otherwise.
    pub fn validate(
        &self,
        defs: Option<&crate::perception::PerceptionDefs>,
    ) -> Result<(), Problem> {
        use crate::perception::Exposure;
        use pocket_contract::Pointer;
        use pocket_contract::codes::definition_invalid;
        let bad = |at: Pointer, why: String| Err(definition_invalid(&at, &why));
        for (i, c) in self.controls.iter().enumerate() {
            let at = Pointer::root().key("controls").index(i);
            if self.controls[..i].iter().any(|o| o.name == c.name) {
                return bad(at, format!("the control {} is declared twice", c.name));
            }
            if c.channel.is_empty() {
                return bad(at, format!("the control {} names no channel", c.name));
            }
        }
        for (i, e) in self.intents.iter().enumerate() {
            let at = Pointer::root().key("intents").index(i);
            if self.intents[..i].iter().any(|o| o.def.name == e.def.name) {
                return bad(at, format!("the intent {} is declared twice", e.def.name));
            }
            if e.def.channels.is_empty() {
                return bad(at, format!("the intent {} occupies no channel", e.def.name));
            }
        }
        for (kind, list) in &self.affordances {
            for (i, a) in list.iter().enumerate() {
                let at = Pointer::root()
                    .key("kinds")
                    .key(kind)
                    .key("affordances")
                    .index(i);
                match &a.effect {
                    super::defs::Effect::Intent { intent, .. } => match self.intent(intent) {
                        Some(e) if e.def.target.kinds().contains(kind) => {}
                        Some(_) => {
                            return bad(at, format!("{intent} does not take a {kind} as target"));
                        }
                        None => return bad(at, format!("no intent {intent} is declared")),
                    },
                    super::defs::Effect::Pulse { control } => match self.control(control) {
                        Some(ControlDef {
                            kind:
                                super::defs::ControlKind::Pulse {
                                    target_kinds: Some(ks),
                                },
                            ..
                        }) if ks.contains(kind) => {}
                        _ => {
                            return bad(at, format!("{control} is not a pulse targeting a {kind}"));
                        }
                    },
                }
                let Some(defs) = defs else { continue };
                for r in &a.requires {
                    let super::defs::Requirement::Fact { of, fact, .. } = r else {
                        continue;
                    };
                    let known = match of {
                        super::defs::Side::Actor => defs.instrument(fact).is_some(),
                        super::defs::Side::Target => defs.kind(kind).is_some_and(|k| {
                            k.facts.iter().any(|f| {
                                f.name == *fact
                                    && !matches!(f.exposure, Exposure::Hidden | Exposure::Owner)
                            })
                        }),
                    };
                    if !known {
                        return bad(
                            at,
                            format!("the requirement on {fact} reads nothing a player may know"),
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
