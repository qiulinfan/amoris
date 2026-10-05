# Actions

- Status: Draft, slice 0. Maintained in Amoris's `shared/contract` (README, Contract record).
- Charter: 2.4.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.10, 5.1, 7 (item 9).
- Contract version: 0.1 (draft).

## What this fixes

Actions are layered (charter 3.4): **controls** are the low-level inputs a seat's body takes (a
rudder angle, a sheet, a button), and **intents** are high-level goals the engine carries out over
many ticks by setting those controls ("come to heading 090", "sail to Mark1"), each with a defined
completion and defined failures. Entities declare the interactions they support as **affordances**,
which an agent can list, with the reason when one is not available now. Every call is validated
whole before anything of it is applied, and an illegal one is refused with a structured error
(errors.md).

Master's agents acted through simulated keys only: `input.hold {action, ticks}` and `input.press`
through the human input path (master `docs/design/input.md`, Agents), and the environment
interface's `env.step {actions}` held them for a step (master `docs/design/environment.md`,
Commands). A key-timing subtlety (a press of a key still down only held it longer) cost one agent 85
calls on a dialogue task until presses a tick apart were made two presses (master
`docs/agent-eval.md`, Conversations as text); Amoris's predecessor charter lists the same failure under
its lessons ("actions were low-level input only"). The rebuild gives agents intents with completion
and failure, and gives controls a value or pulse semantics with no key timing in it at all.

Precedents: PySC2's `available_actions` and function arguments, and TextWorld's admissible commands,
for affordances listed with what is possible now; ROS 2 actions (a goal accepted or rejected,
feedback while it runs, a result of succeeded, aborted or canceled) for the intent lifecycle;
behaviour trees' running, success and failure; Mineflayer's pathfinder goals, which Voyager built
its skills on, for intents an LLM can compose.

## Controls

A **control** is one input of a seat's body, declared by the game. A control is either a latched
value, which stays until it is set again, or a pulse, which acts for exactly one tick.

```rust
pub struct ControlDef {
    pub name: String,                  // "rudder"
    pub doc: String,                   // says the sign: "positive turns the bow to starboard"
    pub kind: ControlKind,
    /// Intents occupy channels; setting a control supersedes the intent holding its channel.
    pub channel: String,               // "helm"
}

#[serde(tag = "kind")]
pub enum ControlKind {
    /// Latched number in [min, max].
    Axis { min: f64, max: f64, default: f64 },
    /// Latched boolean.
    Toggle { default: bool },
    /// Latched choice among names.
    Choice { values: Vec<String>, default: String },
    /// Acts for one tick; may name a target entity of the given kinds.
    Pulse { target_kinds: Option<Vec<String>> },
}

#[serde(untagged)]                    // the wire form; persisted externally tagged (README, World state)
pub enum ControlValue { Bool(bool), Number(f64), Name(String) }
```

The body's controls are world state:

```rust
#[derive(Component)]
pub struct Controls {
    /// Latched values, every declared latched control present (its default until set).
    pub values: BTreeMap<String, ControlValue>,
    /// Pulses waiting, per control, oldest first. Each tick the front pulse of each control is
    /// the one that acts in that tick; it is removed at the end of that tick.
    pub pulses: BTreeMap<String, VecDeque<Pulse>>,
}

pub struct Pulse { pub target: Option<EntityId> }
```

Two pulses of the same control sent before one tick are two pulses: the first acts in the next tick
and the second in the tick after. Nothing merges or swallows a pulse, which is the property master's
presses lacked until it was fixed.

Systems read controls and nothing else from the player: the sailing simulation reads `rudder`,
`sheet`, `hoist`; a game rule reads the `interact` pulse. A human's keyboard, pad and touch reach
the same controls through the human projection's input map (slices 4 and 5; master's action map,
master `docs/design/input.md`, Map, is the model): a human at a seat and an agent at a seat set the
same controls and follow the same rules (charter 2.1).

## Intents

An **intent** is a named goal with parameters that the engine carries out tick by tick by setting
the seat's controls, until it succeeds, fails, is cancelled or is superseded.

