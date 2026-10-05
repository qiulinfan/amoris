# Perception

- Status: Draft, slice 0. Draft, proposed for PocketEngine's `shared/contract` (README,
  Synchronization with PocketEngine).
- Charter: 2.3, 2.4.1, 3.1, 3.2, 3.3, 3.6, 3.7, 3.10, 4.4, 5.1, 7 (item 8).
- Contract version: 0.1 (draft).

## What this fixes

Every observer (a human player, an LLM agent, an RL policy, an NPC) gets information through one
perception layer with occlusion, fog and an attention range (charter 3.1), defined by the game: what
each kind of observer senses, what each kind of entity shows and how far, what a player's
instruments read. Every tick the engine computes what each observer perceives and remembers; agents
pull from that with budgeted queries and receive deltas of the events they perceived; nothing dumps
the whole state. The image, the LLM's text and the RL policy's tensors are projections of it.

Master's agent interface answered every question from the whole world: `world.tree`, `world.query`,
`world.describe` and the environment interface's `state` were omniscient, and "observation tiers
(player-knowable versus omniscient)" were listed as not built (master
`docs/design/agent-perception.md`, Implementation pointers). Master's NPCs had their own sight and
hearing rules (master `docs/design/behavior.md`: `sees` within `sight` and `fov` with a clear line,
`noise` within a radius times `hearing`); this file makes that the one perception every observer
uses, players included.

Precedents: PySC2's visibility map (hidden, fogged with the last known state, visible) and its
feature layers; Unity ML-Agents' sensors (vector observations, ray perception) and Gymnasium's
spaces for the tensor projection; the web accessibility tree, which made web agents reliable, as the
model for a compact, addressable text form (master `docs/design/agent-perception.md`, The problem).

## Observers

An **observer** is an entity with an `Observer` component: it perceives from its own place, through
the senses its **profile** declares. A seat (README, Seats and callers) perceives through the
observer on its body; an NPC's rules perceive through the observer on the NPC. Profiles are game
data.

```rust
/// Declared by the game, one per kind of observer ("skipper", "lookout", "guard").
pub struct ObserverProfile {
    pub name: String,
    pub doc: String,
    pub sight: Option<Sight>,         // None: blind (instruments and private events remain)
    pub hearing: Option<Hearing>,     // None: deaf
    pub attention_m: f64,             // `full` facts within it, `coarse` beyond
    pub memory_s: f64,                // a remembered entity is forgotten this long after last seen
    pub memory_capacity: u32,         // at most this many remembered; longest unseen goes first
    pub event_capacity: u32,          // the ring of perceived events kept for pull queries
    pub sightings: Vec<String>,       // kinds whose first sighting is a `sighted` event (step 6)
    pub chart: bool,                  // entities with a chart position are known from the start
    pub positions: bool,              // percepts carry `pos_m` (as with a chart plotter)
    pub instruments: Vec<String>,     // the instruments it reads, in the order shown
    pub budget_tokens: u32,           // default budget of `observe`
    pub tensor: Option<TensorSpec>,   // the tensor projection's layout, for RL observers
}

pub struct Sight {
    pub range_m: f64,
    pub fov_deg: f64,       // about the body's forward axis on the ground; 360 sees all round
    pub eye_m: Vec3,        // the eye in the body's frame (metres; +y up, -z forward)
    pub occlusion: bool,    // occluders hide what lies behind them
}

pub struct Hearing { pub scale: f64 }   // multiplies every sound's radius; 1 ordinary, 0 deaf
```

The engine components (world state, part of the world hash, copied by a fork; spec-persist
serializes them like any component):

```rust
/// One seat per body: a body observes for at most one seat (README, Seats and callers).
#[derive(Component)]
pub struct Observer { pub profile: String, pub seat: Option<SeatId> }

/// What the observer remembers: one entry per entity it has seen and not yet forgotten.
/// Updated only by the perception update (below), never by a query.
#[derive(Component)]
pub struct ObserverMemory {
    pub entries: BTreeMap<EntityId, MemoryEntry>,   // iterated in EntityId order (spec-sim)
}

pub struct MemoryEntry {
    pub kind: String,
    pub name: Option<String>,
    pub seen_tick: Tick,
    pub pos_m: Vec3,                                // exact, rounded only when projected
    /// Facts as they were when last seen, rounded as projection.md (Rounding, Stored values)
    /// says; `relative` facts are not stored (Perceivable entities).
    pub facts: BTreeMap<String, FactValue>,
    /// Whether the full facts were known (seen within attention) at that sighting.
    pub detail: Detail,
}

/// The perceived events, a ring of `event_capacity`; `next_seq` counts every event ever pushed,
/// so an event's `seq` is unique for this observer and contiguous. Each entry keeps the world
/// event's `EventSeq` (spec-sim; none for a sighting) beside what the observer perceived of it.
#[derive(Component)]
pub struct ObserverEvents {
    pub next_seq: u64,
    pub ring: VecDeque<(Option<EventSeq>, PerceivedEvent)>,
}
```

