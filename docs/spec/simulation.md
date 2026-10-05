# Simulation: ticks, the schedule, entities and iteration order

Status: Draft, slice 0

Charter: 3.2 (state in the world), 3.3 (determinism, fork and replay), 3.5 (the hooks the time modes
use), 3.7 (checks), 3.10 (performance benchmarks), 4.1 (`bevy_ecs`, single-threaded or explicitly
ordered), 4.2.6 (scripts swapped at a tick boundary), 5.1 (the game thread owns the world), 7.1
(tick semantics), 7.4 (entity iteration order).

This specification defines what one tick of the simulation is: the fixed timestep, the boundary
between ticks and what may happen there, the phases of a tick and the order of its systems, when
events are delivered, the hooks through which the time modes pause and step the world, entity
identity and its allocation, and the order in which systems visit entities. Its companions are
[numeric.md](numeric.md) (which state may be floating point, and the deterministic math library) and
[rng.md](rng.md) (random numbers). The code lives in the crate `pocket-sim`
([architecture.md](architecture.md), 4.1).

The goal throughout is the charter's: the same seed and the same inputs give the same world hash at
every tick (3.3), on every target, in debug and in release, in the original world and in any fork or
restore of it. Every rule below either closes a source of nondeterminism or gives a check that
catches one.

## 1. Related specifications

| Owner | Concepts used here by name | File |
|---|---|---|
| spec-persist | canonical serialization, the world hash, snapshot, restore, fork, replay, the first divergence, the classes of world state (Resource, Component, Cache, Derived) | `persistence.md`, `replay.md`, `versions.md` |
| spec-script | the script host API, `ScriptPhase`, the system transaction, the global freeze, the lint, the execution budget, faults, hot update | `script-host.md`, `script-sandbox.md`, `hot-update.md` |
| spec-contract | perception, actions, intents and affordances, observers, seats, time modes and pause-on-decision, the error protocol `{code, message, detail}` and its type `Problem` | [shared/contract/](../../shared/contract/README.md): [perception.md](../../shared/contract/perception.md), [actions.md](../../shared/contract/actions.md), [time.md](../../shared/contract/time.md), [errors.md](../../shared/contract/errors.md) |
| spec-mcp | the MCP tools (`step {until, watch}` among them) | [shared/contract/mcp.md](../../shared/contract/mcp.md) |
| spec-arch | crates, the game thread, its command queue and `Pace`, the local check command, budgets, the web build | `architecture.md`, `threads.md`, `checks.md`, `budgets.md` |

Error codes defined here travel in spec-contract's error protocol and follow its dotted snake_case
spelling `<family>.<reason>` (shared/contract/errors.md, Codes); the `sim`, `rng` and `number`
families are this specification set's.

## 2. Terms

- **World**: the `bevy_ecs` world the game thread owns (charter 5.1): every entity's components and
  the simulation's resources. Everything a tick reads or writes is in it (charter 3.2).
- **Tick n**: the n-th step of the simulation, which turns state n-1 into state n. State 0 is the
  world as loaded, before any tick. Tick numbers start at 1.