```rust
pub struct IntentDef {
    pub name: String,                   // "come_to_heading": a verb phrase
    pub doc: String,                    // what it does, when it succeeds, how it can fail
    /// JSON Schema of `params` (from a Rust type through schemars, or a project declaration).
    pub params: schemars::Schema,
    pub target: TargetSpec,
    /// Channels it occupies for as long as it is active or holding.
    pub channels: Vec<String>,
    /// Whether `keep: true` is accepted: on reaching its goal it holds it instead of ending.
    pub can_hold: bool,
    pub default_timeout_s: f64,
    /// The failure codes it can end with (errors.md), documented in `describe`.
    pub failures: Vec<String>,
    /// The progress readings it reports, declared like facts (perception.md).
    pub progress: Vec<FactDef>,
}

#[serde(tag = "takes")]
pub enum TargetSpec {
    None,
    Entity { kinds: Vec<String> },
    Point,
    EntityOrPoint { kinds: Vec<String> },
}

#[serde(untagged)]
pub enum Target { Entity(EntityRef), Point(PointRef) }

/// A point in the contract frame; `y` may be left out where the intent works on the ground or the
/// sea surface, and is then documented as unused (README, Positions and entity references).
pub struct PointRef { pub x: f64, pub y: Option<f64>, pub z: f64 }
```

### The lifecycle

```text
           start accepted
                 |
                 v
              active --------------------> succeeded
              |    |   goal met, keep false
   goal met,  |    |
   keep true  v    +-----------------------> failed {code}       (timeout or the intent's failures)
            holding ---------------------->  failed {code}
              |    |
              +----+-----------------------> cancelled           (a cancel action)
                   +-----------------------> superseded {by}     (an intent or a control on its
                                                                   channel)
```

```rust
pub enum IntentStatus { Active, Holding, Succeeded, Failed, Cancelled, Superseded }
```

