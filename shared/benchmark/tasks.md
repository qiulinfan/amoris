# Benchmark tasks

Status: Draft, slice 0. Maintained in Amoris's `shared/benchmark/`.

Charter: 2.4.1 (the sailing showcase), 3.1 (limited perception), 3.2 (all state in the world), 3.8
(narrow scope), 6.1 item 2 (a shared benchmark), 8 (material from `master`), 9 items 1 and 2 (the
developer and player benchmarks).

This file is the catalogue: the fixtures all configurations implement, how each of `master`'s 72 developer
tasks ports, the tasks added for this rebuild's principles, and the sailing tasks of the player
suite. [README.md](README.md) covers the harness that runs them. Tasks and checkers come from
`master` at `50514a2d` (`tools/scripts/agent_eval.py`, `docs/agent-eval.md`; the island sample,
`samples/island`); a commit that brings their code over names that commit (charter 8).

## 1. Fixtures

A fixture is a small Amoris project matching the description below. Runtime configurations use
the same scene format and TypeScript scripts. A fixture's numbers are part of the contract: checkers and briefs rely on
them. Every fixture keeps all its state in components (charter 3.2): where `master`'s samples kept a
score in a script variable, these keep it in a project component on a named entity. Each configuration tests
its fixtures with the reference solutions before a task may use them (README, Baselines).

Common to all: the contract's units, frame and names (`../contract/README.md`, Conventions every
file uses), time in simulated seconds at the fixed tick rate of 60 per second (spec-sim's default
`TickRate`). Every fixture but the sailing ones declares a seat `player` whose body is the entity
`Player` (when there is one) and whose controls are the axes `move_x`, `move_y`, `move_z` and the
pulses `jump` and `use`, bound for a human to A and D, S and W, W and S, Space and E, and set by an
agent through `act` (actions.md, Controls). Visual components are optional until slice 3: no check
before then reads them.