- **Boundary n**: the moment after tick n completed (or after loading, for n = 0) and before tick
  n+1 starts, while the world shows state n. It is the one name every specification uses
  (`threads.md`, 2; `persistence.md`'s "boundary `t`"); "the boundary before tick n" is boundary n-1
  said from the tick's side. A point inside a boundary, after some of its writes, is named
  `(tick, writes)`: the tick the world shows and the number of that boundary's writes applied so far
  (persistence.md's `SnapshotHeader`, threads.md's `WorldSnapshot`).
- **Boundary write**: anything that changes the world from outside a tick: a player's action or
  control, an editor or developer command, a hot update, a reseed. Boundary writes happen only at
  boundaries (`threads.md`, 2 and 5).
- **Phase**: one of the fixed stages of a tick (section 4.3), the Rust enum `TickPhase`.
- **System**: a named unit of tick work with a stable key (`physics.step`, `script:take_crates`),
  run on every tick (or on the ticks its run condition names) in its phase, at its place in the
  order.
- **Invocation**: one run of one system within one tick.
- **Event**: a record a system or a boundary write emits; the systems of the next tick read it
  (section 5).

## 3. Time

### 3.1 The fixed timestep

```rust
/// The number of ticks the world has completed; Tick(0) is the world as loaded.
/// At most 2^53 - 1, so it crosses to TypeScript as an exact number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Tick(pub u64);

/// Ticks per simulated second, fixed for a world's whole history.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct TickRate(pub u32); // 1..=1000; projects default to 60

/// The simulation clock: a persisted resource.
#[derive(Resource, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SimClock { pub tick: Tick, pub rate: TickRate }

impl SimClock {
    /// The step length in seconds: 1.0 / rate as f64, correctly rounded, the same on every target.
    pub fn dt(&self) -> f64;
    /// Simulated seconds at the end of the current tick: tick as f64 / rate as f64.
    pub fn time(&self) -> f64;
}
```

- The rate is set when a world is created (from the project's manifest; spec-persist records it in
  snapshots and replay headers) and never changes for that world or its forks. A rate outside
  1..=1000 is refused with `sim.tick_rate_invalid`. The showcase runs at 60, the rate of master's
  tick and of Rapier's default integration step.
- Simulated time is always derived from the tick count, never accumulated: `time()` is one
  integer-to-double conversion (exact below 2^53) and one division (correctly rounded by IEEE 754),
  so it is the same on every target and does not drift. Master derived time the same way ("the tick
  times its length", `docs/design/water.md`, The component).
- During tick n the clock already reads `Tick(n)`, and `time()` is the time state n will have:
  `sim.begin` advances the clock first (section 4.3), as Bevy advances `Time<Fixed>` before running
  its fixed schedule. A system that needs the start of the step uses `time() - dt()`. Scripts see
  the same values as `ctx.tick`, `ctx.dt` and `ctx.time` (`script-host.md`, 5.6).
- Durations that gate gameplay (cooldowns, timeouts, respawn delays) are counted in ticks, as
  integers (numeric.md, 3.2). A countdown in seconds (`t -= dt` until `t <= 0`) ends on a tick that
  depends on rounding; one in ticks ends where it says.

### 3.2 No wall clock

Nothing in a tick reads the wall clock, the date, the environment, files, the network or a thread's
identity. The tick-code crates hold this by the disallowed-methods list of section 10, scripts by
spec-script's sandbox (`Date` and `performance` absent, `script-sandbox.md`, 2.2). Timing a tick for
the performance budgets is done by the caller through `StepHooks` (section 6), outside the
simulation's code.

### 3.3 Pacing is not simulation

How fast ticks run against real time, when they pause and how many run to catch up after a slow
frame are decided outside the tick: by the time mode's `Pace` answer (spec-contract) in the game
thread's loop (`threads.md`, 3.2 and 3.3, Fiedler's fixed-timestep accumulator with a catch-up cap).
None of that changes what a tick does: a world stepped 600 times in one burst, or once a minute with
pauses between, reaches the same state 600. Pacing state (paused, the speed, the accumulator) is not
world state. Time-mode state that decides what is legal or what a tick does (whose turn it is, which
decision is awaited) is world state, owned by spec-contract and persisted like any resource
(`persistence.md`, 2).

## 4. The step

### 4.1 Boundary invariants

At every boundary the following hold, and `Sim::step` asserts them at its start and end in every
build: the constant-time checks (an empty outbox, an empty RNG table, no staged structural change,
no invocation running) always, and the walk of the entity index against the world's `EntityId`s only
with `pocket-sim`'s feature `invariants` (architecture.md 7.4), which the test builds and the
check's debug cross-build turn on, since it costs time proportional to the entities:

1. No invocation is running and no structural change (spawn, despawn, component insertion or
   removal) is pending.
2. The event outbox is empty, and the inbox holds exactly the events of the last tick and of the
   boundary writes applied since (section 5).
3. The RNG's per-tick stream table is empty (rng.md, 5.3): between ticks the random state is the
   world seed alone.
4. The world holds the whole simulation state in persisted classes (`persistence.md`, 2):
   components, the clock, the entity allocator, the event counter and inbox, the world seed, the
   time mode's game state, and the caches of physics and other systems. Nothing that influences a
   later tick lives elsewhere, in particular not in the script VM (charter 3.2, `script-host.md`,
   9).

These invariants are what make a boundary a valid point for a snapshot, a fork, a hash and a hot
update.

### 4.2 Writes at a boundary

Boundary writes are applied by the runtime (`pocket-runtime`, [architecture.md](architecture.md),
4.8) one at a time, in the canonical order of `threads.md`, 5.2 (sorted by source and sequence). For
the simulation the order is the order of application: each write sees the world as the writes before
it left it.

- A write is atomic: it validates against the world as it stands, then applies all of its effects or
  none, and answers with its result or a structured error (charter 3.4). A refused write changes
  nothing and is not recorded (`threads.md`, 5.3); ids it allocated return through the allocator's
  rollback (7.2).
- A write may set components, spawn and despawn entities (allocating ids from the world's allocator,
  section 7.2), emit events (into the inbox, read by the next tick; section 5) and swap scripts
  (section 4.7). It MUST NOT draw random numbers (rng.md, 5.3); a write whose effect should be
  random sets state that a system rolls in the next tick.
- Writes are applied as they arrive, whether the game is paused or waiting for its next tick in real
  time (`threads.md`, 3.2; the threads spike measured 1.0 to 1.6 ms from a command to its visible
  effect this way, against 8.7 to 9.5 ms at the median when held for the next tick), so a paused
  agent that sets a value reads it back at once. Both are one semantics: the writes applied at
  boundary n-1 are the inputs of tick n, recorded in application order with that tick (`threads.md`,
  5.3, `Applied { tick, index, .. }`), so `state(n) = tick_n(apply(state(n-1), writes(n)))`.
- The hash of tick n (spec-persist) is taken when tick n completes, before any write of boundary n.
  A snapshot or fork taken at a boundary after some writes holds them and is named `(tick, writes)`
  (section 2), which `Applied.index` already gives, so the first divergence can point at a write as
  well as at a tick.

### 4.3 Phases

A tick runs these phases in this order. Each phase is a `bevy_ecs` system set; the sets are chained,
and the systems inside each set are chained, so the order is total (charter 4.1: "single-threaded or
explicitly ordered"; this design is both).

```rust
pub enum TickPhase { Begin, Control, Update, Forces, Physics, Finish }
```

| # | Phase | What runs there |
|---|---|---|
| 1 | `Begin` | `sim.begin`: advances the clock to tick n |
| 2 | `Control` | intent controllers and control processing (spec-contract, implemented in `pocket-interface`): every intent's deadline checked, then the Rust executors, turning the controls and intents that boundary writes set into this tick's low-level controls |
| 3 | `Update` | the project's TypeScript systems and script intent executors, the `ScriptPhase` `"update"` (`script-host.md`, 4.2 and 5.5), in the order of the project's `systems` array, run by `script.update` |
| 4 | `Forces` | environment and force systems: wind, buoyancy, sail, hull and keel (`pocket-physics`) |
| 5 | `Physics` | the rigid-body step of `dt`, its write-back, contact events |
| 6 | `Finish` | the perception update and the turn rule (spec-contract, `pocket-interface`), which read the tick's state and event record and write components only, emitting no event; then `sim.finish`: moves the outbox to the inbox (section 5.2), clears the RNG table, compacts the entity index, asserts the boundary invariants, collects decision requests |

Scripts run before physics, as Unity's `FixedUpdate` scripts run before its physics step and Avian
(a Bevy physics integration) steps in `FixedPostUpdate` after the user's `FixedUpdate`. Scripts see
a tick's contacts as events in the next tick (section 5). Hashing, snapshot publication
(`threads.md`, 4.3) and the time mode's next `Pace` happen after `Finish`, outside the tick, in the
runtime.

`Update` is the only script phase in slice 1. A second script phase after `Physics` (rules that must
see this tick's positions before the boundary) is added here, with its place in the table, when a
project needs it.

### 4.4 The system order

The order of slice 1 (the headless sailing scene) is below. Crates register their systems into these
slots ([architecture.md](architecture.md), 4.4 and 4.5); adding, removing or moving a system changes
this table and the schedule golden of section 12 together.

| Order | Phase | System key | Crate | Work |
|---|---|---|---|---|
| 1 | Begin | `sim.begin` | pocket-sim | clock to tick n |
| 2 | Control | `interface.intents` | pocket-interface | check every intent's deadline, Rust and script intents alike, in `IntentId` order; run the Rust executors (come to a heading, trim the sail, sail to a waypoint) into helm and sail controls |
| 3 | Update | `script.update` | pocket-script | the project's systems and script executors, `script:<name>` each, in `systems` order |
| 4 | Forces | `physics.wind` | pocket-physics | wind velocity at each body, gusts from position and time |
| 5 | Forces | `physics.buoyancy` | pocket-physics | buoyancy and water drag per hull cell, waves at `time() - dt()` |
| 6 | Forces | `physics.sail` | pocket-physics | sail force from the apparent wind and the trim |
| 7 | Forces | `physics.hull` | pocket-physics | keel, rudder and hull drag |
| 8 | Physics | `physics.step` | pocket-physics | sync changed bodies in, one step of `dt`, write back positions, velocities and readouts |
| 9 | Physics | `physics.contacts` | pocket-physics | contact begin and end events, in pair order (section 8.4) |
| 10 | Finish | `interface.perception` | pocket-interface | every observer's memory and perceived events from the events appended since its last run: the boundary writes' (the inbox from `boundary_from`, 5.1) and the tick's own (still in the outbox), in `EventSeq` order (shared/contract/perception.md, The perception update) |
| 11 | Finish | `interface.turns` | pocket-interface | ends a turn's resolving phase when its rule holds (shared/contract/time.md, Turn structure) and evaluates the episode's `done` predicate (time.md, Episodes); writes the turn and episode state only |
| 12 | Finish | `sim.finish` | pocket-sim | events, RNG table, index, invariants, decisions |

Rows 4 to 9 follow the physics spike (`docs/spikes/physics.md`, Recipe 3): forces are computed from
the state at the tick's start and applied as user forces after Rapier's `reset_forces` and
`reset_torques`, then one step runs. The physics crate's own specification may reorder them inside
`Forces` and `Physics`, which changes this table.

### 4.5 Invocations

- **Run conditions.** A system runs on every tick unless it declares `Every { period, offset }`
  (ticks n with n % period == offset; period at least 1, offset below period) or `Start` (tick 1
  only). Conditions depend on the tick number alone, so a hot update at tick 500 does not rerun a
  `Start` system, and reloading unchanged scripts changes nothing (charter 3.7, reload equivalence).
- **Transactions.** An invocation's effects (component writes, structural changes, events, RNG
  draws, entity ids) take effect when it returns normally, together. When it fails (a script throws,
  exceeds its execution budget, or a write breaks the numeric rules), none of them do: the script
  host discards its staged commands (`script-host.md`, 5.4), rolls the entity allocator back to the
  mark taken when the invocation began (section 7.2), and rolls the RNG table back to its mark
  (rng.md, 5.3). The world is as if the system had not run, and later systems run normally. Engine
  systems in Rust do not fail this way; their failures are internal errors (4.6).
- **Structural changes** requested by an invocation are applied when it returns, in request order. A
  spawned entity's id is known at once (7.2); the entity becomes visible to queries from the next
  invocation on. For engine systems this is `bevy_ecs`'s deferred `Commands` with the automatic sync
  points between chained systems; for scripts the host applies its staged commands.
- **No change detection.** `bevy_ecs`'s change ticks (`Added`, `Changed`, `Ref::is_changed`,
  `is_added`, `RemovedComponents`) and component hooks or observers MUST NOT drive simulation logic:
  change ticks are not world state, and a restored world inserts every component afresh, so a system
  that read them would act differently after a restore (`persistence.md`, 2). A system that needs
  "what changed" keeps it as data in a component. Section 10 enforces the ban.
- **No parallelism inside a tick.** The schedule runs on `bevy_ecs`'s single-threaded executor,
  built without its `multi_threaded` feature ([architecture.md](architecture.md), 6; `threads.md`,
  3.4 for a later data-parallel helper under its own rule).

### 4.6 Errors inside a tick

A failed invocation never stops a tick: it becomes an entry in the step's report (section 6) with
the tick, phase, system key, the entity when one is involved, and the cause in `detail` (a script's
error record with its TypeScript location comes from `script-sandbox.md`, 5). A failed system runs
again on its next scheduled tick like any other; a system is never disabled silently.

Three things poison the world instead: a panic inside a system, a panic inside a native a script
called (caught at the native, `script-sandbox.md`, 4.3), and a script fault (`script.out_of_memory`
or `script.stack_overflow`, whose occurrence depends on the platform, `script-sandbox.md`, 4.3). The
step catches each (a panic natively, where the release profile unwinds;
[architecture.md](architecture.md), 7.2), stops the tick there, reports `sim.internal` or the
fault's code, and marks the world poisoned. A poisoned world refuses `step` with
`sim.world_poisoned` until a snapshot is restored into it, because a half-run tick cannot be trusted
to satisfy the boundary invariants, and the recorder writes a `Fault` record instead of the tick's
(replay.md 2.2). The game thread keeps running and answering commands; only a panic outside
`Sim::step` stops the game (`threads.md`, 8). On the web profile a panic aborts the worker.

### 4.7 Hot update

A hot update is a boundary write (`hot-update.md`; `threads.md`, 5.2 and 6): the runtime swaps the
script bundle at a boundary, and the new bundle's systems take their places in `Update` from the
next tick on, in its `systems` order with its run conditions. A bundle that fails to load leaves the
old one running and answers with a structured error (charter 4.2.6). Because the swap happens only
at a boundary, no tick runs half on old scripts and half on new. Trying a bundle in a fork first is
a fork (spec-persist) followed by the same write on the fork.

## 5. Events

### 5.1 The record

```rust
/// A world-wide sequence number: allocated when an event is appended, never reused, at most 2^53 - 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EventSeq(pub u64);

pub struct Event {
    pub seq: EventSeq,
    pub tick: Tick,               // the clock when it was appended: n during tick n, n-1 at the boundary before tick n
    pub kind: EventKind,          // a type such as "contact.begin" or "crate.taken" (script-host.md, 5.5, for the syntax)
    pub subject: Option<EntityId>,
    pub cause: Option<EventSeq>,  // the event that led to this one, when the emitter names it
    pub data: EventData,          // plain data in spec-persist's canonical form
}

/// Persisted resources (`persistence.md`, 2): the counter, and the events the next tick reads.
#[derive(Resource)] pub struct EventCounter { next: u64 } // 1 in a new world
#[derive(Resource)] pub struct EventInbox {
    events: Vec<Event>,   // ascending seq
    boundary_from: u32,   // events from this index on were appended by boundary writes
}
```

Sequence numbers are allocated when an event is appended: when the emitting invocation commits (a
failed invocation allocates none, `script-host.md`, 5.4) or when the boundary write that emits it
applies. Systems run in a fixed order and visit entities in id order (section 8), so the numbers are
deterministic. A single number serves scripts' `cause` (master's events carried one,
`docs/design/world-model.md`, Events with causes).