- **Active**: the executor runs every tick. A deadline of `started_tick + ceil(timeout_s * rate)`
  (spec-sim's `TickRate`) applies; reaching it while still active fails the intent with
  `intent.timeout`.
- **Holding**: the goal was reached with `keep: true` and the executor goes on keeping it (an
  autopilot holding a heading, a sail kept trimmed as the wind shifts). Holding has no deadline. It
  ends only by failure, cancel or supersession.
- **Succeeded, failed, cancelled, superseded** are final. A failed intent carries a
  `{code, message, detail}` problem (errors.md); a superseded one names what superseded it (an
  intent id, or `null` for a control set directly).

Each transition emits a world event whose subject is the seat's body and whose kind is declared with
`Scope::Private` (perception.md, Events and their scope), so only that seat perceives it:
`intent.started`, `intent.reached` (active to holding), `intent.succeeded`, `intent.failed`,
`intent.cancelled`, `intent.superseded`. Their data are scalar facts, declared like any event's
(`EventDef.data`): `intent_id` (count), `intent` (text), `tag` (text, when the caller gave one),
`code` and `message` (text, on `intent.failed`: the failure's code and its rendered message), and
each of the intent's progress readings flattened under its own name. So the text event line shows
`code=sail.no_progress message="No progress toward the target: ..."` where the failure arrives, and
the whole problem, detail included, is in the instance (`intents`, below). They reach the seat as
perceived events, and they are decision points (time.md).

### Channels and supersession

A channel names a part of the body only one thing may drive at a time (`helm`, `sail`, `hands`). An
intent occupies its channels while active or holding.

- Starting an intent supersedes every active or holding intent of the same seat that shares a
  channel with it; the old intent's status becomes `superseded` with `by` the new id.
- Setting or pulsing a control directly supersedes the intent of the same seat that occupies the
  control's channel (`by: null`): a hand on the wheel disengages the autopilot.
- One call that would both start an intent and set a control on the same channel, or start two
  intents sharing a channel, or set the same control twice, is refused whole with `action.conflict`,
  since its meaning would depend on the order inside it.

So at any time each channel of a seat is driven by at most one intent, and executors never contend
for a control.

### Intent instances

Intents are world state: they decide controls, so the world after a fork or a restore MUST hold them
(charter 3.3).

```rust
/// Allocated from the intent table's counter, from 1; never reused.
pub struct IntentId(pub u64);

#[derive(Resource)]
pub struct IntentTable {
    pub next_id: u64,
    pub by_id: BTreeMap<IntentId, IntentInstance>,     // IntentId order
}

pub struct IntentInstance {
    pub id: IntentId,
    pub seat: SeatId,
    pub intent: String,
    pub tag: Option<String>,             // the caller's own label, echoed (at most 64 bytes)
    /// Canonical: aliases resolved, defaults filled, so the instance never depends on defaults
    /// that change later. An object in spec-persist's `PlainData` (keys in canonical order).
    pub params: PlainData,
    pub target: Option<ResolvedTarget>, // entity references resolved to EntityId
    pub started_tick: Tick,             // the first tick its executor runs
    pub deadline_tick: Option<Tick>,
    pub status: IntentStatus,
    pub finished_tick: Option<Tick>,
    /// Persisted as its code and detail only; the message is rendered when shown (README, World
    /// state and wire forms).
    pub failure: Option<Problem>,
    pub superseded_by: Option<IntentId>,
    pub progress: Vec<Reading>,         // rounded at each reading's precision (projection.md)
    /// The executor's own state between ticks, `PlainData`, hashed with the world.
    pub state: PlainData,
}

#[serde(untagged)]                    // the wire form; persisted externally tagged
pub enum ResolvedTarget { Entity(EntityId), Point(Vec3) }
```

The 32 most recently finished intents of each seat are kept for status queries; older finished ones
are removed at the end of the tick in which the 33rd finishes, oldest `finished_tick` first, then
lowest id. The counter is world state, so a fork allocates the same next id in both branches.

What callers see of an instance:

```rust
pub struct IntentView {
    pub id: IntentId,
    pub intent: String,
    pub status: IntentStatus,
    pub tag: Option<String>,
    pub params: serde_json::Map<String, serde_json::Value>,
    pub target: Option<TargetView>,      // {id, name} or a point
    pub started_tick: Tick,
    pub finished_tick: Option<Tick>,
    pub progress: Vec<Reading>,
    pub failure: Option<Problem>,
    pub superseded_by: Option<IntentId>,
}
```

The request `intents` answers a seat's intents as `IntentView`s, newest first, failed ones with
their whole problem; it is a player tool (mcp.md 5), because observations include only the active
and holding ones (perception.md) and a failed intent drops out of them:

```rust
pub struct IntentsRequest {
    pub seat: Option<SeatId>,             // player: omitted or its own; developer, checker: any
    pub ids: Option<Vec<IntentId>>,       // each a number or "#3"
    pub active_only: Option<bool>,        // default false
    pub budget_tokens: Option<u32>,
}
// Answer: {tick, omniscient, intents: [IntentView], omitted, tokens}
```

### Executors

An executor is the code that carries out one intent. It is engine Rust (the sailing intents) or a
script; the contract is the same. A script executor is declared as such (spec-script's
`executor(def)`, `docs/spec/script-host.md`, 5.5) with the intent kinds it carries out; it receives
every active or holding instance of them each tick in `ctx.intents`, in `IntentId` order, reports
progress, reaching, success and failure through commands of its transaction, and keeps its state in
the instance. Its context is restricted by the host to what the rules below allow: the actor's
`PerceptionView` and a `ControlWriter` for the intent's channels, and no world access, queries,
events or entity random streams (`script.restricted` otherwise). Its failures carry a code the game
declares with the `Fail` use (errors.md, Game codes), whose message is rendered from the code's
template. Scripts run only inside ticks, so a script intent's `accept` is its schema, its target
spec and its affordance requirements; a script's own refusal comes in the intent's first tick, as
`failed` with its problem, and reaches the sender as the `intent.failed` event and decision point.
`ctx.intent(actor, kind, params)`, an NPC's rule starting an intent, is an `act` with origin
`script`, applied inside the tick by the script host's transaction (`docs/spec/script-host.md`, 5.4)
rather than through the queue; it is an output of the tick and is never recorded (What a replay
records).

```rust
pub trait IntentExecutor: Send + Sync {
    fn def(&self) -> &IntentDef;
    /// At acceptance: check the parameters against what the seat perceives, and give the initial
    /// state and progress, or refuse with problems (nothing is applied then).
    fn accept(&self, cx: &AcceptCx) -> Result<Accepted, Vec<Problem>>;
    /// Every tick while active or holding, in IntentId order.
    fn tick(&self, cx: &mut TickCx) -> Step;
}

pub struct AcceptCx<'a> {
    pub params: &'a PlainData,                                    // canonical
    pub target: Option<&'a ResolvedTarget>,
    pub view: &'a PerceptionView<'a>,                             // the seat's observer
}

pub struct Accepted {
    pub state: PlainData,
    pub progress: Vec<Reading>,
    pub warnings: Vec<Problem>,
}

pub struct TickCx<'a> {
    pub instance: &'a IntentInstance,          // params, target, status, started_tick
    pub state: &'a mut PlainData,
    pub view: &'a PerceptionView<'a>,
    pub controls: ControlWriter<'a>,           // this seat's controls on this intent's channels
}

pub enum Step {
    Continue { progress: Vec<Reading> },
    Reached { progress: Vec<Reading> },        // the goal is met: succeed, or hold if keep
    Succeeded { progress: Vec<Reading> },      // the goal is met and the intent ends regardless
    Failed { problem: Problem, progress: Vec<Reading> },
}
```

Two rules make intents a convenience and never a cheat:

1. **An executor sees only what its seat perceives.** Its inputs are its seat's `PerceptionView`
   (instruments, percepts, perceived events; perception.md) and its own instance; it has no access
   to the world. An autopilot that steered round a reef the skipper cannot see would leak the reef
   through the boat's course.
2. **An executor acts only through its seat's controls on its own channels.** `ControlWriter`
   refuses any other control (a programming error: the engine reports `internal.error` and fails the
   intent). It cannot move a body, write a component or emit a game event.

So anything an intent achieves, the same seat could have achieved by setting the same controls at
the same ticks, which the transparency check proves (Checks, below).

The system `interface.intents`, in spec-sim's `Control` phase (`docs/spec/simulation.md`, 4.3 and
4.4), runs after the actions of the tick boundary were applied and before the rules, forces and
physics that read controls. It checks the deadline of every active intent, Rust and script intents
alike, in `IntentId` order (a deadline reached fails the intent before its executor runs in that
tick), then runs the Rust executors in `IntentId` order; a `Step` other than `Continue` changes the
status at once and emits the lifecycle event. Script executors run next, in the `Update` phase's
`script.update`, in the project's `systems` order, each over its instances in `IntentId` order; the
status changes and lifecycle events their commands make are applied when each one's transaction
commits. Control writes of both reach the forces and physics of the same tick.

## Affordances

An affordance is an interaction an entity's kind offers: "a crate can be taken aboard", "a door can
be opened". It names a verb, what it does (it starts an intent or sends a pulse with the entity as
target) and what it requires. Affordances are declared on kinds (perception.md,
`KindDef.affordances`), so they are part of what a player can know about an entity it perceives.