| Fixture | Contents |
|---|---|
| `blank` | An empty project: no entities, the input actions above, an empty entry script. |
| `hello` | `Ground`: a static box 20 by 1 by 20, top face at y 0. `Ball`: a dynamic sphere of radius 0.5, mass 1, which the entry script places at (0, 3, 0) when the game starts, from a constant named for its height. |
| `bodies` | `hello`'s ground and twelve dynamic bodies named `Body_0` to `Body_11` (spheres of radius 0.5 and boxes of side 1, alternating), dropped from y 2 at fixed places in a 12 by 12 square around the origin, listed in the fixture's scene. |
| `arena` | `Ground` 40 by 40. `Player`: a kinematic capsule (radius 0.4, height 1.8) the actions `move_x` and `move_z` move at 5 a second, with the project component `Health {current, max, invulnerable}` (60, 60, 0.5). `Enemy` entities: the script spawns one every 2 seconds at the arena's edge, up to 6, each with the project component `Enemy {kind, damage, speed}` (kind a value name: `grunt` or `brute`; 10, 3) and walking straight at the Player. An Enemy within 1 of the Player hits it: `player.hit {damage}` with the enemy's spawn event as its cause, then no more hits from it for 1 second. A project component `Hazard {damage, every}` on any entity with a trigger collider hurts what stands in it, every `every` seconds. `Health.current` reaching 0 emits `health.depleted` once. |
| `hopper` | A 3D platformer moved by its script: `Ground` 60 by 10; `Player` at (0, 0.5, 0) with the project component `Motion {vx, vy, grounded}`, walking at 5 a second on `move_x`, gravity 20, a jump speed of 9.6 (a jump rises 2.304); `Coin_0` to `Coin_4` at x 3, 6, 9, 12, 15, y 1, taken when the Player's centre comes within 0.8: the coin is destroyed and `coin.taken {coins}` emitted; a `Game` entity with `Score {coins}`. |
| `open_sea` | The sailing game of the contract (`../contract/sailing.md`) on open water: `Sea` at height 0 to the horizon, its waves a deterministic function of time; `Breeze`, a steady wind; `Sloop`, the body of seat `skipper` (the game's only seat), at the origin, bow north (-z), sail furled (`hoist` 0), under slice 1's sailing simulation (no drive within the no-go half-angle of 45 degrees). A `Course` entity carries the fixture's project component `Course {goal, next, rounded, crates, finished_tick, aground}` (`goal` a value name: `arrive`, `out_and_back`, `round`, `crates`; `finished_tick` 0 until finished), whose JSON form is the same on all configurations, kept by the fixture's course rule: it counts marks rounded (`mark.rounded`), crates taken and running aground (`boat.aground`), and when the task's goal is met (6.1, Done) it sets `finished_tick` and emits `course.finished`. The wind, the marks, the crates and the goal come from the fixture's parameters (6.1, Seeds), read when the world is made. |
| `open_sea_basic` | `open_sea` whose only seat, `skipper`, lacks the intent `sail_to` (`come_to_heading` and `trim_sail` only), for the task that asks the agent to tack: one seat per body (`../contract/README.md`, Seats and callers). |
| `island` | `open_sea` with three islands (kind `island`, occluders), each a static footprint, a vertical cylinder from the sea bed to 15 above the water, laid out after `master`'s `samples/island` and scaled to the contract's sight ranges: `Isle`, radius 150 at (0, 0); `Islet1`, radius 60 at (700, -480); `Islet2`, radius 55 at (-660, 540). The Sloop starts at (0, 400), south of the Isle, bow north, sail furled. Crates come aboard only through `take_aboard` within 3 (../contract/sailing.md, Taking a crate aboard). |

The sailing fixtures emit `mark.rounded`, `crate.taken` and `boat.aground` as perception.md's
sailing game declares them (../contract/sailing.md), and the player suite uses the skipper seat's
default decision filter (../contract/sailing.md, Time): an intent finished or failed, a new
sighting, a crate taken, a mark rounded, running aground, ten seconds idle, two minutes at most.

## 2. Briefs in this rebuild

`master`'s briefs change in four ways when they port; nothing else about a task changes.

1. **Language-neutral.** A brief names `{entry}` (the configuration's entry script, `engines/<line>.toml`)
   and "the project's scripts", never a file type; reference solutions are kept per line.
2. **State in components.** Where a brief said "expose `score`", it now says which project component
   on which entity holds it ("`GameState {alive, time_alive}` on an entity named `Game`"), since
   scripts are stateless and the world is the only state (charter 3.2). Whether an agent keeps state
   out of its scripts without being told is part of what the suite measures (charter 6.3, the third
   row).
3. **The shared tools.** A brief that named `master`'s commands (`world.set`, `input.hold`) names
   the shared tools (`world_edit`; `act` setting the `player` seat's controls) or no tool at all.
4. **The world, not the picture.** Before slice 3 a brief asks for no look; a "red cube" becomes a
   named entity with a collider of that size, or the task waits for slice 3.

## 3. The developer suite: porting `master`'s 72 tasks

Tiers say when a task can run: **S2** with the core and the player interface (slices 1 and 2) once
`apply` and the developer tools are in (`../contract/mcp.md`, open choice 2); **S3** with rendering
and assets (slice 3); **S4** with the showcase's sea and vegetation (slice 4); **D**, deferred: the
feature is not in the charter's slice plan, and charter 3.8 keeps breadth out until the core is
validated. A deferred task ports, unchanged in substance, when a slice brings its feature. "Fixture"
is the rebuild's.

| # | Task | `master` sample | Tier | Fixture | Note |
|---|---|---|---|---|---|
| 1 | `spawn_named` | hello | S3 | hello | Needs mesh and colour components. |
| 2 | `recolor` | hello | S3 | hello | Same. |
| 3 | `tile_edit` | sprites | D | | Tile maps. |
| 4 | `clear_enemies` | playground | S2 | arena | Enemies walk by script instead of navigation. |
| 5 | `play_clip` | assets | D | | Animation. |
| 6 | `path_length` | playground | D | | Navigation. |
| 7 | `count_overlap` | physics | S2 | bodies | |
| 8 | `event_cause` | playground | S2 | arena | |
| 9 | `night_lamp` | hello | S3 | hello | Lights and shadows. |
| 10 | `expose_count` | hello | S2 | hello | Becomes `entity_count`: a component, not an exposed value. |
| 11 | `ball_lower` | hello | S2 | hello | |
| 12 | `spawn_stars` | hello | S3 | hello | Draws spheres. |
| 13 | `double_jump` | sprites | S2 | hopper | A 3D hopper instead of the 2D sprites game. |
| 14 | `coin_respawn` | sprites | S2 | hopper | Same. |
| 15 | `jump_sound` | sprites | D | | Audio. |
| 16 | `coin_chime` | sprites | D | | Audio. |
| 17 | `theme_song` | sprites | D | | Audio. |
| 18 | `merchant_talk` | talk | D | | Dialogue, game interface. |
| 19 | `lamp_prefab` | hello | S3 | hello | Prefabs with lights. |
| 20 | `hill_raise` | hills | D | | Terrain. |
| 21 | `yard_flag` | walker | D | | Cloth. |
| 22 | `flowers` | hills | D | | Terrain, scattering. |
| 23 | `car_speed` | drive | D | | Vehicles. |
| 24 | `knockout` | walker | D | | Ragdolls. |
| 25 | `fps_shotgun` | fps | D | | Hitscan sample, enemies that hear. |
| 26 | `farm_rain` | farm | D | | Weather. |
| 27 | `guards_footsteps` | guards | D | | Behaviour, hearing. |
| 28 | `walker_sprint` | walker | D | | Character controller. |
| 29 | `raft` | hills | S2 | open_sea | On the sea instead of a lake. |
| 30 | `sand_road` | hills | D | | Terrain painting. |
| 31 | `reed_field` | hills | D | | Scattering. |
| 32 | `stormy_dusk` | hills | D | | Sky and sun. |
| 33 | `sea_around` | hills | S4 | island | The sea to the horizon, clouds. |
| 34 | `meadow` | hills | S4 | island | Grass on the islands. |
| 35 | `sailboat` | blank | S2 | blank | The showcase's own task. |
| 36 | `snowy_evening` | hills | D | | Weather. |
| 37 | `dirt_patch` | hills | D | | Terrain layers. |
| 38 | `glass_window` | hello | D | | Transmissive materials. |
| 39 | `persian_locale` | ui | D | | Localization, game interface. |
| 40 | `blender_level` | assets | D | | Blender. |
| 41 | `calm_lake` | hills | S2 | open_sea | Becomes `calm_sea`. |
| 42 | `spike_trap` | sprites | S2 | arena | 3D, with the arena's `Hazard` and `Health`. |
| 43 | `brute_enemies` | playground | S2 | arena | |
| 44 | `roof_mesh` | hello | S3 | hello | Meshes made from numbers. |
| 45 | `walled_room` | sprites | D | | Tile maps. |
| 46 | `plank_bridge` | crates | D | | 2D joints. |
| 47 | `dodge` | blank | S2 | blank | |
| 48 | `snake` | blank | S2 | blank | |
| 49 | `pause_menu` | blank | D | | Game interface. |
| 50 | `breakout` | blank | S2 | blank | |
| 51 | `platformer` | blank | D | | Tile maps, 2D bodies. |
| 52 | `sokoban` | blank | S2 | blank | |
| 53 | `villagers` | blank | D | | Humanoids, animation. |
| 54 | `village_weather` | blank | D | | Weather, sky. |
| 55 | `wolves` | blank | S2 | blank | The chase written in script; the brief never needed `Behavior`. |
| 56 | `fireworks` | blank | D | | Particles. |
| 57 | `glade` | blank | D | | Props, particles, audio. |
| 58 | `watchman` | blank | D | | Behaviour, navigation. |
| 59 | `key_door` | blank | S2 | blank | |
| 60 | `grey_flashback` | crates | S3 | hello | Post effects. |
| 61 | `svg_coin` | hello | D | | SVG images. |
| 62 | `orange_ball` | hello | S3 | hello | WGSL materials. |
| 63 | `night_level` | sprites | D | | 2D lighting. |
| 64 | `guard_view` | dungeon | D | | Tile-map sight. |
| 65 | `vault_level` | dungeon | D | | Tile maps. |
| 66 | `slow_swarm` | blank | S2 | blank | Broken game per line. |
| 67 | `leaky_cannon` | blank | S2 | blank | Broken game per line. |
| 68 | `sinking_crate` | blank | S2 | blank | Broken scene per line. |
| 69 | `voxel_house` | hello | D | | Voxel models. |
| 70 | `frozen_coins` | blank | S2 | blank | Broken game per line. |
| 71 | `festival_flags` | blank | D | | Cloth. |
| 72 | `wait_for_coin` | sprites | S2 | hopper | |

Counts: 23 in S2, 8 in S3, 2 in S4, 39 deferred. With the four tasks of section 5, the developer
suite of slice 2 has 27 tasks.

## 4. The S2 tasks

Each entry gives the brief as it changes (the rest of `master`'s text stands) and the checker. Every
answer is a JSON value; "answer null" unless said. Tolerances in a checker are stated in its brief.

**Commands and questions** (the agent works through the developer tools; nothing is edited on disk):

- `clear_enemies` (arena, 900 ticks before): "Destroy every entity named Enemy and nothing else."
  Check: no `Enemy` left; every other entity there before is still there.
- `count_overlap` (bodies, 120 ticks before): "Answer with the number of entities whose colliders
  overlap a sphere of radius 4 at the origin, as the physics query counts them." Check: the answer
  equals the physics overlap query run by the checker through `eval`.
- `event_cause` (arena, 1500 ticks before): "Find the most recent `player.hit` and answer with the
  type of the event at the root of its causes." Check: the root of `why` on that event (an enemy's
  spawn event).
- `spike_trap` (arena, 30 ticks before): "Give the Player 60 health (current and max) with half a
  second of invulnerability after each hit, put an entity named Spikes where the Player stands, a
  trigger box of half extents 1 by 0.5 by 1 with a `Hazard` of 15 every half second, run the game
  with the Player left where it is until its health is used up, and answer with the tick at which
  `health.depleted` was emitted." Check: `Health`, the `Spikes` collider and `Hazard`, and the tick
  against the event log.
- `brute_enemies` (arena, 600 ticks before): `master`'s text. Check: every `Enemy` has `kind`
  `brute` and `damage` 25; the answer is their count; the tick has not moved.
- `wait_for_coin` (hopper): "Set the `player` seat's `move_x` to 1 and run the game until the first
  coin is taken; answer with that tick." Check: after `reset`, the same control and a `step` until
  `coin.taken` reach it at the answered tick.
- `raft` (open_sea): "Spawn Raft, a dynamic body with a box collider 2 long (x), 0.2 thick (y) and 1
  wide (z), drop it into the sea from a little above the water at (10, 10), run 180 ticks, and
  answer with the sea's surface height under it." Check (per line, `check_<line>.py`: the collider
  and the water query are engine components): after 120 more ticks its centre is within 0.3 of the
  surface (the water query through `eval`) and within 5 of (10, 10).
- `calm_sea` (open_sea): "Make the sea perfectly still and raise its surface by half a unit; answer
  with the surface height at (0, 0)." Check (per line): the water's wave fields are zero and the
  surface is 0.5 at five sampled points over 60 ticks.
- `sailboat` (blank): `master`'s text with the contract's convention for the wind, "a steady
  westerly of 6 (blowing toward +x)", and without "the project's ground taken away" (blank has
  none). Check (per configuration: the sail, the motor and the wind are engine components): the Sloop has its
  sail hoisted and no motor, the wind is from 270 at 6, five seconds of sailing carry it 5 or more
  along +x, and it stays afloat.

**Script edits** (the agent edits files and loads them with `apply`; the harness restarts the
project before the check):

- `entity_count` (hello): "Edit {entry} so that an entity named Answer carries a project component
  `Answer {value}` holding, every tick, the number of entities in the world." Check: after a
  restart, the value against `world_query`'s total, again after the checker spawns three entities
  and steps a tick.
- `ball_lower` (hello): "Edit {entry} so that the ball starts its fall from a height of 1.0 instead
  of 3.0." Check: the Ball at y 1.0 (within 0.05) after the first tick, at rest on the ground after
  two seconds.
- `double_jump` (hopper): "Edit the project's scripts to give the player a double jump: pressing
  `jump` in the air, once per time off the ground, gives the jump speed again (a jump from the
  ground rises 2.3 today; with a second jump pressed 0.2 seconds later it must rise more than 3)."
  Check: one press rises 2.3 (within 0.1); two presses 12 ticks apart rise more than 3; a third
  press in the air adds nothing.
- `coin_respawn` (hopper): "A taken coin comes back where it was three seconds of game time after it
  was taken, takeable again and counted in `Score.coins`, and not before." Check: the checker moves
  the Player onto `Coin_0`; at 1.7 seconds it is gone, at 3.2 back in place; taken again,
  `Score.coins` is 2.

**Whole games from a brief** (blank; `master`'s brief with the changes of section 2, its checker
playing the game through `act` and `world_edit`):

- `dodge`: `alive` and `time_alive` in `GameState` on `Game`.
- `key_door`: `has_key` in `GameState` on `Game`; the door's stopping tolerance (x up to 8.05)
  stated in the brief.
- `snake`: `length` and `alive` in `GameState` on `Game`; the grid in the XY plane, as `master`'s
  brief came to say.
- `sokoban`: `moves` and `solved` in `GameState` on `Game`; `undo` on Z.
- `breakout`: `score`, `lives` and `bricks` in `GameState` on `Game`; the Ball's motion its
  `Velocity` project component.
- `wolves`: unchanged; the checker counts only the `sheep.caught` events of its own run.

**Diagnose and fix** (a broken project written by `setup`, one per line, the same bug in both
languages; the checker compares the fixed game with the broken one where they ought to agree):

- `slow_swarm`: 120 bees and 16 flowers whose state (`Bee {carrying, target}`,
  `Flower {given, turn}`, `Hive {pollen}`) is in components, and a rule that queries every flower
  once per bee per tick. Check: the same places and pollen tick for tick over 6 seconds, and the
  scripts' time a tick under a quarter of the broken game's (`profile`).
- `leaky_cannon`: pellets with `Pellet {vx, vy, landed}`; the sweep removes pellets below the ground
  while landed pellets rest on it, so they pile up. Check: 30 seconds of play against the broken
  game's shots and landings, and the pellets left about.
- `sinking_crate`: the BlueCrate's collider has the floor's collision group, which the Ghost shares
  on purpose. Check (per configuration: the answer names a field of the configuration's own collider component):
  BlueCrate at rest on the floor, the others as they were, the Ghost through the floor, the answer
  the field's name in the configuration's collider component.
- `frozen_coins`: the rule lights the next coin by index after each one taken, and past the last one
  the index runs off the end and the script errors. Check: four coins taken walking right with no
  script error, `coin.taken` four times, `level.complete` once, the next coin lit each time.

## 5. New developer tasks

These test what this rebuild adds: stateless scripts, replay, versions and causality (charter 3.2,
3.3, 7 item 13). They are in the suite of slice 2.

- `hidden_state` (blank, written by `setup`): a spinner game as an agent used to another engine
  would write it, its lap count in a module-level variable, which this engine refuses to load
  (spec-script freezes module values and its lint forbids mutable module bindings). "This game will not load. Make it load and play as it was written to:
  the same laps tick for tick, the count kept across a hot update." Check: it loads;
  `checks {only: ["reload"]}` passes; the laps over 600 ticks equal the reference's tick for tick.
  It measures how the script host's refusal leads an agent to put state in the world.
- `first_divergence` (island, two recordings made by `setup`): the same inputs under two versions of
  the course rule, the second counting a mark rounded only when the boat passes within 60 of it, so
  the runs agree until the boat passes the first mark wide. "Answer with the first tick at which the
  two runs differ and the component field that differs first." Check: the tick of
  `replay {first_divergence}` and a field among those `replay {diff_at}` lists at that tick.
- `schema_migration` (arena with `Health {hp, max, invulnerable}` at version 1 and a snapshot saved
  under it, made by `setup`): "Rename the field `hp` of the project's `Health` component to
  `current`. A save made before the change must still load with every value carried over." Check:
  the snapshot restored into the changed project gives each entity's `Health.current` the old `hp`,
  and the component's version went up (spec-persist, migrations).
- `aground_cause` (island, a recording made by `setup` in which the Sloop runs aground): "Answer
  with the tick at which the Sloop ran aground and the id of the intent whose course took it
  ashore." Check: both against the recording's event log and its causes.

## 6. The player suite: sailing

### 6.1 Common rules

- **Seat and grant.** The agent's player grant names seat `skipper` with `clock: true`,
  `max_wall_ms: 600000` (so where a step stops never depends on a configuration's tick speed) and `rewind`
  off; the runtime runs in stepped pacing, the benchmark's default (../contract/sailing.md, Time),
  with the game's default decision filter. `beat_to_windward` runs on `open_sea_basic`, whose seat
  has no `sail_to`.
- **Perception** is the `skipper` profile (`../contract/sailing.md`, The skipper's perception): the
  instruments (heading, speed, the wind, the point of sail, the sail's trim, the position), islands
  and course marks on the chart, marks in sight within 400 and crates within 120, islands hiding
  what lies behind them. A crate never seen is in no answer, not even as a name.
- **Actions** are the sailing game's (`../contract/sailing.md`, Actions): the controls `rudder`,
  `sheet`, `hoist`, `interact`; the intents `come_to_heading`, `trim_sail` and `sail_to`, which
  beats to windward by itself but plans no route round land, so the agent splits a passage past an
  island into legs; `take_aboard` on a crate alongside. Coming alongside takes nothing: the agent
  must act.
- **Seeds.** A task's world for seed `s` (the wind, marks, crates) is computed by the harness, not
  the engine: `random.Random(s)`, only its `random()` method (whose sequence Python keeps across
  versions), in the fixed order and formulas below. The harness writes them as the fixture's
  parameters, one JSON object all configurations read the same way, and hands them to the configuration's `new`
  command (`--params`, README 3.2), so the world is made with them and no engine component's JSON
  form is involved:

```json
{"goal": "arrive", "wind_from_deg": 268.4, "wind_mps": 6.3,
 "marks": [{"name": "M1", "x": 379.9, "z": -10.6, "order": 1, "round_to": "port"}],
 "crates": [], "arrive_m": 10, "start_line": null}
```

(An illustration for `run_to_mark`, M1 380 downwind of the start; not a real seed's numbers.) `goal`
is `arrive`, `out_and_back`, `round` or `crates`; `marks` and `crates` may be empty; `start_line` is
`{"from": {"x", "z"}, "to": {"x", "z"}, "cross_toward_deg": 90}` for `round_the_island`, else
`null`. All configurations therefore sail identical worlds for the same seed, which the paired comparison
needs (README, section 5); the object goes into the row as `world_params`.
- **Setup sequence.** `new` with the parameters, `serve` with the seed and the task's tick limit
  (`--tick-limit`), then the setup developer session's `step {ticks: 1}`, so the perception update
  has run once on the world the parameters made and the agent's first `observe` shows what is in
  sight; the setup session closes. The episode's `Start` decision point, the tick limit and
  `sim_ticks` count from that boundary.
- **Done.** Each task's goal is the fixture's `done` predicate (time.md, Episodes), implemented in
  its course rule, which sets `Course.finished_tick` and emits `course.finished` the tick the goal
  is met: within `arrive_m` (10) of M1 for `run_to_mark` and `beat_to_windward`; M1 and then back
  within 10 of the start for `reach_and_return`; the three marks rounded in order and then the start
  line crossed for `round_the_island`; all eight crates aboard for `gather_crates`. The episode ends
  there with `stopped: "done"` and the `result` readings `finished`, `marks_rounded`, `crates_taken`
  and `aground` (`../contract/sailing.md`, Time). Each brief states its goal, the arrival radius
  and, for `round_the_island`, the rounding rule and the start line, since the checker holds the
  agent to them.
- **Conditions.** View perception or omniscient (the seat bound to the reserved profile
  `omniscient_player`, `../contract/perception.md`, The omniscient view), times fork off or on: four
  per task. With fork on, `max_branches` is 4, `branch_ticks` ten times the task's tick limit and
  `branch_ticks_per_decision` 600 (ten seconds of lookahead per decision answered on main), and
  reports name the condition "exact lookahead" (`../contract/mcp.md` 5.5): without the bound a
  branch could sail round the whole island to find every crate and main then sail straight to them,
  which would measure free scouting rather than lookahead. Answers come in the text projection,
  MCP's default (perception.md, Queries); `run_to_mark` and `gather_crates` also run in JSON, a
  fifth condition, to measure what the projection costs.
- **Limits.** A tick limit per task, in simulated seconds: twice the `reference` policy's time under
  perception on seed 1, rounded up to a minute, set when that reference first passes. No reference
  policy has run yet, so the values below are proposals, not measurements. 300 tool calls (the proxy
  refuses the 301st, README 3.4), 900 seconds of wall time.
- **Checks** read, through the checker's session after the agent has finished, contract types and
  the fixture's own component only: the `Course` component (`world_get`; its `finished_tick` is when
  the goal was met), the omniscient event log after the episode's start
  (`events {omniscient: true}`: `mark.rounded`, `crate.taken`, `boat.aground`, `course.finished`),
  the Sloop's place from `observe {omniscient: true}` (its `pos_m`), and the player session's
  statistics (`session {all: true}`). `score` is the share of the goal reached; `ok` is
  `Course.finished_tick` set within the tick limit without running aground; `sim_ticks` is
  `finished_tick` minus the episode's start, or the limit.

### 6.2 Tasks

| Task | Fixture, seat | Goal | Score | Limit (proposed) |
|---|---|---|---|---|
| `run_to_mark` | open_sea, `skipper` | Arrive within 10 of mark M1, 380 downwind (goal `arrive`). | 1 if arrived; else the share of the start's distance closed | 240 s |
| `reach_and_return` | open_sea, `skipper` | Arrive within 10 of M1, 380 across the wind, then back within 10 of the start (goal `out_and_back`). | 0.5 a leg | 480 s |
| `beat_to_windward` | open_sea_basic, `skipper` | Arrive within 10 of M1, 380 straight upwind, without `sail_to`: the agent tacks with `come_to_heading` and `trim_sail` (goal `arrive`). | 1 if arrived; else the share of the 380 made good to windward | 600 s |
| `gather_crates` | island, `skipper` | Take aboard 8 crates adrift between 250 and 700 from the Isle's centre, most of them out of sight at the start (goal `crates`). | crates aboard over 8 | 1,800 s |
| `round_the_island` | island, `skipper` | Sail once round the Isle counter-clockwise, leaving marks M1, M2, M3 to port in order, then cross the start line eastward, without running aground (goal `round`). | marks rounded over 3, halved if aground | 1,200 s |

Seed formulas (`r()` is the seed's `random()`; bearings in degrees clockwise from north, the wind
given as where it blows from, as the contract writes it):

- Every task: the wind from `270 + 40 * (r() - 0.5)` (a westerly) at `5 + 2 * r()`.
- `run_to_mark`, `reach_and_return`, `beat_to_windward`: M1 380 from the start at the downwind
  bearing, at the downwind bearing minus 90 (across the wind), and at the wind's bearing (upwind)
  respectively.
- `gather_crates`: each crate at radius `250 + 450 * r()` and bearing `360 * r()` from the Isle's
  centre, drawn again (with the next two draws) when it falls within 20 of an island's footprint or
  within 150 of the start.
- `round_the_island`: M1, M2, M3 at radius `300 + 30 * r()` from the Isle's centre and bearings 90,
  330 and 210 (counter-clockwise from the start's bearing of 180), each turned by
  `20 * (r() - 0.5)`, each with `round_to: port` and its `order`. The fixture's rule counts a mark
  rounded when the boat's bearing from the Isle's centre passes the mark's bearing counter-clockwise
  with the boat farther out than the mark, and emits `mark.rounded`. The start line is the segment
  of the radial at bearing 180 from the Isle's centre between radius 330 and radius 500 (from (0, 330)
  to (0, 500) in the contract frame), crossed eastward (`cross_toward_deg: 90`: the boat's x goes
  from negative to positive while its z lies between 330 and 500); crossing it after M3 is rounded
  ends the course. The brief states both rules and the line.

### 6.3 Reference policies

Python in each task's directory, through player tools only, deterministic, each decision an `act`
and a `step {ticks: 36000, until: "decision"}` (the loop ../contract/sailing.md, Time, describes):

- `run_to_mark`, `reach_and_return`: `trim_sail {hoist: 1, keep: true}`, then `sail_to` the mark and
  back.
- `beat_to_windward`: close-hauled on the tack nearer the mark's bearing with
  `come_to_heading {keep: true}` and `trim_sail {keep: true}`, tacking (a `come_to_heading` through
  the wind) when the mark's bearing reaches the other tack's close-hauled heading: the layline rule
  `sail_to`'s executor uses.
- `gather_crates`: `sail_to` the nearest crate in sight with `arrive_m` 2, then `use` it with
  `take_aboard`; with none in sight, the next waypoint of a circuit of eight round the Isle at
  radius 450, each leg checked against `sail.land_on_course` and split at a waypoint when it warns.
- `round_the_island`: `sail_to` waypoints 40 outside each mark, in order, then the start, splitting
  legs the same way.

`reference-omniscient` runs the same code with the seat bound to the omniscient profile: on
`gather_crates` it sees every crate from the start and skips the search, which is the gap perception
costs a scripted sailor.

### 6.4 What the suite reports

Per task and condition: completion (pass rate and mean score), `decision_points`,
`decisions_answered`, `calls`, agent tokens, `sim_ticks`, `forks` and `branch_ticks`, with means and
variances over the seeds (README, sections 4 and 5). The two differences charter 9 asks for come
from paired rows: fork on minus fork off, and omniscient minus perception, for each metric.

## 7. Adding a task

1. Write `tasks/<suite>/<name>/task.py` with the text, the answer's form, `solve` (or per-configuration
   solution files), `check` and the limits; a fixture change goes into section 1 first, on both
   lines.
2. The brief states the whole contract the checker relies on: names, units, tolerances, what the
   game must read from the world every tick (`master`'s `villagers` and `sokoban` failed on briefs
   that left these out).
3. Check it both ways on all configurations (`reference` passes, `null` fails) and run `reference` twice for
   identical rows.
4. Add its row to this file and run one model on it before it joins a recorded run.
5. A failed agent call in a run is traced to an engine change, a documentation fix or a brief or
   checker fix, and the results entry says which (charter 9, item 5).