### 5.2 Delivery: one tick later, to every reader

Events are double-buffered, Bevy's event model, as `script-host.md` (5.5) recommends:

- During tick n, every system reads the **inbox**: the events appended during tick n-1 and by the
  boundary writes before tick n. Events appended during tick n go to the **outbox** (a per-tick
  buffer outside the persisted state, empty at boundaries).
- `sim.finish` replaces the inbox with the outbox. The inbox at a boundary is therefore exactly the
  events of the last tick plus those of the writes applied since, in sequence order; it is persisted
  and hashed with the world, so a divergence in events shows in the hash of the tick that emitted
  them.
- Every system that reads events reads the whole inbox (a script system the kinds it declares, in
  sequence order, `script-host.md`, 5.5), so each event reaches each interested system exactly once
  whatever the system order, and no reader keeps a cursor, which would be hidden state.
- A chain of reactions takes a tick per step. An endless chain (two rules that answer each other)
  costs events every tick and shows in the inbox; nothing in the engine has to break it.
- Contacts from the physics of tick n reach the scripts of tick n+1, one 1/60 s later at the
  showcase's rate.

### 5.3 The tick's event record

After tick n the inbox is the tick's event record. The step's report hands it to the runtime
(section 6), and `Boundary` hands over the events each boundary write appends; the runtime feeds
both to the presenters' event stream (`threads.md`, 4.4), the perception layer's push deltas
(spec-contract) and the transcript. Older events leave the world when the next tick ends; any
history beyond one tick is kept outside the world by its consumers (`persistence.md`, 5.2: "the
event history agents read" is not covered by the hash).

## 6. Hooks for the time modes

The time modes (turns, lockstep, pausable real time, stepping, pause-on-decision; spec-contract) and
the game thread's loop (`threads.md`) drive the simulation through these functions of `pocket-sim`,
which the runtime's `Game` ([architecture.md](architecture.md), 4.8) composes:

```rust
pub struct SimConfig { pub rate: TickRate, pub seed: u64 }

impl Sim {
    pub fn new(config: SimConfig) -> Result<Sim, Problem>;
    /// The clock: the last completed tick, and the rate.
    pub fn clock(&self) -> SimClock;
    /// Mutable access between ticks, for boundary writes (section 4.2).
    pub fn boundary(&mut self) -> Boundary<'_>;
    /// Runs exactly one tick. Never blocks, never waits for input, never reads the clock.
    pub fn step(&mut self, hooks: &mut dyn StepHooks) -> Result<StepReport, Problem>;
    /// Read-only access at a boundary, for perception, hashing and snapshot publication.
    pub fn world(&self) -> &World;
}

/// What a boundary write may do: component writes, structural changes through the allocator, events.
pub struct Boundary<'a> { /* &mut World */ }

pub struct StepReport {
    pub tick: Tick,                      // the tick just completed
    pub events: Vec<Event>,              // the new inbox: the tick's event record (5.3)
    pub decisions: Vec<DecisionRequest>, // in request order
    pub errors: Vec<Problem>,        // failed invocations, in the order they failed
}

/// Raised by a system during a tick: an observer needs a decision (charter 3.5). `reason` is a name
/// the game declares (shared/contract/time.md, Decision points), never free text.
pub struct DecisionRequest { pub observer: EntityId, pub reason: String, pub event: Option<EventSeq> }

/// Called around phases and systems by step, and around each script system by `script.update`
/// (`script-host.md`, 9); timing and profiling live here, outside the simulation, with a clock the
/// caller injects (`threads.md`, 3.1).
pub trait StepHooks {
    fn phase(&mut self, phase: TickPhase, begin: bool) {}
    fn system(&mut self, key: &SystemKey, begin: bool) {}
}
```

- **Stepping** is `step` called once; `step {until, watch}` (spec-mcp, spec-contract) is a loop of
  `step` calls that tests its condition at each boundary. A tick is atomic: nothing pauses inside
  one, and the LLM is never inside it (charter 3.5); an execution budget that runs out ends the
  invocation, not the tick. (A native debugger's breakpoint stops the game thread inside a tick,
  `threads.md`, 3.5; the tick resumes and completes.)
- **Pause-on-decision.** Any system may raise a `DecisionRequest` with a reason the game declares;
  the tick still completes, and the report carries the requests. The time controller delivers one to
  a player only under the contract's rule (shared/contract/time.md, Decision points), so a request's
  timing cannot reveal what the player does not perceive. The time mode decides what follows; in
  pause-on-decision its `Pace` is `WaitForCommand` until the observer's decision arrives as a
  boundary write (`threads.md`, 5.2). Requests are tick output, deterministic like events. A time
  mode that must remember an outstanding request across a snapshot keeps it in its persisted state
  (3.3).
- **Turns and lockstep** need nothing more: a turn is a run of `step` calls between boundaries where
  writes are accepted; lockstep applies every peer's writes for tick n at the boundary before tick n
  in the canonical order, then steps.
- `Problem` is spec-contract's error type, declared in the leaf crate `pocket-contract`
  ([architecture.md](architecture.md), 4.16). `SystemKey` is the system's stable key string.
  `TickPhase` is not `threads.md`'s `LoopState` (the game thread's status), which is presentation
  data.

