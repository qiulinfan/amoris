# The shared contract

- Status: Draft, slice 0. Maintained in this repository's `shared/contract/`.
- Charter: 2.2, 3.1, 3.4, 3.5, 3.6, 3.7, 6.1, 6.2, 7 (items 8, 9, 10 and 12).
- Contract version: 0.2 (draft).

## What the contract is

The shared contract is the interface a running game presents to whoever plays it or checks it: an
LLM agent over MCP, an RL policy through the environment interface, a human through the rendered
image and the HUD, a benchmark checker. Amoris uses the same observations, actions, time modes and
errors for all clients. This lets benchmark configurations measure perception and action behaviour
through one interface.

The contract is part of the game's definition (charter 3.1). A game declares who can play it
(seats), what each player can perceive (observer profiles, instruments, kinds and their facts), what
each player can do (controls, intents, affordances), how time passes (its turn structure), and the
error codes its own rules add. The engine turns those declarations into the player-facing interface,
its JSON Schemas (charter 3.6) and the `describe` answer an agent reads first.

| File | What it fixes |
|---|---|
| `README.md` | Versioning, the contract record, the conventions every file uses (units, frame, numbers, names, entity references), seats and caller roles, the game definition, the conformance checks. |
| [`perception.md`](perception.md) | Observers, limited perception (occlusion, fog, attention range, memory, charts), its geometry, pull queries with token budgets, pushed event deltas, the marked omniscient view and the `omniscient_player` profile. |
| [`projection.md`](projection.md) | Rounding and stored rounded values, the JSON projection, the text projection's grammar (shared byte for byte), the tensor projection, the image. |
| [`actions.md`](actions.md) | Low-level controls, intents and their lifecycle, executors, affordances declared by entities, atomic validation, what a replay records. |
| [`time.md`](time.md) | Turn structure, stepped, lockstep and pausable real-time pacing, thinking-time budgets, decision points and pause-on-decision, `step`, `commit`, `wait` and `continue`, halts, episodes. |
| [`errors.md`](errors.md) | The `{code, message, detail}` protocol, the code families and table, refusal of unknown parameters and fields with "did you mean" suggestions, atomicity, warnings. |
| [`mcp.md`](mcp.md) | The MCP tools that carry these requests (spec-mcp's): sessions and grants, worlds and branches, snapshots and replay queries, developer tools, budgets outside perception. |
| [`sailing.md`](sailing.md) | The sailing showcase's declarations: the skipper's perception, the sailing controls and intents (come to a heading, trim the sail, sail to a waypoint) with their completion and failure, crates taken aboard, its time and its codes. |

The MCP tools that carry these requests and responses (their names, envelopes, sessions, fork,
restore and replay queries) and the benchmark are spec-mcp's (see References to other
specifications, below); the tools carry the types defined here unchanged.

## What is shared and what is not

Shared by all Amoris clients:

- The JSON shapes of every request and response in these files, their field names, units and value
  ranges, and the JSON Schemas generated from them.
- The semantics: what a query returns for a given world, when an action takes effect, what each
  intent does, when each code is returned.
- The text projection (projection.md, Text projection): the same observation renders to the same
  bytes across clients, because it is what an LLM reads and a difference in it would confound the
  comparison.
- The error codes, their detail fields and their message templates.

The contract does not define the crate layout, internal storage, script host, physics, or the byte
formats of snapshots, replays and hashes. Their specifications live under `docs/spec/`; public
answers use this contract's frame, unit and rounding.

## Versioning

The contract has one version, `MAJOR.MINOR`, stated at the top of every file and in the game
definition's `contract` field.

- A **minor** change adds without changing meaning: a new optional request field, a new response
  field, a new error code or detail field, a new intent, instrument or kind in a game, a new query.
  Clients MUST ignore response fields they do not know. The engine still refuses request fields it
  does not know (charter 3.4), so a client that sends a field only a newer engine takes gets
  `request.unknown_field` from an older one, which is the intended, explicit outcome.
- A **major** change removes or renames anything, changes a unit, a default, a range or the meaning
  of a field or code, or changes the text projection's grammar.
- While the major version is 0 (slice 0 to the first player benchmark), minor changes MAY break;
  each such change is listed in the version's entry below.