```rust
pub struct AffordanceDef {
    pub verb: String,                   // "take_aboard"
    pub doc: String,
    pub effect: Effect,
    pub requires: Vec<Requirement>,     // all must hold
}

#[serde(tag = "effect")]
pub enum Effect {
    /// Starts the intent with the entity as its target; `params` are fixed values merged under
    /// the caller's (a caller's value for the same key is refused with action.conflict).
    Intent { intent: String, params: serde_json::Map<String, serde_json::Value> },
    /// Sends one pulse of the control with the entity as its target.
    Pulse { control: String },
}

#[serde(tag = "requires")]
pub enum Requirement {
    /// The entity is seen now (not only remembered or charted).
    Seen,
    /// The entity is within this range of the actor's body.
    Within { max_m: f64 },
    /// The entity's bearing is within this angle of the actor's heading.
    Facing { max_off_deg: f64 },
    /// A fact of the entity (`of: "target"`) or an instrument of the actor (`of: "actor"`)
    /// compares as given.
    Fact { of: Side, fact: String, op: Op, value: FactValue },
    /// Only these seats may use it.
    Seats { seats: Vec<SeatId> },
}

pub enum Side { Target, Actor }

pub enum Op { Equals, NotEquals, Above, Below, AtLeast, AtMost }
```

Requirements are evaluated through the actor's `PerceptionView`, so they can only depend on what the
actor perceives; a `Fact` requirement on a `Hidden` or `Owner` fact of another entity is a
definition error found at load (`definition.invalid`). Each unmet requirement is a problem with its
own code: `action.not_seen`, `action.out_of_reach {entity, range_m, max_m}`,
`action.not_facing {entity, off_deg, max_off_deg}`,
`action.requirement_unmet {verb, of, fact, op, value, actual}`, `seat.not_allowed {seat, seats}`.

**Pulses that are an affordance's effect.** When a pulse control is the `effect` of an affordance on
the target's kind, a direct `pulse` of that control at such a target is validated against that
affordance's requirements as `use` would be, and refused with `action.unavailable` when one is
unmet, so a pulse that could do nothing never answers success. A game rule that still cannot act on
a pulse it reads (the state changed at the same boundary) emits a private `<control>.ignored` event
whose data carries a code the game declares and its message (sailing.md, Taking a crate aboard).

