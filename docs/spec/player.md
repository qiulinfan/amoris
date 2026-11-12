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
- Sources: `amoris-interface` is the rebuild line's (qiulinfan/amoris-pioneer `origin/rebuild`,
  commit `57ed4ea1`, Slice 2 wave A), brought over whole; its runtime glue was rewritten for this
  runtime. Amoris's Python gateway (`tools/eval/agent_gameplay.py` with
  `site/demos/agent-sailing/scripts/helm.ts`) showed what agents needed: an observation of their own
  boat, the wind and the cargo in range with bearings and distances, a way to steer, to sail to
  something, to take a crate, and to let time pass, with errors that say what to do instead.

## 1. What exists

| Piece | Where |
|---|---|
| Perception: observers and profiles, perceivables, occluders, the `interface.perception` update (range, field of view, occlusion by physics ray casts, fog, attention, memory and forgetting, sightings, event scopes), **team vision**, `PerceptionView`, pull queries with token budgets, push deltas, the marked omniscient view | `amoris-interface/src/perception/` |
| Projections: rounding, JSON, text (the LLM's), tensors (an RL observer's) | `amoris-interface/src/projection/` |
| Actions: controls, intents with lifecycle, channels and supersession, the Rust executors `come_to_heading`, `trim_sail`, `sail_to`, affordances (`take_aboard`), the `act` request validated in three phases | `amoris-interface/src/action/` |
| A game's own intents: `intent(name, def)` declarations, TypeScript executors and `ctx.intent` (script-host.md 5.5), merged into the action catalog at each swap and after a restore | `amoris-script/src/natives/intents.rs`, `scripts.rs`; `amoris-runtime/src/player.rs` |
| Time: turns and episodes, decision points and filters, `until`, the session controller (stepped, lockstep, real time with pause-on-decision and thinking clocks), `step`, `commit`, `continue`, the real-time `wait` | `amoris-interface/src/time/` |
| The declaration as world state, the three interface systems in every game, the `player.*` commands, the session beside the world | `amoris-runtime/src/player.rs`, `game.rs` |
| Players' waits on a loop, decision points after every tick, pace from the players' session: one core the game thread and the browser worker share | `amoris-runtime/src/boundary.rs` (`PlayerWaits`), `thread.rs`, `amoris-web/src/worker.rs` |
| The reference skipper (a decision model outside the tick) | `amoris-runtime/src/skipper.rs` |
| Fork lookahead for the skipper, the course's seeded variants, `Game::fork_into` | `amoris-runtime/src/lookahead.rs`, `game.rs` |
| Branches for an agent outside the process: `player.fork`, `.discard`, `.branches`, `world` (7.1) | `amoris-runtime/src/branches.rs`, `thread.rs` |
| The `player` MCP tool, seat-bound MCP sessions | `amoris-mcp`, `amoris-server/src/mcp.rs` |
| Calls made as a seat over HTTP, `amoris player`, `amoris mcp --seat` | `amoris-server`, `amoris-app` |
| The sailing course | `samples/sailing-course/` |

## 2. Declaring players

A project opts in with `[player]` in `project.toml`:

```toml
[player]
perception = "perception.json"   # default; the observer profiles, instruments, kinds, events
actions = "sailing"              # the engine's catalog: "sailing", "first_person", "third_person" or "none"
done_event = "course.finished"   # the episode ends in the tick one is emitted (optional)
camera = "look"                  # who aims the camera: "look" (default) or "game"
controls = "WASD move · Mouse look" # optional plain-text legend for the browser's pause veil
# decisions = { events = ["intent.", "sighted"], idle_s = 10, every_s = 120 }  # default: sailing's
```

- `perception.json` holds a `PerceptionDecl` (perception.md, Observers and What can be perceived;
  perception-slice2.md 3 for the `{"from": "data"}` source of event data): profiles, instruments,
  kinds with facts and affordances, events with their scope. It is decoded strictly and validated at
  load (`definition.invalid` with a JSON pointer at the first fault). Derived facts name the
  engine's registered functions (`sailing.*`, `perception/sailing.rs`).
- `camera = "game"` hands the native window's and browser player's camera to the game:
  `amoris play --walk` sends the
  mouse look and the keys to the seat as before but draws the scene's active `Camera` as the rules
  pose it, so a third-person or chase camera can follow a craft the seat does not sit in. The
  default, `"look"`, keeps the camera on the seat's head, turned at once by the mouse.
  The browser packages the camera choice and optional `controls` legend as presentation metadata;
  neither enters the persisted `PlayerSpec`. With a game camera it loads at the first completed
  tick, starts real time on the first click and pauses when the mouse is released. Embedded
  browsers that refuse pointer lock use keyboard play with drag-to-look; Escape or window blur
  pauses that mode (web-export.md 3).
- `actions` names an engine catalog (README, The game definition: engine parts are Rust values
  beside their systems). `sailing` is one seat, `skipper`; the controls `rudder`, `sheet`, `hoist`
  (a `Boat`'s) and `interact` (a pulse written into the project's `Crew.take`, which the game's rule
  reads and clears); the intents with their Rust executors; the crate's `take_aboard`; the `sail.*`
  codes. `first_person` is voxels.md 6's, `third_person` section 5.1's. `none` declares no action.
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
classes, game data). Since 2026-10-15 it keeps those already derived from the same spec (the world
remembers which, `DerivedFrom`): deriving them is a pure function of the spec, and doing it again
took 18 ms of the course's restore, a fork's whole cost (bench/player.md 5.2). `interface.intents`
(Control), `interface.perception` and `interface.turns` (Finish) are installed in every game, so a
replay built from a recording alone (`Game::for_replay`, an empty scene) gets the player layer back
from its start snapshot. Charter 5.1 (Pioneer) records the reason: a replay carries snapshots and
script bundles, nothing else; a declaration that lived only in the project's files would leave the
replay without perception and executors, and the hash chain unverifiable. The schedules of
`samples/anim` and `samples/sailing` gained the three rows and their world hashes did not change
(nothing inserted without a spec).

