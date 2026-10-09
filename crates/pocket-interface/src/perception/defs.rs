//! What a game declares perceivable (shared/contract/perception.md, Observers and What can be
//! perceived): observer profiles, instruments, kinds with their facts, and events with their scope.
//! These are game data: the sailing showcase reads them from `samples/sailing-course/perception.json`,
//! and they are installed with the game ([`super::plugin`]), never part of the world hash.
//!
//! Derived facts and instruments name Rust functions registered with the declarations
//! ([`PerceptionDefs::derive`]); script-implemented ones are not in slice 2 (perception.md, open
//! choices; docs/spec/perception-slice2.md 3).

use std::collections::BTreeMap;

use bevy_ecs::prelude::Resource;
use pocket_contract::codes::definition_invalid;
use pocket_contract::{Pointer, Problem};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::facts::DerivedFn;
use crate::projection::tensor::TensorSpec;

/// The reserved profile of the benchmark's omniscient condition (perception.md, The omniscient
/// view): a seat bound to it sees every perceivable entity with every fact.
pub const OMNISCIENT_PLAYER: &str = "omniscient_player";

/// The kind of the perceived event the perception update pushes for a first sighting.
pub const SIGHTED: &str = "sighted";

/// A point or offset in the contract frame, metres (README, Positions): `{"x", "y", "z"}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    pub fn of(v: [f64; 3]) -> Vec3 {
        Vec3 {
            x: v[0],
            y: v[1],
            z: v[2],
        }
    }
}

/// One kind of observer ("skipper", "lookout", "guard").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserverProfile {
    pub name: String,
    pub doc: String,
    /// `None`: blind (instruments and private events remain).
    #[serde(default)]
    pub sight: Option<Sight>,
    /// `None`: deaf.
    #[serde(default)]
    pub hearing: Option<Hearing>,
    /// `full` facts within it, `coarse` beyond.
    pub attention_m: f64,
    /// A remembered entity is forgotten this long after it was last seen.
    pub memory_s: f64,
    /// At most this many remembered; the longest unseen goes first.
    pub memory_capacity: u32,
    /// The ring of perceived events kept for pull queries.
    pub event_capacity: u32,
    /// Kinds whose first sighting is a `sighted` event.
    #[serde(default)]
    pub sightings: Vec<String>,
    /// Entities with a chart position are known from the start.
    #[serde(default)]
    pub chart: bool,
    /// Percepts carry `pos_m` (as with a chart plotter).
    #[serde(default)]
    pub positions: bool,
    /// The instruments it reads, in the order shown.
    #[serde(default)]
    pub instruments: Vec<String>,
    /// The default budget of `observe`, `nearby`, `describe` and `events`.
    pub budget_tokens: u32,
    /// The tensor projection's layout, for RL observers.
    #[serde(default)]
    pub tensor: Option<TensorSpec>,
}

/// What an observer sees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sight {
    pub range_m: f64,
    /// About the body's forward axis on the ground; 360 sees all round.
    pub fov_deg: f64,
    /// The eye in the body's frame (metres; +y up, -z forward).
    pub eye_m: Vec3,
    /// Occluders hide what lies behind them.
    pub occlusion: bool,
}

/// What an observer hears: `scale` multiplies every sound's radius (1 ordinary, 0 deaf).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hearing {
    pub scale: f64,
}

/// What entities of a kind show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KindDef {
    pub kind: String,
    pub doc: String,
    #[serde(default)]
    pub facts: Vec<FactDef>,
    /// actions.md's `AffordanceDef`s, read by the action layer; perception carries them as data.
    #[serde(default)]
    pub affordances: Vec<serde_json::Value>,
}

/// A named reading with a unit, a precision and an exposure level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactDef {
    /// With its unit suffix (README, Names).
    pub name: String,
    pub doc: String,
    pub unit: Unit,
    /// Decimal places in projections; 0 makes the value a JSON integer.
    #[serde(default)]
    pub precision: u8,
    pub exposure: Exposure,
    pub source: FactSource,
    /// Depends on the observer: never stored in memory, recomputed from the known position.
    #[serde(default)]
    pub relative: bool,
}