These listings give the content of world state; the wire forms of their parts (`FactValue` untagged,
omitted `None` fields) apply only when an answer shows them, and each line persists them in its own
form (README, Conventions, World state and wire forms).

`Observer`, `ObserverMemory` and `ObserverEvents` live in the world because what an observer
remembers is state the game's outcome can depend on (an NPC chasing where it last saw the player; an
intent steering for a remembered mark), and a fork MUST carry it (charter 3.3). Recomputing memory
on demand was rejected: a query would then change state, and the world hash would depend on how
often an agent looked.

## What can be perceived

### Perceivable entities, kinds and facts

An entity can be perceived only if it has a `Perceivable` component; everything else (the sea's
mesh, the camera, bookkeeping entities) is invisible to every observer but the omniscient one.

```rust
#[derive(Component)]
pub struct Perceivable {
    pub kind: String,      // a `KindDef` name: "island", "mark", "boat", "crate"
    pub detect_m: f64,     // how far it can be seen at all (its size and conspicuity)
    pub height_m: f64,     // its top above its origin; occlusion aims at the origin and the top
    pub priority: u8,      // ranking, higher first (Token budgets)
    /// On the chart at this position, known to observers whose profile has `chart`; None: not
    /// charted. Fixed when the entity is charted (at load, or by the rule that lays it); a rule
    /// that moves the entity later moves the chart only by setting this.
    pub chart_m: Option<Vec3>,
}

#[derive(Component)]
pub struct Occluder;                 // its colliders (physics') block sight: terrain, walls

#[derive(Resource)]
pub struct VisibilityScale(pub f64); // weather: 1 clear; 0.1 a fog cutting every range to a tenth
```

A **kind** declares what entities of that kind show, as **facts**: named readings with a unit, a
precision and an exposure level. Facts are the game's vocabulary for what a player can know; they
need not be component fields (a derived fact such as an island's distance to its nearest shore is
computed).

```rust
pub struct KindDef {
    pub kind: String,
    pub doc: String,
    pub facts: Vec<FactDef>,
    pub affordances: Vec<AffordanceDef>,   // actions.md
}

pub struct FactDef {
    pub name: String,                      // with its unit suffix (README, Names)
    pub doc: String,
    pub unit: Unit,
    /// Decimal places in projections; 0 makes the value a JSON integer.
    pub precision: u8,
    pub exposure: Exposure,
    pub source: FactSource,
    /// Depends on the observer (a distance from the observer's boat): never stored in memory, and
    /// for a remembered entity recomputed from its remembered position when shown.
    pub relative: bool,
}

#[serde(tag = "unit")]
pub enum Unit {
    Metres, MetresPerSecond, Seconds, Kilograms,
    Bearing,                               // degrees in [0, 360), shown as three digits
    Angle,                                 // degrees in (-180, 180]
    Fraction { min: f64, max: f64 },       // dimensionless, usually [0, 1] or [-1, 1]
    Count, Bool, Text,
    Enum { values: Vec<String> },
    Position,                              // Vec3, metres
}

#[serde(tag = "from")]
pub enum FactSource {
    /// A component field, by the component's registered name and a dotted field path.
    Field { component: String, path: String },
    /// A pure function of the world, the entity and the observer's body, registered by name (Rust
    /// or script); it may read the observer only when the fact is `relative`.
    Derived { function: String },
}

/// Who may know a fact, from the most to the least widely known.
pub enum Exposure {
    Chart,    // on the chart: known whenever the entity is known at all
    Coarse,   // whenever it is seen, at any range, and kept in memory
    Full,     // when it is seen within the observer's attention range
    Owner,    // only to the seat whose body it is (its own boat's rudder, for example)
    Hidden,   // never to a player: the omniscient view only
}

#[serde(untagged)]
pub enum FactValue { Bool(bool), Number(f64), Text(String), Position(Vec3) }
```