Listing them: `affordances {seat?, entity?, kinds?, within_m?, available_only?, budget_tokens?}`
answers, for each perceived entity matching (ranked as perception ranks percepts), each verb its
kind offers with `available` and, when not, `unmet` (every unmet requirement, not only the first):

```json
{"tick":2400,"omniscient":false,"affordances":[
  {"entity":{"id":21,"name":"Crate4"},"kind":"crate","verb":"take_aboard","available":true,
   "unmet":[]},
  {"entity":{"id":25,"name":"Crate7"},"kind":"crate","verb":"take_aboard","available":false,
   "unmet":[{"code":"action.out_of_reach",
             "message":"Crate7 is 14.2 m away; it must be within 3 m.",
             "detail":{"entity":{"id":25,"name":"Crate7"},"range_m":14.2,"max_m":3}}]}],
 "tokens":103}
```

(Compact and without its `tokens` member the answer is 410 bytes, counted with Python's
`json.dumps(..., separators=(",", ":"))`, so 103 tokens by the estimate.)

A percept's `can` (perception.md) lists only the verbs available now, which is cheap; `describe` and
`affordances` say why the others are not.

## The act request

Everything a player does goes through one request, carrying one or more actions, validated whole and
applied whole.

```rust
pub struct ActRequest {
    /// Player role: omitted or its own seat. Developer role: required.
    pub seat: Option<SeatId>,
    pub actions: Vec<Action>,           // 1 to 32
    /// Budget for the events delta in the answer (perception.md, Push).
    pub budget_tokens: Option<u32>,
    /// After applying, answer the seat's pending decision as `continue` does (time.md).
    pub resume: Option<bool>,
}

#[serde(tag = "do")]
pub enum Action {
    /// Latch control values.
    Set { controls: BTreeMap<String, ControlValue> },
    /// One pulse of a pulse control, with a target if it takes one.
    Pulse { control: String, target: Option<EntityRef> },
    /// Start an intent.
    Start {
        intent: String,
        #[serde(default)] params: serde_json::Map<String, serde_json::Value>,
        target: Option<Target>,
        tag: Option<String>,
    },
    /// Use an affordance of an entity: starts its intent or sends its pulse.
    Use {
        entity: EntityRef,
        verb: String,
        #[serde(default)] params: serde_json::Map<String, serde_json::Value>,
        tag: Option<String>,
    },
    /// Cancel an intent; cancelling a finished one answers its final status (idempotent).
    /// `intent_id` is a number or the text projection's "#3" (README, Positions and entity
    /// references).
    Cancel { intent_id: IntentId },
    /// End this seat's turn (turn-based games, time.md).
    EndTurn,
}
```

On the wire:

```json
{"seat": "skipper", "actions": [
  {"do": "start", "intent": "trim_sail", "params": {"hoist": 1, "sheet": "best", "keep": true}},
  {"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 45}, "tag": "beat-1"}
]}
```

An intent's parameters go under `params`. A parameter written beside it
(`{"do": "start", "intent": "come_to_heading", "heading_deg": 45}`) is refused with
`request.misplaced_field`, whose detail names where it belongs (`/actions/0/params/heading_deg`); a
call with `do` at the top level and no `actions` is refused the same way, naming `/actions/0`.

### Validation and atomicity

Nothing of a call is applied unless all of it is valid (charter 3.4). Validation runs in three
phases on the game thread, against the world the actions will apply to, and stops after the first
phase that finds a problem, reporting every problem that phase found (errors.md, Several problems):

1. **Shape**: the request against its JSON Schema: unknown fields, misplaced fields, missing fields,
   wrong types, ranges, the number of actions. Intent parameters are checked against the intent's
   own schema.
2. **Names**: the seat and the caller's right to it; each control, intent and verb exists and the
   seat may use it (`action.unknown_control`, `action.unknown_intent`, `action.unknown_verb`,
   `seat.*`, with suggestions); each entity reference resolves among what the seat knows
   (`perception.unknown_entity`, `request.ambiguous_ref`); each control value fits its control; each
   target fits its intent's `TargetSpec`.
3. **Semantics**: conflicts inside the call (`action.conflict`); affordance requirements, for `use`
   and for a direct pulse of a control that is an affordance's effect (`action.unavailable` with
   every unmet requirement); `cancel` of an id that never existed for this seat
   (`intent.unknown_id`); each intent's `accept`, in action order, as if the earlier actions of the
   call had been applied.

