# MCP tools

Status: Draft, slice 0. Proposed for PocketEngine's `shared/contract/` (charter 6.2); not yet
synced, so no PocketEngine commit is recorded. Contract version 0.1 (README, Versioning).

Charter: 3.1 (pull perception, token budgets, the marked omniscient view), 3.3 (fork, restore,
replay), 3.4 (structured errors, refusal of unknown fields), 3.5 (time model), 3.6 (tools with
schemas from Rust types), 3.7 (checks an agent can run), 4.1 (`rmcp`), 5.1 (two threads), 6.1 (the
shared external contract), 7 item 11 (MCP tools), 9 (evaluation), 10 (slices 2 and 5).

This file fixes the Model Context Protocol tools both engines serve. The player-facing requests and
answers are the contract's own types ([perception.md](perception.md), [actions.md](actions.md),
[time.md](time.md)), carried unchanged; this file adds what only the MCP layer has: sessions and
their grants, which tools a session lists, the worlds a session can address (main and its forks),
snapshots, restore and replay queries, the developer-facing tools, how answers fit a token budget
outside perception, how the schemas come from Rust types through `rmcp`, and the tests that hold all
of it. An agent sees the same tools on Pocket3D and on PocketEngine; only the script language behind
them differs (charter 6.1).

## 1. Concepts used from other specifications

Used by name and not defined here. Names in `code` are the owners'.

| Concept | Owner | File |
|---|---|---|
| `Tick`, `TickRate` (60 for the showcase), tick boundaries, `EntityId` (1 to 2^53 - 1) and `Name`, iteration order, events and their dispatch, the RNG | spec-sim | `docs/spec/simulation.md`, `docs/spec/rng.md` |
| `WorldHash` (16 bytes, 32 hex digits), `Snapshot`, `restore`, `fork`, `diff`, replays and their segments, `Divergence`, `FieldDiff` | spec-persist | `docs/spec/persistence.md`, `docs/spec/replay.md` |
| Bundle hash, engine version, component versions and migrations, saves | spec-persist | `docs/spec/versions.md` |
| Hot update (what `apply` does), transpiling, the type check, the lint, `script.*` and `lint.*` codes | spec-script | `docs/spec/script-host.md`, `docs/spec/hot-update.md` |
| Roles (`player`, `developer`, `checker`), seats (`SeatId`), the `GameDefinition`, conventions (units, frame, numbers, `EntityRef`), the token estimate | spec-contract | [README.md](README.md) |
| `ObserveRequest`, `NearbyRequest`, `DescribeRequest`, `EventsRequest`, `Observation`, `PerceivedEvent`, projections, `budget_tokens` and the filling rule, the push cursor, the omniscient view | spec-contract | [perception.md](perception.md) |
| `ActRequest`, `ActResult`, affordances and their listing, intents, `IntentId`, `IntentsRequest` and `IntentView` | spec-contract | [actions.md](actions.md) |
| `Pacing`, the clock holder, `StepRequest`, `CommitRequest`, `WaitRequest`, `ContinueRequest`, `Until`, `TimeResult`, `DecisionPoint`, `DecisionFilter`, episodes | spec-contract | [time.md](time.md) |
| The error object `{code, message, detail}` (`Problem`), code families, strict decoding with suggestions, aliases; halts after a script error | spec-contract | [errors.md](errors.md), [time.md](time.md) (Halts) |
| Crates (`pocket-mcp`), `GameClient`, `Source`, the command catalog and queue, `WorldSnapshot`, the local check command and `pocket check`, budgets | spec-arch | `docs/spec/architecture.md`, `docs/spec/threads.md`, `docs/spec/checks.md` |

## 2. Decisions

1. **One tool per request type, schemas from Rust types.** Each player tool takes one of the
   contract's request types and answers its response type; each developer tool takes and answers
   types defined here. All derive `serde` and `schemars`, and `rmcp` publishes the generated schemas
   (charter 3.6). Both lines serve the same names and JSON shapes, held by golden schemas (test M1).
   Byte formats inside each engine may differ (charter 6.1, item 3).
2. **Typed tools, no gateway, and a tool list that fits the session.** Master served about thirty
   MCP tools plus `runtime_command`, a gateway to over two hundred untyped commands, and needed
   `help`, `commands {text}` and usage lines so agents could find them (master `docs/mcp.md`, Asking
   the engine how to call it). Here every capability is a typed tool, breadth comes from parameters
   checked at run time against what the engine publishes (`describe`, `schema`, `affordances`), and
   a session lists only the tools its role, its policy and the pacing let it call. The list reaches
   the model in every turn, so its size is budgeted (8.4).
3. **The engine enforces roles.** A session is bound to a grant naming one of the contract's roles
   and, for a player, its seat. A player's session lists only player tools and a call that asks for
   more is refused. The omniscient view is marked in every answer it shapes (charter 3.1;
   perception.md, The omniscient view).
4. **Reads never change a world; writes are commands.** Each tool call is one command of the
   catalog, or a fixed sequence of them, sent through the session's `GameClient` (`threads.md`,
   5.1). Reads change no hash, journal or random stream; writes apply at a tick boundary and are
   recorded.
5. **Every answer names its world.** Forks make "which world am I looking at" a real question, so
   every answer says `world` (`main` or a branch) beside the contract's `tick`, and a developer's or
   checker's answer carries the world hash.
6. **Budgets everywhere, and nothing cut mid-JSON.** Master cut its tool text at 24,000 bytes and
   appended a note (master `tools/pocket/src/mcp.rs`, `text_result`), leaving invalid JSON.
   Perception answers follow the contract's filling rule; every other answer drops whole items,
   lowest priority first, and says how many and how to get them (section 8).
7. **Errors are tool results.** A refused call returns `isError: true` with the contract's
   `{code, message, detail}`, so the model reads it and corrects itself (charter 3.4). Only a
   malformed request or an unknown tool is a JSON-RPC error.
8. **The same answer for the same question.** Same world, grant, parameters and session cursors: the
   same bytes, outside values under a `timing` key (section 9). MCP sessions are therefore
   replayable and comparable across runs, lines and the native and web builds.

## 3. Sessions

A session is one MCP connection bound to one grant for its life. Its state (push cursors, branches,
snapshots, pending decisions, statistics) is session state: never world state, never hashed
(time.md, Determinism).

### 3.1 Grants