/// A fact's unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum Unit {
    Metres,
    MetresPerSecond,
    Seconds,
    Kilograms,
    /// Degrees in [0, 360), shown as three digits.
    Bearing,
    /// Degrees in (-180, 180].
    Angle,
    /// Dimensionless, usually [0, 1] or [-1, 1].
    Fraction {
        min: f64,
        max: f64,
    },
    Count,
    Bool,
    Text,
    Enum {
        values: Vec<String>,
    },
    /// A point, metres.
    Position,
}

/// Where a fact's value comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactSource {
    /// A component field, by the component's registered name and a dotted field path.
    Field { component: String, path: String },
    /// A pure function of the world, the entity and the observer's body, registered by name.
    Derived { function: String },
    /// A member of an event's data, by a dotted path (event data fields only;
    /// docs/spec/perception-slice2.md 3).
    Data { path: String },
}

/// Who may know a fact, from the most to the least widely known.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Exposure {
    /// On the chart: known whenever the entity is known at all.
    Chart,
    /// Whenever it is seen, at any range, and kept in memory.
    Coarse,
    /// When it is seen within the observer's attention range.
    Full,
    /// Only to the seat whose body it is.
    Owner,
    /// Never to a player: the omniscient view only.
    Hidden,
}

/// A reading the observer has without seeing anything, evaluated with its body as the entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstrumentDef {
    pub name: String,
    pub doc: String,
    pub unit: Unit,
    #[serde(default)]
    pub precision: u8,
    pub source: FactSource,
}

/// Who can perceive an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    /// Only the observer whose body is the event's subject.
    Private,
    /// Observers that could see `at_m` at the end of the tick.
    Sight,
    /// Observers within `radius_m` times their hearing scale of `at_m`.
    Sound { radius_m: f64 },
    /// Every observer.
    Global,
    /// No player: the omniscient view only.
    Hidden,
}

/// An event kind's declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    pub kind: String,
    pub doc: String,
    pub scope: Scope,
    /// The data fields an observer of the event gets (`from: data`).
    #[serde(default)]
    pub data: Vec<FactDef>,
}

/// The game's perception declarations, as a file holds them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PerceptionDecl {
    pub observers: Vec<ObserverProfile>,
    #[serde(default)]
    pub instruments: Vec<InstrumentDef>,
    #[serde(default)]
    pub kinds: Vec<KindDef>,
    #[serde(default)]
    pub events: Vec<EventDef>,
}

/// The declarations installed in a world, with the derived functions they name. A resource of
/// game data (Ignored for persistence: the game installs it, a fork's fresh world too).
#[derive(Resource, Clone, Default)]
pub struct PerceptionDefs {
    pub decl: PerceptionDecl,
    derived: BTreeMap<String, DerivedFn>,
}