Only when all three pass is the call applied, in action order. A refused call leaves no trace in the
world: no control changed, no pulse queued, no intent id allocated, no event emitted. The engine
reports the refusal to the caller and records nothing in the replay.

### When an action takes effect

An accepted call is applied at a **tick boundary**, the moment between two ticks (time.md). Actions
always act from the next tick that runs.

- While the world is paused at a boundary (between steps in stepped pacing, during a pause or a
  decision, in a turn's deciding phase), that boundary is now: the call is applied before it is
  answered, and observations, intent lists and forks from then on include it. The answer's
  `applied_at` is the next tick to run.
- In real-time pacing the call goes through the game thread's queue (spec-arch) and is validated and
  applied as soon as it arrives, at the boundary the world is at while it waits for its next tick
  (`docs/spec/threads.md`, 3.2; the threads spike measured 1.0 to 1.6 ms from sending to the effect
  at 60 Hz), or after the tick in progress; the answer follows the application.
- In lockstep pacing a seat's actions apply at the boundary before that seat's next uncommitted
  tick, which may be later than the world's next tick. Such a call is validated when it is
  submitted, answered with `pending: true` and its `applied_at`, and validated again when that
  boundary comes; if it fails then (the world changed meanwhile), it is dropped, and the session
  hands the seat the warning `action.dropped` with its next answer (session state: nothing is
  written into the world for an action that never applied). Intent ids are allocated when an action
  is applied, so a pending start answers its `tag` and the id arrives with `intent.started`.
- Within one boundary, the game thread's queue sorts what it applies: the host's own writes, then
  the editor's, then developer sessions', then the players' in seat order, each source's calls in
  its submission order (`docs/spec/threads.md`, 5.2). The order therefore depends on which calls
  reached the boundary, never on how threads interleaved, and every lockstep peer applies the same
  sequence.

### The answer

```rust
pub struct ActResult {
    pub tick: Tick,                      // the world's tick when answered
    pub applied_at: Tick,                // the first tick the actions act in
    pub pending: bool,                   // lockstep only: not applied yet
    pub outcomes: Vec<Outcome>,          // one per action, in order
    pub warnings: Vec<Problem>,          // errors.md, Warnings
    pub events: Vec<PerceivedEvent>,     // the events delta (perception.md, Push)
    pub cursor: u64,
}

#[serde(tag = "did")]
pub enum Outcome {
    Set { controls: BTreeMap<String, ControlValue>, superseded: Vec<IntentId> },
    Pulse { control: String, target: Option<EntityName>, superseded: Vec<IntentId> },
    Start { intent_id: Option<IntentId>, intent: String, tag: Option<String>,
            params: serde_json::Map<String, serde_json::Value>,   // canonical
            superseded: Vec<IntentId>, progress: Vec<Reading> },
    Use { entity: EntityName, verb: String, then: Box<Outcome> },
    Cancel { intent_id: IntentId, status: IntentStatus },
    EndTurn { turn: u64 },
}
```

`Start` answers the canonical parameters, defaults filled in, so an agent sees what was actually
asked of the engine without a second call (master's `world.set` answered the component as it then
was for the same reason: master `docs/mcp.md`, Asking the engine how to call it).

## What a replay records

The world at tick T is a function of the initial state, the seed and the actions applied at each
boundary before T (time.md, The world function). Every applied call reaches spec-persist's recorder
as a spec-arch `Applied` write (`docs/spec/threads.md`, 5.3: the tick it is an input of, its index
at that boundary, its command source, name and params); for an `act`, the params are the canonical
call, whose content per action is:

```rust
pub struct AppliedAction {
    pub tick: Tick,              // the tick it is an input of: applied at the boundary before it
    pub seat: SeatId,
    pub origin: ActOrigin,       // who acted; never `Script` in a recording (below)
    pub action: Action,          // canonical: params with defaults filled and aliases resolved,
                                 // entity references as EntityId, targets resolved
}

pub enum ActOrigin { Agent, Human, Editor, Script, Peer }
```

Refused calls are not recorded; they changed nothing. Acts a script makes inside a tick
(`ctx.intent`, origin `Script`) are outputs of that tick, which replaying the tick reproduces, so
they are never recorded; a recorded `AppliedAction` carries only `Agent`, `Human`, `Editor` or
`Peer`.

A replay applies each recorded action through the live application path: the world-level checks of
validation (names, entity ids, values, conflicts, affordance requirements) and each intent's
`accept`, which recomputes the instance's initial `state` and `progress`, run again, so the replayed
world holds what the live one did. Only session-level checks are skipped, since a replay has no
session and the recorded source stands for them: the caller's role, its grant and policy, and
whether it holds the clock. A world-level refusal on replay means the replaying engine or scripts
disagree with the recording: `replay.write_refused` when a replay is verified, and a `Write`
divergence when two runs are compared in lockstep (`docs/spec/replay.md`, 2.1).

## Sailing

The showcase's controls (`rudder`, `sheet`, `hoist`, `interact`), its intents (`come_to_heading`,
`trim_sail`, `sail_to`) with their parameters, completion, failure and steering, and taking a crate
aboard are the sailing game's declarations, in [sailing.md](sailing.md), Actions.

