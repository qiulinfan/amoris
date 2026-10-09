# The player layer

- Status: built (Pioneer, 2026-10-09), track `player`. Charter 2 (proof target 2: agent-native
  gameplay), 3 (principles 2, 5 and 6), 5.1 (the Pioneer note on how the player layer is built).
- Contract: [perception.md](../../shared/contract/perception.md),
  [projection.md](../../shared/contract/projection.md),
  [actions.md](../../shared/contract/actions.md), [time.md](../../shared/contract/time.md),
  [mcp.md](../../shared/contract/mcp.md), [sailing.md](../../shared/contract/sailing.md), version
  0.2. The decisions the rebuild line took where the contract was open stay in
  [perception-slice2.md](perception-slice2.md), [actions-slice2.md](actions-slice2.md) and
  [time-slice2.md](time-slice2.md); this file says what this line built on top of them and how a
  game, an agent and a test reach it.
- Sources: `pocket-interface` is the rebuild line's (qiulinfan/amoris-pioneer `origin/rebuild`,
  commit `57ed4ea1`, Slice 2 wave A), brought over whole; its runtime glue was rewritten for this
  runtime. Amoris's Python gateway (`tools/eval/agent_gameplay.py` with
  `site/demos/agent-sailing/scripts/helm.ts`) showed what agents needed: an observation of their own
  boat, the wind and the cargo in range with bearings and distances, a way to steer, to sail to
  something, to take a crate, and to let time pass, with errors that say what to do instead.

## 1. What exists

| Piece | Where |
|---|---|
| Perception: observers and profiles, perceivables, occluders, the `interface.perception` update (range, field of view, occlusion by physics ray casts, fog, attention, memory and forgetting, sightings, event scopes), **team vision**, `PerceptionView`, pull queries with token budgets, push deltas, the marked omniscient view | `pocket-interface/src/perception/` |
| Projections: rounding, JSON, text (the LLM's), tensors (an RL observer's) | `pocket-interface/src/projection/` |
| Actions: controls, intents with lifecycle, channels and supersession, the Rust executors `come_to_heading`, `trim_sail`, `sail_to`, affordances (`take_aboard`), the `act` request validated in three phases | `pocket-interface/src/action/` |
| Time: turns and episodes, decision points and filters, `until`, the session controller (stepped, lockstep, real time with pause-on-decision and thinking clocks), `step`, `commit`, `continue`, the real-time `wait` | `pocket-interface/src/time/` |
| The declaration as world state, the three interface systems in every game, the `player.*` commands, the session beside the world | `pocket-runtime/src/player.rs`, `game.rs` |
| Players' waits on the game thread, pace from the players' session | `pocket-runtime/src/thread.rs` |
| The reference skipper (a decision model outside the tick) | `pocket-runtime/src/skipper.rs` |
| The `player` MCP tool, seat-bound MCP sessions | `pocket-mcp`, `pocket-server/src/mcp.rs` |
| Calls made as a seat over HTTP, `pocket player`, `pocket mcp --seat` | `pocket-server`, `pocket-app` |
| The sailing course | `samples/sailing-course/` |

## 2. Declaring players

A project opts in with `[player]` in `project.toml`:

```toml
[player]
perception = "perception.json"   # default; the observer profiles, instruments, kinds, events
actions = "sailing"              # the engine's action catalog: "sailing" or "none"
done_event = "course.finished"   # the episode ends in the tick one is emitted (optional)
# decisions = { events = ["intent.", "sighted"], idle_s = 10, every_s = 120 }  # default: sailing's
```

- `perception.json` holds a `PerceptionDecl` (perception.md, Observers and What can be perceived;
  perception-slice2.md 3 for the `{"from": "data"}` source of event data): profiles, instruments,
  kinds with facts and affordances, events with their scope. It is decoded strictly and validated at
  load (`definition.invalid` with a JSON pointer at the first fault). Derived facts name the
  engine's registered functions (`sailing.*`, `perception/sailing.rs`).
- `actions` names an engine catalog (README, The game definition: engine parts are Rust values
  beside their systems). `sailing` is one seat, `skipper`; the controls `rudder`, `sheet`, `hoist`
  (a `Boat`'s) and `interact` (a pulse written into the project's `Crew.take`, which the game's rule
  reads and clears); the intents with their Rust executors; the crate's `take_aboard`; the `sail.*`
  codes. `none` declares no action.
