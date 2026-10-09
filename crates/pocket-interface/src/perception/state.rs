//! What perception keeps in the world (shared/contract/perception.md, Observers and Perceivable
//! entities): who observes and through which profile, what can be perceived, what each observer
//! remembers and the ring of events it perceived. All of it is world state in its persisted form
//! (README, World state and wire forms: enums externally tagged, every `Option` encoded), so a
//! fork, a restore and a replay carry it and the world hash covers it; answers convert to the wire
//! form only in the projections.

use std::collections::{BTreeMap, VecDeque};

use bevy_ecs::prelude::{Component, Resource};
use pocket_sim::persisted::{Persisted, RegisterPersisted};
use pocket_sim::{EntityId, EventSeq, Tick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_reflection::{Samples, Tracer};

use super::defs::{PerceptionDefs, Vec3};

/// An entity that perceives from its own place through a profile. One seat per body: a body
/// observes for at most one seat (README, Seats and callers).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observer {
    /// The `ObserverProfile` it perceives through.
    pub profile: String,
    /// The seat whose body this is, if a player perceives through it.
    #[serde(default)]
    pub seat: Option<String>,
    /// Bound to the reserved `omniscient_player` profile (the benchmark's omniscient condition),
    /// derived from `profile`: every perceivable entity seen in full, every declared event
    /// (perception.md, The omniscient view; docs/spec/perception-slice2.md 2).
    #[serde(default)]
    pub omniscient: bool,
    /// The observer's team: observers of one team share what they see (charter 5.1, Pioneer
    /// 2026-10-09; docs/spec/player.md). Each keeps its own memory, events and ranges; an entity a
    /// teammate sees at the end of a tick is seen by every member of the team, at the detail the
    /// best-placed member sees it. `None`: alone.
    #[serde(default)]
    pub team: Option<String>,
}

/// An entity some observer can perceive; everything else is invisible to every observer but the
/// omniscient one.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Perceivable {
    /// A `KindDef` name: "island", "mark", "boat", "crate".
    pub kind: String,
    /// How far it can be seen at all (its size and conspicuity), metres.
    pub detect_m: f64,
    /// Its top above its origin; occlusion aims at the origin and the top.
    #[serde(default)]
    pub height_m: f64,
    /// Ranking, higher first.
    #[serde(default)]
    pub priority: u8,
    /// On the chart at this position, known to observers whose profile has `chart`.
    #[serde(default)]
    pub chart_m: Option<Vec3>,
}

/// Its colliders block sight: terrain, walls, islands.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Occluder {}

/// Weather: 1 clear; 0.1 a fog cutting every range to a tenth.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VisibilityScale(pub f64);

impl Default for VisibilityScale {
    fn default() -> Self {
        VisibilityScale(1.0)
    }
}

/// A fact's or an instrument's value. The wire form is untagged (`true`, `3.4`, `"good"`,
/// `{"x", "y", "z"}`); this persisted form is externally tagged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum FactValue {
    Bool(bool),
    Number(f64),
    Text(String),
    Position([f64; 3]),
}

/// How an entity is known.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Visible at the end of this tick.
    Seen,
    /// Seen before, in memory, not visible now: its last known state.
    Remembered,
    /// Never seen (or forgotten) but on the chart.
    Charted,
}

impl Visibility {
    pub fn name(self) -> &'static str {
        match self {
            Visibility::Seen => "seen",
            Visibility::Remembered => "remembered",
            Visibility::Charted => "charted",
        }
    }
}

/// How much of an entity is known.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Chart,
    Coarse,
    Full,
}

impl Detail {
    pub fn name(self) -> &'static str {
        match self {
            Detail::Chart => "chart",
            Detail::Coarse => "coarse",
            Detail::Full => "full",
        }
    }
}

/// An entity as an observer names it: its id and, if it has one, its name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Named {
    pub id: EntityId,
    pub name: Option<String>,
}