```rust
/// What a session is and may do; fixed when the session opens. Code conventions: section 4.
pub struct Grant {
    pub role: Role,                         // spec-contract: player | developer | checker
    pub seat: Option<SeatId>,               // required for a player; the seat it plays
    /// Replaces the seat's observer profile; given only by the process owner. The benchmark's
    /// omniscient ablation binds the seat to the omniscient profile (README, Seats and callers).
    pub profile: Option<String>,
    pub decision: Option<DecisionFilter>,   // time.md; default: the game's for the seat
    pub clock: bool,                        // a player that holds the clock in stepped pacing
    /// The default `max_wall_ms` of this session's time requests (time.md, Requests); the
    /// benchmark sets 600,000 so where a step stops never depends on a line's tick speed.
    pub max_wall_ms: Option<u32>,
    pub policy: Policy,
    pub label: String,                      // for logs and statistics: "player", "checker"
}

#[serde(default)]
pub struct Policy {
    pub fork: bool,              // may fork worlds (lookahead); default false for a player
    pub max_branches: u32,       // live branches at once; default 4, at most MAX_BRANCHES (16)
    pub branch_ticks: u64,       // ticks simulated in branches, in all; 0: no limit
    /// Ticks simulated in branches between two decision points answered on main; 0: no limit.
    /// The benchmark's fork condition sets 600 (exact lookahead per decision, 5.5).
    pub branch_ticks_per_decision: u64,
    pub rewind: bool,            // may restore main from a snapshot or a branch; default false
    pub pause: bool,             // may pause and resume main in real time; default false
}
```

- **Profiles.** `profile` replaces the seat's observer profile; the only value a benchmark uses is
  the reserved `omniscient_player` (perception.md, The omniscient view).
- **Who grants.** A client never chooses its grant. Over stdio the grant is given to the serving
  process (`--grant <json>`); over HTTP the process owner (the editor, the benchmark harness)
  registers grants when it starts the runtime, each under a random 128-bit token, and a client
  connects to `/mcp/<token>`. An agent therefore cannot raise its role by asking (master's benchmark
  agents reached the harness by absolute path: master `docs/agent-eval.md`, Out of the repository).
- **Sources.** A player session sends as `Source::Player` with its seat's index in the seat order; a
  developer or checker session as the next `Source::Developer(n)` (`threads.md`, 5.1). A checker
  session's tools send only Read commands of the catalog.
- **Several sessions, one runtime.** In the benchmark the agent's session and the harness's checker
  share main.
- A player grant whose seat does not exist, or is held by another player session, is refused when
  the session opens, with `seat.unknown` or `session.seat_taken`.

### 3.2 Pacing, the clock and episodes

- The runtime's pacing is chosen when it starts (`--pacing`, default the game's first,
  `GameDefinition.pacing`) and changed by a developer with `time_control` (time.md, Pacing). It is
  one per world; branches always run stepped.
- In stepped pacing the clock holders of main are every developer session and the player session
  whose grant has `clock: true` (at most one); anyone else's `step` gets `time.not_clock_holder`.
  The session that made a branch holds its clock alone.
- An episode's tick limit (time.md, Episodes) is a runtime option (`--tick-limit`) or a parameter of
  the developer's `reset`.

### 3.3 Which tools a session lists

| Tool | Player | Checker | Developer | Section |
|---|---|---|---|---|
| `session`, `describe`, `observe`, `nearby`, `events`, `affordances`, `intents`, `replay` | yes | yes, omniscient allowed | yes | 5 |
| `act` | its seat | no | any seat | 5 |
| `step` | stepped pacing, clock holder | no | yes | 5 |
| `commit`, `wait` | lockstep; `wait` also in real time | no | as a player | 5 |
| `continue` | real time and lockstep | no | as a player | 5 |
| `fork`, `discard` | if `policy.fork` | no | yes | 5.5 |
| `snapshot`, `restore` | if `policy.fork` or `policy.rewind`; a player restores main only with `rewind`, its own branches with `fork` | no | yes | 5.6 |
| `why`, `world_query`, `world_get`, `schema`, `project_brief`, `docs_search`, `assets` | no | yes | yes | 6 |
| `time_control` | pause and resume only, if `policy.pause` | no | yes | 6 |
| `reset`, `world_edit`, `scene_save`, `apply`, `asset_import`, `checks`, `profile`, `eval` | no | no | yes | 6 |

A tool the session may not call is not listed. When a developer changes the pacing, every session
whose list changes gets MCP's `notifications/tools/list_changed`. A call to an unlisted tool (a
client may cache an old list) is refused with the code that says why: `permission.denied` (the
role), `time.wrong_mode` (the pacing) or `session.policy_denies` (the policy).

## 4. Requests, answers and conventions

### 4.1 The envelope

A player tool's parameters are the contract's request type plus `world`; its answer is the
contract's response plus `world` and, for a developer or checker, `hash`:

```rust
/// Parameters of a tool that carries a contract request `R`.
pub struct ToolParams<R> {
    pub world: Option<WorldRef>,            // default "main"
    #[serde(flatten)] pub request: R,       // e.g. ObserveRequest, unchanged
}

/// The answer of such a tool: the contract's response `T` and the MCP layer's two fields.
pub struct Answer<T> {
    pub world: WorldRef,
    /// The world hash at the answer's tick (spec-persist), 32 lowercase hex digits. Developer and
    /// checker only: equal hashes in two branches would tell a player that hidden state is equal.
    pub hash: Option<String>,
    #[serde(flatten)] pub body: T,          // e.g. Observation, unchanged
}

/// "main", or a branch made by `fork`: "b1", "b2", ... in order of creation, never reused
/// within the runtime's life. #[serde(transparent)]
pub struct WorldRef(pub String);
```

- `serde`'s `deny_unknown_fields` does not combine with `flatten`, so unknown keys are refused by
  the contract's strict decoder, which checks the call against the composed JSON Schema
  (`additionalProperties: false` at every level) before `serde` reads it (section 7.2).
- In the text projection the two fields are one first line, `world b1 hash 9f3c...`, written only
  when the world is a branch or the caller has a hash; a player on main reads exactly the contract's
  projection, the bytes both lines share. The omniscient mark stays on the projection's header line
  (projection.md, Text projection), which then is the second line.
- Developer tools' answers carry `tick`, `world`, `hash` and `omniscient: true` (everything they
  read is unfiltered: perception.md, The omniscient view) at their head.

### 4.2 Conventions

The contract's conventions hold for every tool (README, Conventions every file uses): names, unit
suffixes (`_m`, `_deg`, `_s`), the frame, numbers that are finite and refused rather than clamped,
`EntityRef`, `tick` and `t_s`. In addition:

- Parameter types are `#[serde(deny_unknown_fields)]` with every `Option` and `Vec`
  `#[serde(default)]`; result fields that are empty are omitted, never `null`, so the same answer
  has the same bytes. Enums are `snake_case`; tagged enums name their tag.
- Integer parameters carry their ranges in their schemas (`#[schemars(range(min = 1, max = 16))]`).
- Doc comments become schema descriptions (section 8.4 bounds their length).
- Code blocks in this file omit the derives these bullets imply.

### 4.3 What a result carries

- **The contract's tools** (the first eight rows of section 5's table) send `content` only and
  declare no `outputSchema`; the JSON forms' schemas are the contract's generated ones
  (`contract.schema`). A perception query (`observe`, `nearby`, `describe` of an entity, `events`)
  sends one text block in the projection the call chose: text by default for MCP (perception.md,
  Queries), `projection: "json"` for JSON. `describe` of the game definition, `affordances`, `act`
  and the time requests send their result's compact JSON; when it carries a text observation
  (`step {observe}` and the like), that goes as a second text block rather than as a JSON string
  full of escaped newlines. In the text projection the events delta of `act` and the time requests
  is written as `event` lines in the text block (projection.md, Text projection), not as JSON.
- **This file's tools** (`session`, `fork`, `discard`, `snapshot`, `restore`, `replay` and the
  developer tools) send `structuredContent` and one text block with its compact JSON, as `rmcp`'s
  `CallToolResult::structured` builds, and declare `outputSchema`.
- Sending an answer only once keeps what the model reads to one copy: pretty printing alone nearly
  doubled what master's models read (master `tools/pocket/src/mcp.rs`). Which clients hand the model
  `structuredContent` as well as `content` is measured by the benchmark's proxy
  (`../benchmark/README.md`, The recording proxy); open choice 5.

## 5. Player-facing tools

| Tool | Request | Answer | Owner of both |
|---|---|---|---|
| `observe` | `ObserveRequest` | `Observation` | perception.md, Queries |
| `nearby` | `NearbyRequest` | `{tick, omniscient, entities, omitted, tokens}` | perception.md |
| `describe` | `DescribeParams` (5.2) | the entity, or the `GameDefinition` for the caller | perception.md; README, The game definition |
| `events` | `EventsRequest` | `{tick, omniscient, events, cursor, omitted, tokens}` | perception.md |
| `affordances` | `{seat, entity, kinds, within_m, available_only, budget_tokens}` | the affordance list | actions.md, Affordances |
| `intents` | `IntentsRequest` | `{tick, omniscient, intents: [IntentView], omitted, tokens}`: a player its own seat's, a checker or developer any seat's; failed intents with their whole problem | actions.md, Intent instances |
| `act` | `ActRequest` | `ActResult` | actions.md |
| `step`, `commit`, `wait`, `continue` | `StepRequest`, `CommitRequest`, `WaitRequest`, `ContinueRequest` | `TimeResult` | time.md, Requests |
| `session`, `fork`, `discard`, `snapshot`, `restore`, `replay` | below | below | this file |

Each takes `world` (4.1). A developer names the seat in requests that take one; a checker may set
`omniscient: true` where a request has it. The rules of each request (validation, atomicity, when
actions take effect, `until`, decision points, the filling rule) are the owners'; the MCP layer adds
only the following.

### 5.1 `session`

The first call: who this session is, what it may do, and what the world waits for.

```rust
pub struct SessionParams { pub all: bool }        // developer and checker: every session too

pub struct SessionResult {
    pub tick: Tick, pub world: WorldRef,
    pub role: Role, pub label: String, pub seat: Option<SeatId>,
    pub policy: Policy, pub clock: bool,
    pub contract: String,                         // "0.1"
    pub status: TimeStatus,                       // time.md, Requests: filtered for the caller
    pub halted: Option<Problem>,                  // time.md, Halts: `time.halted` for a player
    pub tainted: Option<Tick>,                    // a debugger evaluation marked the run (replay)
    pub branches: Vec<BranchInfo>,                // {world, parent, forked_at, tick}
    pub snapshots: Vec<SnapshotInfo>,             // {snapshot, world, tick, label}
    pub stats: SessionStats,
    pub sessions: Vec<SessionRow>,                // `all` only: {label, role, seat, stats}
}

pub struct SessionStats {
    pub calls: u64, pub failed_calls: u64, pub acts: u64, pub branch_acts: u64,
    pub decision_points: u64,                     // decision points that arose for the seat on main
    pub decisions_answered: u64,                  // of those, answered (time.md, Decision points)
    pub steps: u64, pub wall_limit_stops: u64,    // steps that stopped on `max_wall_ms`
    pub forks: u64, pub branch_ticks: u64, pub restores: u64,
}
```

The benchmark's checker reads the player's statistics through `session {all: true}`
(`../benchmark/README.md`, Metrics).

### 5.2 `describe`

The contract asks two things of `describe`: the game definition filtered for the caller, which an
agent reads first (README, The game definition), and one entity as the observer knows it
(perception.md, Queries). One tool serves both, with one parameter type:

```rust
pub struct DescribeParams {
    pub world: Option<WorldRef>,
    pub seat: Option<SeatId>,
    pub entity: Option<EntityRef>,       // one entity: perception.md's DescribeRequest
    pub part: Option<DescribePart>,      // one part of the definition; neither: the whole
    pub name: Option<String>,            // with `part`: one item of it ("sail_to")
    pub budget_tokens: Option<u32>,      // default 2,000 for the definition (8.3)
    pub projection: Option<Projection>,
    pub omniscient: Option<bool>,
}
pub enum DescribePart { Seats, Instruments, Controls, Intents, Kinds, Events, Codes, Structure,
                        Pacing, Decisions, Episode }
```

`entity` and `part` together are refused with `request.conflict`; `name` without `part` with
`request.not_applicable`. One type means the strict decoder always knows which fields to suggest
from (`{"entty": "Mark1"}` suggests `entity`). Beyond the budget the definition lists names only,
with the `describe {"part": ...}` call that gives the rest. Open choice 4 says why one tool.

### 5.3 Push cursors

The contract's push (perception.md, Push) needs "the last response that carried events to the same
caller". The session keeps that cursor per world and per seat: a branch has its own cursor, starting
at its parent's when forked, and two sessions on one seat each get every event once.
`replay {kind: "events"}` reads before the cursor.

### 5.4 `step` for developers

A developer's `step` may also follow values through the run, as master's `step {watch}` did (master
`docs/mcp.md`, Tools), so the top of a jump or the lowest a value fell is one call:

```rust
pub struct DevStepParams {
    pub world: Option<WorldRef>,
    pub watch: Vec<String>,                 // "Entity:Component.field.path", at most 16
    pub every: Option<u32>,                 // also every n-th tick's value
    #[serde(flatten)] pub request: StepRequest,
}
// The answer adds `watched: [{path, first, last, changes, min, max, min_tick, max_tick,
// series?: [[tick, value]]}]`, numbers only for the extremes.
```

`StepRequest`'s `max_wall_ms` (default 30,000) also keeps a call inside MCP clients' patience:
master's agents lost runs to a client's one-minute limit (master `docs/agent-eval.md`, Fifty-seven
on opencode).

### 5.5 `fork` and `discard`

```rust
pub struct ForkParams { pub world: Option<WorldRef>, pub label: Option<String> }  // a branch too
/// The new branch's tick is its parent's.
pub struct ForkResult {
    pub tick: Tick, pub world: WorldRef, pub parent: WorldRef, pub hash: Option<String>,
}

pub struct DiscardParams { pub branch: WorldRef }
pub struct DiscardResult { pub tick: Tick, pub world: WorldRef, pub discarded: WorldRef }
```

- A fork is spec-persist's `fork`: the branch's entity ids, random streams and simulation caches
  equal its parent's (charter 3.3; `persistence.md`, 7). Every tool takes `world` to act on it. The
  fork alone costs the game thread the budget `fork.sail`; a branch ready to step, its bundle
  instantiated in a script host, costs `branch.sail` (`docs/spec/budgets.md`, 5.1).
- A branch belongs to its session; another naming it gets `session.unknown_world`. It ends with
  `discard`, with its session, or when the runtime stops.
- A branch is detached and stepped: stepping it never moves its parent, and where main runs in real
  time, main runs on while the agent looks ahead (`docs/spec/threads.md`, 3.6, says how branch ticks
  share the game thread). A player's actions in a branch count as `branch_acts`, its ticks against
  `policy.branch_ticks` and `branch_ticks_per_decision`. A branch `step` whose `ticks` exceed what
  is left of either is refused up front with `session.limit_reached {what: "branch_ticks"}`, never
  stopped partway.
- For a player a branch is lookahead: it simulates the true world, hidden state included, and the
  player sees it only through its perception. Reports name this condition "exact lookahead". The
  budget per decision bounds it, so a branch cannot scout the whole task and main then sail straight
  to what it found. Whether lookahead is allowed is policy, and the benchmark measures its effect
  (`../benchmark/tasks.md`, The player suite); open choice 6.
- `discard {branch: "main"}` is refused with `session.cannot_discard_main`. The branch's bundle is
  the one main ran when it was forked (`docs/spec/versions.md`, 7.5); `apply {world}` changes a
  branch's scripts alone.

### 5.6 `snapshot` and `restore`

```rust
pub struct SnapshotParams { pub world: Option<WorldRef>, pub label: Option<String> }
pub struct SnapshotResult { pub tick: Tick, pub world: WorldRef, pub snapshot: String }  // "s1"

/// Exactly one of `snapshot` and `branch`: both are `request.conflict`, neither
/// `request.missing_field`.
pub struct RestoreParams {
    pub world: Option<WorldRef>,            // the world to overwrite
    pub snapshot: Option<String>,
    pub branch: Option<WorldRef>,           // make `world` a copy of this branch (adopt it)
}
pub struct RestoreResult { pub tick: Tick, pub world: WorldRef, pub applied_at: Tick }
```

- Snapshots are spec-persist's `Snapshot`, held in memory by the session, at most MAX_SNAPSHOTS
  (16); the seventeenth is refused with `session.limit_reached`, nothing is dropped silently. An
  unknown snapshot id is `session.unknown_snapshot` with the session's snapshots.
- A restore replaces the whole world and may move its tick backwards, so it is recorded not as a
  write but as the end of the world's recording segment and a `Rebase` record holding the snapshot
  the world now holds (`docs/spec/replay.md`, 2.5); `reset` (6) is recorded the same way.
- A player restores its own branches from its own snapshots with `policy.fork` (a branch overwritten
  by an earlier state of itself or of main), which is lookahead, not a retry. Restoring main, from a
  snapshot or by adopting a branch, needs `policy.rewind`, which the benchmark leaves off: it would
  let a player retry a failed attempt, which the fork comparison must not mix in.

### 5.7 `replay`

Questions about the past. A player asks about its own seat; a checker or developer about anything.

```rust
pub struct ReplayParams {
    pub world: Option<WorldRef>, pub query: ReplayQuery, pub budget_tokens: Option<u32>,
}

#[serde(tag = "kind")]
pub enum ReplayQuery {
    Actions { since_tick: Option<u64>, until_tick: Option<u64> },   // the seat's applied actions
    Events { since_tick: Option<u64>, until_tick: Option<u64>, kinds: Vec<String> },
    ObserveAt { tick: u64, request: Option<ObserveRequest> },     // perception after `tick`
    Recordings,                                 // developer and checker, as are the three below
    Load { path: String },                      // a recording file, listed from then on
    FirstDivergence { a: RunRef, b: RunRef },   // replay.md, 3.1
    DiffAt { a: RunRef, b: RunRef, tick: u64 }, // replay.md, 3.2
}

#[serde(tag = "run")]
pub enum RunRef {
    World { world: WorldRef },
    Recording { id: String },
    /// A recording's inputs run again under its own scripts or the current ones (versions.md, 7.3).
    Rerun { recording: String, scripts: ScriptsAt },   // recorded | current
}
```

**Recordings.** A recording id is `main` (the session's history of main, its own replay,
`docs/spec/replay.md` 2.4), a branch's world id (its own recording), or the id `Load {path}` or the
serve command's `--recording <file>` gave a file, all listed by `Recordings`; any other id is
`replay.unknown_recording` with the ids there are.

`Events` gives perceived events in a range of ticks. Recordings hold writes and hashes, not events,
and the world keeps one tick of them (`docs/spec/simulation.md`, 5.3), so for a player the events
the seat's ring no longer holds come from re-simulating the recording and reading the seat's
`ObserverEvents` there, with the observer's own `seq`s, never from the world's event record, which
would show events the seat never perceived; ticks the recording cannot re-simulate are
`replay.out_of_range`. A developer or checker reads the world's events, marked omniscient, with
spec-sim's `EventSeq`s. `ObserveAt` re-simulates from the recording and projects the seat's
perception after `tick`; a tick outside the recording is `replay.out_of_range`. The answers:
`actions` (spec-persist's applied inputs of the seat: `{tick, action, outcome}`), `events`,
`observation`, `recordings` (`{id, segments, ticks, seed, bundle_hash, engine_version}`),
`divergence` (spec-persist's `Divergence`, or none) and `fields` (`FieldDiff`s), each list budgeted.
`FirstDivergence` of a `Recording` against its `Rerun {scripts: "current"}` is charter 4.2.6's
"replay the same recording with old and new scripts to find the first diverging tick". Re-simulation
runs in a throwaway world on a runtime worker (`docs/spec/threads.md`, 6) and never touches main.

## 6. Developer-facing tools

The tools of charter 2.2's agent as developer; the editor's panels call the same commands (charter
5.1). A checker has the read-only ones (3.3).

| Tool | Parameters | Result | Notes |
|---|---|---|---|
| `project_brief` | `budget_tokens` | `{tick, world, text}` | One screen of text: files with sizes, the script language and entry, components in use and the project's own with their fields, seats, intents and affordances, the pacing, scene roots, the last checks' verdicts, what the lint finds, where to look next. Master's agents started here (master `docs/mcp.md`, Asking the engine how to call it). |
| `docs_search` | `query`, `limit` (1 to 10, default 3) | `{sections: [{file, heading, text}]}` | Sections of the engine's agent-facing documentation, each at most 2,500 characters, ranked by stemmed word matches with rare and heading words weighted most. Master's agents read a 75 KB reference whole and carried it in every later turn (master `docs/agent-eval.md`, Reading less). |
| `schema` | `component?`, `search?`, `full?`, `budget_tokens` | `{components: [ComponentSchema], others}` | Engine and project components: name, source (`engine` or `project`), version, doc, fields with type, unit, default and value names; the JSON Schema with `full: true`. An unknown name is refused with the nearest names. |
| `world_query` | `world`, `with`, `without`, `name` (glob), `under`, `where: [{field, op, value}]`, `fields`, `limit` (default 50), `budget_tokens` | `{tick, world, hash, omniscient, total, entities: [{id, name, path, fields}], omitted}` | Rows in ascending `EntityId` (spec-sim). `fields` names `Component` or `Component.field.path`. |
| `world_get` | `world`, `entities` (1 to 16), `components` (empty: all), `budget_tokens` | `{..., entities: [{id, name, path, parent, children, components}]}` | Components as their JSON forms, through the registry's schemas. |
| `why` | `world`, `seq` | `{..., chain: [event]}` | An event's causes up to its root (master `events.why`), omniscient. `seq` is spec-sim's `EventSeq`, as `events {omniscient: true}` lists it (perception.md, Queries). |
| `world_edit` | 6.1 | 6.1 | Spawn, set, remove, destroy. |
| `scene_save` | `path?`, `roots` | `{path, bytes, entities}` | Writes the live entities under `roots` to the project's scene file in its canonical text form, so edits made with `world_edit` can be kept. |
| `apply` | 6.2 | 6.2 | Loads changed project files into the running world. |
| `time_control` | `pause?`, `pacing?` | `time.status` | time.md's `pause`, `resume` and `time.set_pacing`. |
| `reset` | `seed?`, `tick_limit?` | `{tick, world, hash}` | A new episode (time.md, Episodes): a fresh world from the scene and the loaded scripts with the seed; branches and snapshots of the old main are dropped, cursors restart. Recorded as the end of main's segment and a `Rebase` (5.6). |
| `checks` | `only?`, `seeds?` | `pocket check`'s report | The project's determinism, fork, replay and reload checks (`checks.md`, 2 and 8), run in throwaway worlds; a failure carries its code and divergence. |
| `profile` | `world`, `ticks` (default 120) | `{tick, ticks, systems, scripts, timing}` | Runs `ticks` in a fork and reports time per system and per script handler with file and line, against the budget `tick.sail` (`docs/spec/budgets.md`). Main is untouched. |
| `eval` | `world`, `code`, `at_tick?`, `budget_tokens` | `{tick, world, value, logs}` | Evaluates code in the line's script language in a throwaway fork of `world` (at `at_tick`, re-simulated, when given), compiled through the script host's loader and lint as a module, since the sandbox has no `eval` (`docs/spec/script-sandbox.md`, 2.2); a script error comes back source-mapped (charter 4.2.6: restore to any tick, inspect and evaluate). |
| `assets`, `asset_import` | `prefix?`, `kind?`; `path` or `job`, `wait_ms?` | the asset list; `{job, status, progress, result?}` | Slice 3. Import runs off the game thread (charter 4.4); the call returns after `wait_ms` (default 0) with the job's state. |

Reserved for slice 3: `capture` (the frame as an image, with the entities visible in it) and
`look_around` (an entity from six sides in one picture), after master's (`docs/mcp.md`, Tools),
specified with rendering.

### 6.1 `world_edit`

```rust
pub struct WorldEditParams {
    pub world: Option<WorldRef>,
    #[schemars(length(min = 1, max = 64))]
    pub edits: Vec<Edit>,
    pub cause: Option<String>,              // recorded as the cause of the events the edits raise
}

#[serde(tag = "op")]
pub enum Edit {
    /// An entity with components, and children nested as a scene nests them.
    Spawn { name: Option<String>, parent: Option<EntityRef>, components: JsonMap,
            children: Vec<SpawnNode> },
    /// Write the listed fields, adding the component when missing; other fields keep their
    /// values. A key may be a path into the component ("position.y", "layers.1.height").
    Set { entity: EntityRef, component: String, value: JsonMap },
    Remove { entity: EntityRef, component: String },
    Destroy { entity: EntityRef },          // with its descendants
}
```

The result is `{tick, world, hash, applied_at, results}`, one outcome per edit:
`{op: "spawned", ids}`, `{op: "set", value}` (the component as it now is, as master's `world.set`
answered), `{op: "removed"}`, `{op: "destroyed", count}`.

- Every edit is checked before any is applied: component and field names, value types and ranges,
  integer conversion and entity references, with the contract's strict decoding against the
  registry's schemas. A field typed as an entity takes an id, a name or a path.
- The edits of one call are one Write command at one boundary, recorded like actions in their
  canonical form (`docs/spec/threads.md`, 5.3: every name or path resolved to its `EntityId`), so a
  replay reproduces them without resolving a name again.
- Edits into a branch are allowed: that is how a developer tries a change without touching main.

### 6.2 `apply`

```rust
pub struct ApplyParams {
    pub world: Option<WorldRef>, // whose bundle changes; default main (versions.md 7.5)
    pub parts: Vec<Part>,        // scripts, components, scene, settings; empty: all changed
    /// The type check (tsc on this line): "report" (the default), "refuse" or "skip", as
    /// hot-update.md's `scripts.apply` takes it.
    pub types: Option<TypeCheck>,
    /// Run the new scripts in a fork for this many ticks first (at most 3,600); main loads them
    /// only if the trial raised no script error (charter 4.2.6: they MAY run first in a fork).
    pub trial_ticks: Option<u32>,
    pub restart: bool,           // a fresh world from the scene instead of a hot update
    pub ticks: Option<u32>,      // ticks to step main after loading; default 0
}

pub struct ApplyResult {
    pub tick: Tick, pub world: WorldRef, pub hash: Option<String>,
    pub applied_at: Tick,
    pub bundle: BundleInfo,                 // {hash, modules}: versions.md, 3.2
    pub warnings: Vec<Problem>,             // script.* and lint.* warnings, located
    pub migrations: Vec<MigrationRun>,      // versions.md, 8: {component, from, to, entities}
    pub type_errors: Vec<Problem>,          // with types "report": located tsc findings
    pub trial: Option<TrialReport>,         // hot-update.md, 8
    pub timing: Timing,
}
```

The scripts part of `apply` is hot-update.md's `scripts.apply`. When nothing loads (a transpile,
lint or load error, a type error with `types: "refuse"`, a failed trial, a failed migration),
`apply` answers `isError: true` with `session.apply_failed`, every problem in `detail.problems`
(spec-script's located `script.*`, `lint.*` and `types.error` objects, file, line and column in the
TypeScript source), and the running world keeps its old scripts and data (charter 4.2.6). With the
default `"report"` the scripts load and the type errors come back in `type_errors`, as master's
apply did. Master's agents spent most of their tokens on script tasks until one call bundled,
checked, reloaded and stepped (master `docs/mcp.md`, Applying an edit); this is that call. A hot
update keeps the world (hot-update.md 3); `restart: true` is this file's `reset` with the new
scripts: a fresh world from the scene and the seed, with nothing held over.

## 7. Errors

### 7.1 What goes where

| What went wrong | Answer |
|---|---|
| Malformed JSON-RPC, unknown method | A JSON-RPC error, as MCP prescribes, with the problem (`request.malformed`) in `data`. |
| Unknown tool name | JSON-RPC error -32602 whose `data` is the problem `request.unknown_method` with its suggestions (errors.md, Where problems appear). |
| Anything about the call's arguments, the session or the world | A tool result with `isError: true`: `structuredContent` is the problem (one, the rest of its validation phase in `detail.also`: errors.md, Several problems) and the text block is its `message` followed by its compact JSON, on every tool (errors.md, Where problems appear). |

A session's refusals use the owners' codes where one fits: `permission.denied` for a tool or
parameter the role may not use, `time.wrong_mode` for one the pacing does not take,
`time.not_clock_holder`, `seat.unknown`, `seat.not_yours`, `perception.omniscient_forbidden`. The
`session` family is this file's (errors.md, Codes):

| Code | When | `detail` | Message template |
|---|---|---|---|
| `session.policy_denies` | The session's policy does not allow the request (`fork`, `restore`, `pause`). | `request`, `policy` | `This session may not {request}: its policy has {policy} off.` |
| `session.seat_taken` | A player session opens on a seat another player session holds. | `seat` | `Seat {seat} is already played by another session.` |
| `session.unknown_world` | The world does not exist, was discarded or is another session's. | `path`, `world`, `worlds` | `There is no world '{world}' in this session; it has {worlds}.` |
| `session.limit_reached` | Branches, snapshots, batch sizes, `branch_ticks` and `branch_ticks_per_decision`, the benchmark's call limit. | `limit`, `value`, `what` | `This session already has {value} {what}; the limit is {limit}.` |
| `session.unknown_snapshot` | `restore` names a snapshot the session does not hold. | `snapshot`, `snapshots` | `There is no snapshot '{snapshot}'; this session has {snapshots}.` |
| `session.cannot_discard_main` | `discard {branch: "main"}`. | | `Main cannot be discarded; only branches can.` |
| `session.budget_too_small` | A developer tool's mandatory part does not fit (perception answers use `perception.budget_too_small`). | `budget_tokens`, `min_tokens` | `A budget of {budget_tokens} tokens cannot hold this answer's head; give at least {min_tokens}.` |
| `session.apply_failed` | `apply` loaded nothing. | `problems`, `trial` | `Nothing was loaded: {first problem's message}` |

The runtime never exits on a tool or script error. A script error halts the simulation (time.md,
Halts; `script-sandbox.md`, 5.4); time requests answer `stopped: "halted"` with the problem (for a
player, `time.halted`, whose detail holds no script internals), and `session` shows it until a
developer's resume, an `apply` or a `restore` clears it (master's runtime once exited under an
agent: master `docs/agent-eval.md`, The first failure was the engine's). `act` is still accepted
during a halt; after the episode ended it is refused with `time.episode_over`.

### 7.2 Strict parameters in `rmcp`

`rmcp`'s `Parameters<P>` turns a deserialization failure into a JSON-RPC `invalid_params` error
(rmcp 3.5.0, `handler/server/tool.rs`), which many clients show the model poorly or not at all, and
`serde` stops at the first unknown field. So every tool takes its own extractor:

```rust
/// Tool parameters through the contract's strict decoder. Never fails at the protocol level:
/// a refusal reaches the model as a tool result with isError.
pub struct Strict<P>(pub Result<P, Problem>);

impl<S, P: DeserializeOwned + JsonSchema> FromContextPart<ToolCallContext<'_, S>> for Strict<P> {
    fn from_context_part(cx: &mut ToolCallContext<'_, S>) -> Result<Self, rmcp::ErrorData> {
        let args = cx.arguments.take().unwrap_or_default();
        Ok(Strict(strict_decode::<P>(serde_json::Value::Object(args))))   // errors.md
    }
}
```

`#[tool]` infers an input schema only from a parameter type named `Parameters` (rmcp-macros 3.5.0,
`find_parameters_type_in_sig`), so every tool names its schema:

```rust
/// The seat's situation now, ranked for relevance, within a token budget.
#[tool(name = "observe", input_schema = input_schema::<ToolParams<ObserveRequest>>(),
       annotations(read_only_hint = true))]
async fn observe(&self, Strict(p): Strict<ToolParams<ObserveRequest>>) -> CallToolResult {
    self.call("observe", p).await       // the catalog command through the session's GameClient
}
```

`input_schema::<P>()` is `rmcp`'s `schema_for_input`: draft 2020-12 schemas generated by `schemars`
1.x, the root title and description stripped. Pin `rmcp = "=3.5.0"` and `schemars = "1.2"` until
slice 2 revisits them. Annotations: `readOnlyHint` on every tool a checker may call and on `profile`
and `eval` (they work in throwaway worlds); `destructiveHint` on `restore`, `discard`, `reset`,
`world_edit` and `apply`.

The tool names, descriptions and annotations are not the handlers' doc comments in each line's
server: they are a table in `pocket-contract` beside the types (`tools::TOOLS`, one entry per tool:
name, description, annotations, parameter and result types), which both lines' servers read, so the
whole `tools/list` an agent reads every turn is shared. The `initialize` result carries no
`instructions` string on either line.

## 8. Token budgets

### 8.1 The unit

The contract's estimate, `tokens(bytes) = ceil(bytes / 4)` over the UTF-8 bytes of the answer's text
blocks (README, Token estimates), the part every client hands the model; master's perception
benchmarks used the same (master `docs/design/scenarios.md`, Perception benchmarks). The benchmark
reports real token counts beside it (no client has run yet, so the ratio is not measured). Every
tool with a list-shaped answer takes `budget_tokens`, at most 16,000 (perception.md, Token budgets).

### 8.2 Fitting a developer answer

Perception answers are filled by the contract's rule. A developer tool's answer is fitted by:

```rust
pub trait Budgeted: Serialize {
    /// Drop the least important items of this answer's lists until its compact JSON fits in
    /// `max_bytes`, counting each list's drops in `omitted`. Returns false when the mandatory
    /// part (the head, scalar fields) alone does not fit.
    fn fit(&mut self, max_bytes: usize) -> bool;
}
```

- Each list has an order fixed by its type, and the end goes first: query rows by `EntityId`,
  problems and diagnostics by file and line, events oldest first (the newest kept), documentation
  sections by rank. Long strings are cut at a word boundary and end with " [cut]".
- `omitted` says per list how many were left out and names the parameter that narrows or pages it
  ("pass `limit` and `since`", "narrow `with`"), as perception's omitted section does.
- `fit` depends on the answer and the budget only, so it is deterministic; when it fails,
  `session.budget_too_small`.

### 8.3 Defaults

| Tools | Default `budget_tokens` |
|---|---|
| Perception and time tools, `intents` | the seat's profile's `budget_tokens` (perception.md) |
| `describe` of the game definition | 2,000: the sailing definition (20 instruments, 3 intents with their schemas, 4 kinds, its events and codes) cannot fit a profile's 400; measured in slice 2 and held by M4 and M9 |
| `world_query`, `world_get`, `docs_search` | 2,000 |
| `replay`, `project_brief`, `schema`, `apply`, `checks`, `profile`, `eval`, `why` | 1,500 |
| `session {all}`, `assets` | 1,000 |
| `session`, `fork`, `discard`, `snapshot`, `restore`, `world_edit`, `scene_save`, `time_control`, `reset`, `asset_import` | none: under 1,000 by construction |

The defaults are proposals for the player benchmark to tune; none was measured in slice 0. Master's
cut was 24,000 bytes, about 6,000 tokens, and its agents asked for less when told how.

### 8.4 The tool list

Every listed tool's name, description and schema reach the model in every turn. Descriptions come
from `pocket-contract`'s tool table (7.2), at most 300 bytes; field descriptions at most 120 bytes.
A stepped player's list must fit 3,000 tokens and a developer's 9,000, both proposed and not
measured (no tool list exists yet); slice 2 measures them and test M9 holds them.

## 9. Determinism

- **Reads are pure.** A read tool changes no world, journal or random stream (perception.md,
  Queries; `contract.perception.read_only`). Developer reads decode a snapshot or run in a throwaway
  fork.
- **Writes are inputs.** `act`, `world_edit` and `apply` (through the `scripts.swap` it produces)
  are Write commands, recorded in their canonical form with the boundary they applied at; `restore`
  and `reset` end the world's recording segment and open the next with a `Rebase`
  (`docs/spec/replay.md`, 2.5), so replaying the recording reproduces every world hash across them
  (spec-persist). `step`, `commit`, `wait`, `continue` and pausing are Controls: they decide when
  ticks run, not what they compute, and are not recorded (time.md, Requests).
- **Answers are functions of their inputs.** Same world, grant, parameters and session cursors: the
  same bytes outside `timing`. Wall-clock measurements appear only under `timing` (`apply`,
  `checks`, `profile`).
- **Wall time that cannot leak into a world.** Where a `step` stops on its wall limit changes only
  when the agent next looks (time.md, Checks, Wall limit). In real time the boundary an action lands
  on depends on the agent's thinking time; the recording keeps it, so the run replays exactly.
- **Branches are worlds.** A fork's determinism is spec-persist's; this file adds only that branch
  ids are allocated in order per runtime, so a session's transcript names the same branches when run
  again.

## 10. Transports, threads and the crate

- **Transports**: stdio, one session, for an agent that starts a runtime or attaches to one; and
  Streamable HTTP on 127.0.0.1, several sessions on one runtime (the benchmark, the editor), which
  this file asks `pocket-mcp` to export beside `serve_stdio` (`architecture.md`, 4.12). Both are
  `rmcp`'s (`transport-io`, `transport-streamable-http-server`). The handshake targets MCP
  2025-11-25 (`rmcp`'s `LATEST_WITH_INITIALIZE`); clients on 2026-07-28 are served as `rmcp`
  negotiates (open choice 3).
- **Attaching**: `pocket mcp --attach <url> --grant-token <token>` serves one stdio session bound to
  a grant of a running runtime; the benchmark harness gives agents exactly this command
  (`../benchmark/README.md`, The engine adapter).
- **Threads** (spec-arch's): the transport runs on `tokio` threads in `pocket-mcp`, which never
  links the runtime and never touches a world. A tool call becomes catalog commands sent through the
  session's `GameClient`; every Read is answered on the game thread (perception.md, open choice 6:
  not from a published snapshot in slice 2, since perception's queries need `pocket-interface` and
  physics' ray casts, which `pocket-mcp` does not link); Writes and Controls apply at boundaries in
  the queue's order (`threads.md`, 5.2). A slow agent never stalls a tick, and a long `step` keeps
  the queue served for other sessions (time.md, Threads and the web).
- **Branches** live in the `Game` beside main (`docs/spec/threads.md`, 3.6): stepped on the game
  thread between main's boundaries, in 8 ms slices while main runs in real time, or on a runtime
  worker with its own script host when a step is long. A branch made ready to step costs the budget
  `branch.sail`, the fork alone `fork.sail`, and main's lateness while a branch steps
  `branch.main_lateness` (`docs/spec/budgets.md`).
- **The web**: `pocket-mcp` is native only; in a browser the page's `window.pocket` is the same
  command surface (`threads.md`, 7.4). The tool layer's types and budget code have no transport
  dependency, so a relay can serve these tools for a browser-local game later (open choice 7).

## 11. Tests and checks

These run in the local check command (spec-arch) on each line, natively and, where marked, against
the `wasm32` build through `window.pocket`. Fixtures live in `shared/contract/conformance/mcp/` and
are synchronized like the prose, so both lines run the same ones. Perception, action, time and error
behaviour is held by the contract's own conformance checks (README, Conformance); these hold the MCP
layer.

| Id | What it proves | How |
|---|---|---|
| M1 | The tool list is the contract. | The whole `tools/list` of a stepped player, a checker and a developer session, for each pacing and policy of 3.3 (names, descriptions, annotations, input and output schemas, normalized with keys sorted), and the `initialize` result's `instructions` (none), equal the generated copies under `shared/contract/schema/mcp/`, as `contract.schema` holds the request types; the catalog's schemas for the same commands equal them too. The `intents` tool is listed for every role. |
| M2 | Unknown fields are refused, nothing applied. | For every tool, generated calls with an unknown key at each nesting depth, beside a flattened request included: `isError`, `request.unknown_field` with every unknown key (the rest in `detail.also`) at its JSON Pointer with its suggestions, the world hash and the recording unchanged. |
| M3 | Reads are pure. | On a seeded sailing fixture, every tool a checker may call, with fixture parameters: the hash, the recording and every random stream equal before and after. |
| M4 | Developer budgets hold. | Each budgeted developer tool at budgets from its minimum to 16,000: the tokens of the text blocks at most the budget, the text parses, shown items plus `omitted` equal the total, and a larger budget shows a superset. |
| M5 | Answers are deterministic. | The fixture session `session-*.jsonl` (a list of calls, its time requests with `max_wall_ms: 600000` so no step stops on the wall clock) twice on fresh runtimes with the same seed: answers byte-identical outside `timing`, with no `wall_limit` stop. The same calls through `window.pocket` in the browser: the same bytes for every projection tool (web). |
| M6 | MCP sessions replay. | M5's recording replayed, segment by segment: the per-tick world hashes equal the live run's, across a `restore` of a snapshot, a branch adopted as main and a `reset` in the session (each a `Rebase`). |
| M7 | Forks through MCP are consistent. | Charter 3.7's fork check driven only through `fork`, `act`, `step` and `world_get`: the same actions on main and branch give the same hashes; acting only on the branch leaves main's hash unchanged; the branch's push cursor starts at its parent's. |
| M8 | Roles hold. | Each role lists exactly the tools of 3.3 for each pacing and policy; every unlisted tool and every restricted parameter is refused with the code of 7.1 that says why (`permission.denied`, `time.wrong_mode`, `session.policy_denies`, `perception.omniscient_forbidden`, `seat.not_yours`); a checker's calls send only Read commands; a player with `fork` and no `rewind` restores its own branch from its own snapshot and is refused restoring main; with `rewind` it restores main; a player's `intents` shows only its seat's. |
| M9 | The tool list fits. | `tokens` of each role's `tools/list` text within 3,000 (stepped player) and 9,000 (developer) tokens; every description within its byte limit. |
| M10 | Both lines agree on meaning. | `conformance/mcp/*.jsonl`: calls with expected answers on the minimal sailing fixture, compared after normalization (ids mapped by name, floats within the contract's rounding). |
| M11 | MCP does not slow the game. | Release build, sailing fixture: an `observe` answers within the budget `mcp.observe`; tick time with a client calling `observe` in a loop stays within `mcp.tick_overhead` percent of tick time without one (charter 3.10). Both are rows of `docs/spec/budgets.md` (5.6), not measured in slice 0. |

## 12. Rollout

- **Slice 2** (the player interface): every player tool, `intents` included; for the benchmark's
  checkers `why`, `world_query`, `world_get` and `session {all}`; `world_edit`, `reset` and
  `time_control` for setting up tasks; grants, both transports, strict parameters, budgets; M1 to
  M11.
- **Slice 3**: `assets`, `asset_import`, `capture`, `look_around`.
- **Slice 5** (the editor and developer tools): the rest, unless pulled forward (open choice 2).

## 13. Open choices

1. **One shared types crate.** As README's open choice 1 recommends: `pocket-contract` under
   `shared/contract/rust/` holds this file's types with the contract's, which makes M1 hold by
   construction. Recommended.
2. **Developer tools before slice 5.** The developer benchmark (TypeScript against Lua, charter 6.3)
   needs `apply`, `project_brief`, `docs_search`, `schema` and `checks`, thin over slice 1's
   machinery. Recommended: pull them into slice 2.
3. **Protocol version.** MCP 2026-07-28 replaces the `initialize` handshake with per-request
   metadata. Recommended: serve 2025-11-25 and newer as `rmcp` negotiates, and record in each
   benchmark report which version each client spoke.
4. **One `describe`.** Settled with the contract: one tool with `entity` (perception.md's
   `DescribeRequest`) or `part` (README, The game definition), since agents reach for that word for
   both (5.2).
5. **`structuredContent` beside the text.** The contract's tools send their answer once (4.3); this
   file's tools send both forms, as MCP recommends. If the benchmark's proxy and traces show a
   client handing the model both, those tools send text only and keep `outputSchema` for
   documentation.
6. **Exact lookahead.** A player's branch simulates the true future, hidden state and random draws
   included, so lookahead is an oracle within what perception shows. Recommended for slice 2: exact
   forks, allowed by policy and bounded per decision (`branch_ticks_per_decision`, 600 in the
   benchmark), their effect measured and reported as "exact lookahead"; branches with reseeded
   streams (spec-persist would define them) only if the oracle makes tasks trivial.
7. **MCP for the browser-local form.** Recommended: none in slice 2; a relay over a WebSocket later,
   serving the same tools through `window.pocket`.
8. **Idempotent retries.** A client that retries a timed-out `act` would apply it twice. Not in
   slice 2; if traces show it, an optional client `request_id` remembered per session.