## 7. Entities

### 7.1 EntityId

```rust
/// A simulation entity's identity. Allocated once, never reused within a world's history, the same
/// in every fork, restore and replay of that history, and exact as a JavaScript number.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema)]
#[component(immutable)]
pub struct EntityId(NonZeroU64); // 1..=MAX_ENTITY_ID

pub const MAX_ENTITY_ID: u64 = (1 << 53) - 1;

impl EntityId {
    /// Exact: every id is a positive integer below 2^53.
    pub fn to_f64(self) -> f64;
    /// From a script or JSON number: finite, integral, 1..=MAX_ENTITY_ID, else sim.entity_id_invalid.
    pub fn from_f64(x: f64) -> Result<EntityId, Problem>;
}
```

- Every simulation entity carries its `EntityId` as an immutable component (`bevy_ecs` 0.19's
  `#[component(immutable)]`), so no system can change an entity's identity.
- Components refer to other entities by `EntityId`, never by `bevy_ecs::Entity` (charter 3.2).
  `Option<EntityId>` costs eight bytes (the niche of `NonZeroU64`); scripts see `null`, or 0 in a
  column (`script-host.md`, 6).
- `bevy_ecs::Entity` never leaves `pocket-sim` and the systems that run inside the world: it is not
  in components, events, snapshots, hashes, the shared contract, or anything a script sees. Its
  index and generation depend on the history of the storage, which a restored or forked world does
  not share.
