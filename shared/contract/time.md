# Time

- Status: Draft, slice 0. Draft, proposed for PocketEngine's `shared/contract` (README,
  Synchronization with PocketEngine).
- Charter: 2.4.1, 3.3, 3.5, 3.7, 5.1, 7 (item 10).
- Contract version: 0.1 (draft).

## What this fixes

The engine supports turn-based play, lockstep, pausable real time with a thinking-time budget per
agent, and single stepping; the LLM is never inside the tick, and the simulation pauses when a
decision is needed instead of waiting for inference (charter 3.5). This file says how each works,
what a decision point is, and the requests that move time (`step`, `commit`, `wait`, `continue`,
`pause`, `resume`).

The design rests on one split. A game's **turn structure** (continuous, or turns) is part of its
rules and changes what the world computes; it is declared by the game and never changed at run time.
The **pacing** (stepped, lockstep, real time) decides only when ticks run and therefore at which
boundary an action lands; it is chosen per session and can change at any boundary without changing
what the world computes. The charter's four modes are turn-based (a structure) and lockstep,
pausable real time and single stepping (pacings).

From master: the runtime ran paused with a control server and `step {ticks, until, watch}` (master
`docs/mcp.md`, Tools); lockstep networking ran a tick only when every player's input for it had
arrived, with an input delay and a periodic hash check (master `docs/design/networking.md`, A tick,
Staying one game); scenarios could not pause or step themselves (master `docs/design/scenarios.md`,
Limits); there was no turn structure and no pause-on-decision. Precedents: PettingZoo's
agent-environment cycle (turns) and parallel API (simultaneous moves); deterministic lockstep in RTS
games (Age of Empires' "1500 archers on a 28.8"); "WEGO" simultaneous turns (Frozen Synapse); real
time with pause and auto-pause conditions in computer RPGs (Baldur's Gate); chess clocks with a
Fischer increment for the thinking budget; the fixed-timestep accumulator ("Fix Your Timestep!", G.
Fiedler) for real-time pacing.

## The world function

The invariant every pacing keeps and every check tests:

> The world at the end of tick T is a function of the initial world, the seed, and the actions
> applied at each tick boundary up to the one before T, in their applied order.

Ticks are spec-sim's (`Tick`, the fixed timestep `1 / rate` of `TickRate`, the phases and the system
order; `docs/spec/simulation.md`). A **tick boundary** is the moment between two ticks: the world is
consistent, no system runs, and the game thread applies the actions, editor commands and hot updates
queued for that boundary (charter 5.1; the queue and its ordering are spec-arch's). The boundary
before tick T is where actions that act from T are applied (actions.md, When an action takes
effect).

Pacing, pauses, thinking time, decision points and `until` conditions decide only *which boundary*
an action lands on and *when the next tick runs*; none of them is an input to the world function.
The replay records the applied actions per boundary (actions.md, What a replay records), so any run,
however it was paced, replays tick for tick in stepped pacing.

## Turn structure

Declared by the game (`GameDefinition.structure`, README):

```rust
#[serde(tag = "structure")]
pub enum TurnStructure {
    /// Time runs on; there are no turns (the sailing game).
    Continuous,
    /// Players decide in turns; the world runs only to resolve them.
    Turns { order: TurnOrder, resolve: Resolve },
}

#[serde(tag = "order")]
pub enum TurnOrder {
    /// One seat decides at a time, in this order; its turn resolves before the next seat's.
    RoundRobin { seats: Vec<SeatId> },
    /// All these seats decide, in any order; the turn resolves when all have ended it (WEGO).
    Simultaneous { seats: Vec<SeatId> },
}

#[serde(tag = "resolve")]
pub enum Resolve {
    /// Resolution runs exactly this many ticks.
    Ticks { ticks: u32 },
    /// Resolution runs until the game's registered `quiet` predicate holds, at most max_ticks.
    UntilQuiet { max_ticks: u32 },
}
```

The turn state is world state, because the game's rules may read it and because its progress follows
from actions:

```rust
#[derive(Resource)]
pub struct TurnState {
    pub turn: u64,                         // from 1
    pub phase: TurnPhase,
    pub to_move: BTreeSet<SeatId>,         // seats that have not ended this turn
}

#[serde(tag = "phase")]                 // the wire form; persisted externally tagged (README, World state)
pub enum TurnPhase {
    Deciding,
    Resolving { started: Tick, end_by: Tick },
}
```

- In the **deciding** phase no tick runs, whatever the pacing; seats in `to_move` act (their actions
  apply at the boundary at once, actions.md) and end their turn with the `end_turn` action. A seat
  not in `to_move` that acts or ends the turn is refused with `time.not_your_turn`.
- When `to_move` becomes empty (the round-robin seat ended its turn, or the last simultaneous seat
  did), applying that `end_turn` sets the phase to **resolving** (`started` the next tick, `end_by`
  its last allowed tick): ticks run under the session's pacing until `Resolve` is satisfied. The
  system `interface.turns`, in spec-sim's `Finish` phase, ends the resolving phase in the tick that
  satisfies it and begins the next turn's deciding phase (`turn + 1`, `to_move` refilled: the next
  round-robin seat, or all simultaneous seats), so every change of the turn state happens inside a
  tick or through an action. Each start of a deciding phase is a decision point for the seats in
  `to_move` (reason `turn`).
- `end_turn` is an action, recorded in the replay like any other, so a turn-based game replays
  exactly.

A continuous game has no `TurnState`, and `end_turn` there is refused with `time.wrong_mode`.

## Pacing

Chosen per session (spec-mcp creates sessions), from the pacings the game lists
(`GameDefinition.pacing`, default first), and changeable at any boundary by a developer with
`time.set_pacing {pacing}`:

```rust
#[serde(tag = "pacing")]
pub enum Pacing {
    /// Ticks run only when the clock holder asks (`step`).
    Stepped,
    /// Tick T runs when every listed seat has committed through T.
    Lockstep { seats: Vec<SeatId>, delay_ticks: u32 },
    /// Ticks follow the wall clock at `speed` times real time, with pauses.
    RealTime { speed: f64, pause_on_decision: bool, clock: Option<ThinkingClock> },
}

/// A chess clock with a Fischer increment, per agent seat, in wall-clock seconds.
pub struct ThinkingClock {
    pub initial_s: f64,      // time available at the start
    pub increment_s: f64,    // added after each answered decision
    pub max_s: f64,          // the clock never holds more than this
}
```

### Stepped (single stepping)

The world runs only on `step` from the session's **clock holder**: the developer, or in a
single-player session the player (spec-mcp says who holds it). Anyone else's `step` is refused with
`time.not_clock_holder`. Between steps the world is paused at a boundary, so actions apply at once
and every observation sees them. This is the pacing of tests, scenarios, the benchmark and agents
that think between moves.

### Lockstep

For several seats playing at once, each driven by its own agent or peer. Every seat in `seats` has a
commitment, the last tick it has declared itself ready through; tick T runs when every seat's
commitment is at least T (master's lockstep ran a tick when every player's input for it had arrived;
master `docs/design/networking.md`, A tick).

- A seat acts with `act` for its next uncommitted tick (actions.md, When an action takes effect) and
  then commits with `commit {ticks}`: "I have nothing more until then".
- With `delay_ticks` > 0 (networked peers, to cover the round trip), actions submitted for a seat's
  next tick apply `delay_ticks` later; locally it is 0.
- Within a boundary, actions are applied by seat order, then by each seat's submission order, so
  every peer applies the same sequence.
- A seat that commits with `until: "decision"` ends its commitment at the tick its decision point
  arises, so the world stops there for everyone: pause-on-decision in lockstep.
- **Every Write comes through the lockstep input.** A write that lands on one peer only, at a
  boundary that depends on its wall clock, is how lockstep games desynchronize (master
  `docs/design/networking.md`, Staying one game). So in lockstep pacing the runtime refuses, with
  `time.wrong_mode`, every Write that does not arrive as a lockstep input: an editor's or a
  developer's edit, and the engine's own Host writes too, a hot update's script swap and a finished
  import among them. A swap or an import is broadcast as a lockstep input for a tick T (`at: T`) and
  applied by every peer at the boundary before T (`docs/spec/threads.md`, 5.2).
- **Peers compare hashes.** Every 60 ticks each peer sends the others its `TickHash` of every tick
  since the last exchange (spec-persist's per-tick world hash), as master's lockstep checked a hash
  periodically (master `docs/design/networking.md`, Staying one game). A difference stops the match
  at the next boundary with `time.desync`, whose detail is the first diverging tick that
  spec-persist's `first_divergence` names; nothing is silently played on.

Stepped pacing is lockstep with one committing party (the clock holder), and `step` is `commit`
followed by a wait for the world to get there.

### Real time (pausable)

Ticks follow the wall clock at `speed` times real time, with the fixed-timestep accumulator: wall
time accumulates, a tick runs for every `1 / (rate * speed)` seconds accumulated, and when the game
thread falls behind by more than spec-arch's catch-up limit the excess is dropped (the world slows
down rather than spiralling). A **pause** stops ticks and stops wall time accumulating.

- **Pauses** come from a developer's `pause`, from a seat the session allows to pause
  (`time.cannot_pause` otherwise), and from decision points with `pause_on_decision`. The world
  resumes when every source of pause has ended.
- **Pause-on-decision**: when, after a tick, a decision point arises for an agent seat, and that
  seat's clock has time left (or there is no clock), the world pauses before the next tick. The
  seat's clock runs (wall time) while it has a decision pending. The seat may observe and act as it
  likes meanwhile; its actions apply at the paused boundary at once. It answers the decision with
  `continue` (or `act {resume: true}`), which stops its clock and adds `increment_s`, capped at
  `max_s`. When the seat's clock runs out before it answers, the decision is answered for it: the
  seat gets the warning `time.clock_out` with its next answer (the time controller is outside the
  world, so this is never a world event), and the world resumes; the seat can still act, but now in
  real time.
- Several agent seats: the world stays paused while any of them has a pending decision with time
  left on its clock; each seat's clock runs only for its own pending decisions.
- A human seat gets no pause-on-decision; its player plays in real time, as with any game that has a
  tactical pause.
- **Hidden pages.** A browser page reports whether it is hidden with the Control
  `time.visibility {hidden: bool}` from its local player's source (`docs/spec/threads.md`, 7.5).
  Real time pauses while every human seat's page is hidden, as a pause source of its own, and
  resumes when one is shown; a game with no human seat ignores it. Visibility is session state and
  is not recorded, like every pause.

The thinking clocks are session state on the game thread, never world state; they decide when the
world resumes, which decides at which boundary later actions land, which the replay records.

## Decision points

A decision point says: this seat should decide now. It is the unit the player benchmark counts
(charter 9.2: decision calls) and the trigger of pause-on-decision.

```rust
pub struct DecisionPoint {
    pub id: String,                          // "<tick>.<seat>", unique in a session
    pub tick: Tick,                          // the tick after which it arose
    pub seat: SeatId,
    pub reasons: Vec<DecisionReason>,        // every reason that arose in that tick, in order
    pub clock_s: Option<f64>,                // thinking time left (real-time pacing with a clock)
}

#[serde(tag = "reason")]
pub enum DecisionReason {
    Start,                                   // the first boundary of an episode
    Event { seq: u64, kind: String },        // a perceived event of a kind the filter watches
    Idle { idle_s: f64 },                    // nothing driving the seat for idle_s
    Interval { every_s: f64 },               // a heartbeat since the seat's last decision point
    Turn { turn: u64 },                      // its turn to decide (turn structure)
    /// A system asked for one: spec-sim's `DecisionRequest` for the seat's body, from the tick's
    /// report (`docs/spec/simulation.md`, 6); `reason` is a name the game declares.
    Requested { reason: String, event: Option<u64> },   // event: its perceived seq
}

/// A decision a game's rules may request, declared in `GameDefinition.decisions`.
pub struct DecisionDef {
    pub reason: String,                      // `[a-z][a-z0-9_]*`
    pub doc: String,
    /// The rule decides from the seat's own instruments only, so the request reveals nothing the
    /// seat cannot perceive (a "sail luffing" alarm), and is delivered without a perceived event.
    pub from_instruments: bool,
}

/// Per seat, set by the session (spec-mcp) from the game's default for the seat.
pub struct DecisionFilter {
    /// Perceived event kinds, or prefixes ending in ".", that raise a decision point.
    pub events: Vec<String>,
    pub idle_s: Option<f64>,
    pub every_s: Option<f64>,
}
```

After every tick, once `interface.perception` has run, the time controller computes the decision
points of every attached seat:

- `event`: each event pushed to the seat's ring in this tick whose kind matches the filter. The
  intent lifecycle (`intent.succeeded`, `intent.failed`, `intent.reached`; actions.md) and sightings
  (`sighted`; perception.md) are events, so "my intent finished" and "a new mark came into view" are
  decision points through the same rule as the game's own events.
- `idle`: the seat has no active or holding intent, and no action of the seat has been applied, for
  `idle_s`; raised once, then not again until the seat acts.
- `interval`: `every_s` since the seat's last decision point.
- `turn` and `start` as defined above.
- `requested`: every `DecisionRequest` in the tick's report whose observer is the seat's body and
  whose reason the game declares (`definition.invalid` at load for a rule that requests an
  undeclared one; an undeclared reason at run time is an internal error and is dropped). A rule has
  the whole world in view, so its timing could reveal what the seat cannot perceive (a rule that
  notices an unseen boat closing in). A request reaches a player seat only when its `event` is in
  the seat's ring (the seat perceived it) or its declaration is `from_instruments`; otherwise it is
  dropped for players and kept for developers. No filter suppresses a delivered one.

A decision point is **pending** until the seat answers it: with `continue`, with
`act {resume: true}`, or with its next `step` or `commit` (moving time on answers it). Observing
does not. A `step` or `commit` without `until: "decision"` passes over the decision points that
arise while it runs without answering them: each is counted in the answer's `passed_decisions`, and
the last one stays pending. Decision points and their filters are session state, not world state:
they decide where a run stops or pauses, never what the world computes, so two agents with different
filters still produce worlds that replay identically from their recorded actions. The number of
decision points that arise on main is a function of the world and the filter alone, which the
benchmark counts apart from how many an agent answered (shared/benchmark/README.md 4).

## Requests

The MCP tools that carry these are spec-mcp's; these are their payloads. Every answer carries the
events delta (perception.md, Push) and may carry an observation. `step`, `commit`, `wait`,
`continue`, `pause`, `resume` and `time.visibility` are Controls (`docs/spec/threads.md`, 2): they
decide when ticks run, not what they compute, and are never recorded; a lockstep commitment is
session state.

```rust
pub struct StepRequest {
    pub ticks: u32,                          // 1 to 36000 (ten minutes at 60 Hz)
    pub until: Option<Until>,
    /// Stop early, at a boundary, after this much wall time (default: the session's, mcp.md 3.1,
    /// else 30000; at most 600000).
    pub max_wall_ms: Option<u32>,
    pub observe: Option<ObserveRequest>,     // an observation at the stop, in the same answer
    pub budget_tokens: Option<u32>,          // for the events delta
    /// Developer and checker roles only: evaluate `until` against the whole world (marked).
    pub omniscient: Option<bool>,
}

pub struct CommitRequest {
    pub seat: Option<SeatId>,
    pub ticks: u32,                          // 1 to 36000 beyond the seat's commitment
    pub until: Option<Until>,                // ends the commitment early
    pub max_wall_ms: Option<u32>,
    pub observe: Option<ObserveRequest>,
    pub budget_tokens: Option<u32>,
}

pub struct WaitRequest {
    pub seat: Option<SeatId>,
    pub until: Option<Until>,                // default "decision"
    pub max_wall_ms: Option<u32>,            // default 30000, at most 600000
    pub observe: Option<ObserveRequest>,
    pub budget_tokens: Option<u32>,
}

pub struct ContinueRequest {
    pub seat: Option<SeatId>,
    pub budget_tokens: Option<u32>,
}
```

- **`step`** (stepped pacing, clock holder): runs up to `ticks` ticks, checking after each tick, in
  this order: the episode's end, a script halt, the `until` condition, and the wall limit. It
  answers when one holds or the ticks are done. In other pacings it is refused with
  `time.wrong_mode`, whose detail names the requests that pacing takes. While one `step` runs, a
  `step` from another clock holder of the same world is refused with `time.busy`, whose detail names
  the tick the running one has reached; it never queues behind it.
- **`commit`** (lockstep): extends the seat's commitment and answers when the world has run through
  it, when `until` ended it, or at `max_wall_ms` with `stopped: "waiting"` and the seats the world
  is waiting for. The commitment stands after a `waiting` answer; the seat then calls `wait`.
- **`wait`** (real time and lockstep): answers when `until` holds (by default, when the seat has a
  decision point), at the episode's end, or at `max_wall_ms`. It is the long poll by which an agent
  in real time learns of its decisions (perception.md, Push). In stepped pacing it is refused with
  `time.wrong_mode`.
- **`continue`**: answers the seat's pending decision. Without one it answers with the warning
  `time.no_decision` and changes nothing.
- **`pause`** and **`resume`** (real time; a developer's `resume` also ends a halt in any pacing,
  Halts below): a developer, or a seat allowed to; others get `time.cannot_pause`.
- **`time.status`**: a `TimeStatus`, filtered for the caller:

```rust
pub struct TimeStatus {
    pub tick: Tick, pub t_s: f64,
    pub structure: TurnStructure, pub turn: Option<TurnState>,
    pub pacing: Pacing,
    pub paused: bool,
    pub paused_for: Vec<SeatId>,             // seats whose pending decisions hold the world
    pub paused_by: Vec<PauseSource>,         // developer | seat | decision | hidden_page
    pub halted: bool,                        // the problem itself is in time answers (Halts)
    pub decisions: Vec<DecisionPoint>,       // pending ones
    pub clocks: Vec<SeatClock>,              // {seat, clock_s, running}
    pub commitments: Vec<SeatCommitment>,    // lockstep: {seat, through_tick}
    pub waiting_for: Vec<SeatId>,            // lockstep: seats not yet committed
    pub episode: Option<EpisodeOutcome>,     // once it ended
    pub behind_ms: Option<f64>,              // real time: how far the game is behind
}
pub enum PauseSource { Developer, Seat, Decision, HiddenPage }
```

A developer or checker sees every field for every seat. A player sees its own decision point and its
own clock only; other seats appear only by name, in `paused_for` and `waiting_for`, and
`commitments` holds its own. So two seats racing each other cannot learn from `time.status` that a
rival has sighted something or is deliberating (`contract.perception.noninterference` covers it).

### `until`

```rust
pub enum Until {
    /// "decision": the caller's seat has a decision point.
    Decision,
    /// {"intent": 3}: the intent has finished or started holding.
    Intent(IntentId),
    /// {"event": "crate.taken"}: a perceived event of this kind, or of a prefix ending in ".".
    Event(String),
    /// {"fact": {...}}: a perceived fact or instrument crosses a line.
    Fact(FactCondition),
    /// {"any": [...]}: the first of several.
    Any(Vec<Until>),
}

pub struct FactCondition {
    /// None: one of the seat's instruments; Some: a fact of that entity.
    pub entity: Option<EntityRef>,
    pub name: String,
    pub op: CondOp,
    pub value: Option<FactValue>,            // required except with "changes"
}

pub enum CondOp { Equals, NotEquals, Above, Below, AtLeast, AtMost, Changes }
```

`until` is evaluated after each tick through the caller's perception, at the precision the fact
declares (projection.md, Rounding), so a player can wait only on what it can know: a condition on a
hidden fact is refused at the request with `perception.not_perceivable`, one on an entity the seat
does not know with `perception.unknown_entity`, and an entity that drops out of the seat's knowledge
during the run makes the condition false, not an error. A developer may set `omniscient: true` in
`step` to evaluate `until` against the whole world; the answer is then marked `omniscient: true`,
and a player's `omniscient` is refused with `perception.omniscient_forbidden`.
`{"fact": {"name": "speed_mps", "op": "above", "value": 3}}` waits for the boat to make three metres
a second; `{"any": ["decision", {"event": "boat.aground"}]}` for whichever comes first. Master's
`step {until}` took an event prefix, an exposed value or a component field with the same comparisons
(master `docs/mcp.md`, Tools); the rebuild keeps the comparisons and evaluates them through
perception.

### The answer

```rust
pub struct TimeResult {
    pub from_tick: Tick,
    pub tick: Tick,                          // the world's tick now
    pub ran: u32,                            // ticks run by this call
    pub stopped: StopReason,
    pub decision: Option<DecisionPoint>,     // the caller's pending decision, if any
    pub passed_decisions: u32,               // decision points that arose and were passed over
    pub until: Option<UntilMet>,
    pub waiting_for: Vec<SeatId>,            // lockstep: seats not yet committed
    pub outcome: Option<EpisodeOutcome>,
    pub halted: Option<Problem>,             // the halt (Halts): the script error for a developer
    pub events: Vec<PerceivedEvent>,
    pub cursor: u64,
    pub observation: Option<serde_json::Value>,   // the observe answer, in its projection
    pub omniscient: bool,
    pub warnings: Vec<Problem>,
}

pub enum StopReason { Ticks, Until, Decision, Done, Halted, WallLimit, Waiting, Answered }

pub struct UntilMet {
    pub tick: Tick,
    pub condition: Until,                    // the condition (of an `any`, the one) that held
    pub value: Option<FactValue>,            // the fact's value then
    pub event: Option<PerceivedEvent>,       // the event that met it
}
```

`step`, `commit`, `wait` and `continue` all answer `TimeResult`; `continue` answers
`stopped: "answered"` with `ran: 0`. In the text projection the events delta is written as `event`
lines in the answer's text block (projection.md, Text projection).

Refusals of `until`: an `{"intent": n}` naming an id the caller's seat never had is
`intent.unknown_id`; a `FactCondition` whose `op` does not fit the fact's unit (`above` on an enum
or a bool) is `request.invalid_value` with the ops the unit takes, and one whose `value` is of
another type than the fact (a number for the enum `trim`) is `request.wrong_type`, each with the
`path` into the request.