- The scene puts `Observer {profile, seat, team?}` on a seat's body,
  `Perceivable {kind, detect_m, height_m, priority, chart_m?}` on what can be perceived and
  `Occluder {}` on what blocks sight (its colliders); all three are engine components a scene or
  `world.edit` names.

A project without `[player]` is unchanged: the interface systems find nothing to do.

## 3. The declaration is world state

`PlayerSpec {perception, actions, decisions, done_event}` (the perception file's text as given) is a
persisted resource: hashed, snapshotted, forked and restored with the world. A rebuild registered
with the persistence registry (`player.declarations`) installs, after every restore, what the spec
declares: `PerceptionDefs`, the `ActionCatalog` with its field writer, and the `TimeRules` (Ignored
classes, game data). `interface.intents` (Control), `interface.perception` and `interface.turns`
(Finish) are installed in every game, so a replay built from a recording alone (`Game::for_replay`,
an empty scene) gets the player layer back from its start snapshot. Charter 5.1 (Pioneer) records
the reason: a replay carries snapshots and script bundles, nothing else; a declaration that lived
only in the project's files would leave the replay without perception and executors, and the hash
chain unverifiable. The schedules of `samples/anim` and `samples/sailing` gained the three rows and
their world hashes did not change (nothing inserted without a spec).

## 4. Perception as built

The rebuild line's update, as perception.md and perception-slice2.md describe it: per observer in
`EntityId` order, candidates in range (`min(sight.range_m, detect_m) * visibility_scale`), in the
field of view, and visible past occluders by one of two rays (to the origin and the top) cast with
`pocket_physics::query::cast_ray_among` against the colliders of `Occluder` entities other than the
observer and the target; `Full` detail within `attention_m`, `Coarse` beyond; memory with forgetting
by sight of the empty place, by `memory_s` and by capacity; `sighted` events for watched kinds;
world events pushed by scope (private, sight, sound, global, hidden).

**Team vision** (new here): `Observer.team`. Every observer's own visible set is computed first;
then each member of a team sees the union of its teammates' sets, each entity at the best detail any
member sees it. What is shared is what is seen at the end of the tick; memory, forgetting, the
sighting events, the event ring and the measures (bearings and ranges from the member's own body)
stay each observer's own, and an omniscient binding shares nothing. Not shared: sight-scoped events
a teammate saw (an observer perceives an event by its own senses), and the forgetting test (a
teammate seeing the empty place of a remembered entity does not make the observer forget it).

Answers are written in three projections: text (the LLM's, a few hundred tokens for the skipper),
JSON, and tensors for an RL observer whose profile declares a layout (the course's skipper does:
five instruments, the four nearest crates, marks or islands, eight rays). Every answer fits its
token budget (`ceil(bytes / 4)`), says what it left out and how to get it, and carries `omniscient`.

## 5. Actions

`player.act` takes actions.md's `ActRequest`: `set` controls, `pulse` a control, `start` an intent,
`use` an affordance, `cancel`, `end_turn`. It is validated whole in three phases (shape, names,
semantics: conflicts, affordance requirements, each intent's `accept`) against the world the call
applies to and through the seat's own perception; a refused call changes nothing and is answered
with every problem of the phase that found them, with suggestions. An accepted call is applied at
the boundary and recorded as the Write `player.act` in its canonical form (the seat named, defaults
filled, references as ids); a replay applies it through the same path as a developer naming the
recorded seat (actions.md, What a replay records: session-level checks are skipped).

Executors are Rust: `come_to_heading` (an autopilot with an integral term), `trim_sail` (the sheet
to a value or to the best for the apparent wind), `sail_to` (straight or beating, trimming the sail;
it does not steer round land). Each sees only its seat's `PerceptionView` and writes only its
channels' controls (actions.md, Executors, rules 1 and 2), so an intent is a convenience the seat
could have done with controls (`contract.intents.transparency`, the interface's tests). Script
executors are not built here (section 10).

## 6. Time

The players' session is pocket-interface's `Controller`, kept by `Game` beside the world (session
state: never hashed, cloned into a fork such as Play's): the pacing, every seat's decision state
(filter, pending point, idle and heartbeat counters, push cursor), thinking clocks, the halt after a
script failure. Every seat of a built game starts with the episode's `start` decision point.

- **Stepped** (the default): the player holds the clock. `player.wait` is time.md's `step` with
  `until: "decision"` unless the request names another condition, at most `ticks` (default and
  largest 36000) ticks: time runs until the seat's next decision point (an intent finished or
  started holding, a sighting, a mark rounded, a crate aboard, ten idle seconds, a two-minute
  heartbeat: the sailing filter), the episode's end or a halt. Moving time on answers the pending
  decision. On a game driven directly the run is inline; on the game thread the loop asks its time
  model for the run's ticks and serves its queue between them.
- **Real time with pause-on-decision** (game thread):
  `player.pacing {"pacing": {"pacing": "real_time", "speed": 4, "pause_on_decision": true, "clock": {...}}}`
  from a developer. The boundary's pace then comes from the players' session: a pending decision
  holds the world (as long as the seat's thinking clock has time, if it has one) until the seat
  answers it with `player.continue` or `player.act {resume: true}`; `player.wait` is the long poll
  that answers when the seat's decision comes (at once when one is pending), at its wall limit, or
  at the episode's end. Decision points are computed once after every tick, whatever ran it.
- **The episode** ends in the tick the spec's `done_event` is emitted (`interface.turns`, Finish
  phase); its data are the result. Waits answer `stopped: "done"` with the outcome, and further acts
  and waits are refused with `time.episode_over`.

Lockstep pacing exists in pocket-interface (`commit`) but `player.pacing` refuses it: no command
carries `commit` yet (section 10).

## 7. The player tools

| Command | Kind | Answers |
|---|---|---|
| `player.session {seat?}` | Read | role, seat, the seat's index, contract version, pacing, `time.status` filtered for the caller, the pending decision, the push cursor, decision statistics |
| `player.describe {entity}` / `{part?, name?}` | Read | one entity as the seat knows it (facts, affordances with why not, latest events); or the game definition for the caller within 2000 tokens (names only beyond, with the call for each part) |
| `player.observe {budget_tokens?, projection?, since?}` | Read | instruments, active intents, the pending decision, ranked percepts, events since |
| `player.nearby`, `player.events`, `player.affordances`, `player.intents` | Read | perception.md and actions.md's answers |
| `player.act` | Write | `ActResult` with the events delta since the seat's push cursor |
| `player.wait {until?, ticks?, max_wall_ms?, observe?}` | Control | time.md's `TimeResult` with the events delta (a developer's carries the world hash, a player's never) |
| `player.continue` | Control | answers the pending decision |
| `player.pacing {pacing}` | Control | a developer's: stepped, or real time |

The caller is the command's source: `Source::Player(i)` is a player at the declared seat of index
`i`, restricted to its own seat's perception and actions (`seat.not_yours`,
`perception.omniscient_forbidden`); every other source is a developer, who names a seat (or the only
one is meant) and may ask for the marked omniscient view.

The commands are projected like every catalog command:

- **MCP**: the `player` tool,
  `{action: session|describe|observe|nearby|events|affordances|intents|act|wait|continue|pacing, ...}`.
  Reads an LLM reads (observe, nearby, events, an entity described) come as the text projection
  unless the call asks for JSON, sent as the text itself. A session made with
  `pocket mcp <project> --seat skipper` (against a running host or in process) calls as the seat's
  player, lists the `player` tool alone, refuses the others with `permission.denied`, and gets
  player instructions.
- **HTTP**: `/api/call {method, params, seat}` makes the call from the seat's player source (the
  host asks the game for the seat's index once and keeps one client per seat).
- **CLI**: `pocket player <action> ['<json>'] [--seat s]`; `act` also takes the actions alone;
  observations print as text, other answers as one line of JSON.

Not built from mcp.md: grants with tokens over HTTP MCP, branches (`fork`, `discard`), snapshots and
`replay` for players, `world` in answers, statistics across sessions (section 10).

## 8. The sailing course

`samples/sailing-course`: the Sloop (the `skipper` seat) starts running east before a 6 m/s
westerly; three marks to round in order (the rule `rounding` rounds the next one when the boat comes
within 15 m, emits `mark.rounded`, and after the last `course.finished`, which ends the episode);
the Isle, an occluding island that hides Mark3 and Crate4 from the start; four crates the crew takes
aboard through the interact pulse (`take_aboard`, within 3 m). Its `check.toml` runs determinism,
fork, replay and reload equivalence with a player scenario: the skipper's acts as players' Writes
beside a developer's edit of the wind.

The **reference skipper** (`pocket_runtime::skipper::Skipper`) plays it through the player tools
alone: at each decision point it observes (JSON, budget 1500), takes aboard a crate it can, else
sails to the nearest crate it sees within 45 m that is not upwind (twice at most per crate), else
sails to the course's next mark (named from the chart before it is in sight), and waits. It finishes
the course at every seat seed in 12 decisions, 9 acts and 5148 ticks (86 s of game time), with two
crates aboard; [bench/player.md](../bench/player.md) has its costs and token sizes.

## 9. Determinism

All of the player layer that runs in a tick is tick code under the determinism lists
(`tools/clippy-determinism.toml`): iteration in `EntityId` order (team vision merges into a
`BTreeMap` by id), the deterministic math library, physics' ray casts for occlusion, no wall clock.
The wall clock reaches only the players' session (wall limits, thinking clocks), which decides where
a run stops and when real time resumes, never what a tick computes; decision points and filters,
push cursors and pacing are session state. The course's recorded run replays to the same hash at
every tick, and its player scenario passes the four checks.

## 10. Not built, and what was learnt

- **Script executors and `ctx.intent`**: `executor()` is still refused by the script host
  (`define.rs`) and `ctx.intent` throws `script.restricted`. The rebuild line had them (`57ed4ea1`:
  `pocket-script` natives, an interface bridge and executors, about 3,000 lines of source and 2,500
  of tests) against a script host this line has since changed (the debugger, types, the prelude);
  porting them is the next step. A game's own intents are therefore engine catalogs in Rust for now.
- **Lockstep for players, branches, snapshots and `replay` for players, grants over HTTP MCP**:
  pocket-interface has lockstep's `commit`; nothing carries it, nor mcp.md's fork tools.
- **Perception cost grows with observers times candidates**: two rays per candidate in range, even
  where no occluder lies between. With 32 observing boats the update took about 5 ms a tick
  (provisional, bench/player.md); testing the ray against the occluders' bounds first is the obvious
  next step.
- **`sail_to` does not steer round land** (actions.md, open choice 3).
- **The web build**: the package carries `PlayerSpec`, but no browser run of a player game was made.
- **A default budget cuts what a policy needs.** The first run of the reference skipper read the
  observation at the profile's 400 tokens, in which Mark2 was among the omitted percepts; it found
  no next mark, never acted again and waited through heartbeats for 2.3 million ticks before the
  test's limit. It now names the next mark from the `next_mark` instrument (the chart knows it) and
  reads JSON at 1500 tokens; an LLM would call `nearby {kinds: ["mark"]}`, which the omitted line
  names.
- **Real time did not pause at the start** until a built game attached its seats: the session made a
  seat's decision state, and with it the `start` point, only on the first player command, so a
  real-time run started before any had already run 180 ticks.
- The players' session decides the pace only for a game with a `PlayerSpec`: it halts real time
  after a script failure, which would have changed every other game's behaviour. Nor does it hold
  a developer's `time.step`: once the course was finished it held every tick, and a developer's
  step waited for ever; a step under way now runs whatever holds the players.
- A game without `[player]` answers every `player.*` command with `player.not_declared` rather than
  with whatever the first missing piece would have said (`request.missing_field` for the seat).

## 11. Tests and checks

| What | Where |
|---|---|
| The rebuild line's interface tests: visibility edges, noninterference (proptest), budgets, golden projections, actions, lifecycle, transparency, pacing, time | `crates/pocket-interface/tests/` |
| Team vision | `pocket-interface/tests/perception_team.rs` |
| A player is its seat; a game without players says so; one bad action applies nothing; the scripted agent plays the course through the player tools and the run replays | `pocket-runtime/tests/player.rs` |
| A stepped wait on the thread; real time paused on each decision; the skipper finishes through a player client of the thread, after which waits are refused and a developer still steps | `pocket-runtime/tests/thread_player.rs` |
| The course's schedule | `pocket-runtime/tests/schedule.rs` |
| The player scenario passes the four checks | `pocket-check/tests/checks.rs`, `samples/sailing-course/check.toml` |
| The `player` tool's routing; `pocket player`'s forms | `pocket-mcp/src/tools.rs`, `pocket-app/src/client.rs` |