A fact without a declaration is not perceivable: there is no default exposure other than `Hidden`.
That is the charter's rule made concrete: what a player can know is part of the game's definition,
opted into fact by fact, and a component added later never leaks into observations by accident.

A derived fact or an instrument implemented in a script runs on the game thread under the script
host's budget for such calls; when it throws or overruns, the fact is absent for that observer and
tick, and the error goes to the step report (spec-script, `docs/spec/script-sandbox.md`, 4.4).

### Instruments

An **instrument** is a reading the observer has without seeing anything: its boat's compass and
speed log, the wind on its cheek, the course card. Instruments give a player exact numbers about its
own state and about world-level quantities (wind, time of day) that belong to no entity.

```rust
pub struct InstrumentDef {   // evaluated with the observer's body as the entity
    pub name: String, pub doc: String, pub unit: Unit, pub precision: u8, pub source: FactSource,
}
```

Instruments are evaluated when an observation is built (they are pure functions of the world at that
tick), never stored. The HUD a human sees reads the same instruments, so a human and an agent at the
same seat know the same numbers.

### Events and their scope

A world event is spec-sim's `Event` (`seq: EventSeq`, `tick`, `kind`, `subject`, `cause`, `data`;
`docs/spec/simulation.md`, 5), emitted by systems and scripts (the script API is spec-script's). Its
kind's declaration gives it a **scope**, which says who can perceive it. A kind with no `EventDef`
is `Hidden`, as an undeclared fact is: an event a script emits without declaring it, or an engine
event such as `script.failed`, reaches no player. Every `EventDef` names its scope; there is no
default, and a declaration without one fails the game's load with `definition.invalid`. Where it
happened, `at_m`, is the event's `at_m` data field when it has one, else its subject's position at
the end of the tick; an event with neither (its subject despawned) can be perceived only through
`Private` or `Global` scope.

```rust
#[serde(tag = "scope")]
pub enum Scope {
    /// Only the observer whose body is the event's subject (intent lifecycle events, actions.md).
    Private,
    /// Observers that could see `at_m` at the end of the tick.
    Sight,
    /// Observers within `radius_m` times their hearing scale of `at_m`.
    Sound { radius_m: f64 },
    /// Every observer (a race's start gun announced to all).
    Global,
    /// No player: the omniscient view only (bookkeeping).
    Hidden,
}

pub struct EventDef {
    pub kind: String,              // "crate.taken"
    pub doc: String,
    pub scope: Scope,
    pub data: Vec<FactDef>,        // the data fields an observer of the event gets
}
```

Event data fields are declared like facts, so an event says to its observers only what its
declaration exposes; fields it carries beyond them reach the omniscient view only.

## The perception update

Once per tick, a system with the key `interface.perception` computes what every observer perceives.
It runs after every system that moves entities or emits events in the tick, so that an observation
at tick T describes the world at the end of T: in spec-sim's `Finish` phase, before `sim.finish`
(`docs/spec/simulation.md`, 4.4, row 10). It reads the world and the events appended since its last
run (those the boundary writes before the tick appended, then the tick's own, in `EventSeq` order;
`docs/spec/simulation.md`, 5) and writes only `ObserverMemory` and `ObserverEvents`; it emits no
event, as `Finish` requires.

For each observer O (in `EntityId` order) with profile P, eye position `E` (the body's transform
applied to `P.sight.eye_m`) and forward bearing `h`:

1. **Candidates.** Every entity C with `Perceivable` other than O's body, from the spatial index (a
   general primitive, spec-sim), within the largest effective range.
2. **Range.** `r = min(P.sight.range_m, C.detect_m) * visibility_scale`. C is in range if
   `dx*dx + dy*dy + dz*dz <= r*r`, evaluated in that order, where `(dx, dy, dz)` is C's origin minus
   `E` (Geometry, below).
3. **Field of view.** If `P.sight.fov_deg < 360`, the absolute relative angle between `h` and the
   bearing from `E` to C (Geometry) is at most `fov_deg / 2`.