- The bound 2^53 - 1 keeps ids exact as doubles (charter 4.2.7); at a million spawns a second it
  lasts 285 years. `from_f64` is the inverse `script-host.md` (6) asks for; the host reports its
  refusal as `script.entity_missing`. The script-native spike met the hazard this rule removes: its
  first spawned entity was 1, not 0, because `bevy_ecs` 0.19's world holds an entity of its own at
  index 0 (`docs/spikes/script-native.md`, Problems 1).
- `Name(String)` is the engine's plain-data name component: a human-readable name of at most 64
  UTF-8 bytes, not necessarily unique, persisted like any component. The shared contract resolves an
  `EntityRef` given as a string through it (shared/contract/README.md, Positions and entity
  references).

### 7.2 Allocation

```rust
/// A persisted resource.
#[derive(Resource, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EntityAllocator { next: u64 } // the id the next spawn receives; 1 in a new world

impl EntityAllocator {
    pub fn allocate(&mut self) -> Result<EntityId, Problem>; // sim.entity_ids_exhausted past the bound
    pub fn mark(&self) -> AllocMark;                             // taken when an invocation begins
    pub fn rollback(&mut self, mark: AllocMark);                 // a failed invocation's ids return
}
```

- Allocation happens at the spawn call, so the caller has the id at once; insertion follows section
  4.5. A failed invocation's ids are returned by `rollback`; they were never visible outside the
  invocation, so handing them out again names no stale reference.
- Ids increase in allocation order and are never reused once an entity holding them was committed. A
  stale id therefore never names a newer entity, so the generation counter of a generational index
  (what `bevy_ecs::Entity` carries) is unnecessary. Factorio documents its `LuaEntity::unit_number`
  the same way: allocated sequentially, not reused.
- Allocation order is deterministic because everything that allocates is: boundary writes in their
  applied order, systems in schedule order visiting entities in id order.
- Only the simulation allocates ids, and only for entities of the simulation. The engine never
  spawns hidden entities in the world for its own use (cameras, editor gizmos, render proxies and
  debug drawing live in the presenters, [architecture.md](architecture.md), 2). Master's state hash
  had to identify entities by their place in the world's walk instead of their ids because "the ids
  depend on how many entities the build made before the scene, and a web build makes fewer"
  (`docs/design/networking.md`, Determinism); here every build makes exactly the entities the scene
  and the ticks make, so ids are the same on every target and the hash uses them (`persistence.md`,
  5.2).

### 7.3 Spawn and despawn

- `spawn(components) -> Result<EntityId, Problem>` allocates and stages the entity with its initial
  components.
- `despawn(id) -> Result<bool, Problem>` stages the removal of the entity and all its components and
  returns `true`. For an id that was allocated but whose entity is already despawned or staged for
  despawn it does nothing and returns `false`, so two rules that remove the same crate in one tick
  are not an error (`script-host.md`, 5.4: "a second `despawn` is a no-op"). An id that was never
  allocated (0, at least `next`, or not an integer) is refused with `sim.entity_id_invalid`.
- Despawning never despawns other entities implicitly. Components that pointed at a despawned entity
  keep the id; reading through it finds nothing: a script's `get` answers `undefined` and a write
  naming it throws `script.entity_missing` (`script-host.md`, 5.4). A hierarchy feature that wants
  recursive despawn says so in its own specification.

### 7.4 Scenes, restore and fork

- Loading a scene or instantiating a prefab allocates ids in document order, depth first, a parent
  before its children, and rewrites the document's local references to the new ids. The same
  document loaded into the same world state yields the same ids.
- Restore (spec-persist) recreates every entity with its recorded `EntityId` and restores the
  allocator. The `bevy_ecs::Entity` values and the storage order of the restored world differ from
  the original's; nothing observable depends on them (section 8).
- A fork copies the allocator with the rest of the world, so the original and the branch allocate
  the same ids for the same spawns (charter 3.3).

### 7.5 The index

```rust
/// Derived (persistence.md, 2): rebuilt on restore and fork, never hashed or serialized.
#[derive(Resource, Default)]
pub struct EntityIndex { by_id: Vec<(EntityId, Entity)> } // ascending by id
```

Since committed ids only increase, inserting a spawned entity is a push that keeps the vector
sorted; a despawn marks its slot, and `sim.finish` compacts the vector once per tick. Lookup by id
is a binary search. Restore and fork rebuild the index in one sorted pass.

## 8. Iteration order

### 8.1 The rule

Every iteration whose result reaches the world, the events, the RNG draws, the order of allocation
or the step's report visits entities in ascending `EntityId` order (charter 7.4), never in the ECS's
storage order. Storage order in `bevy_ecs` depends on the storage's history (rows are swap-removed
on despawn, entities move between tables when components are added or removed, archetypes are
visited in creation order), so it differs between a world and its restored copy even when their
states are identical. Floating-point sums, the order of emitted events and RNG draws, and the order
of spawns all depend on visiting order, so a storage-ordered loop makes a restored world drift from
the original.

### 8.2 Engine systems in Rust

- The default is `bevy_ecs`'s sorted query iteration,
  `query.iter_mut().sort_unstable::<&EntityId>()` (`QueryIter::sort_unstable`, added in Bevy 0.14).
  Ids are unique, so an unstable sort yields the one total order.
- A loop whose per-entity work is independent of order may iterate in storage order: each entity's
  update reads only that entity and values no other entity changes in the loop, and the loop emits
  no events, draws no random numbers, spawns nothing and accumulates no floating-point value across
  entities (`position += velocity * dt` qualifies; a sum of forces over bodies does not). Such a
  loop carries `#[allow(clippy::disallowed_methods)]` with a comment saying why it is
  order-independent. The storage-shuffle check (section 12) is the backstop for a wrong claim.
- Measured (section 13): over 10,000 entities with fragmented storage, the sorted loop costs about
  seven times the storage-ordered one for a trivial body, because the sort and the random access per
  entity dominate. At the showcase's scale (hundreds of bodies) that is microseconds; it is why
  order-independent loops may skip the sort.

### 8.3 Scripts

Every batch the script host hands a script lists entities in ascending id order (`QueryResult.ids`,
`script-host.md`, 5.2), and the host applies a script's commands in call order and its column
write-backs by ascending id (5.4). Inside an invocation a script's own collections follow
ECMAScript's specified orders (arrays by index; `Map`, `Set` and string keys by insertion,
integer-like keys first ascending); scripts keep no state between invocations (charter 3.2), so no
order survives a call inside the VM.

### 8.4 Pairs, collections and hierarchies