/// One remembered entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEntry {
    pub kind: String,
    pub name: Option<String>,
    pub seen_tick: Tick,
    /// Exact, rounded only when projected.
    pub pos_m: [f64; 3],
    /// Facts as they were when last seen, rounded at their precision; `relative` facts are not
    /// stored.
    pub facts: BTreeMap<String, FactValue>,
    /// Whether the full facts were known (seen within attention) at that sighting.
    pub detail: Detail,
    /// Its `detect_m`, `height_m` and `priority` as last seen, so ranking and the
    /// forgetting rule never read an entity the observer cannot see now.
    pub detect_m: f64,
    pub height_m: f64,
    pub priority: u8,
}

/// What an observer remembers: one entry per entity it has seen and not yet forgotten, in
/// `EntityId` order. Written only by the perception update, never by a query.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObserverMemory {
    /// The tick of the update that wrote it: the entries whose `seen_tick` is this one are the
    /// ones seen, at a boundary and inside the next tick alike (docs/spec/perception-slice2.md 2).
    pub updated: Tick,
    pub entries: BTreeMap<EntityId, MemoryEntry>,
}

/// How an event was perceived.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Sense {
    Sight,
    Sound,
    Private,
    Global,
}

impl Sense {
    pub fn name(self) -> &'static str {
        match self {
            Sense::Sight => "sight",
            Sense::Sound => "sound",
            Sense::Private => "private",
            Sense::Global => "global",
        }
    }
}

/// One perceived event, as the observer perceived it: its declared data fields only, its bearing
/// and range rounded when stored (projection.md, Stored values).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PerceivedEvent {
    /// This observer's own sequence number, from 1, contiguous.
    pub seq: u64,
    pub tick: Tick,
    pub kind: String,
    /// Only if the subject was known to the observer.
    pub subject: Option<Named>,
    pub sense: Sense,
    pub bearing_deg: Option<f64>,
    pub range_m: Option<f64>,
    /// The declared data fields, in declaration order, rounded at their precision.
    pub data: Vec<(String, FactValue)>,
    /// The `seq` of the perceived causing event.
    pub cause: Option<u64>,
}

/// One entry of the ring: the world event's sequence number (none for a sighting) beside what the
/// observer perceived of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RingEntry {
    pub world: Option<EventSeq>,
    pub event: PerceivedEvent,
}

/// The perceived events, a ring of the profile's `event_capacity`; `next_seq` counts every event
/// ever pushed, so the next one gets `next_seq + 1`.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObserverEvents {
    pub next_seq: u64,
    pub ring: VecDeque<RingEntry>,
}

impl ObserverEvents {
    /// The seq of the last event pushed (0 before any).
    pub fn latest(&self) -> u64 {
        self.next_seq
    }

    /// The oldest seq still in the ring, or `latest + 1` when it is empty.
    pub fn oldest(&self) -> u64 {
        self.ring.front().map_or(self.next_seq + 1, |e| e.event.seq)
    }
}

macro_rules! persisted {
    ($($t:ty => $name:literal $(, $inner:ty)*;)*) => {
        $(impl Persisted for $t {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;

            fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
                $(t.trace_type::<$inner>(s)?;)*
                t.trace_type::<Self>(s).map(|_| ())
            }
        })*
    };
}

persisted! {
    Observer => "Observer";
    Perceivable => "Perceivable";
    Occluder => "Occluder";
    VisibilityScale => "VisibilityScale";
    ObserverMemory => "ObserverMemory", Detail, FactValue;
    ObserverEvents => "ObserverEvents", Sense, FactValue;
}

/// Declares perception's types (persistence.md 2): the observer, perceivable and occluder
/// components, the memory and the event ring, and the visibility scale as persisted; the game's
/// declarations as ignored (the game installs them, a fork's fresh world too).
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Observer>()
        .component::<Perceivable>()
        .component::<Occluder>()
        .component::<ObserverMemory>()
        .component::<ObserverEvents>()
        .resource::<VisibilityScale>()
        .ignore::<PerceptionDefs>("the game's perception declarations, installed with the game");
}