4. **Occlusion.** If `P.sight.occlusion`, cast rays from `E` to C's origin and to its top
   (`origin + (0, height_m, 0)`) against the colliders of `Occluder` entities other than C and O's
   body. C is visible if either ray is clear. Two sample points let a mast show over a low headland;
   more points cost more rays (open choice 5). The rays are physics queries and inherit physics'
   determinism.
5. **Detail.** A visible C within `P.attention_m` is seen with `Detail::Full`; beyond it, with
   `Detail::Coarse`.
6. **Memory.** Every visible C is written to O's memory (its kind, name, position, facts of its
   detail level, rounded as projection.md's Stored values says, its `relative` facts left out,
   `seen_tick = T`). A visible C that had no memory entry before this tick and whose kind is in
   `P.sightings` is a **sighting**: a perceived event of kind `sighted`, subject C, sense `sight`,
   data `{kind}`, is pushed to O's ring (in `EntityId` order, before the tick's other events).
   Sightings are how "a new mark came into view" reaches an agent and becomes a decision point
   (time.md). For every remembered entity M that is not visible this tick: if M's remembered
   position is itself visible to O now (steps 2 to 4 applied to the point, with M's `detect_m`), O
   can see that M is no longer there, and the entry is dropped; otherwise it is kept until
   `T - seen_tick > memory_s * rate`, after which it is dropped. When more than `memory_capacity`
   entries remain, the ones with the oldest `seen_tick` (then the lowest `EntityId`) are dropped.
7. **Events.** Every event appended since the last update, in `EventSeq` order, is pushed to O's
   ring when its scope admits O: `Private` when its subject is O's body, `Global` always, `Sight`
   when `at_m` passes steps 2 to 4 as a point (range `P.sight.range_m * visibility_scale`), `Sound`
   when the distance from `E` to `at_m` is at most `radius_m * P.hearing.scale`. Its perceived
   `cause` is the `seq` of the causing event's entry when that event is itself in O's ring,
   otherwise `None`, so a player never learns of a cause it did not perceive.

The **perceived visibility** of an entity for O at tick T is then:

```rust
pub enum Visibility {
    Seen,        // visible at the end of this tick
    Remembered,  // seen before, in memory, not visible now: its last known state
    Charted,     // never seen (or forgotten) but on the chart: its chart facts, at its chart position
}

pub enum Detail { Chart, Coarse, Full }
```

An entity that is none of these is unknown to O and appears in nothing O can query: not in
observations, not in name resolution, not in error suggestions. Its facts appear by exposure: `Seen`
with `Full` detail shows `Chart`, `Coarse` and `Full` facts; `Seen` with `Coarse` detail shows
`Chart` and `Coarse`; `Remembered` shows the facts stored at the last sighting, and its `relative`
facts recomputed from its remembered position; `Charted` shows `Chart` facts. A `Charted` entity's
`bearing_deg`, `range_m` and `pos_m` are taken from its chart position (`Perceivable.chart_m`),
never its live one, so a charted entity that moves unseen reveals nothing. `Owner` facts show only
on the seat's own body, which is always known to it with every non-hidden fact.

The visibility state for `Seen` is not stored separately: it is the set of memory entries whose
`seen_tick` is T. So the world holds one structure per observer, and a fork carries it whole.

### Geometry

Both lines compute these expressions exactly as written, with the deterministic math library
(spec-sim), so a value at the edge of a range or a field of view falls on the same side on both.

- **Visibility range** (step 2): `dx*dx + dy*dy + dz*dz <= r*r`, the sum in that order, from the
  eye.
- **Bearing** of an offset `(dx, dz)` on the ground: `b = math::atan2(dx, -dz).to_degrees()`; then
  `b + 360.0` when `b` is negative, and `b - 360.0` when the result is at least 360.0. When `dx` and
  `dz` are both zero the bearing is 0.
- **Relative angle** of a bearing `b` to a heading `h`: `a = b - h`; `a - 360.0` when above 180.0,
  `a + 360.0` when at most -180.0, so it lies in (-180, 180].
- **Reported `bearing_deg` and `range_m`** of a percept or a perceived event are horizontal measures
  from the observer's body origin, not from the eye: `range_m = math::sqrt(dx*dx + dz*dz)` and the
  bearing above, for the offset from the body's origin to the entity's origin (a remembered or
  charted entity: to its remembered or chart position; an event: to `at_m`). An event on the
  observer's own body therefore has range 0 and bearing 0.

## Queries

Every query is **read-only**: it is a pure function of the world at the current tick, the caller's
observer and the request, and it changes nothing (no memory, no event ring, no RNG, no counter).
This is checked (`contract.perception.read_only`, below). Queries are answered on the game thread
(open choice 6); a script function they call (a derived fact, an instrument) runs only there.

The queries (spec-mcp exposes them as tools; the request and response types are these):

```rust
pub struct ObserveRequest {
    /// Player role: omitted or its own seat. Developer role: required unless `omniscient`.
    pub seat: Option<SeatId>,
    pub budget_tokens: Option<u32>,          // default: the profile's budget_tokens
    pub projection: Option<Projection>,      // default: spec-mcp's for tools, json otherwise
    /// Events after this per-observer seq are included (Push, below); default: none.
    pub since: Option<u64>,
    /// Developer and checker roles only: the whole world, marked (Omniscient view, below).
    pub omniscient: Option<bool>,
}

pub struct NearbyRequest {
    pub seat: Option<SeatId>,
    pub kinds: Option<Vec<String>>,          // default: every kind
    pub within_m: Option<f64>,               // default: no limit beyond perception's own
    pub sector: Option<Sector>,              // default: all round
    pub visibility: Option<Vec<Visibility>>, // default: all three
    pub limit: Option<u32>,                  // default and maximum: 100
    pub budget_tokens: Option<u32>,
    pub projection: Option<Projection>,
    pub omniscient: Option<bool>,
}

pub struct Sector {
    pub from_deg: f64,                       // clockwise from here ...
    pub to_deg: f64,                         // ... to here, inclusive
    pub relative: bool,                      // true: angles off the heading, -30..30 is ahead
}

pub struct DescribeRequest {
    pub seat: Option<SeatId>,
    pub entity: EntityRef,
    pub budget_tokens: Option<u32>,
    pub projection: Option<Projection>,
    pub omniscient: Option<bool>,
}

pub struct EventsRequest {
    pub seat: Option<SeatId>,
    pub since: u64,                          // events with seq > since
    pub kinds: Option<Vec<String>>,          // exact kinds, or prefixes ending in "."
    pub limit: Option<u32>,                  // default and maximum: 200
    pub budget_tokens: Option<u32>,
    pub projection: Option<Projection>,
    /// Developer and checker roles only: the world's events, marked; `seq` and `cursor` are then
    /// spec-sim's `EventSeq`, the number `why` takes (mcp.md 6).
    pub omniscient: Option<bool>,
}

pub enum Projection { Text, Json, Tensor }
```

What each answers:

- **`observe`**: the default call, a summary of the observer's situation ranked for relevance: the
  header, every instrument, the seat's active intents (actions.md), a pending decision (time.md),
  percepts ranked (Token budgets), perceived events after `since`, and what was left out.
- **`nearby`**: percepts filtered by kind, distance, sector and visibility, ranked the same way.
  "What is ahead within 500 m" is
  `nearby {within_m: 500, sector: {from_deg: -30, to_deg: 30, relative: true}}`.
- **`describe`**: one entity as the observer knows it: every fact its visibility and detail allow,
  its affordances with whether each is available now and, if not, why (actions.md, Affordances), and
  the last five perceived events whose subject it is. An entity the observer does not know is
  `perception.unknown_entity`.
- **`events`**: perceived events after `since`, oldest first, up to `limit` and the budget.

The JSON responses:

```rust
pub struct Observation {
    pub tick: Tick,
    pub t_s: f64,
    pub seat: Option<SeatId>,
    pub observer: Option<EntityName>,        // {id, name}; None for the omniscient view
    pub omniscient: bool,                    // always present (Omniscient view)
    pub instruments: Vec<Reading>,           // in the profile's order
    pub intents: Vec<IntentView>,            // actions.md
    pub decision: Option<DecisionPoint>,     // time.md
    pub entities: Vec<Percept>,              // ranked
    pub events: Vec<PerceivedEvent>,         // seq ascending
    pub cursor: u64,                         // the seq of the last event included, or `since`
    pub omitted: Omitted,
    pub tokens: u32,                         // estimate of this response's own projection
}

pub struct EntityName { pub id: EntityId, pub name: Option<String> }

pub struct Reading { pub name: String, pub value: FactValue }

pub struct Percept {
    pub id: EntityId,
    pub name: Option<String>,
    pub kind: String,
    pub visibility: Visibility,
    pub detail: Detail,
    pub bearing_deg: f64,                    // horizontal, from the body's origin (Geometry)
    pub range_m: f64,                        // to the live, remembered or chart position
    pub pos_m: Option<Vec3>,                 // when the profile has `positions`
    pub age_s: Option<f64>,                  // Remembered only: seconds since it was last seen
    pub facts: Vec<Reading>,                 // in the kind's declared order
    pub can: Vec<String>,                    // verbs available now (actions.md, Affordances)
}

pub struct PerceivedEvent {
    pub seq: u64,
    pub tick: Tick,
    pub kind: String,
    pub subject: Option<EntityName>,         // only if the subject is known to the observer
    pub sense: Sense,
    pub bearing_deg: Option<f64>,            // to `at_m` from the body's origin at the event's tick
    pub range_m: Option<f64>,                // both rounded when stored (projection.md)
    pub data: Vec<Reading>,                  // the declared data fields only
    pub cause: Option<u64>,                  // seq of the perceived causing event
}

pub enum Sense { Sight, Sound, Private, Global }

pub struct Omitted {
    pub entities: BTreeMap<String, u32>,     // kind -> how many ranked percepts were left out
    pub events: u32,                         // perceived events after `cursor` not included
    pub lost: u32,                           // events after `since` already overwritten in the ring
}
```

`nearby` answers `{tick, omniscient, entities, omitted, tokens}`, `describe` answers
`{tick, omniscient, entity: Percept, affordances, events, tokens}`, `events` answers
`{tick, omniscient, events, cursor, omitted, tokens}`. Every response carries `omniscient`.

For script-side observers (NPC rules) and intent executors (actions.md) the same queries are
available unprojected through `PerceptionView`, a read-only accessor over one observer:

```rust
impl<'w> PerceptionView<'w> {
    pub fn tick(&self) -> Tick;
    pub fn body(&self) -> EntityId;
    pub fn instrument(&self, name: &str) -> Option<FactValue>;
    pub fn percept(&self, entity: EntityId) -> Option<Percept>;   // None if unknown to the observer
    pub fn nearby(&self, request: &NearbyRequest) -> Vec<Percept>; // ranked, no budget
    pub fn events_since(&self, seq: u64) -> impl Iterator<Item = &PerceivedEvent>;
}
```

The script host exposes it to scripts in batch form (spec-script); its answers equal the queries'
JSON answers before projection.

## Token budgets

Every query that returns text or JSON takes `budget_tokens` (charter 3.1: every call SHOULD accept a
token budget). The estimate is `tokens(bytes) = ceil(bytes / 4)` over the UTF-8 bytes of the
projected response (README, Token estimates). The maximum budget is 16000; a larger value is
`request.out_of_range`.

The answer is built so that it never exceeds the budget and a larger budget answers a superset:

1. **The mandatory part**: the header, the instruments, the seat's active intents and a pending
   decision. If its size exceeds the budget, the query is refused with `perception.budget_too_small`
   and `detail.min_tokens`, the smallest budget that would fit.
2. **The ranking** of percepts, lexicographic, so it is total and deterministic:
   1. the targets of the seat's active intents first;
   2. by visibility, `seen` before `remembered` before `charted`;
   3. by `priority`, higher first;
   4. by `range_m`, nearer first (compared as the rounded values, so a difference below the shown
      precision never reorders);
   5. by `EntityId`, lower first.
3. **One sequence of candidates.** The ranked percepts and the perceived events after `since`
   (oldest first) are merged into one sequence by alternating them, a percept first (p1, e1, p2, e2,
   ...; when one list runs out the other continues alone). A section's lines are still written in
   its own order; the merge only decides which items are chosen.
4. **Filling.** The answer takes the longest prefix of that sequence, of length k, such that the
   mandatory part, the k items and the omitted section for what is left after them together project
   to at most `budget_tokens * 4` bytes. Because the chosen set is a prefix and the longest one that
   fits, a larger budget never chooses fewer items, and events always come as a contiguous run from
   `since`, so the cursor advances over exactly what was shown and the next pull continues there.
5. **The omitted section** says per kind how many percepts were left out, how many events, and in
   the text projection the call that would show them (`nearby {"kinds":["crate"]}`,
   `events {"since":4411}`), as master's compact answers named their filters when they had to cut
   (master `docs/mcp.md`, Asking the engine how to call it). It is written only when something was
   left out.

Every JSON response reports its `tokens`, the estimate of its projection without the `tokens` member
itself (the budget check reserves 16 bytes for that member), so a benchmark meters perception
exactly as charged; the text projection has no `tokens` member and reserves nothing (projection.md,
Text projection).

## Push: event deltas

The engine never pushes state; it pushes only perceived events, as deltas (charter 3.1).

- Every response to `act`, `step`, `commit`, `wait` and `continue` (actions.md, time.md) carries the
  caller's perceived events since the last response that carried events to the same caller, within
  the response's budget, with the `cursor` after them. That cursor is session state (spec-mcp), not
  world state: two callers on one seat each get every event once.
- A caller that wants a range again, or wants more than a budget allowed, pulls with
  `events {since}`.
- In real-time pacing an agent that is waiting uses `wait` (time.md), which returns when a decision
  point, an awaited event or a timeout comes: the long poll is the push channel an MCP client can
  always consume. A transport that offers server notifications MAY also notify decision points
  (spec-mcp); the notification then carries the decision only, and the events are pulled.
- If the ring has overwritten events after the caller's cursor, the response says how many in
  `omitted.lost`; that is not an error.

## The omniscient view

The omniscient view sees every entity, `Perceivable` or not, with every fact including `Hidden`
ones, every event including `Hidden` ones, with no range, field of view, occlusion or memory. It
exists for debugging, developer tools and benchmark checkers, never as a player agent's default
observation (charter 3.1).

- It is marked in every response that uses it: `"omniscient": true` in JSON, and the text
  projection's header line ends with ` OMNISCIENT` (projection.md). Developer tools that read the
  world directly (spec-mcp) carry the same mark, so every answer produced without perception
  filtering says so.
- A player-role request for it is refused with `perception.omniscient_forbidden`; a player's
  response always says `"omniscient": false`.
- Every percept in it carries `pos_m`, whatever the profile says, so a checker reads positions
  through contract types alone (shared/benchmark/README.md 3.5).
- In the raw omniscient view, for developers and checkers only, a non-`Perceivable` entity has kind
  `"entity"`, `detail: "full"`, and its components appear as facts named `Component.field` with no
  rounding beyond the JSON number's own. Those names differ between the lines, so nothing shared (a
  benchmark condition, a checker) relies on them.