A request MAY carry `"contract": "0.1"`; an engine whose major version differs, or whose minor
version is lower, refuses it with `request.unsupported_version` (errors.md). Without the field the
engine's own version applies.

| Version | Change |
|---|---|
| 0.1 | First draft (2026-10-03), integrated the same day with the slice 0 specifications and spikes: names settled (`Problem`, `EventSeq`, `ActOrigin`, applied writes by `tick`), halts after a script failure (time.md), `describe` taking `entity` or `part` (mcp.md 5.2). Revised the same day after review: world state separated from wire forms; projections and the sailing declarations in files of their own; the `intents` tool; entity references in the projections' forms; the text grammar for every perception answer; undeclared events hidden; declared requested decisions and episodes; `omniscient_player`; one seat per body. |
| 0.2 | As built on Pioneer (2026-10-09, `docs/spec/player.md`; charter 5.1, Pioneer): `Observer.team` and team vision (perception.md, The perception update, step 5a); `wait` in stepped pacing, for the seat holding the clock, is a `step` until its next decision point (time.md, Requests); the player tools are the runtime's `player.*` commands, carried by one MCP tool `player` with an `action`, and a session is bound to a seat by the process that serves it (mcp.md 14); a game declares its players in `project.toml`'s `[player]`, and the declaration is world state (The game definition). |

## Contract record

The authoritative contract and benchmark live in this repository under `shared/contract/` and
`shared/benchmark/`. `shared/SYNC.toml` records only an optional reference `commit`
(`docs/spec/checks.md`, 5.4). An empty `commit` means no reference commit is recorded. Shared files
have no checksum baseline (owner, 2026-10-10; charter 3.9).

The procedure:

1. Edit the contract here, with the version bumped and the version table updated when the contract
   changes as described above.
2. Review shared changes with the contract's version and Git diff. Do not create file checksum
   records. Generated outputs are compared directly with what their generator writes now.
3. `contract.sync` is a planned comparison with the optional reference commit. It reports
   `Skipped` when no reference commit is recorded or the comparison is unavailable, never
   `Passed`. An implemented comparison reads the reference content from Git directly; it does
   not make another checkout the authority or add a persistent file checksum baseline.
4. Generated JSON Schemas belong under `shared/contract/schema/` (one file per request, response
   and declaration type, never edited by hand); the generation check reports `gen.stale` when
   they differ from what the generator writes now.

The contract's types-only crate, `pocket-contract`, lives under `shared/contract/rust/` and is used
by Amoris's workspace crates. One source of types keeps request and response schemas consistent.

## Conventions every file uses

### Rust listings

The listings omit what every type shares: types derive `Clone, Debug, Serialize, Deserialize` and,
when they cross the wire or are declared by a game, `JsonSchema`; ids used as map keys also derive
`PartialEq, Eq, PartialOrd, Ord, Hash`; request and declaration structs carry
`#[serde(deny_unknown_fields)]`; enums serialize in `snake_case`. Shown are only the attributes that
change the wire form (`tag`, `untagged`) and `#[derive(Component)]` or `#[derive(Resource)]` for
world state (`bevy_ecs`). `Option` fields are omitted from JSON when `None` unless a listing says
otherwise.

### World state and wire forms

A type marked `#[derive(Component)]` or `#[derive(Resource)]` is world state: hashed, snapshotted
and forked (`Controls`, `IntentTable`, `ObserverMemory`, `ObserverEvents`, `TurnState`). Its listing
gives its content. The serde attributes shown on it and its parts (untagged and internally tagged
enums, omitted `None` fields, `Problem.detail` as a JSON map) describe its **wire form**, used only
when an answer or a tool shows the value. Amoris persists world state in the form that the
contract does not share (What is shared, above), converting to the wire form only at the interface.
On Amoris the persisted forms are `docs/spec/persistence.md` 3.5's: every enum externally tagged,
every `Option` encoded, free-form data as `PlainData`, and an intent's failure stored as its code
and detail only, its message rendered from the code's template when the failure is shown, so no
English text enters the world hash.

### Names

- Fields, queries, controls, intents, instruments, facts, kinds, verbs, seats and event kinds are
  `snake_case` ASCII (`[a-z][a-z0-9_]*`). Event kinds and error codes are dotted (`crate.taken`,
  `request.unknown_field`).