- Pairs of entities (contacts, overlaps, proximity results) are ordered by `(min id, max id)`, then
  by the pair's own record order where a pair has several; contact events are emitted in that order.
- Collections inside components and resources have a defined order: a `Vec` in a meaningful order,
  or `BTreeMap` and `BTreeSet`. `std`'s `HashMap` and `HashSet`, `hashbrown`'s and `bevy_platform`'s
  are disallowed in the tick-code crates (section 10): `std`'s default `RandomState` seeds each map
  randomly per process, and even a fixed-seed map iterates in an order that depends on its insertion
  and growth history, which a restored or forked world does not share. A hash map may serve lookups
  only behind a type that offers no iteration, with a comment saying why (`checks.md`, 5.3). rustc
  guards the same hazard in its own code with the internal lint `potential_query_instability`.
- Child order in a hierarchy is explicit data (a `Vec<EntityId>` in the parent's component, in
  insertion order), never derived from storage.
- Ties in any sort that affects state are broken by `EntityId`.

### 8.5 Physics

The physics crate adds bodies to its solver in `EntityId` order (when a scene loads and, within a
tick, for the bodies that newly appear), applies forces in id order, and writes results back in id
order. The solver's internal order (Rapier's arena handles, its contact graph) depends on the
sequence of insertions and removals, which a solver rebuilt from components does not reproduce. The
solver's state is therefore a Cache that snapshot and fork carry as it is (charter 3.3;
`persistence.md`, 8).

## 9. Why this is deterministic

| Source of nondeterminism | How it is closed |
|---|---|
| Threads and scheduling | One thread owns the world (charter 5.1); single-threaded executor; no thread pools in a tick (4.5) |
| System order | Chained phases and chained systems: a total order, pinned by the schedule golden (4.3, 4.4, 12) |
| Storage order | Ascending `EntityId` iteration; the storage-shuffle check (8, 12) |
| Hash-map iteration | Disallowed types (8.4, 10) |
| Change detection, hooks, observers | Not used by simulation logic (4.5, 10) |
| Wall clock, environment, I/O | Disallowed methods; time derived from the tick (3) |
| Floating-point functions, NaN, flush-to-zero, `min` and `max` | numeric.md |
| Random numbers | Derived from the world seed and keys; nothing between ticks but the seed (rng.md) |
| When outside changes land | Only at boundaries, in a recorded order (4.2) |
| Entity identity | An allocator in the world; no hidden engine entities (7.2) |
| Events | Sequence numbers from a persisted counter; delivery by a persisted inbox (5) |
| Script state | Stateless scripts, the frozen global, the lint, the reload-equivalence check (charter 4.2.5, spec-script) |
| Failure halfway | Transactional invocations; a panic poisons the world (4.5, 4.6) |

## 10. Enforcement

The tick-code crates (`pocket-sim`, `pocket-persist`, `pocket-physics`, `pocket-interface`,
`pocket-script`) and `pocket-runtime`, which applies the boundary writes whose order reaches the
world (4.2), carry a `clippy.toml`, and the local check runs clippy with warnings as errors, for the
host target and again for `wasm32-unknown-unknown` (`checks.md`, 5.3). Every copy is generated from
one source, `tools/clippy-determinism.toml`, by `cargo xtask gen`, so a copy that drifts fails as
`gen.stale`. `pocket-runtime` takes the lists without the wall-clock entries except in its
loop-clock module, which reads `Instant::now` under `#[allow(clippy::disallowed_methods)]` with its
reason (`threads.md`, 3.2). This specification's entries follow; numeric.md, 5 lists the numeric
ones.

```toml
disallowed-methods = [
    { path = "std::time::Instant::now", reason = "no wall clock in a tick (simulation.md, 3.2)" },
    { path = "std::time::SystemTime::now", reason = "no wall clock in a tick (simulation.md, 3.2)" },
    { path = "std::env::var", reason = "no environment in a tick (simulation.md, 3.2)" },
    { path = "std::thread::spawn", reason = "one thread owns the world (simulation.md, 4.5)" },
    { path = "bevy_ecs::system::Query::iter", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::system::Query::iter_mut", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::system::Query::iter_many", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::system::Query::iter_many_mut", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::system::Query::par_iter", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::system::Query::par_iter_mut", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::query::QueryState::iter", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::query::QueryState::iter_mut", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::world::World::iter_entities", reason = "iterate in EntityId order (simulation.md, 8.2)" },
    { path = "bevy_ecs::change_detection::DetectChanges::is_changed", reason = "no change detection (simulation.md, 4.5)" },
    { path = "bevy_ecs::change_detection::DetectChanges::is_added", reason = "no change detection (simulation.md, 4.5)" },
    { path = "bevy_ecs::world::World::add_observer", reason = "no observers in simulation logic (simulation.md, 4.5)" },
    # ... the numeric list of numeric.md, 5
]
disallowed-types = [
    { path = "std::collections::HashMap", reason = "iteration order (simulation.md, 8.4)" },
    { path = "std::collections::HashSet", reason = "iteration order (simulation.md, 8.4)" },
    { path = "hashbrown::HashMap", reason = "iteration order (simulation.md, 8.4)" },
    { path = "bevy_platform::collections::HashMap", reason = "iteration order (simulation.md, 8.4)" },
    { path = "bevy_ecs::query::Added", reason = "no change detection (simulation.md, 4.5)" },
    { path = "bevy_ecs::query::Changed", reason = "no change detection (simulation.md, 4.5)" },
    { path = "bevy_ecs::lifecycle::RemovedComponents", reason = "no change detection (simulation.md, 4.5)" },
    { path = "bevy_ecs::system::Local", reason = "no state outside the world (simulation.md, 4.1)" },
]
```

Verified with clippy 1.98.1 and `bevy_ecs` 0.19.1 on 2026-10-03, with a scratch crate that uses each
item: every path above resolves and is reported, including primitive methods (`f64::sin`), trait
methods (`DetectChanges::is_changed`), re-exported paths (`bevy_ecs::system::Query::iter`) and types
of crates the scratch crate depends on (`hashbrown`, `bevy_platform`). The seven paths added after
review (`Query::iter_many`, `iter_many_mut`, `par_iter`, `par_iter_mut`, `QueryState::iter` and
`iter_mut`, which exclusive systems and the snapshot of rows use, and `World::iter_entities`) were
verified the same way on 2026-10-03: a scratch crate with `bevy_ecs =0.19.1` (default features off,
`std` on) calling each once, `cargo clippy` 0.1.98 reported "use of a disallowed method" for all
nine iteration paths, `Query::iter` and `iter_mut` included. Two gaps: `for x in &query` goes
through `IntoIterator` and is not reported, and component hooks declared in a derive attribute have
no path to disallow. The storage-shuffle and restore checks, not the lint, prove those rules. The
order of 4.4 is total by construction, so there is no ambiguity to detect (14, choice 7).