## Determinism

- Controls, pulses, intent instances and the intent counter are world state, so the world hash
  covers them and a fork copies them (charter 3.3).
- Executors are pure functions of their seat's perception (itself deterministic, perception.md,
  Determinism), their instance and their state, using the deterministic math library (spec-sim);
  they run in `IntentId` order.
- Applied actions are canonical (defaults filled, references resolved), recorded in applied order;
  replaying them gives the same worlds. The wall clock decides only which boundary an action lands
  on in real-time pacing, and the replay records that.

## Checks

- **`contract.actions.atomic`**: for every action kind, a call holding one valid action followed by
  one invalid one (each class of phase 1, 2 and 3 problem in turn) is refused; afterwards the world
  hash, the intent counter and the seat's event ring equal those before the call, and the replay
  holds nothing for it.
- **`contract.actions.refusal`**: for every request type and every field and intent parameter,
  generated variants with one edit (a letter dropped, doubled, swapped or changed, a camelCase
  spelling, a missing unit suffix) are each refused with `request.unknown_field` whose suggestions
  contain the original name, or accepted with the warning `request.alias_used` where the variant is
  a declared alias (errors.md); nothing is applied. Property-based (proptest), all fields, 64
  variants each.
- **`contract.intents.transparency`**: for the Rust executors and for a script executor of a fixture
  intent alike, a seeded sailing run drives the boat with intents only for 18000 ticks (five
  minutes: trim, come to a series of headings, sail to two marks with a beat); every control write
  by an executor is recorded as a `Set` at the boundary before the tick it was made in. A second run
  applies those `Set`s and starts no intent. At every tick the two runs' section digests
  (spec-persist's world hash is a tree of per-section digests, `docs/spec/persistence.md`, 5) must
  be equal for every section but `IntentTable` and `ObserverEvents`, and their event records must be
  equal once `intent.*` events are left out.
- **Supersession and lifecycle tests**: each transition of the lifecycle diagram, including a
  deadline reached in the same tick as success (the deadline wins: it is checked first), a control
  set on a holding intent's channel, and the 32-finished retention.
- **Sailing acceptance**: sailing.md, Checks.

## Performance

Rows of spec-arch's framework (`docs/spec/budgets.md`, 5.6); no slice 0 spike built actions, so none
was measured.

| Measure | Budget |
|---|---|
| Validating and applying a call of 4 actions, sailing scene (release) | `act.call`: not measured |
| `interface.intents` per tick with the three sailing intents active | `intent.tick`: not measured |

## Open choices

1. **Intents implemented in scripts as well as Rust.** Recommended: the same executor contract on
   both, with scripts using the host API's stateless function form (spec-script) and their state in
   the instance, so a game's own intents hot-update like its rules (charter 4.2.6). The sailing
   intents are Rust because they run every tick for every boat.
2. **Lockstep actions validated twice.** Recommended (above): validated at submission for a prompt
   answer and again at application, with `action.dropped` when the second fails. The alternative,
   validating only at application, leaves the agent without an answer for up to the input delay.
3. **Route planning round land in `sail_to`.** Not in slice 2: the executor sails straight or beats,
   and warns when land is on the course. A water-navigation primitive (a general primitive, charter
   4.2.4) can later plan round islands from the chart; the contract would then add a `route`
   parameter, a minor change.
4. **`interact` as the only way to take a crate.** Recommended for the showcase (the agent decides).
   Master's automatic pickup could return as a game option without touching the contract.