**The `omniscient_player` profile.** The benchmark's omniscient condition (charter 9.2) binds the
seat to this reserved profile (README, Seats and callers), derived from the seat's own: the same
instruments, budget, tensor layout and ranking; every `Perceivable` entity `Seen` with `Full`
detail, `Hidden` facts and `pos_m` included; every event of a declared kind, whatever its scope; no
range, field of view, occlusion or memory; non-`Perceivable` entities excluded. Its answers are
marked omniscient. So the condition differs from the perception condition only in what is known, and
both lines give it the same bytes.

## Projections

How answers are written (rounding, the JSON and text projections, the tensor projection and the
image) is [projection.md](projection.md). The sailing skipper's profile, instruments, kinds, events
and an example observation are [sailing.md](sailing.md), The skipper's perception.

## Determinism

- The perception update runs in the deterministic tick, iterates in `EntityId` order (spec-sim),
  uses the deterministic math library and physics' ray casts, so `ObserverMemory` and
  `ObserverEvents` are identical in every run, fork and replay with the same inputs, and the world
  hash covers them.
- Queries are pure; ranking is a total order ending in `EntityId`; rounding uses one formatter and
  stored rounded values one rule (projection.md); the geometry is fixed expression by expression;
  token estimates count bytes. The same world, observer and request give the same bytes, natively
  and on the web. Instruments and derived facts are registered pure functions (a script one runs on
  the game thread under the script host's determinism setup and budget, spec-script, and writes
  nothing).