## 11. Error codes

| Code | When | `detail` |
|---|---|---|
| `sim.tick_rate_invalid` | a world is created with a rate outside 1..=1000 | `{rate, min, max}` |
| `sim.entity_ids_exhausted` | an allocation past `MAX_ENTITY_ID` | `{next}` |
| `sim.entity_id_invalid` | an id that was never allocated, 0, or not an integer in range | `{id, next}` |
| `sim.entity_not_found` | an operation needs a live entity and the id's entity is despawned | `{id}` |
| `sim.entity_alive` | a revival (the editor's undo of a destroy, `entity::revive`) names an id whose entity is live | `{id}` |
| `sim.system_failed` | an invocation failed; its effects were discarded | `{tick, phase, system, entity?, cause}`, `cause` in the error protocol's shape |
| `sim.internal` | a system panicked | `{tick, phase, system, message}` |
| `sim.world_poisoned` | `step` on a world whose last tick panicked | `{tick}` |
| `sim.collider_density` | a dynamic convex hull, trimesh or compound collider declares a density, whose mass properties Rapier computes with the platform's math natively (numeric.md, 5); slice 1 refuses it on a fixed body too (numeric.md 11, choice 11) | `{entity, collider}` |
| `sim.collider_invalid` | a collider whose shape Rapier cannot build (a size that is not positive and finite in the solver's `f32`, a hull of fewer than four points or spanning no volume, a mesh index out of range, an empty compound) or whose friction, restitution or density is out of range, or whose density gives a mass or inertia of 0 or infinity in `f32` (slice 1, pocket-physics; numeric.md 11, choices 11 and 15) | `{entity, reason}` |
| `sim.body_invalid` | a dynamic body with no mass, or with mass both stated and from a density; stated mass properties not positive and finite in `f32`; damping or gravity scale out of range; a `Collider` without a `Transform`, or a `Transform` whose position is not finite in `f32` or whose rotation is zero (numeric.md 11, choices 11 and 15; slice 1, pocket-physics) | `{entity, reason}` |
| `sim.name_too_long` | a `Name` of more than 64 bytes (7.1; slice 1) | `{bytes, max}` |
| `sim.event_kind_invalid` | an event kind that is not dotted snake_case with at least one dot (5.1; slice 1) | `{kind}` |
| `sim.system_key_invalid` | a system key that is not 1 to 64 bytes of lowercase letters, digits and `_ . : -` (slice 1) | `{system}` |
| `sim.system_duplicate` | a second system under one key (slice 1) | `{system}` |
| `sim.system_phase` | a key of the order of 4.4 registered in another phase (slice 1) | `{system, phase, expected}` |
| `sim.run_condition_invalid` | `Every` with a period of 0 or an offset not below it (4.5; slice 1) | `{period, offset}` |
| `sim.component_name_invalid` | a component name not matching `^[A-Z][A-Za-z0-9]*$` or a field name not matching `^[a-z][a-z0-9_]*$` (script-host.md 7.1; slice 1) | `{name, pattern}` |
| `sim.component_field_invalid` | a component field repeated, without a doc, with a default that does not fit its type or is not finite, with enum variants missing or repeated, or past 65,535 numeric slots (slice 1) | `{component, field}` |
| `sim.component_duplicate` | a second component registered under one name (slice 1) | `{name}` |
| `sim.data_invalid` | plain data whose object keys repeat or are out of byte order (persistence.md 3.4; slice 1) | `{reason, key}` |

Numeric codes are in numeric.md, 9; RNG codes in rng.md, 9.

## 12. Checks and tests

Each item names what it proves. The whole-run checks of charter 3.7 (determinism, fork consistency,
replay, reload equivalence) are specified in `checks.md` (8) and `persistence.md` (12); these are
what tick semantics add.

1. **Schedule golden** (`sim.schedule`): the flattened schedule of a slice's runtime (phase, system
   key, run condition, in order) printed and compared with a committed file. A change to section 4.4
   changes the file in the same commit.
2. **Storage shuffle** (`sim.shuffle`): a project run N ticks twice, the second time with the
   world's storage permuted before every tick (a check-only hook despawns and respawns every
   `bevy_ecs` entity in an order drawn from a non-simulation generator, keeping each `EntityId` and
   component, and rebuilds the index), must give the same hash chain. This is the direct proof of
   section 8, and it catches what the lint misses (`for x in &query`, a wrong order-independence
   claim). Its negative control is a test-only system that sums a float over a query in storage
   order; the shuffled run must diverge.
3. **Entity ids** (`sim.entity_ids`, unit tests): ids start at 1 and increase; a committed id is
   never reused; a failed invocation's ids return through `rollback`; `despawn` of a despawned id
   returns `false` and of a never-allocated id refuses with `sim.entity_id_invalid`; restore and
   fork continue with the same `next`; allocation past the bound refuses; `from_f64` refuses 0, 1.5,
   -1, 2^53 and NaN.
4. **Boundary invariants** (`sim.invariants`): asserted at the start and end of every step (4.1: the
   constant-time ones in every build, the index walk with the feature `invariants`); a test that
   leaves an outbox event or a staged spawn behind must trip them, in a release build too.
5. **Events** (`sim.events`, unit tests): sequence numbers follow append order; an event appended in
   tick n is read by every system of tick n+1 and by none of tick n+2; a boundary write's event is
   read in the next tick; a failed invocation appends none.