- A numeric field's name ends in its unit (the Prometheus naming practice): an agent reading
  `heading_deg: 87` or `range_m: 412` never has to guess the unit, and two quantities of different
  units can never share a name.

| Suffix | Unit | Notes |
|---|---|---|
| `_m` | metres | Distances, positions, sizes. |
| `_mps` | metres per second | Speeds. |
| `_deg` | degrees | Bearings in [0, 360); relative angles in (-180, 180]. |
| `_s` | seconds | Durations and simulated times. |
| `_kg` | kilograms | Masses. |
| none | dimensionless | Fractions in [0, 1] or [-1, 1] (documented per field), counts, booleans, enumerations, `tick`. |

### The frame

The contract has its own world frame, which the runtime maps its internal frame onto: right-handed,
metres, `+y` up, the ground plane `x`-`z`, **north is `-z` and east is `+x`** (so a map drawn from
above has east to the right and north up).

- A **bearing** is a direction on the ground in degrees clockwise from north, in [0, 360): east is
  90, south 180, west 270. With `dx`, `dz` the offset on the ground, `bearing = atan2(dx, -dz)` in
  degrees, normalized to [0, 360). A **heading** is the bearing of an entity's forward axis
  projected on the ground.
- A **relative angle** is in (-180, 180], positive clockwise seen from above, which for a vessel is
  to starboard: `relative = normalize(bearing - heading)`.
- A wind direction is a bearing **from which** the wind blows (the meteorological and sailing
  convention: a wind from 270 is a westerly, blowing toward the east). Master's `Wind.direction` was
  the direction it blew toward, in degrees counter-clockwise from `+x` (master
  `docs/design/wind.md`, The component); the contract uses the convention weather reports and
  sailors use, which is the one an agent's corpus holds, and names it in the field
  (`wind_from_deg`).
- Angles are computed with the deterministic math library (spec-sim) wherever the result feeds world
  state or an intent executor; the projections then round them (below).
- Simulation state is in SI units and radians (`docs/spec/numeric.md`, 3.3); degrees are the
  contract's, converted at the interface: in the projections, and in `PerceptionView`, which intent
  executors and NPC rules read inside a tick (perception.md, Queries). That conversion uses the
  deterministic library too, so an executor's decisions are the same on every target.

### Numbers

- On the wire, numbers are JSON numbers and always finite. A request carrying a string where a
  number is expected is refused (`request.wrong_type`); there is no `NaN` or infinity in JSON, and
  none is produced.
- An integer field takes a JSON number with no fractional part (`3`, `3.0`), else
  `request.not_integer`. Integer fields are at most 2^53 - 1 so that every JSON parser reads them
  exactly.
- Requests are taken exactly as sent: an out-of-range value is refused with its range
  (`request.out_of_range`), never clamped or wrapped. A bearing of 360 is refused with the hint that
  it is 0.
- Output numbers are rounded to the precision their declaration gives (projection.md, Rounding), so
  an observation is compact, stable and identical across clients.

### Positions and entity references

- A position is an object `{"x": .., "y": .., "z": ..}` in metres in the contract frame (`Vec3`).
  Where a request is about the ground or the sea surface it takes `{"x": .., "z": ..}`, and a `y`
  given there is a declared field that is documented as unused, so a position copied from an
  observation is accepted as it is.