- Nothing in perception reads the wall clock, the render, or session state other than the event
  delivery cursor, which only selects which already-determined events a response includes.

## Checks

These run in the local check command (spec-arch) on the sailing showcase from slice 2 and on a small
fixture world from slice 1, and the projection golden cases in both lines.

- **`contract.perception.read_only`**: a seeded run of 3600 ticks with a fixed action script, run
  twice, once with no queries and once with every query kind issued every tick for every seat and
  the omniscient view; the per-tick world hashes (spec-persist) must be equal.
- **`contract.perception.deterministic`**: at 20 ticks of a seeded run, every query kind's response
  bytes for every seat are recorded; they must be equal in a second run, after `fork` (in both
  branches, before either acts), and after `restore` of a snapshot taken at that tick.
- **`contract.perception.noninterference`**: from a seeded world at tick T, a mutation is applied
  that the player cannot perceive: a `Hidden` fact changed, an entity outside every effective range
  moved within that region, an entity behind an occluder respawned elsewhere behind it, a
  `Hidden`-scope event emitted, an event of an undeclared kind emitted, a charted mark the player
  has not seen moved, a hidden change that makes a game rule raise a requested decision (time.md,
  Decision points), and another seat's thinking clock and pending decision changed. Then a fixed
  scripted player session runs on the mutated world and on the unmutated control: a list of calls
  covering every player tool (`session`, `describe`, `observe`, `nearby`, `events`, `affordances`,
  `intents`, `act`, `step` with `until`, `replay`, and refused calls among them: an unknown entity,
  an unmet affordance, a misspelt field). Every answer must be byte-identical between the two runs,
  the tick at which each `step` stops and the decision points included, and so must the control
  writes the seat's intent executors record. Property-based (proptest) over random mutations of
  those classes, 256 cases.