**Halts.** When a script system fails (spec-script's `script.failed` event and structured error,
`docs/spec/script-sandbox.md`, 5.4), the time controller halts the world at the boundary after that
tick, in every pacing, until a developer sends `resume` (which ends a halt in any pacing) or a hot
update lands. Meanwhile every time request that would wait for ticks answers `stopped: "halted"`
with the halt's problem in `halted` instead of running. A developer or checker gets the script's
problem (its TypeScript file and line, `js_error`, entity ids); a player gets the generic problem
`time.halted` ("The game stopped on an internal error; only a developer can resume it."), since the
failure is the game's, not the player's, and its detail is script internals. Actions are still
applied during a halt (they land at the halted boundary and act when ticks run again). The failed
system's effects were discarded and the tick completed (`docs/spec/simulation.md`, 4.6), so a halt
decides only when ticks run, never what they compute; whether a run halts is run configuration,
recorded with its replay (`docs/spec/versions.md`, 3.7). Master found that a stopped game with the
error in its state let agents find script bugs, which is spec-script's reason for the policy.

## Episodes

A game declares when an episode ends with a registered predicate `done` and the facts it reports at
the end (`GameDefinition.episode`, README: its `doc` and the `result` readings as `FactDef`s); the
session (spec-mcp) MAY set a tick limit. `done` is a pure function of the world (Rust or script),
evaluated by `interface.turns` in the `Finish` phase of every tick (`docs/spec/simulation.md`, 4.4),
so whether and when an episode ends is part of the deterministic run; a game that keeps progress in
the world (a course's `finished_tick`) makes `done` read it.

```rust
pub struct EpisodeOutcome {
    pub terminated: bool,                    // the game's `done` held
    pub truncated: bool,                     // the session's tick limit was reached
    pub tick: Tick,
    pub result: Vec<Reading>,                // declared by the game: winner, score, crates taken
}
```

The tick after which `done` holds ends the episode (Gymnasium's terminated and truncated): `step`
and `commit` stop with `stopped: "done"`, real time pauses, and further time requests and `act` are
refused with `time.episode_over`, whose detail carries the outcome. Starting a new episode (reset, a
new seed) is a session operation (spec-mcp).

## Threads and the web

The time controller runs on the game thread, which alone runs ticks (charter 5.1). Time requests
reach it through the command queue (spec-arch) and are answered from it; a long `step` or `wait`
keeps the queue served between ticks, so another caller's `observe` or the editor's snapshot reads
are never blocked by it. In real time the editor thread only presents; a stalled editor never delays
a tick, and a paused world never freezes the editor. On the web the game thread is a Web Worker,
real time and the thinking clocks measure wall time with the worker's `performance.now()`, and these
requests cross as messages (spec-arch fixes the form).

At every boundary the game thread's loop asks the time controller how to go on, and the answer is
spec-arch's `Pace` (`docs/spec/threads.md`, 3.3):

| Pacing and state | `Pace` |
|---|---|
| Stepped, a `step` running with ticks left and no stop | `RunTick` |
| Stepped, otherwise | `WaitForCommand` |
| Lockstep, every seat committed through the next tick | `RunTick` |
| Lockstep, otherwise | `WaitForCommand` |
| Turns, deciding phase (any pacing) | `WaitForCommand` |
| Real time, paused (any source) | `WaitForCommand` |
| Real time, running | `WaitUntil` the next tick's due instant, or `RunTick` when it is due |
| The episode ended | `WaitForCommand` |
| Halted after a script failure (any pacing; Halts) | `WaitForCommand` |

The loop's catch-up limit (5 ticks, threads.md 3.3) is the one real-time pacing uses above.

## Sailing

The sailing game's structure, pacing, default decision filter and episode are in
[sailing.md](sailing.md), Time.

## Determinism

- The world function (above) holds in every pacing, because the world advances only through ticks
  and applied actions, and pacing decides neither the systems nor the actions, only their timing,
  which the replay records.
- The turn state is world state and moves only through `end_turn` actions and the deterministic
  resolution rule.
- Decision points, filters, commitments, pauses and thinking clocks are session state; `until` is
  evaluated through perception, which is read-only (perception.md). None of them is written into the
  world.
- Real time reads the wall clock only in the time controller, to decide when to run the next tick;
  no system sees it.

## Checks

- **`contract.time.pacing_equivalence`**: a sailing session in real time at speed 4 with
  pause-on-decision, an agent stand-in that answers each decision after a random wall delay (seeded)
  and starts random intents, runs 7200 ticks; its applied actions are recorded. The same actions
  replayed in stepped pacing give the same world hash at every tick. A lockstep session of two seats
  committing in random chunk sizes (1 to 300 ticks, seeded) and acting at random boundaries,
  replayed in stepped pacing, also matches at every tick.
- **Pause-on-decision**: when a decision point arises after tick T, no tick after T runs until the
  seat answers or its clock runs out; with a clock of 0.2 s and no answer, the world resumes within
  2.3 ms of the clock running out on the reference hardware (the game thread's 1 ms sleep slices:
  the threads spike measured a 1 ms `thread::sleep` at 1.54 ms median and 1.79 ms worst, times the
  1.25 headroom of `docs/spec/budgets.md`, 8.1); `continue` adds the increment up to `max_s`.
- **Turns**: in the deciding phase no tick runs over 1000 wall milliseconds of real-time pacing;
  `end_turn` by a seat not in `to_move` is `time.not_your_turn`; resolution runs exactly `ticks`
  ticks, or until `quiet` capped at `max_ticks`; a turn-based fixture replays exactly from its
  actions.
- **`until`**: each variant stops after the first tick it holds, not before and not later; a hidden
  fact is refused; `any` reports which condition held.
- **Wall limit**: a `step {ticks: 36000, max_wall_ms: 50}` on a scene slowed by a test system stops
  at a boundary with `stopped: "wall_limit"`, and the world is consistent at that tick (its hash
  equals that tick's hash in an uninterrupted run).
- **Decisions**: a `step` without `until` over three decision points answers `passed_decisions: 3`
  and leaves the last one pending; a requested decision raised by a rule that saw a hidden boat is
  not delivered to the player and is to a developer.
- **Lockstep writes**: an editor's write and a hot update sent outside the lockstep input are
  refused with `time.wrong_mode`; the same sent as lockstep inputs land on both peers at one
  boundary; a forced difference between the peers stops the match with `time.desync` naming its
  first tick.
- **Status**: a player's `time.status` in a two-seat race shows its own clock and decision only.

## Performance

Rows of spec-arch's framework (`docs/spec/budgets.md`, 5.6); no slice 0 spike built the time
controller, so none was measured.

| Measure | Budget |
|---|---|
| Per-tick overhead of the time controller in `step` (until, decision points, queue service), sailing scene | `step.overhead`: not measured |
| `step {ticks: 3600}` headless, sailing scene, release build, wall time | `step.3600`: not measured |
| Latency from `continue` to the next tick in real time | `resume.latency`: not measured (the threads spike's commands took effect 1.04 ms p50 after sending) |

## Open choices

1. **Turn structure as game rules, pacing as session choice.** Recommended (above). The alternative,
   one time-mode setting covering both, would let a session turn a continuous game into a turn-based
   one, which changes its rules and breaks replay equivalence.
2. **Pause-on-decision in a human and agent match.** Recommended: allowed, bounded by the agent's
   thinking clock and shown to the human (the HUD shows that the world is paused for the agent and
   its clock), as computer RPGs show a tactical pause. The alternative, no pauses and the agent
   playing in real time through intents, is fairer to the human's tempo but makes the agent's result
   depend on model latency; the slice 4 benchmark can run both and report the difference.
3. **Who holds the clock in stepped pacing.** Recommended: the single player in a single-player
   session, else the developer; spec-mcp's sessions decide. Several agents needing to step together
   use lockstep.
4. **Default decision filters.** The sailing defaults above are a starting point for the player
   benchmark, which measures decision calls and tokens per filter (charter 9.2).