- An entity is identified by its `EntityId` (spec-sim), a JSON number from 1 to 2^53 - 1
  (`docs/spec/simulation.md`, 7.1). Wherever a request takes an entity it takes an `EntityRef`,
  which accepts the forms the projections write, so an agent can copy what it read: a number is an
  `EntityId`; a string matching `^(.*)#([0-9]+)$` (`Mark1#40`, `#40`, the text projection's form)
  names the entity with that id, and a name given before `#` must be that entity's name, else
  `perception.unknown_entity` (or `request.ambiguous_ref` when the name alone would be ambiguous and
  the id does not resolve); an object `{"id": .., "name": ..}` (the JSON projection's `EntityName`,
  `name` optional) is read the same way; any other string is the entity's name. Names are spec-sim's
  plain-data `Name(String)` component (`docs/spec/simulation.md`, 7.1), not necessarily unique. A
  name is resolved among the entities the caller may know (for a player, those it perceives,
  remembers or has on its chart: perception.md); a name two of them share is refused with
  `request.ambiguous_ref` and the candidates. A reference to an entity the caller may not know is
  answered exactly as one to an entity that does not exist (`perception.unknown_entity`), so an
  error never reveals a hidden entity.
- Observations write an entity as `Name#id` in the text projection and as `{"id": .., "name": ..}`
  in JSON. An intent id likewise: a request that takes an `IntentId` accepts the number `3` and the
  text projection's `"#3"`.

### Time

`tick` is the simulation tick (spec-sim's `Tick`, an unsigned integer from 0); `t_s` is
`tick / rate` with spec-sim's `TickRate` (`SimClock::time()`; 60 for the showcase). Every response
that depends on the world carries the `tick` it describes.

### Token estimates

Wherever the contract speaks of tokens it means the deterministic estimate
`tokens(bytes) = ceil(bytes / 4)` over the UTF-8 bytes of the projected output (perception.md, Token
budgets). It is the same for all clients and needs no tokenizer; the player benchmark (spec-mcp)
reports the real token counts beside it.

## Seats and callers

A **seat** is a place in the game for one player: the helm of a boat, a side in a strategy game.
Seats are declared by the game; a session (spec-mcp) attaches a caller to a seat (mcp.md 3.1: a
player's grant names its seat). A seat binds an observer profile (what the player perceives,
perception.md), a body (the entity it observes from and controls), the controls and intents it may
use (actions.md), and whether a human, an agent or either may take it. One seat per body: an
observer, the `Private` event scope and intent ownership all assume it, so two seats naming one body
fail the game's load with `definition.invalid`. A task that needs a seat with fewer intents uses a
game variant whose only seat lacks them (shared/benchmark/tasks.md 1, `open_sea_basic`).

```rust
/// A seat id: `[a-z][a-z0-9_]*`, unique in the game. Seats have a declared order, which is the
/// order actions from different seats are applied in within one boundary (actions.md); the game
/// thread's queue orders players' commands by the seat's index in it (`docs/spec/threads.md`, 5.1,
/// `Source::Player`).
pub struct SeatId(pub String);

pub struct SeatDef {
    pub id: SeatId,
    pub doc: String,
    /// The observer profile (perception.md) this seat perceives through.
    pub observer: String,
    /// The name of the entity the seat observes from and controls; spawned by the scene.
    pub body: String,
    /// Control names (actions.md) this seat may set.
    pub controls: Vec<String>,
    /// Intent names (actions.md) this seat may start.
    pub intents: Vec<String>,
    pub takers: Takers,
}

pub enum Takers { Human, Agent, Any }
```

Every request reaches the engine from a **caller** with one role. The role decides what the caller
may see and do; spec-mcp decides which tools a role is offered.

| Role | Sees | May do |
|---|---|---|
| `player` | Its own seat's perception only. | Act for its own seat; step, commit, wait and continue as the time mode allows (time.md). |
| `developer` | Anything, through the omniscient view, every answer marked `omniscient: true`. | Act for any seat (naming it), change the pacing, everything developer tools offer (spec-mcp). |
| `checker` | Anything, through the omniscient view, marked. | Read only: no actions, no stepping. Benchmark checkers use this role (charter 9.2). |

A player-role request that names another seat is refused with `seat.not_yours`; one that asks for
the omniscient view, with `perception.omniscient_forbidden`. A benchmark that measures the
difference the omniscient view makes (charter 9.2) binds the player's seat to the reserved profile
`omniscient_player` (perception.md, The omniscient view) through its grant, which only the process
owner gives (mcp.md 3.1); that seat's every response is marked, so the run cannot be mistaken for a
limited-perception one. The raw omniscient view, with engine component names, is for developers and
checkers only.

## The game definition

What a game declares for the contract, in one type. The engine answers `describe` (the tool is
spec-mcp's) with it: the first thing an agent reads, as master's `project.brief` and `env.describe`
were (master `docs/mcp.md`, Asking the engine how to call it; `docs/design/environment.md`,
Commands).

```rust
pub struct GameDefinition {
    pub contract: String,                    // "0.1"
    pub game: String,                        // "sailing"
    pub doc: String,                         // a paragraph: the goal, how it ends
    pub seats: Vec<SeatDef>,                 // in seat order
    pub observers: Vec<ObserverProfile>,     // perception.md
    pub instruments: Vec<InstrumentDef>,     // perception.md
    pub kinds: Vec<KindDef>,                 // perception.md, with affordances (actions.md)
    pub events: Vec<EventDef>,               // perception.md
    pub controls: Vec<ControlDef>,           // actions.md
    pub intents: Vec<IntentDef>,             // actions.md; params as JSON Schema
    pub structure: TurnStructure,            // time.md
    pub pacing: Vec<Pacing>,                 // time.md: the pacings it supports, default first
    pub decisions: Vec<DecisionDef>,         // time.md: the decisions its rules may request
    pub episode: EpisodeDef,                 // time.md: how an episode ends and what it reports
    pub codes: Vec<CodeDef>,                 // errors.md: the game's own codes
}

pub struct EpisodeDef {
    pub doc: String,                         // when the episode ends, in a sentence
    pub result: Vec<FactDef>,                // the readings EpisodeOutcome.result carries
}
```

Engine-provided parts (the sailing systems' instruments, intents and kinds) are declared as Rust
values beside their systems; a project's own parts are declared through the project definition,
whose syntax is spec-script's (project components and their declarations). Either way the engine
assembles one `GameDefinition`, validates it at load (every name referenced exists, names are
unique, units match suffixes) and refuses to start a game whose definition fails, with the problems
as `{code, message, detail}` objects (errors.md, code `definition.invalid`).

As built on Pioneer (0.2): a project declares its players with `[player]` in `project.toml`, naming
its perception file (profiles, instruments, kinds with their facts and affordances, events) and an
engine action catalog. That declaration is world state (`PlayerSpec`), so a snapshot, a fork and a
replay carry it and rebuild the definition from it (`docs/spec/player.md` 2 and 3). Intents a
project declares in its scripts are not built there yet.

`describe` (with `part`, or nothing; with `entity` it describes one entity, perception.md, Queries;
its parameters are mcp.md 5.2's `DescribeParams`) answers the definition filtered for the caller: a
player sees its own seat, its own profile and the kinds, controls and intents that seat can use. Its
size is bounded by the same token budget rule as observations, with a default budget of its own,
2,000 tokens (mcp.md 8.3): beyond the budget it lists names only and says which
`describe {"part": ...}` call gives the rest. The parts are `seats`, `instruments`, `controls`,
`intents`, `kinds`, `events`, `codes`, `structure`, `pacing`, `decisions` and `episode`, and `name`
selects one item of a part.

## Conformance

The local check command (spec-arch) runs these checks; their names are fixed here so reports
from different runtime configurations can be compared.

| Check | File | What it proves |
|---|---|---|
| `contract.sync` | README | The files equal the optional reference commit `shared/SYNC.toml` records. |
| `gen.stale` (spec-arch) | README | The generated JSON Schemas equal the committed ones. |
| `contract.perception.read_only` | perception | Observing never changes the world hash. |
| `contract.perception.deterministic` | perception | The same world, observer and request give the same bytes, also after fork and restore. |
| `contract.perception.noninterference` | perception | Changing what a player cannot perceive leaves its observation unchanged. |
| `contract.perception.budget` | perception | Every answer fits its token budget; a larger budget answers a superset. |
| `contract.projection.golden` | perception | Shared golden observations render to the shared golden text. |
| `contract.actions.atomic` | actions | A call with one bad action applies nothing. |
| `contract.actions.refusal` | actions, errors | Every misspelt field of every request type is refused with the right suggestion. |
| `contract.intents.transparency` | actions | An intent's control trace replayed as plain controls gives the same hashes. |
| `contract.time.pacing_equivalence` | time | Real-time, lockstep and stepped runs of the same applied actions give the same hashes. |
| `contract.errors.golden` | errors | The shared request cases give the shared codes, details and messages. |
| `contract.mcp.*` (M1 to M11) | mcp | The MCP layer: tool lists, strict parameters, pure reads, budgets, determinism, roles (mcp.md 11). |

The game-independent cases (projection golden files, error cases) live in
`shared/contract/conformance/` as JSON Lines, one case a line,
`{"case": name, "given": .., "expect": ..}`; they are written with the first implementation of each
part (slice 1 for errors, slice 2 for perception and actions) and recorded like the prose. The
sailing cases run against Amoris's sailing showcase.

## The sailing showcase in the contract

The first showcase (charter 2.4.1) exercises every file; its declarations are in
[sailing.md](sailing.md):

- Perception: the skipper's instruments (heading, speed, wind direction and strength, the point of
  sail, the sail's state), islands on the chart and in sight, marks, other boats and crates adrift,
  with islands hiding what lies behind them (sailing.md, The skipper's perception).
- Actions: `rudder`, `sheet`, `hoist` and `interact` controls; `come_to_heading`, `trim_sail` and
  `sail_to` intents with their completion and failure codes; crates that afford `take_aboard`
  (sailing.md, Actions).
- Time: continuous structure, pausable real time with pause-on-decision for a watched match, stepped
  pacing for tests and the benchmark (sailing.md, Time).
- Errors: the `sail.*` codes (sailing.md, Codes).

The sailing game itself (buoyancy, the sail's force, the keel) is simulation, specified with slice
1; the contract names what it must expose (`best_sheet`, the no-go angle, `afloat`) and nothing of
how it computes it.

## References to other specifications

These files use concepts other slice 0 specifications own; they are used here by name and not
redefined.

| Concept | Owner | File |
|---|---|---|
| `Tick`, `TickRate` and `SimClock`, the fixed timestep, the tick boundary, phases, system order (with this contract's `interface.intents`, `interface.perception` and `interface.turns` slots), `Event`, `EventSeq` and dispatch, `DecisionRequest`, `EntityId` and its wire form, `Name`, entity iteration order, the deterministic math library, numeric rules, the RNG | spec-sim | `docs/spec/simulation.md`, `docs/spec/numeric.md`, `docs/spec/rng.md` |
| Canonical serialization, the world hash and its section digests, snapshot, restore, fork, replay and its byte format, the first divergence, script and data versions | spec-persist | `docs/spec/persistence.md`, `docs/spec/replay.md`, `docs/spec/versions.md` |
| The script host API (`ctx.intents`, `ctx.reject`, `ctx.intent`), project components and their declaration syntax, script-side instruments and derived facts, `.d.ts` generation, structured script errors (`script.*`) | spec-script | `docs/spec/script-host.md`, `docs/spec/script-sandbox.md` |
| MCP tool names and envelopes, sessions, `describe`, `observe`, `act`, `step` and the other tools as tools, the benchmark, `session.*` codes | spec-mcp | `shared/contract/mcp.md`, `shared/benchmark/` |
| Crates, the game and editor threads, `Pace`, the command queue, its sources and ordering, `Applied` records, the local check command, performance budgets' framework, the web build | spec-arch | `docs/spec/architecture.md`, `docs/spec/threads.md`, `docs/spec/checks.md`, `docs/spec/budgets.md` |

Every specification uses this contract's names, as the slice 0 integration settled them: `Problem`
(once `PocketError` and `ContractError` in early drafts; the same object carries warnings and intent
failures), `ActRequest`, `AffordanceDef`, `IntentId`, `TurnStructure` with `Pacing`, the four
perception requests, and `SeatId` (a seat's index in the seat order where threads.md needs an
order).

## Open choices

1. **One shared types crate.** `pocket-contract` under `shared/contract/rust/` is the workspace
   source for contract types and their generated schemas (above).
2. **The text projection shared byte for byte.** Recommended, since it is what the LLM reads
   (perception.md). A stable projection keeps benchmark comparisons from changing the prompt.
3. **Declared aliases.** Recommended: a field MAY declare aliases, and every unit-suffixed field
   takes its bare stem (`heading` for `heading_deg`) unless that stem is ambiguous in its object; an
   alias is accepted and reported as the warning `request.alias_used`, never fuzzily inferred
   (errors.md, Aliases). Master's acceptance of any unique three-letter prefix (master
   `docs/mcp.md`, Asking the engine how to call it) is not carried over: adding a field later would
   silently change what an old call meant.
4. **Version handshake.** Recommended: the optional `contract` field on requests (above) plus the
   version in `describe`; no negotiation.