- **`contract.perception.budget`**: for budgets from the minimum to 2000 tokens in steps of 10,
  every response's projected bytes are at most `budget * 4`, and the percepts and events chosen at
  budget b are a prefix of the merged candidate sequence and a subset of those chosen at b + 10.
- **`contract.projection.golden`**: projection.md, Checks.
- **Visibility unit tests**: an entity whose squared distance equals `r*r` exactly and one 1 ulp
  beyond; at the field of view's edge, and at bearings whose `atan2` lands on 0, on -0 and just
  below 360; a zero horizontal offset (bearing 0); behind an occluder, with only its top showing; a
  remembered entity dropped when its place comes into sight, kept when it does not; memory expiry at
  exactly `memory_s`; capacity eviction order; event scopes of each kind, an undeclared kind
  perceived by no player, and a cause kept only when perceived. The edge cases also enter the shared
  golden cases, so both lines put them on the same side.

## Performance

Budgets (charter 3.10), measured with release builds on the reference hardware, are rows of
spec-arch's framework (`docs/spec/budgets.md`, 5.6). No slice 0 spike built perception, so none was
measured; slice 2 sets them by that framework's rule:

| Measure | Budget |
|---|---|
| `interface.perception` per tick, sailing scene (one skipper, about 30 perceivable entities, 3 occluding islands) | `perception.tick`: not measured |
| `interface.perception` per tick, 100 NPC observers over 1000 perceivable entities | `perception.npc`: not measured |
| `observe` at the default budget, text projection, sailing scene | `observe.sail`: not measured |