6. **Transactions** (`sim.transaction`, with spec-script's host): a system that writes, spawns,
   emits and draws and then throws leaves no write, entity, event or draw behind (the next system's
   draws and the next spawn's id are as if it had not run).
7. **Time** (`sim.time`, unit tests): `time()` at tick 216,000 at 60 Hz is exactly 3600.0; `dt()` is
   `1.0 / 60.0`; run conditions fire on the ticks they name.
8. **Lint fixture** (`checks.md`, 5.3): a fixture crate under `tests/fixtures/` that uses every
   disallowed item must fail clippy, so a list entry that stops resolving after a dependency update
   is caught.
9. **Restore continuation** (`persistence.md`, 12): continuing from a restored snapshot gives the
   same hash chain as continuing the original; it also catches change detection or hooks driving
   logic.
10. **Debug against release** and **native against web** (`checks.md`, 7.2): the determinism
    comparison of the sailing scene across build profiles and targets gives the same hash chain.

## 13. Performance

Reference figures are set in `budgets.md` (charter 3.10), and none is a pass condition. This
specification's overheads, measured or to be measured:

| Quantity | Value | How |
|---|---|---|
| Tick time of the sailing scene | budget `tick.sail` | `budgets.md`, 5.1: not measured in slice 0 as a whole; the physics spike's sailing scene (forces and Rapier, one boat and six crates) took 26.7 to 27.4 µs a tick natively |
| Empty tick (all phases, no entities) | 0.23 µs; 0.30 µs with an empty system in every row of 4.4 | slice 1 bench `sim.empty_tick`, below |
| 1,000 / 10,000 fragmented entities: `order::by_id_mut` against storage order | 13.4 / 157 µs against 0.67 / 6.6 µs | slice 1 bench `sim.sorted_iter` |
| 10,000 entity streams drawn once each in one tick | 0.27 ms with rng.md 12's table; in an earlier, busier run 0.54 ms against 2.1 ms with a `BTreeMap` | slice 1 bench `rng.entity_streams` |
| 1,000 entities, storage-order loop (three f64 adds and a hash per entity) | 2.8 to 3.2 µs | spec-sim probe, below |
| 1,000 entities, `sort_unstable::<&EntityId>()` loop | 16 to 20 µs | spec-sim probe |
| 10,000 entities: storage order / `sort_unstable` / `sort` | 28 to 31 / 204 to 210 / 336 to 358 µs | spec-sim probe |
| 10,000 entities: sorting the (id, entity) list alone / lookups in a cached order alone | 119 / 109 µs | spec-sim probe |
| 100,000 entities: storage order / `sort_unstable` | 320 to 327 / 4,420 to 5,089 µs | spec-sim probe |

The probe (2026-10-03): `bevy_ecs` 0.19.1, Rust 1.98.1, release, Windows 11 on the reference laptop
(`budgets.md`), while other agents' builds shared the machine; two runs each, giving the ranges. The
world held the given number of entities with `EntityId` and three-double position and velocity, half
with a marker component, fragmented by despawning a tenth, spawning as many, and toggling the marker
on a tenth. The probe's source is scratch code kept outside the repository. Slice 1's bench is
`cargo run -p pocket-sim --example sim_bench --release` (the same laptop and sharing, the median of
five runs, two runs on 2026-10-03 agreeing within 2%).

At the showcase's scale the sorted order costs microseconds per system. At 10,000 entities one
sorted system costs about 0.2 ms, so crowd-scale systems either qualify as order-independent (8.2)
or are measured against the budget `tick.sail` (`budgets.md`). If a slice needs sorted iteration
over tens of thousands of entities every tick, the next step is a sorted index maintained per query
(the cached-order row shows the lookups alone cost about half the sorted loop), measured before it
is built.

## 14. Open choices

1. **Tick numbering.** Recommendation: as specified, the clock reads n during tick n and `time()` is
   the end of the step (Bevy's convention). The alternative (the clock reads n-1 during tick n,
   Unity's `fixedTime`) makes "the time of the state being read" simpler and "the tick events carry"
   harder. The shared contract agrees: an observation at tick T describes the world at the end of T
   (shared/contract/perception.md, The perception update), and an event carries the tick it was
   emitted in (5.1).
2. **Event delivery.** Recommendation: double-buffered as specified (5.2), simple, cursor-free and
   persisted, as `script-host.md` (5.5) and the contract's perception update read it. The
   alternative delivers a tick's events to handler systems in a dispatch phase of the same tick,
   saving a tick of latency per reaction at the price of a cascade limit, a handler kind and a
   second ordering rule.
3. **Boundary writes and randomness.** Recommendation: writes never draw (4.2). The alternative
   gives each write a stream keyed by its place in the boundary, which puts the boundary's write
   counter into the world state.
4. **Sorted iteration mechanism.** Recommendation: `bevy_ecs`'s `sort_unstable::<&EntityId>()` with
   the order-independence exemption, as measured. A wrapper system parameter that offers only sorted
   iteration would turn the lint's `IntoIterator` gap into a compile error; it is worth building if
   the shuffle check catches a real violation.
5. **Tick rate range.** Recommendation: integers 1 to 1000, default 60. A rate whose `dt` is exact
   in binary (64 Hz) is unnecessary, since time is derived from the tick and never accumulated.
6. **A second script phase.** Recommendation: none until a project needs one (4.3).
7. to 16. **Slice 1 decisions**, recorded in [simulation-slice1.md](simulation-slice1.md) under
   these numbers: 7, the schedule is a list `Sim` runs, not a `bevy_ecs` `Schedule`, and a system
   its parameters skip did not run; 8, system keys place systems; 9, the tick's bookkeeping is
   Derived resources; 10, "no staged structural change" is checked through the index; 11,
   invocations and boundary writes as transactions, and reserved ids; 12, sorted iteration helpers;
   13, `EntityId::from_f64` has no `next`; 14, the component registry and its checks; 15, decoding
   checks what constructors check; 16, the physics rows as built. Section 13 has their measurements.

## 15. References

- Charter: `docs/charter.md` 3.2, 3.3, 3.5, 3.7, 3.10, 4.1, 4.2.6, 5.1, 7.1, 7.4.
- Master: `docs/design/networking.md` (Determinism; Staying one game), `docs/design/world-model.md`
  (Determinism and hashing; Events with causes; Physics as events: "bodies are processed in entity
  id order"), `docs/design/scenarios.md` (runs deterministic per seed), `docs/design/water.md` (time
  from the tick).
- Amoris predecessor charter (`origin/rebuild:docs/charter.md`, 8.2): reloads that left handlers and
  random seeds behind, and loads that changed entity ids and lost RNG state and contact caches, are
  the failures the boundary invariants and the allocator answer.
- Glenn Fiedler, "Fix Your Timestep!" (2004).
- Bevy: `bevy_ecs` 0.19.1 `QueryIter::sort_unstable`, `#[component(immutable)]`,
  `ScheduleBuildSettings::ambiguity_detection`, `SingleThreadedExecutor`, double-buffered events;
  `Time<Fixed>` advanced before the fixed schedule. Avian: the physics step in `FixedPostUpdate`.
- Unity: `FixedUpdate` before the physics step.
- Factorio, `LuaEntity::unit_number`: allocated sequentially, not reused.
- rustc's internal lint `potential_query_instability` (iteration over hash maps).