impl std::fmt::Debug for PerceptionDefs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerceptionDefs")
            .field("decl", &self.decl)
            .field("derived", &self.derived.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn valid_name(s: &str) -> bool {
    let mut b = s.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn valid_event_kind(s: &str) -> bool {
    s.contains('.') && s.split('.').all(valid_name)
}

impl PerceptionDefs {
    /// Declarations with no derived functions yet.
    pub fn new(decl: PerceptionDecl) -> PerceptionDefs {
        PerceptionDefs {
            decl,
            derived: BTreeMap::new(),
        }
    }

    /// Declarations from their JSON text, decoded strictly (unknown fields refused with
    /// suggestions).
    pub fn from_json(text: &str) -> Result<PerceptionDefs, Problem> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            definition_invalid(
                &Pointer::root(),
                &format!("the declarations are not JSON: {e}"),
            )
        })?;
        let decl = pocket_contract::decode::<PerceptionDecl>(
            &v,
            &pocket_contract::CheckOptions::new("the perception declarations"),
        )?
        .value;
        Ok(PerceptionDefs::new(decl))
    }

    /// Registers a derived function under `name` (`sailing.shore_m`).
    pub fn derive(mut self, name: &str, f: DerivedFn) -> PerceptionDefs {
        self.derived.insert(name.to_owned(), f);
        self
    }

    pub fn derived(&self, name: &str) -> Option<DerivedFn> {
        self.derived.get(name).copied()
    }

    pub fn profile(&self, name: &str) -> Option<&ObserverProfile> {
        self.decl.observers.iter().find(|p| p.name == name)
    }

    pub fn kind(&self, kind: &str) -> Option<&KindDef> {
        self.decl.kinds.iter().find(|k| k.kind == kind)
    }

    pub fn instrument(&self, name: &str) -> Option<&InstrumentDef> {
        self.decl.instruments.iter().find(|i| i.name == name)
    }

    pub fn event(&self, kind: &str) -> Option<&EventDef> {
        self.decl.events.iter().find(|e| e.kind == kind)
    }

    /// Checks the declarations (README, The game definition: names exist and are unique, units
    /// match suffixes, numbers are sane, every derived function is registered), else
    /// `definition.invalid` at the first fault.
    pub fn validate(&self) -> Result<(), Problem> {
        let d = &self.decl;
        let root = Pointer::root();
        let bad = |path: Pointer, why: String| Err(definition_invalid(&path, &why));
        unique(
            d.observers.iter().map(|p| p.name.as_str()),
            &root.key("observers"),
        )?;
        unique(
            d.instruments.iter().map(|i| i.name.as_str()),
            &root.key("instruments"),
        )?;
        unique(d.kinds.iter().map(|k| k.kind.as_str()), &root.key("kinds"))?;
        unique(
            d.events.iter().map(|e| e.kind.as_str()),
            &root.key("events"),
        )?;
        for (i, p) in d.observers.iter().enumerate() {
            let at = root.key("observers").index(i);
            if !valid_name(&p.name) || p.name == OMNISCIENT_PLAYER {
                return bad(
                    at.key("name"),
                    format!("'{}' is not a profile name", p.name),
                );
            }
            let finite = [p.attention_m, p.memory_s]
                .iter()
                .all(|x| x.is_finite() && *x >= 0.0);
            if !finite || p.budget_tokens == 0 || p.budget_tokens > super::query::MAX_BUDGET {
                return bad(
                    at,
                    "ranges, memory and budget must be finite and positive".into(),
                );
            }
            if let Some(s) = &p.sight {
                let ok = s.range_m.is_finite()
                    && s.range_m >= 0.0
                    && s.fov_deg > 0.0
                    && s.fov_deg <= 360.0
                    && s.eye_m.array().iter().all(|x| x.is_finite());
                if !ok {
                    return bad(
                        at.key("sight"),
                        "a sight needs a finite range and a field of view in (0, 360]".into(),
                    );
                }
            }
            if let Some(h) = &p.hearing
                && !(h.scale.is_finite() && h.scale >= 0.0)
            {
                return bad(
                    at.key("hearing"),
                    "a hearing scale is finite and not negative".into(),
                );
            }
            for (j, k) in p.sightings.iter().enumerate() {
                if self.kind(k).is_none() {
                    return bad(at.key("sightings").index(j), format!("no kind '{k}'"));
                }
            }
            for (j, n) in p.instruments.iter().enumerate() {
                if self.instrument(n).is_none() {
                    return bad(
                        at.key("instruments").index(j),
                        format!("no instrument '{n}'"),
                    );
                }
            }
            if let Some(t) = &p.tensor {
                t.validate(self, p, &at.key("tensor"))?;
            }
        }
        for (i, inst) in d.instruments.iter().enumerate() {
            let at = root.key("instruments").index(i);
            self.check_fact(&inst.name, &inst.unit, &inst.source, false, &at)?;
        }
        for (i, k) in d.kinds.iter().enumerate() {
            let at = root.key("kinds").index(i);
            if !valid_name(&k.kind) {
                return bad(at.key("kind"), format!("'{}' is not a kind name", k.kind));
            }
            unique(k.facts.iter().map(|f| f.name.as_str()), &at.key("facts"))?;
            for (j, f) in k.facts.iter().enumerate() {
                self.check_fact(
                    &f.name,
                    &f.unit,
                    &f.source,
                    false,
                    &at.key("facts").index(j),
                )?;
            }
        }
        for (i, e) in d.events.iter().enumerate() {
            let at = root.key("events").index(i);
            if !valid_event_kind(&e.kind) {
                return bad(
                    at.key("kind"),
                    format!("'{}' is not a dotted event kind", e.kind),
                );
            }
            if let Scope::Sound { radius_m } = e.scope
                && !(radius_m.is_finite() && radius_m >= 0.0)
            {
                return bad(
                    at.key("scope"),
                    "a sound's radius is finite and not negative".into(),
                );
            }
            unique(e.data.iter().map(|f| f.name.as_str()), &at.key("data"))?;
            for (j, f) in e.data.iter().enumerate() {
                let at = at.key("data").index(j);
                self.check_fact(&f.name, &f.unit, &f.source, true, &at)?;
                // Whoever perceives an event gets its data: `full` and `owner` would need an
                // attention or an owner the event does not have, and its data is the same for
                // every observer (docs/spec/perception-slice2.md 3).
                if matches!(f.exposure, Exposure::Full | Exposure::Owner) {
                    return bad(
                        at.key("exposure"),
                        "an event's data field is chart, coarse or hidden".into(),
                    );
                }
                if f.relative {
                    return bad(
                        at.key("relative"),
                        "an event's data field does not depend on the observer".into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn check_fact(
        &self,
        name: &str,
        unit: &Unit,
        source: &FactSource,
        event: bool,
        at: &Pointer,
    ) -> Result<(), Problem> {
        if !valid_name(name) {
            return Err(definition_invalid(
                &at.key("name"),
                &format!("'{name}' is not a name"),
            ));
        }
        if let Some(why) = suffix_misfit(name, unit) {
            return Err(definition_invalid(&at.key("unit"), &why));
        }
        match source {
            FactSource::Derived { function } if self.derived(function).is_none() => {
                Err(definition_invalid(
                    &at.key("source"),
                    &format!("no derived function '{function}'"),
                ))
            }
            FactSource::Data { .. } if !event => Err(definition_invalid(
                &at.key("source"),
                "only an event's data fields come from its data",
            )),
            FactSource::Field { .. } | FactSource::Derived { .. } if event => {
                Err(definition_invalid(
                    &at.key("source"),
                    "an event's data fields come from its data",
                ))
            }
            _ => Ok(()),
        }
    }
}

/// `definition.invalid` for a name given twice.
fn unique<'a>(names: impl Iterator<Item = &'a str>, at: &Pointer) -> Result<(), Problem> {
    let mut seen: Vec<&str> = Vec::new();
    for n in names {
        if seen.contains(&n) {
            return Err(definition_invalid(at, &format!("'{n}' is declared twice")));
        }
        seen.push(n);
    }
    Ok(())
}

/// Why a name's unit suffix does not match its unit (README, Names), if it does not.
fn suffix_misfit(name: &str, unit: &Unit) -> Option<String> {
    let want = match unit {
        Unit::Metres | Unit::Position => "_m",
        Unit::MetresPerSecond => "_mps",
        Unit::Seconds => "_s",
        Unit::Kilograms => "_kg",
        Unit::Bearing | Unit::Angle => "_deg",
        _ => return None,
    };
    (!name.ends_with(want)).then(|| format!("'{name}' must end in '{want}' for its unit"))
}