The rays dominate: two per candidate in range per occluding observer. A candidate whose range test
fails costs no ray; an observer without occlusion costs none.

## Open choices

1. **Memory and the event ring as world state.** Recommended (above). Rejected alternative:
   recompute on demand, which makes reads change state.
2. **The token estimate.** Recommended: bytes over four, deterministic and tokenizer-free, with the
   player benchmark reporting real tokens beside it so the ratio is known per model. A real
   tokenizer would differ per model and could not be shared.
3. **Default exposure `Hidden`.** Recommended: a fact is perceivable only when declared. The
   alternative, exposing component fields by default, would leak every new field to players.
4. **Raw `EntityId`s in observations.** Recommended: one addressing scheme for all tools. The cost
   is a small leak (gaps between ids hint at entities made elsewhere); per-observer handles would
   close it at the price of a mapping table per observer in the world.
5. **Two occlusion sample points.** Recommended (origin and top). More (the bounds' corners) catch a
   hull half behind a rock; slice 2 measures their cost first.
6. **Queries off the game thread.** Not in slice 2: every query is answered on the game thread. They
   may return later together with an allowed `pocket-mcp -> pocket-interface` edge and a budget for
   decoding a snapshot into a query world (Rapier's query pipeline rebuilt from the cache), when
   byte-identical answers can be proved.
7. **Defaults of the skipper profile** (ranges, attention, budget). Slice 2's player benchmark sets
   them; the values above are a starting point.