## 4. Perception as built

The rebuild line's update, as perception.md and perception-slice2.md describe it: per observer in
`EntityId` order, candidates in range (`min(sight.range_m, detect_m) * visibility_scale`), in the
field of view, and visible past occluders by one of two rays (to the origin and the top) cast with
`amoris_physics::query::cast_ray_among` against the colliders of `Occluder` entities other than the
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

The engine's executors are Rust: `come_to_heading` (an autopilot with an integral term),
`trim_sail` (the sheet to a value or to the best for the apparent wind), `sail_to` (straight or
beating, trimming the sail; it does not steer round land). Each sees only its seat's
`PerceptionView` and writes only its channels' controls (actions.md, Executors, rules 1 and 2), so
an intent is a convenience the seat could have done with controls (`contract.intents.transparency`,
the interface's tests). A game declares its own intents in TypeScript and carries them out with
script executors under the same two rules (script-host.md 5.5): the script host installs their
declarations into the catalog the engine's `[player] actions` names, and a player or an agent starts
them with `player.act` or an affordance, as any intent; `tests/fixtures/intents/errand` walks a
first-person seat to a waypoint and interacts there.

### 5.1 The `third_person` catalog

(2026-11-12; charter 5.1, the note of that day.) A character seen from behind: the walker
(voxels.md 5.2) moves and collides, its `Avatar` is the body seen standing at its feet, and a
`CameraRig` follows its look as an orbit camera (animation.md 6). Before it, `samples/thornkeep`
played through `first_person` with its own buttons for every verb and kept its lock-on in a
project component, and nothing let an agent move relative to anything but the camera.

`actions = "third_person"` in `[player]` declares one seat, `player`, whose body is the entity with
`Observer {seat: "player"}`, a `Walker` and a `ThirdPerson`. Its controls, with the keyboard and
mouse defaults the native window and the browser player send (the gamepad's, the touch controls
and a project's own bindings are input.md 3.1's):

| Control | Kind | Binding | Default |
|---|---|---|---|
| `forward`, `strafe` | Axis -1..1 | `ThirdPerson.forward`, `.strafe`: the move, in `frame` | W/S, D/A |
| `frame` | Choice `camera` (default), `world`, `body` | `ThirdPerson.frame` | — |
| `yaw` | Axis -pi..pi | `Walker.yaw`: the orbit's turn | the mouse |
| `pitch` | Axis -1.55..1.55 | `Walker.pitch`: the orbit's tilt | the mouse |
| `zoom` | Axis 0.5..2, default 1 | `ThirdPerson.zoom`: the arm's length over the rig's `distance_m` | the wheel, 10% a notch |
| `run` | Toggle | `Walker.run`: sprint | Shift |
| `guard` | Toggle (held) | `ThirdPerson.guard`: guard or aim, as the game's rules say | right button |
| `jump`, `primary`, `secondary` | Pulse | `Walker.jump`, `.primary`, `.secondary` | Space, left button, E |
| `dodge`, `interact` | Pulse | `ThirdPerson.dodge`, `.interact` | C, F |
| `lock` | Pulse | `ThirdPerson.lock_toggle`: lock on to the best target in front, or let go | Q, middle button |
| `lock_on` | Pulse, a target of any kind the perception declares | `ThirdPerson.lock_request` | — |
| `interact_with` | Pulse, a target of any kind the perception declares | `ThirdPerson.interact_target` | — |
| `held`, `pressed` | Axis 0..65535 | `Buttons` (animation.md 6): the game's other buttons | `[player] buttons` |

- **One system turns the controls into the walker's.** `physics.third_person` (Control phase, after
  `interface.intents`, so this tick's pulses are in) takes every walker with a `ThirdPerson`, in
  `EntityId` order: it reads `facing`, the yaw of the first `Avatar` (by id) standing for the
  walker, else the look's; settles the lock; while locked turns the look's yaw toward the target
  at most `lock_turn_rate` (8 rad/s) so the camera swings behind the line to it; and writes
  `Walker.forward` and `.strafe` as the move turned from its frame into the look's: `camera` as
  given (W walks away from the camera), `world` with forward toward north (-z) and strafe toward
  east (+x), so a percept's bearing b is `{forward: cos b, strafe: sin b}`, and `body` along the
  body's facing. Rules in the Update phase therefore read the walker's controls as `physics.walk`
  will use them; a third-person walker's `forward` and `strafe` are the engine's, not a rule's.
- **The lock.** `lock_toggle` lets go of a lock, or picks among the entities with `Lockable`, other
  than the walker and its avatars, within `lock_range_m` (20) of the eye, in front (the cosine of
  the bearing off the look's yaw at least 0.2, or within 4 m), and in sight of the eye past fixed
  colliders: the least `distance * (1.6 - cosine)`, the lower id on a tie. `lock_request` locks on
  to its target if it is `Lockable`, and is ignored otherwise; it is applied after `lock_toggle`. A
  lock ends when its target is gone, no longer `Lockable` (a game removes it from a fallen foe) or
  farther than 1.5 times `lock_range_m`. Rules may set or clear `ThirdPerson.lock` themselves.
- **Facing.** An avatar whose `facing` is `motion` faces its walker's lock while the walker is
  locked and not running, its motion otherwise; `target` and `fixed` are unchanged, so an attack's
  committed direction stays the rules'.
- **The camera.** The rig's arm is `distance_m * zoom`, at least `min_distance_m`, shortened at
  once to the first fixed collider its probe meets and lengthened again at `return_mps`: the
  pull-in against level geometry is the rig's, in the tick. Everything above is world state and
  enters the hash; every input that changes it is a recorded `player.act`.
- **Presentation.** The window and the page draw the rig's camera between its last two ticks, as
  every pose, and smooth its position with a time constant of 0.08 s (`Renderer::
  set_camera_smoothing`; Unreal's camera lag), leaving the rotation exact; a jump of more than 8 m
  (a respawn) is not smoothed. The smoothing is the presenter's alone: it never reaches the world,
  so it changes no hash. They send the mouse's look as `set {yaw, pitch}` and, while the mouse is
  still, take the game's camera look as their own (a lock turns it).
- **Pulses** `dodge`, `interact`, `lock_toggle`, `lock_request` and `interact_target` are cleared
  in the Finish phase of the tick they act in (`physics.walk.clear`), as the walker's are, so a rule
  sees each once. `ThirdPerson` and `Lockable` are persisted and script-usable; a rule removes a
  fallen foe's `Lockable` with `ctx.world.remove(e, "Lockable")`.
- **Agents.** The seat is played over MCP, HTTP and the CLI like the others. Perception derives
  `third_person.heading_deg` (the body's facing as a bearing; the instrument an affordance's
  `facing` requirement reads as `heading_deg`), `third_person.look_deg` (the camera's) and
  `third_person.locked`; a game names them as instruments. A game's affordances may pulse
  `lock_on` or `interact_with` on its kinds, which validates the target through the seat's own
  perception. No intents and no codes: what `primary`, `guard` or `interact` do is the game's.

Example: `samples/thornkeep`.

A person reaches the same controls through devices: the native window and the browser player
gather the keyboard, the mouse, gamepads and touch, map them through the project's input map and
send `player.act` as the local human player (input.md). Agents are unaffected: they act through
this layer, never see devices or bindings, and a recording names only the acts, whichever device
made them.

## 6. Time

The players' session is amoris-interface's `Controller`, kept by `Game` beside the world (session
state: never hashed, cloned into a fork such as Play's): the pacing, every seat's decision state
(filter, pending point, idle and heartbeat counters, push cursor), thinking clocks, the halt after a
script failure. Every seat of a built game starts with the episode's `start` decision point.

- **Stepped** (the default): the player holds the clock. `player.wait` is time.md's `step` with
  `until: "decision"` unless the request names another condition, at most `ticks` (default and
  largest 36000) ticks: time runs until the seat's next decision point (an intent finished or
  started holding, a sighting, a mark rounded, a crate aboard, ten idle seconds, a two-minute
  heartbeat: the sailing filter), the episode's end or a halt. Moving time on answers the pending
  decision. On a game driven directly the run is inline; on the game thread and in the browser's
  worker the loop asks its time model for the run's ticks and serves its queue between them. A
  developer's step and a wait under way at once share the ticks the model runs: it owes what the
  steps or the stepped waits still ask for, whichever is more (`boundary::owe`), and every tick
  counts against the model and the front step as it runs, before the players' hooks ask the model
  again (`boundary::ran_tick`). Until 2026-10-09 the step counted its tick only after those hooks,
  so a wait ending inside a step left the model owing one tick nobody asked for, on both loops (the
  thread's since the player layer, the worker's once it shared the thread's loop core); and the
  thread's `time.step` added its ticks to a wait's, which `status.steps_due` showed until the wait
  ended. When a loop stops (the thread's shutdown or panic, the worker's `close` or failure), the
  waits under way are answered `game.stopped` with the steps. Until 2026-10-09 the thread dropped
  them, its callers seeing a closed channel, and so did the worker once it held waits;
  `web/amoris.js` fails its own pending calls on `close` and `fatal`, so the page never hung on it.
- **Real time with pause-on-decision** (game thread and worker):
  `player.pacing {"pacing": {"pacing": "real_time", "speed": 4, "pause_on_decision": true, "clock": {...}}}`
  from a developer. The boundary's pace then comes from the players' session: a pending decision
  holds the world (as long as the seat's thinking clock has time, if it has one) until the seat
  answers it with `player.continue` or `player.act {resume: true}`; `player.wait` is the long poll
  that answers when the seat's decision comes (at once when one is pending), at its wall limit, or
  at the episode's end. Decision points are computed once after every tick, whatever ran it. The
  browser's worker does all of this as the game thread does: both drive
  `amoris_runtime::boundary::PlayerWaits` (waits under way, their stop and wall limits, the decision
  points after each tick, the pace) and `boundary::player_pacing`, with the runtime's `TimeModel`
  (the worker's copy of it is gone, threads-slice1.md 13), and the worker refuses a player the
  controls it answers itself (`step`, `time_control`, `snapshot`), as the thread does for its own.
- **The episode** ends in the tick the spec's `done_event` is emitted (`interface.turns`, Finish
  phase); its data are the result. Waits answer `stopped: "done"` with the outcome, and further acts
  and waits are refused with `time.episode_over`.

- **Halts** (time.md, Halts). A rule that throws reaches the tick's report as `sim.system_failed`
  whose `cause` is the script's problem (`script.exception`, a step limit, ...); the session halts
  on it as on any `script.*` problem (`amoris_interface::time::control::script_failure`). A
  player's wait then answers `stopped: "halted"` with the generic `time.halted`, a developer's with
  the failure itself, and real time holds. The halt ends when a hot update lands (every successful
  `scripts.swap`, so `scripts.apply`), when a developer resumes (`time.control {pause: false}` on
  the game thread, the runtime's `resume`), or with a restore (mcp.md 7.1). A developer's
  `time.step` is never held by it. Under `amoris serve`, whose edit world is real time paused,
  `{pause: false}` also starts real time; `{pause: true}` after it holds the world again with the
  halt ended.
- **The world replaced** (`snapshots.restore`, `debug.rewind`, which restores; every
  `Game::restore`). Decision state is session state about one timeline, so the session is rebased
  on the restored world (`Controller::world_replaced`): each seat's event cursor moves to the last
  event its restored observer perceived, its push cursor comes back to it at the latest, the
  pending decision is dropped, the heartbeat and idleness count from the restored tick at the
  latest, and at tick 0 every seat has the episode's `start` point again. A wait after a restore
  to tick 0 therefore stops at the sightings of tick 1 and carries them, as on a fresh world.

Lockstep pacing exists in amoris-interface (`commit`) but `player.pacing` refuses it: no command
carries `commit` yet (section 10).

## 7. The player tools

| Command | Kind | Answers |
|---|---|---|
| `player.session {seat?}` | Read | role, seat, the seat's index, contract version, pacing, `time.status` filtered for the caller, the pending decision, the push cursor, decision statistics |
| `player.describe {entity}` / `{part?, name?}` | Read | one entity as the seat knows it (facts, affordances with why not, latest events); or the game definition for the caller within 2000 tokens (names only beyond, with the call for each part) |
| `player.observe {budget_tokens?, projection?, since?}` | Read | instruments, active intents, the pending decision, ranked percepts, events since |
| `player.nearby`, `player.events`, `player.affordances`, `player.intents` | Read | perception.md and actions.md's answers |
| `player.blocks {seat?, range_m?, budget_tokens?, projection?}` | Read | the block cells the seat's eye sees, nearest first within budget, and the cell it looks at (voxels.md 6.1; the engine's, outside the contract) |
| `player.act` | Write | `ActResult` with the events delta since the seat's push cursor |
| `player.wait {until?, ticks?, max_wall_ms?, observe?}` | Control | time.md's `TimeResult` with the events delta (a developer's carries the world hash, a player's never) |
| `player.continue` | Control | answers the pending decision |
| `player.pacing {pacing}` | Control | a developer's: stepped, or real time |

The caller is the command's source: `Source::Player(i)` is a player at the declared seat of index
`i`, restricted to its own seat's perception and actions (`seat.not_yours`,
`perception.omniscient_forbidden`); every other source is a developer, who names a seat (or the only
one is meant) and may ask for the marked omniscient view.

The push cursor is the seat's own (perception.md, Push). The answers of `player.act`, `player.wait`
and `player.continue` carry the seat's events delta since it, and only the seat's player moves it:
a developer who names the seat reads the same delta and leaves the cursor where the seat's calls
put it, so watching or driving a seat never takes events from the player who plays it
(`amoris_interface::time::session::push_delta`; before 2026-10-09 a developer's call consumed them).

**The role is enforced by the engine** (charter 5.1, the Pioneer note after review). In a game that
declares players, a player source sends the `player.*` commands alone, and the game UI's
`ui.list` and `ui.press` (ui.md 6: the screen is every player's); every other command is a
developer's and is refused with `permission.denied` (`{request, role: "player", needs:
"developer"}`) before anything runs, in three places:

- the game (`amoris_runtime::player::permitted`, in `Game::apply` before the host-only and
  thread-only checks), for every catalog command by its name or an alias and for a game's own
  commands;
- the game thread, for the commands its loop answers itself (`time.step`, `time.control`,
  `play.*`, `snapshots.*`, `status`, `snapshot`), and the browser's worker for its own (`step`,
  `time_control`, `snapshot`);
- the host (`amoris_server::seat_permits`), for every call made as a seat (`Via::Player`): those
  the host answers without the game see no source at all (`events.since` lists every world event,
  `log.since`, `assets.list`, the debugger's methods, a read answered from the last publication
  while the debugger holds the game). `/api/call` refuses such a method before it looks the seat
  up, and a `seat` that is not a string with `request.wrong_type`; `amoris call --seat` refuses it
  before sending.

A replay applies what was recorded and is not asked. In a game without players, `Source::Player`
keeps slice 1's meaning: an input source ordered after developers (threads.md 5.2), as the web
form's human player and `samples/sailing`'s check inputs send `world_edit`; it names no seat and has
no perception to keep it to. The web form plays a game with players through `player.act` instead.

The commands are projected like every catalog command:

- **MCP**: the `player` tool,
  `{action: session|describe|observe|nearby|blocks|events|affordances|intents|act|wait|continue|pacing|fork|discard|branches, ...}`.
  Reads an LLM reads (observe, nearby, blocks, events, an entity described) come as the text
  projection unless the call asks for JSON, sent as the text itself. A session made with
  `amoris mcp <project> --seat skipper` (against a running host or in process) calls as the seat's
  player, lists the `player` tool alone, refuses the others with `permission.denied`, and gets
  player instructions.
- **HTTP**: `/api/call {method, params, seat}` makes the call from the seat's player source (the
  host asks the game for the seat's index once and keeps one client per seat).
- **Agent sessions** (2026-11-12; server.md 3.5): a seat-bound MCP session, and a bridge opened
  with `agent.session.open {seat}` (what `amoris mcp --seat` attached to a host is), are rows of
  role `player` in the host's list, and their `agent` events carry their `mcp-<n>` id (before, a
  seat-bound session's said `player:<seat>`). Their calls pass `seat_permits` before the seat is
  looked up, a refusal recorded as a call that failed; the seat's client is found by the session's
  first permitted call and kept. Calls made as a seat with no session (`/api/call {seat}`) are the
  row `player:<seat>`.
- **A locked host** (2026-11-12; server.md 3.6): on a host started with `--grants` the seat is
  the token's, not the request's. A player grant's token makes every call its seat's player's, over
  `/api/call`, over `/mcp` and `/mcp/<token>`, and through `amoris mcp --attach <url>
  --grant-token <token>`; another seat in the body is `seat.not_yours`, a session `handle` is
  `permission.denied`, and the catalog, the project's files, `/ws` and `/render` answer 403. The
  three places of the engine's own enforcement above stand unchanged beneath it.
- **CLI**: `amoris player <action> ['<json>'] [--seat s]`; `act` also takes the actions alone;
  observations print as text, other answers as one line of JSON. A client of a running host (the
  CLI, `amoris mcp` against a host) waits for a `wait` at least 10 s past its wall limit,
  `max_wall_ms` or the default 30 s, whatever `--timeout` says (`client::timeout_for`).

### 7.1 Branches for lookahead

An agent outside the process looks ahead as the in-process model does (8.1), through three more
player commands (2026-11-12; mcp.md 5.5), which the game thread answers itself
(`amoris_runtime::branches`; a game driven directly answers `command.thread_only`, and forks with
`Game::fork` instead):

| Command | Kind | Answers |
|---|---|---|
| `player.fork {world?, label?}` | Control | `{tick, world, parent, label?}`: a branch `b1`, `b2`, ... (never reused in the thread's life) of main or of one of the caller's branches; a developer's answer also carries the branch's `world_hash`, equal to its parent's |
| `player.discard {branch}` | Control | `{tick, world: "main", discarded}` |
| `player.branches` | Read | the caller's branches `{world, parent, forked_at, tick, label?}`, its `limits`, the branch ticks `left` for main's tick, and its `stats` (`forks`, `branch_ticks`, `branch_acts`) |

Every other player command takes `world` (default `main`): the game thread takes it out of the
parameters and applies the command to that branch, whose answer names it (`world` in a JSON answer,
a first line `world b1` before a text projection; mcp.md 4.1). Main's answers do not change.

- **A branch is the caller's.** It belongs to the command's source: a seat's player (every client
  of the seat, as the seat's decision state is shared, section 7), or one developer client. Another
  source naming it gets `session.unknown_world` with the worlds it has. A branch ends with
  `player.discard`, at the next fork after its source has no live client (the session that made it
  went away), or when the game thread stops.
- **What a branch is.** `Game::fork_branch`: main's snapshot restored into a new game with main's
  scripts and the players' session copied, its pacing stepped whatever main's is. It records
  nothing, publishes nothing, feeds no presenter and pushes no event: nothing it does reaches
  main's world, history, recording or streams. A branch runs the true world, hidden state included,
  and a player reads it only through its seat's perception (8.1, Perception stays honest); a
  player's answers there carry no hash, as on main.
- **Determinism.** A fork's world hash is its parent's, and a branch is stepped by the same tick
  code as main: the same calls on a branch and then on main give the same ticks and the same hashes
  (`crates/amoris-runtime/tests/thread_branches.rs`). The bounds below never stop a run partway, so
  they never change what a run computes.
- **When it runs.** A branch's `wait` runs inline on the game thread between two of main's
  boundaries, while main is stepped, paused or held for a seat's decision (real time with pause on
  decision); while main runs in real time it is refused with `time.wrong_mode`, since no branch
  tick may hold main's (threads.md 3.6's worker slices are not built). Forks and reads are
  answered in any pacing. `player.pacing` on a branch is `request.invalid_value`.
- **The bounds** (the runtime's, `branches::{PER_SOURCE, LIVE, PER_DECISION}`, the defaults of
  mcp.md 3.1's `Policy` and the benchmark's fork condition): at most 4 live branches a source and
  16 on the thread (`session.limit_reached {what: "branches"}`), and at most 600 branch ticks a
  source simulates, over all its branches, while main stays at one tick; the count starts again
  when main's tick moves. A `wait` on a branch whose `ticks` exceed what is left is refused before
  anything runs with `session.limit_reached {what: "branch_ticks", limit, value, asked, left}`; one
  that names no `ticks` runs at most what is left.
- **Who may fork.** On a host without grants every client may (such a host trusts its local
  clients, server.md 3.6); a checker never does (the fork is a Control); a developer always does. On
  a locked host a player grant forks only when its policy says so, `"policy": {"fork": true}` in
  the grants file (server.md 3.6), and is refused `session.policy_denies {request: "fork", policy:
  "fork"}` otherwise, as mcp.md 3.1's default for a player is.
- **Projections.** MCP: the `player` tool's actions `fork`, `discard` and `branches`, and `world`
  on every action; CLI: `amoris player fork|discard|branches` and `'{"world": "b1", ...}'`; HTTP:
  `/api/call` with the method.

Not built from mcp.md: snapshots and `replay` for players, `world` in main's answers, statistics
across sessions (section 10), branch ticks off the game thread (threads.md 3.6); of its grants,
`profile`, `decision`, `clock`, `max_wall_ms` and of `policy` all but `fork` (its bounds are the
runtime's, above). A decision model in process forks a game directly instead (section 8.1).

## 8. The sailing course

`samples/sailing-course`: the Sloop (the `skipper` seat) starts running east before a 6 m/s
westerly; three marks to round in order (the rule `rounding` rounds the next one when the boat comes
within 15 m, emits `mark.rounded`, and after the last `course.finished`, which ends the episode);
the Isle, an occluding island that hides Mark3 and Crate4 from the start; four crates the crew takes
aboard through the interact pulse (`take_aboard`, within 3 m). Its `check.toml` runs determinism,
fork, replay and reload equivalence with a player scenario: the skipper's acts as players' Writes
beside a developer's edit of the wind.

The **reference skipper** (`amoris_runtime::skipper::Skipper`) plays it through the player tools
alone: at each decision point it observes (JSON, budget 1500), takes aboard a crate it can, else
sails to the nearest crate it sees within 45 m that is not upwind (twice at most per crate), else
sails to the course's next mark (named from the chart before it is in sight), and waits. It finishes
the course at every seat seed in 12 decisions, 9 acts and 5148 ticks (86 s of game time), with two
crates aboard; [bench/player.md](../bench/player.md) has its costs and token sizes.

### 8.1 Fork lookahead

`amoris_runtime::lookahead::Lookahead` (Pioneer, 2026-10-15) is the reference skipper with exact
lookahead (mcp.md 5.5) under the benchmark's fork condition (shared/benchmark/tasks.md 6.1: at most
4 live branches and 600 branch ticks between two decision points answered on main; `Limits`). At
each decision point on main it reads the seat's observation (JSON, budget 1500) and:

1. **Builds candidates from main's observation alone**, each leaving the boat sailing somewhere
   else, at most one per branch: the greedy skipper's own act first, then sailing to the next mark
   (skipping crates), then each crate in sight within 120 m, nearest first, that the boat has set
   out for fewer than twice (the greedy skipper's limit). A crate alongside is taken aboard and a
   single candidate is acted on, without branches; so is the greedy act while the boat is aground,
   when every `sail_to` fails at once and no branch could tell the plans apart.
2. **Plays each candidate in a branch of main for an equal share of the 600 ticks** (300, 200 or
   150), as the seat: the candidate's act, then a rollout policy at the branch's decision points
   (the greedy skipper for its candidate; for a crate, sail to it, twice at most, take it aboard,
   then the mark; for the mark, take aboard only a crate that comes alongside), with `player.wait
   {ticks, observe}` up to the share. Branch games are kept between decisions and forked into
   again (`Game::fork_into`: main's snapshot restored into the branch and the players' session
   copied, the branch's script host kept), the first of each slot forked fresh (`Game::fork`).
3. **Scores each branch's last observation** in metres of course: 58.3 m a crate aboard (1000
   ticks of course time at 3.5 m/s, the reference skipper's mean speed along the legs), less the
   charted course still to sail (to the next mark, then the legs after it), less 1000 m aground. An
   act whose answer in the branch warns that its course crosses land before its target
   (`sail.land_on_course`: `sail_to` does not steer round land) is charged as aground: a branch
   shows the warning before main commits to the act. A chase under way at the end is credited with
   its crate when the closing speed measured in the branch would bring the crate alongside within
   two minutes, less that time and the course from the crate on, if that beats sailing on; a
   finished course adds the time left in the branch.
4. **Chooses** the best branch if it beats the greedy candidate's by 15 m (branches of a few
   seconds from a standing start differ by centimetres; the margin was chosen between 5 and 15 m on
   variants the comparison does not use), else the greedy act, and returns the act; the caller
   sends it on main (`lookahead::play` is the loop: observe, decide, act, wait). A candidate whose
   first act its branch refused is never chosen: main would refuse it too.

**Fork off** (`Lookahead::without_forks`) is the control that measures what forking adds: the
same candidates, rules and margin, each candidate valued from main's observation and the chart
alone, in the same metres: a crate in sight is worth its 58.3 m less the straight course to it, on
to the next mark and the legs after it; any other candidate the straight course on. It knows
neither the wind nor the land nor how fast the boat closes on a crate, which is what a branch
measures, and it never forks. Fork on minus fork off is the benchmark's own difference
(tasks.md 6.4).

**Perception stays honest by construction.** `decide` borrows main immutably and only forks it,
so a branch cannot reach main (the test compares main's hash, tick and session around every
decision). The model reaches a branch through `Seat`, which sends `player.*` commands alone as the
seat's `Source::Player`; the runtime refuses a player any other command and the omniscient view
(section 7), so every answer a branch gives is what the seat would observe in that future through
its own profile. Candidates come from main's observation, so something a branch revealed (a crate
behind the Isle) ranks candidates but never becomes a target on main; a rollout may still chase it
inside its branch, as the seat would. Hidden state is simulated: a branch runs the true world,
which the contract calls exact lookahead, bounded by the 600 ticks per decision so a branch cannot
scout the course. The bounds are counted, not trusted: the model's statistics carry its forks,
branch ticks and the most branch ticks of one decision, and the test holds them to 4 and 600.

**The course's seeded variants** (the bench's `examples/lookahead_bench/course.rs`, which
`tests/lookahead.rs` includes) are the engine's own (first written on Pioneer), not the shared
benchmark's seeds: a developer's `world.edit` at tick 0 (not the benchmark's `new --params` and
setup step, tasks.md 6.1) sets the
wind from `270 + 40 (r - 0.5)` degrees at `5 + 2 r` m/s (tasks.md 6.2's formula, but SplitMix64
draws from the seed rather than Python's `random.Random`) and places each crate, by this bench's own
rule, at a point a fraction `r` of the course's length from the start through the marks,
`80 (r - 0.5)` metres to its side, drawn again when within 25 m of the Isle's centre or 30 m of the
start (the start, marks and Isle read from the loaded course). Seed 0 is the course as shipped; the
omniscient condition sets the Sloop's `Observer.omniscient` in the same edit, which binds the seat
to the reserved `omniscient_player` profile. [bench/player.md](../bench/player.md) section 5
compares the three models on the variants 0 to 30. Fork on minus fork off, the benefit of the forks
themselves, is negative on the bench's score: with the same candidates, the lookahead took 73 crates
under perception to fork off's 98 and scored worse on 21 variants and better on 6, though it was
faster on 16 and slower on 11 by time alone; branches of 150 to 300 ticks rarely see a chase pay
that the chart shows. Against the reference skipper the lookahead took 73 crates to 58, better on 22
variants and worse on 3, a gain fork off makes larger without forking (better on 29); its paired
median time is 83 ticks better under perception and level omniscient.

## 9. Determinism

All of the player layer that runs in a tick is tick code under the determinism lists
(`tools/clippy-determinism.toml`): iteration in `EntityId` order (team vision merges into a
`BTreeMap` by id), the deterministic math library, physics' ray casts for occlusion, no wall clock.
The wall clock reaches only the players' session (wall limits, thinking clocks), which decides where
a run stops and when real time resumes, never what a tick computes; decision points and filters,
push cursors and pacing are session state. The course's recorded run replays to the same hash at
every tick, and its player scenario passes the four checks.

## 10. Not built, and what was learnt

- **The transparency check for a script executor**: `contract.intents.transparency` runs for the
  Rust executors only; a script executor keeps the same rules by construction (its context has no
  world, and its control writes are checked against its channels), not yet by the check.
- **Lockstep for players, branches, snapshots and `replay` for players, a grant's policy**:
  amoris-interface has lockstep's `commit`; nothing carries it. An agent outside the process
  forks, steps and discards branches since 2026-11-12 (7.1), inline on the game thread with the
  runtime's bounds; a grant's policy says only whether a player may fork, and the game thread runs
  no branch beside main in real time (threads.md 3.6, `branch.main_lateness`).
- **A grounded boat decides every other tick.** `sail_to` does not steer round land; once the
  Sloop runs aground on the Isle every `sail_to` fails at once (`sail.aground`), the failure is a
  decision point, and the reference skipper starts its mark again two ticks later until the boat
  drifts free, if it does: 1,997 decisions and 24,633 ticks on the course's variant 18, 1,105 and
  15,799 on variant 28 (bench/player.md 5.1); on variant 107 of the tuning seeds the reference
  skipper had not finished at 36,000 ticks, Mark2 still to round, after 799 decisions (whether it
  stayed aground was not recorded). The fork lookahead lets the greedy act stand while aground and
  charges a branch whose act is warned that its course crosses land (`sail.land_on_course`) as if
  aground, but a chase can still leave the boat where every course crosses the Isle (omniscient
  variant 18). A skipper that steered round land, or `sail_to` doing so (actions.md, open choice
  3), would remove the failure.
- **An omniscient seat has fewer decision points.** Bound to `omniscient_player`, the seat knows
  every crate from the first tick, so the sailing filter's `sighted` points never come; the
  reference skipper, which only turns for a crate at a decision point, sails past crates it would
  have chased under perception (the shipped course: no crate aboard against two). The benchmark's
  omniscient condition measures this filter as much as the information (bench/player.md 5).
- **Perception cost grows with observers times candidates**: two rays per candidate in range, even
  where no occluder lies between. With 32 observing boats the update took about 5 ms a tick
  (provisional, bench/player.md); testing the ray against the occluders' bounds first is the obvious
  next step.
- **`sail_to` does not steer round land** (actions.md, open choice 3).
- **The web build**: the package carries `PlayerSpec`. One browser run of a player game was made
  (headless Chrome, the 2D map,
  [evidence/player/web-form.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/player/web-form.json)):
  the form's helm sets the skipper's controls through `player.act`, and the same player's
  `world_edit` and `world.get` are refused in the worker as natively; `samples/sailing`, without
  players, still takes the helm as `world_edit`. No agent played in the browser. Until 2026-10-09
  the worker never fed the players' session: a `player.wait` ran inline through `Game::apply`, up to
  36,000 ticks in one task, with no publication and no tick hashes for those ticks; `player.pacing`
  real time set the session's pacing but not the worker's model, so no tick ran and every wait was
  refused (`command.thread_only`); and decision points never followed the worker's own ticks. The
  worker now drives the loop core the thread does (section 6); its tests run the course's package
  natively (`amoris-web/tests/worker_players.rs`). A browser run of an agent through the worker is
  still to be made.
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
- **An adversarial review found three gaps the first version shipped with** (all fixed, each with
  a test that fails when the gap is put back):
  - *The player role was the MCP layer's alone.* Only a seat-bound MCP session's tool list kept a
    player to the `player` tool; `/api/call {seat}` with `world.query`, `world.get {Crate4}` (an
    entity the seat cannot see), `world.edit` and `time.step` were all answered, unmarked. The
    runtime and the host now refuse them (section 7).
  - *Halts never fired.* The controller looked for a problem of the `script` family, but this
    runtime reports a throwing rule as `sim.system_failed` with the script's error as its cause,
    and nothing ended a halt (no hot update or developer resume reached the session). The
    synthetic `script.failed` the interface's pacing test injected hid both.
  - *A restore left the session on the abandoned timeline.* The seat's event cursor stayed past the
    restored ring's sequence numbers, so after `snapshots.restore {tick: 0}` a wait ran 120 ticks
    past the tick-1 sightings with no decision and no events until the ring caught up.

## 11. Tests and checks

| What | Where |
|---|---|
| The rebuild line's interface tests: visibility edges, noninterference (proptest), budgets, golden projections, actions, lifecycle, transparency, pacing, time | `crates/amoris-interface/tests/` |
| Team vision | `amoris-interface/tests/perception_team.rs` |
| A player is its seat; a game without players says so; one bad action applies nothing; the scripted agent plays the course through the player tools and the run replays | `amoris-runtime/tests/player.rs` |
| A stepped wait on the thread; real time paused on each decision; the skipper finishes through a player client of the thread, after which waits are refused and a developer still steps; a wait ending inside a developer's step leaves no tick owed; a shutdown answers a wait under way `game.stopped` | `amoris-runtime/tests/thread_player.rs` |
| The same in the browser's worker, natively: a stepped wait runs its ticks through the loop (every tick's hash, snapshots and a developer's read meanwhile); decision points after a developer's step; real time paused on each decision, the long poll and its wall limit; a player refused the loop's controls; a wait ending inside a developer's step leaves no tick owed; `close` answers a wait under way `game.stopped` | `amoris-web/tests/worker_players.rs` |
| A tick counts against the model and the front step before the model is asked again | `amoris-runtime/src/boundary.rs` (tests) |
| A developer naming a seat leaves its push cursor | `amoris-runtime/tests/player.rs` |
| A player's developer calls refused by the game and the thread; a throwing rule halts the players until a hot update, a developer's resume or a restore; a restore rebases the seat's decisions and push cursor | `amoris-runtime/tests/player.rs`, `amoris-runtime/tests/player_session.rs`, `amoris-interface/src/time/control.rs` (tests) |
| A seat over HTTP and a seat-bound MCP session (in process, newline JSON-RPC over a pipe) play through the player tools alone; the host refuses the game's and its own developer methods; an unknown seat says `seat.unknown` | `amoris-app/tests/player_host.rs` |
| A locked host: a player grant's token is its seat whatever the request says, over HTTP, MCP by header and by path, the CLI and `amoris mcp --attach --grant-token`; a checker's reads alone; an unlocked host unchanged; the same world hash locked and unlocked | `amoris-app/tests/locked_host.rs` |
| The review's three findings replayed against the release `amoris serve` over HTTP | `tools/host/player_http_probe.py`, [evidence/player/seat-http.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/player/seat-http.txt) |
| Fork lookahead: a decision leaves main's hash, tick and session untouched and keeps to 4 branches and 600 ticks; the same variant decides the same twice, with kept branches as with fresh forks; a branch is reached as the seat alone; a kept branch forked into again matches a fresh fork; `fork_into` refuses a game that is not a branch of main; with forks off the model never forks; the variants follow their seed | `amoris-runtime/tests/lookahead.rs` |
| The lookahead's choice needs the margin and never takes a branch that refused its first act; the fork-off estimate reads the chart alone | `amoris-runtime/src/lookahead.rs` (tests) |
| A restore keeps the declarations only for the spec they came from: another spec's snapshot derives its own, one without a spec removes them | `amoris-runtime/tests/player.rs` |
| The course's schedule | `amoris-runtime/tests/schedule.rs` |
| The player scenario passes the four checks | `amoris-check/tests/checks.rs`, `samples/sailing-course/check.toml` |
| The `player` tool's routing; `amoris player`'s forms | `amoris-mcp/src/tools.rs`, `amoris-app/src/client.rs` |
| `third_person` (5.1): moves turn from their frame into the look's; the toggle picks the nearest lockable in front and in sight; a lock turns the look and the body and ends with its target; the arm follows the zoom and is pulled in at a wall; the same inputs give the same world | `amoris-physics` `third_person` unit tests, `--test third_person` |
| `third_person`: the catalog's controls in reading order; yaws as bearings | `amoris-interface` `action::third_person`, `perception::third_person` tests |
| `third_person` end to end: a project's seat moves in the camera, world and body frames, sprints, guards, dodges, interacts, locks on by toggle and through a kind's affordance, perceives its heading, look and lock; the rules see each pulse once; the recorded run replays identically | `amoris-runtime --features transpile --test third_person` |
| Thornkeep's third-person seat over HTTP walks north in the world frame, locks on to a foe it sees through `lock_on` and swings; over MCP it dodges and reads its instruments | `amoris-app/tests/player_host.rs` |
| The camera lag follows with `1 - exp(-dt / tau)`, at most a quarter second a frame, and jumps a cut | `amoris-render --lib camera_smoothing` |
| Thornkeep in the browser player: the seat walks, sprints, orbits, zooms, guards, dodges, swings and locks on, the drawn camera near the rig's | `node tools/host/web_play_check.mjs <export> --third-person` |
