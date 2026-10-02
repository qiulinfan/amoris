# The agent benchmark

`tools/scripts/agent_eval.py` measures the thing the engine is built for: whether a program can
change a running project through the engine's own interface, given a task in words and the
documentation. It is the "agent-first" claim (`docs/agent-first.md`) turned into a number that any
model, tool or script can be scored on.

## What a task is

Each task starts one of the samples paused in a headless runtime with the JSON-RPC server
(`docs/mcp.md`, the same interface `pocket mcp` exposes), runs the simulation to a point where the
task is real (enemies about, a hit that needs explaining), hands the runner the task text, and then
checks the world through the same commands. A task passes or fails; nothing is scored on style.

| Task | Sample | Asks the runner to | Checked by |
|---|---|---|---|
| `spawn_named` | hello | spawn a named red cube at a point | `world.find`, `world.get` |
| `recolor` | hello | turn the Ball green | `world.get` |
| `tile_edit` | sprites | put a tile into one cell of one layer | `tilemap.tile` |
| `clear_enemies` | playground | destroy every Enemy and nothing else | `world.query`, `world.summary` |
| `play_clip` | assets | play a named clip on the Arm, looping | `world.get` on the Animator |
| `path_length` | playground | answer with a navigation path's length | `nav.path` (within 3 %) |
| `count_overlap` | physics | answer with the count of a physics overlap | `physics.overlap` |
| `event_cause` | playground | answer with the root cause of the last hit | `events.since`, `events.why` |
| `expose_count` | hello (a copy) | edit the script to expose the entity count as `answer` | `state`, `world.summary` |
| `night_lamp` | hello | make it night: dim the sun, hang a warm lamp that casts shadows, and answer with what the renderer counts | `world.get`, `render.stats` (`light_shadows`) |
| `ball_lower` | hello (a copy) | edit the script so the ball starts its fall at 1.0 | `state` |
| `spawn_stars` | hello (a copy) | edit the script to spawn five named spheres on start | `world.find`, `world.get` |
| `double_jump` | sprites (a copy) | design a mechanic: a second jump in the air, once per flight | `input.press`, `state` (the rise of a jump pressed again in the air) |
| `coin_respawn` | sprites (a copy) | design a mechanic: a collected coin returns after three seconds | `world.set`, `state` (the coin count once taken, at 1.7 s and at 3.2 s) |
| `jump_sound` | sprites (a copy) | make an asset and use it: write a WAV under `assets/` and play it on every jump from the ground | `assets.list`, `audio.list` (silent before a jump, a voice of the file after it, again after the next) |
| `theme_song` | sprites (a copy) | write background music as a score of two instruments or more, a melody and a bass line, looping from the start | `assets.list`, `audio.list` (a looping voice of the score from the start, still there after one time through), `project.read` (instruments, tracks, notes), `audio.clips` |
| `coin_chime` | sprites (a copy) | make a sound recipe (`.sfx`) and play it on every coin collected | `assets.list`, `audio.list` (silent before, the recipe's voice after the coin), `audio.clips` (a short sound) |
| `lamp_prefab` | hello (a copy) | write a prefab file and instantiate it three times from the script | `project.read`, `world.find`, `world.get`, `world.describe` (three yellow spheres where asked, a point light under each) |
| `hill_raise` | hills | raise the terrain at a point to a height without touching the ground 12 units away, and answer with the height | `terrain.height` (there, and around at 12 units, against the setup) |
| `flowers` | hills | strew red flowers over the terrain, only on ground no steeper than 20 degrees | `world.find`, `world.get`, `scatter.copies`, `terrain.height` (each flower's ground and its slope) |
| `car_speed` | drive | tune a car's top speed and power, drive it for five seconds through its action, and answer with its speed | `world.get` on the Vehicle (the settings, the speed) |
| `fps_shotgun` | fps (a copy) | make the gun a shotgun: six pellets a shot within 3 degrees of the view, 8 damage each, a magazine of six | `combat.hitscan` events (a held raider five ahead: most of six pellets, 8 each, spread), `state` (`ammo` after one shot and after six) |
| `guards_footsteps` | guards (a copy) | the guards hear the player walking too, from nearer: a noise that carries 4 every half second while it walks, none standing | `events.since` (`noise` while walking, not standing, about 4), a guard three from the walking player leaving its round |
| `farm_rain` | farm (a copy) | the weather answers the field: rain while three or more sown plots are dry (the project's own `Plot` component), until none is; no rain on a timer | the Weather's `rain` over 110 seconds with nothing sown, with two dry plots, with three, and after the rain has watered them |
| `walker_sprint` | walker (a copy) | add an input action in `project.toml` and use it in the script: a sprint on LShift, 9 units a second instead of 5 | `input.describe`, `input.hold`, `state` (a second of walking with and without it) |
| `raft` | hills | spawn a dynamic box collider 2 by 0.2 by 1 and drop it into the lake so it floats (the mass is the runner's to choose), run 180 ticks, answer with the surface's height under it | `world.get` on its RigidBody and Collider, 120 more ticks, then `water.height` under it: within 0.3 of the surface, still over the lake |
| `calm_lake` | hills | make the lake still and clearer (8 units) and raise it half a unit, answer with its surface's height | `world.get` on the Water and its Transform, `water.height` |
| `spike_trap` | sprites | give the player 60 health with half a second of invulnerability, put a 2D zone under it that hurts 15 every half second, run until its health is gone, answer with the tick | `world.spawn` with `Area2D` and `Hitbox`, `world.set` on `Health`, `step {until}` |
| `brute_enemies` | playground | make every enemy a brute doing 25 damage through the project's own `Enemy` component, answer with how many | `world.query {with: ["Enemy"]}`, `world.set` with a value name |
| `yard_flag` | walker | a grey pole and a red cloth flag pinned along it, flying in a wind the agent adds; answer how far it reaches past the pole | `world.get` (Pole, the Flag's Cloth, size and pin, its colour), `world.query` (a Wind of 5 units a second or more), `physics.cloth` (the pinned edge on the pole, the top at its top, the far edge 1.2 or more past it) |
| `knockout` | walker (a copy) | give the player 30 health and make the hero go limp as a ragdoll when it runs out, the player no longer moving | `world.get` (Health), spikes the checker spawns and walks into until `health.depleted`, `world.get` (Ragdoll active, bodies), `animation.pose` (the head on the ground), the player held still against the move actions |
| `merchant_talk` | talk (a copy) | add a potion merchant whose conversation is a dialogue script, two potions bought for three gold each, none offered below three | the Merchant's place, the script read as JSON, the talk played with talk, Space and number keys (`ui.query` for the box and its numbered choices), `state` (gold, potions), `dialogue.choice` events |
| `vault_level` | dungeon | add a 12 by 8 vault with walls, a pillar, three coins and a start, as a map, and show it at a place | `tilemap.info`, `tilemap.solid` on every cell, `tilemap.objects` (on the floor) |
| `wait_for_coin` | sprites | walk right until the first coin is taken, answer with the tick | `input.hold`, `step {until: {event}}` |
| `roof_mesh` | hello | make a square pyramid from numbers and draw it in orange at a place | `mesh.list`, `world.get` (the mesh named, its colour, its bounds from y 2 to 3) |
| `walled_room` | sprites | make a 10 by 8 map by code with a solid wall layer around its border, show it, drop a pebble in it | `tilemap.info`, `tilemap.solid` on every cell, the pebble at rest on the floor |
| `plank_bridge` | crates | hang five planks on hinges between two points in the world | the planks' ends together and at the points after two seconds, the lowest plank's height |
| `grey_flashback` | crates | turn the whole picture grey with a post effect, the interface untouched | `render.post`, the colours of a crate and the car |
| `svg_coin` | hello (a copy) | draw a coin as an SVG file of its own and show it from the start as a sprite at a place | the Sprite's texture an SVG the engine reads, its place and size, gold at its middle and a darker rim |
| `orange_ball` | hello (a copy) | a WGSL material of its own that draws the ball a flat orange, from the start | `world.lint`, the ball's colour at its centre and toward its edge |
| `sailboat` | blank | the ground taken away, an ocean at 0, a wind of 6 toward +x, a boat named Sloop under sail alone, bow to +x | the Sloop's `Boat` (no throttle, afloat), `wind.at`, five seconds of sailing carry it 5 or more along +x |
| `dodge` | blank | a dodge game from a brief: moves, a rock a second, a hit ends it | input, rock count and fall, `game.over`, nothing moves after |
| `key_door` | blank | a key, a door that holds until the key is taken, an exit | the door holds, `key.taken`, `level.complete` |
| `snake` | blank | snake on a 20 by 15 grid from a brief: segments, a cell every 0.15 s, turning but never back, food that grows it, the edge ends it | the start, `food.eaten` with the Food put ahead, five cells in 0.75 s, no reversing, a turn, `game.over` at the top edge, nothing moves after |
| `sand_road` | hills | paint a sand-coloured road along a line over the terrain, fully covered near it and untouched far from it, answer with the coverage on the line | `terrain.height` (the paint across the road and beside it, against the setup) |
| `reed_field` | hills | plant a Scatter of thin reeds on the terrain within an area, all one size, swaying 0.3 and each a collider 0.1 in radius, answer with how many stand | `world.get` on the Scatter, `scatter.copies`, `physics.raycast` onto a reed |
| `stormy_dusk` | hills | make a windy, cloudy sunset (the atmosphere sky with clouds, the sun low in the west, the wind toward +x), answer with the red of the sun light at the ground | `world.get` on the Sky, Sun and Wind, `wind.at`, `render.stats` (`sun_light`) |
| `snowy_evening` | hills | a snowy evening that goes on by itself (snow at 0.7 on ground already white, the time of day at 17:30 in a ten-minute day), answer with the hour after ten seconds | the first `Weather` (`snow`, `cover`), the Sky's `day_length` and `time_of_day`, `render.stats` (`weather_drops`) |
| `sea_around` | hills | the lake made a sea to the horizon at its height, the sky's volumetric clouds over about half of it; answer with the level 400 units east | `water.height` at (400, 0) and far to the north-west, the Sky's `clouds`, `render.stats` (`clouds`) |
| `meadow` | hills | grass on the terrain's grass layer, none below 3.5, drawn out to 30 units | the terrain's `Grass` (`layer`, `min_height`, `reach`), `render.stats` (`grass_blades`) |
| `dirt_patch` | hills | change one textured layer's height rule and paint another in a patch, answer with its share at the centre | `world.get` on the Terrain's layers, `terrain.height` (the share there and six units out) |
| `glass_window` | hello | spawn a pane of clear glass with an index of refraction and give the Ball a clear coat, answer with the glass the renderer drew | `world.get` on both MeshRenderers and the Transform, `render.stats` (`glass`) |
| `night_level` | sprites | make the level night: the map and the player lit, a dark blue ambient, a warm light that goes with the player | `world.get` on the TileMap and Sprite, `render.ambient`, the lights' world positions after the player walks |
| `guard_view` | dungeon | answer how many cells the player sees within 6 and whether a torch has a clear line to it | `tilemap.fov`, `tilemap.sight` |
| `persian_locale` | ui (a copy) | add a language: translate every text into Persian in a new file and name it in every file | `locale.check` (complete), `locale.set`, `ui.query` (the score in Persian script, right to left) |
| `blender_level` | assets (a copy) | drive Blender from the command line: a level whose objects carry engine components as custom properties, instantiated, a ball dropped on it | `assets.describe` (the properties), `world.instantiate` again, the components, a ball resting on the floor |
| `voxel_house` | hello (a copy) | draw a house as a voxel model written as text (`assets/house.voxels`) and show it from the start at a place | the entity drawing it and its place, `assets.describe` (read as voxels), the file through `project.read`: at least 6 cells each way, three colours, standing on its bottom layer |
| `watchman` | blank | a guard with the engine's `Behavior`: a navigation grid, a beat along a two-point `Path` at 2, chase at 4 what it sees within 6, back to the beat once the Player has been out of sight 2 seconds | the Watchman's state and its agent's goal and speed with the Player far, moved 3.4 away in the open, then out of sight: not back on the beat after half a second, back after three |
| `slow_swarm` | blank (a broken game) | a swarm game whose ticks take too long: find why and make the scripts four times faster without changing what the game does | the bees, the flowers and the pollen after six seconds against the broken game's, and `script.profile` over its last second against the broken game's |
| `leaky_cannon` | blank (a broken game) | a game that slows down the longer it runs: find what is wrong and fix it, the pellets fired, flying and landing as before | thirty seconds of play against the broken game's (shots, landings and where they land, the pellets in the air), and how many pellets are left about |
| `frozen_coins` | blank (a broken game) | a game that freezes on the last coin: find why and fix it, the rest as it was | the four coins taken by walking right with no script error, `coin.taken` for each and `level.complete` once, the next coin glowing every time |
| `festival_flags` | blank (a slow level) | a festival whose ticks take too long: make them four times faster through the scene, the flags the same and still flying | the flags' places, sizes, pins, weights and colours against the slow level's, their weave no coarser than 8 by 5, each reaching 1.2 past its pole, and `perf`'s tick against the slow level's |
| `sinking_crate` | blank (a broken level) | a crate falls through the floor: find why, fix the scene, and answer the field that was wrong | the crate resting where it fell, the others as they were, the ghost still drifting through the floor, the floor untouched, the answer |

Sixteen tasks change the world through commands (`night_lamp` then answers from what the renderer
reports about the frame it drew; `hill_raise` reshapes a terrain within a bound and answers with the
height; `flowers` strews a Scatter within a slope limit; `car_speed` tunes a Vehicle and drives it
through the game's own action, since the script sets the throttle every tick; `raft` floats a body
on the lake, `calm_lake` changes the water; `sand_road`, `reed_field`, `stormy_dusk` and
`dirt_patch` paint, plant, light and layer the hills; `glass_window` answers with the glass the
renderer drew), three ask a question about it, and ten ask for a change to the project's files:
three a line or two of TypeScript, two a mechanic the runner has to design (where the jump counter
lives and when it clears; what a coin is, where it was, how to bring it back later), with a check
that plays the result rather than reads the source, and five that span files: a sprint that needs an
action in `project.toml` and the speed it picks in the script, a sound the runner has to make and
play and a prefab it has to write and instantiate, where the check reads the file through the
runtime and then looks at what the world does with it, a new language (`locales/fa.json` and its
name in every locale file, read back by switching the interface to it) and a level the runner makes
by driving Blender headless (`assets/level.blend`, whose objects' custom properties become
components when the check instantiates it again). The question tasks take their truth from the same
commands at check time, so a runner that reads the world correctly passes them and one that guesses
does not. The script tasks run on a copy of the sample (`pocket new --from`, under
`build/agent-eval/`): the runner gets `project_dir` and edits the project there (its entry script,
`scripts/main.ts` or `scripts/main.tsx` for the sprites sample, or the files the task names), and
when it returns the harness bundles the copy again with the tool, reloads the project
(`project.reload`: `project.toml`'s settings read again, a fresh world from the scene, the edited
script started over it) and steps two ticks before the check. The copy and its bundle are removed
afterwards.

### Whole games

Eleven tasks start from a blank project (`pocket new` without a sample, outside the repository) and
a brief of a few sentences, and ask for a small game: `dodge` (a player moved by two actions at 6
units a second, a rock a second falling at 4, a hit ending the game with `game.over`, `alive` and
`time_alive` exposed), `key_door` (a door the player cannot pass going right until it takes a key,
`key.taken` and `level.complete`, `has_key` exposed), `snake` (a grid, growing on food, never
straight back, `game.over` at the edge), `pause_menu` (a title screen with a `start` button, a
`pause` action that stops the game and shows `resume` and `restart` buttons, `screen` and `time`
exposed) and `breakout` (a paddle, a ball moved by its `Velocity`, thirty-two bricks, lives,
`level.clear` and `game.over`; at 203 words its brief is the longest, the geometry being part of the
contract), `platformer` (a tile map drawn from eight rows given in the brief, a `Body2D` player that
walks at 6 and jumps at 11, a pit that kills, a flag that ends the level; the runner draws its own
tiles) and `sokoban` (six rows, two boxes, pushes stopped by walls and by a second box, an `undo`
action, `level.solved` with the move count) and `villagers` (five of the engine's built-in humanoids
in five looks wandering a square, walking when they move, one that stops, faces the player and waves
when it comes near; the brief names neither the humanoid's clips nor what would walk them, so the
agent finds them) and `fireworks` (rockets with trails that rise and burst into at least a hundred
glowing sparks, several up at once) and `glade` (a ground textured with grass, four trees, three
rocks and a campfire with a warm light, glowing flames and smoke, a sound and sparks on a key, all
without a single image, model or sound file in the project: the brief names none of the engine's
patterns, props, presets or `sfx:` clips, so the agent finds them) and `wolves` (two wolves that
each run at whichever of four sheep is nearest at the time and catch one within a unit, a
`sheep.caught` event naming the wolf; the check moves a sheep behind a wolf to see it turn). The
brief is a contract: the names of the entities, the actions and their keys, the events with their
data, the exposed values, and that the game reads positions from the Transforms every tick, so the
check can move things with `world.set`. The check is hidden in the harness and plays the game: it
holds `move_x` for a second and measures the move, counts the rocks and times one's fall, puts a
rock on the player and waits for `game.over`, then holds the actions again and expects nothing to
move; for the door it starts the player between the key and the door and walks it into the door,
puts it on the key, and walks it to the exit; the menu it clicks through by the buttons' names
(`ui.click {id: "start"}`), holding `move_x` on each screen and reading `time`; the breakout it
plays by setting the ball's place and velocity: up into a brick, into a wall, onto the paddle, past
it, into the last brick left, and, after `project.reload`, past the paddle three times; the
platformer it reads back as solid rows (`tilemap.rows`), walks, follows through a jump with
`step {watch}`, drops into the pit and walks onto the flag; the puzzle it plays by presses only (the
game may keep its own grid), into a wall, into two boxes, a move and its undo, a push into a wall,
and the four moves that solve it. None of it is in the brief beyond the contract.

### Follow-ups

A game is rarely asked for once. A task can carry `followups`: later requests on the same project,
each given to an agent of its own once the one before has finished, so the second agent finds the
project as the first left it, reads what is there and changes it, as a person coming back the next
day would ask a fresh session to. The check runs once, after the last, and holds both: what the
first request made must still stand. `village_weather` asks first for a village square (three
houses, a well, an atmosphere) and then for an evening in the rain with four lamps that light after
dark, the engine's `Weather`, `Sky.time_of_day` and `Light.after_dark` named by neither request. The
runner is called once a request (its trace is `<task>-2.jsonl` for the second), and the row's
metrics are the sums.

### Diagnose and fix

Five tasks hand the runner something broken and a report of what a player sees, not of the cause:
the game writes itself into a blank project before the runtime starts (a task's `setup`), and the
check compares the fixed game with the broken one where they ought to agree, both measured in the
same runtime (the broken one before the runner starts, by its `before`). `slow_swarm` is a swarm
whose every bee queries every flower every tick, so the time goes on commands; a fix has to keep the
game the same tick for tick, including a flower that moves in the middle of a tick, and bring the
scripts' time a tick under a quarter (`script.profile` names the handler and the 120 queries a
tick). `leaky_cannon` sweeps away pellets below the ground while a landed pellet rests on it, so
they pile up: thirty seconds leave 300 of them where 14 are in the air. `sinking_crate` puts the
crate in the floor's negative collision group, which the ghost shares to drift through on purpose,
so the fix is the crate's group and not the floor's. `frozen_coins` lights the next coin after each
one taken and so, after the last, a coin past the end: the script error that stops the game names
`glow@scripts/main.ts:20` called from `scripts/main.ts:33` (docs/sdk.md, Errors and cost), and the
fix must keep the glow. `festival_flags` is the one whose time is not the scripts': twenty-four
cloth flags woven 48 by 30, which `perf`'s `systems` puts first; a weave of 16 by 10 flies the same
flags for about an eighth of the cost (5.9 ms a tick against 0.7 in the reference run, with the
sheets on several threads).

## Runners

```bash
python3 tools/scripts/agent_eval.py --runner reference        # the solutions in the harness itself: 39/39
python3 tools/scripts/agent_eval.py --runner null             # does nothing: the floor
python3 tools/scripts/agent_eval.py --runner "claude -p" --tasks spawn_named,recolor --json
```

`reference` is the harness checking itself: every solution is one or two commands, which is the
point. `null` sets the floor. Any other value is a shell command run once per task with the task as
JSON on stdin (`task`, `project`, `rpc_url`, `docs`, `notes`); it may print a JSON object with an
`answer` as its last line, and `POCKET_RPC_URL` is in its environment. A runner that wraps a model
gives it the docs listed and the RPC; what it does in between is its own business. `--json` prints
the report (`runner`, `passed`, `total`, per task `ok`, `seconds`, `detail`, `answer`, `error`), and
the exit code is 0 only for a full pass.

`tests/evidence/agent-eval/reference.json` and `null.json` are the two built-in runners' reports;
coding agents with a model are below.

**Out of the repository.** For any runner but the two built-in ones, every task runs on a copy of
its sample in a directory of its own under the system's temporary directory, with a copy of the
documentation set and an index of it (`docs/INDEX.md` there: a line a file, its title, what it
covers and its size); the copy's `AGENTS.md` points at those docs, and `docs` in the task lists
them. The harness itself holds every check, and the checks are the answers: an agent that found it
could replay a check instead of reading the engine's docs. Until 2026-10-01 the script tasks' copies
were made under the repository's `build/`, and the traces of that day's run show two agents doing
exactly that: on `coin_respawn` the agent searched the repository for the word, found the check in
`agent_eval.py` and followed it step by step, and on `walker_sprint` it read the reference solution.
Those two passes, and the times and costs of the script tasks in the runs before that date, measured
the harness as much as the engine. The agent can still reach the repository by an absolute path (the
tool it runs is there), so `pi_agent.py` reports any call that touches the harness or the evidence
under `peeked`, and the harness prints it beside the task's result.

## Coding agents

`tools/scripts/runners/pi_agent.py` hands each task to a pi-family coding agent, oh-my-pi (`omp`) or
pi, with the model it is set up for or the one `--model` names, and prints the answer with what the
task cost: tokens, dollars, tool calls by tool, turns and seconds; the harness adds them up as
`totals`. `--via` says how the agent reaches the engine: `mcp` through the `pocket` MCP server
(`docs/mcp.md`), declared in a `.mcp.json` the runner writes where the agent starts; `shell` through
`pocket rpc`; `extension` through pi's `pocket` and `pocket_look` tools
(`integrations/pi/pocket.ts`). The agent starts in the project copy for a task that edits files and
in an empty directory otherwise, never in the repository, so nothing it writes can land in the
engine's sources. `--env-file` loads a provider's key file into the agent's environment without
printing it, and `POCKET_EVAL_RUNTIME` names a copy of the runtime for the harness to start, so
builds made while a long run goes on do not change what it measures. The harness passes the same
copy to the agent as `POCKET_RUNTIME`, which the MCP server's `runtime_start`, its scenario, bench
and headless-run tools and the CLI's `run` and `scenario` launch instead of building the checkout:
in the run of fifty-five, an agent's `runtime_start` built the engine while it was being edited,
failed to compile, and the agent spent four calls waiting for the sources to settle.

Copy the runtime to a new file for `POCKET_EVAL_RUNTIME` (`rm` the old copy first: macOS kills a
signed binary that was overwritten in place, and every task then fails with the runtime gone before
it listened), and pass `--rows <file>` to keep each task's result as it finishes, so a run cut short
keeps what it did:

```bash
POCKET_EVAL_RUNTIME=build/eval-runtime/pocket_runtime python3 tools/scripts/agent_eval.py --timeout 900 --json --rows build/eval-rows.jsonl \
    --runner "python3 tools/scripts/runners/pi_agent.py --agent pi --via extension --model deepseek/deepseek-flash --env-file ~/.omp/agent/.env"
```

`tools/scripts/runners/opencode_agent.py` does the same with opencode
(`opencode run --format json --auto`), by default with GLM 5.3 Flash on the Z.ai coding plan
(`zai-coding-plan/glm-5.3-flash`, `--model` for another), the agent used for gameplay runs from
2026-10-01 on. `--via mcp` declares the `pocket` MCP server in an `opencode.json` the runner writes
where the agent starts (opencode merges it over the user's own configuration, which is left alone);
`--via shell` gives `pocket rpc`. opencode waits on a piped stdin for more of the message, so the
runner closes it. On a subscription plan the cost it reports is 0; tokens, turns and tool calls by
tool are counted from its `step_finish` and `tool_use` events.

```bash
POCKET_EVAL_RUNTIME=build/eval-runtime/pocket_runtime python3 tools/scripts/agent_eval.py --timeout 900 --json \
    --runner "python3 tools/scripts/runners/opencode_agent.py --via mcp"
```

With `POCKET_EVAL_TARGET=ios-sim` (and `POCKET_EVAL_DEVICE`, a simulator's name) the game runs as an
app in the iOS Simulator instead (`IosEnv` in `sdk/python/pocket_env.py`:
`pocket run <project> --ios`, paused, its control server on a free port of the Mac), and the runner
and the checks talk to it from the Mac as they talk to a desktop runtime. The app carries its own
copy of the game, so the tasks that edit the project's files are left out. On 2026-10-01 the
reference solutions passed all thirty of the others on a simulated iPhone 18 Pro (iOS 27) in 143
seconds, the renderer's checks among them (glass, the grey post effect, the lamp's shadows), and
hello's state after 60 ticks there was the desktop's and Linux's to the last digit.

With `POCKET_AGENT_TRACES=<dir>` the runners keep each task's event stream there, and
`tools/scripts/trace_report.py <dir>` says what the agents spent their context on: per task the
turns, tokens and calls and the largest output, and over the run the tools' output by tool and
command, the files read and how much of them, and the calls that failed (`--task <name>` lists one
task's calls in order). Most of what this file's Results report about reading and about where an
agent got stuck came from it. The harness keeps the same summary in each task's row (`trace`: turns,
tokens, calls, KB read, failed calls, and the calls before and after the first edit) and marks it
`stuck` when the agent made at least fifteen calls after its first edit and more than twice as many
as before it: the shape of a fix that did not take for a reason the agent had to dig for, as
`frozen_coins` had before a reload dropped the old handlers.

## Results

2026-09-29, the debug build, one run of each:

| Agent | Model | Reach | Passed | Wall time | Tokens | Cost | Tool calls | Report |
|---|---|---|---|---|---|---|---|---|
| oh-my-pi 18.4.3 | DeepSeek V4 Flash | MCP, started at the repository root | 14/15 | 27 min | 10.2 M | $0.30 | 309 | `omp-deepseek-mcp.json` |
| oh-my-pi 18.4.3 | DeepSeek V4 Flash | MCP, started outside the repository | 15/15 | 12.6 min | 8.2 M | $0.22 | 275 | `omp-deepseek-mcp-2.json` |
| pi 0.87.1 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension | 15/15 | 8.4 min | 3.3 M | $0.15 | 208 | `pi-deepseek-extension.json` |
| pi 0.87.1 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all sixteen tasks, after the rendering work | 16/16 | 8.8 min | 3.1 M | $0.15 | 195 | `pi-deepseek-extension-2.json` |
| oh-my-pi 18.4.3 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all twenty-five tasks | 24/25 | 33 min | 12.9 M | $0.22 | 514 | `omp-deepseek-full.json` |
| oh-my-pi 18.4.3 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all twenty-nine tasks, 2026-10-01, the release build | 29/29 | 24 min | 14.2 M | $0.23 | 578 | `omp-deepseek-full-2.json` |
| oh-my-pi 18.4.3 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all thirty-nine tasks, every one outside the repository, 2026-10-01 | 38/39 | 31 min | 14.8 M | $0.32 | 612 | `omp-deepseek-full-3.json` |
| oh-my-pi 18.4.3 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all forty-five tasks, 2026-10-01 | 44/45 | 43 min | 21.0 M | $0.44 | 815 | `omp-deepseek-full-4.json` |

A cheap model passes every task through the engine's commands and docs, for a few cents a task: the
claim of `docs/agent-first.md` measured instead of asserted. What the runs showed:

- **The cost is in design, not in commands.** In the last two runs every command task and one-line
  edit took between 4 and 35 seconds and at most three cents. The two mechanics and the two file
  tasks took most of the time and money (oh-my-pi 588 of 759 seconds and $0.16 of $0.22; pi 323 of
  501 seconds and $0.09 of $0.15): the agent reads the game's script, decides where the state goes,
  edits, and plays the result, often more than once. The coin that comes back after three seconds
  was the most expensive task in all three runs.
- **The first failure was the engine's.** In the first run oh-my-pi destroyed the playground's
  enemies correctly, the game's own script then touched one, and the runtime exited under the
  harness. A served runtime now keeps serving after a script error (the simulation stops, `state`
  says why), and the task passes in every later run.
- **Watching the agents changed the interface.** Before these runs a first trace had an agent pass
  `world.set` its fields under the wrong key, be told `{"ok": true}`, and spend a dozen calls
  reading the engine's C++ to learn why nothing changed. Commands now refuse a parameter they do not
  take and name the ones they do, and every command explains itself (`help`,
  `commands {usage: true}`; `docs/mcp.md`). In these runs the agents still read files in the command
  tasks (25 to 45 reads, greps and shell calls beside 41 to 56 engine calls), mostly the docs the
  task lists.
- **Lighting through commands.** `night_lamp` came after these runs, with the spot lights, the
  clustered lights and their shadows; both agents passed it at the first try, pi in 13 seconds for
  half a cent without opening a file (all 13 of its calls went to the engine) and oh-my-pi in 18
  seconds for a cent after reading two (`night-lamp-pi.json`, `night-lamp-omp.json`).
- **A bigger engine did not cost more.** The last run came after the renderer grew clustered lights,
  their shadows, volumetric fog, reflections, TAA and the rest, with their commands and docs (the
  agent's documentation set gained `docs/design/rendering.md`): pi passed all sixteen tasks in about
  the same time and for the same fifteen cents, with fewer tool calls. The two mechanics and the two
  file tasks were again most of it (395 of 529 seconds, $0.10 of $0.15).
- **New features from the docs alone.** The three tasks on the terrain, scattering and vehicles went
  to pi the day those features landed, with nothing but the documentation set (which gained
  `docs/design/terrain.md` and `docs/design/networking.md`): it passed all three at the first try in
  75 seconds for under three cents (`pi-deepseek-new-features.json`). It raised the hill with
  repeated `terrain.sculpt` calls checked against `terrain.height`, strewed 1092 flowers with a
  `Scatter` within the slope limit, and drove the car through the game's `throttle` action rather
  than the Vehicle's field, which the script overwrites every tick.
- **Water, the same way.** The two tasks on water went to oh-my-pi with the extension the day water
  landed (the documentation set gained `docs/design/water.md`): 2 of 2 at the first try in 69
  seconds for a cent and a half (`omp-deepseek-water.json`). The raft needed a mass the lake could
  hold up, which the task does not give: the default mass sinks a board of that size in water of the
  default density, and the agent chose a lighter one from the component's docs; the lake was
  stilled, cleared and raised with `world.set` and its surface read back with `water.height`.
- **Painting and planting.** The two tasks on terrain painting and scattered copies that sway and
  collide went to oh-my-pi with the extension the day those features landed (2026-09-29): 2 of 2 at
  the first try in two minutes for two cents (`omp-deepseek-terrain-paint.json`). The road took 26
  seconds and eight calls: one stroke of `terrain.paint` through points along the line, checked with
  `terrain.height`. The reed field took 98 seconds and 32 calls, most of them reading how a copy's
  size and `collide` combine; it fixed the copies' size range so each collider is exactly 0.1
  across, which the task asks for and does not spell out.
- **Weather.** `stormy_dusk` (the atmosphere sky, its clouds, the sun set low in the west and the
  Wind) went to oh-my-pi the day the atmosphere landed: passed at the first try in 40 seconds and 18
  calls for under a cent (`omp-deepseek-stormy-dusk.json`); it turned the sun from the component's
  docs and read the reddened sun light back from `render.stats`, the same 0.55, 0.28, 0.05 the
  reference gets.
- **Layers and glass.** The two tasks on terrain texture layers and glass went to oh-my-pi with the
  extension the day those features landed: 2 of 2 at the first try in a minute and a half for under
  three cents (`omp-deepseek-layers-glass.json`). The dirt patch took 82 seconds and 19 calls: it
  read the Terrain's layers, changed the sand's height rule and wrote them back whole (a list field
  is set whole), painted the dirt by name with `terrain.paint {layer}` and read its share back from
  `terrain.height`. The window took 13 seconds and 7 calls, with `transmission`, `ior` and
  `clearcoat` found in the MeshRenderer's documentation and `render.stats.glass` read after a tick.
- **Languages and levels.** The two tasks that span files beyond the script went to oh-my-pi with
  the extension the day after right-to-left text and components from Blender landed: 2 of 2 at the
  first try in five minutes for three cents (`omp-deepseek-locale-blender.json`). The Persian
  language took 226 seconds and 45 calls (22 in the shell, 15 file reads, one file written and three
  edited): the check found every English key translated, Persian's own letters rather than the
  Arabic file copied, the language named in all four files, and the score drawn as `امتیاز ۰` in
  Persian digits through the `{score, number}` placeholder. The Blender level took 72 seconds and 26
  calls (20 in the shell, one file written): the saved `.blend` gave a static floor a ball rests on
  at 1.5 (the agent answered 1.495) and a pillar with its static body, its 0.5 by 1.5 by 0.5 box and
  50 health, all from custom properties.
- **All twenty-five at once.** The full set went to oh-my-pi with the extension after the weather
  work: 24 of 25 in 33 minutes for 22 cents (`omp-deepseek-full.json`). The twenty command tasks
  took 15 minutes and 16 cents together; the two mechanics took 15 minutes and six cents, the coin
  that comes back again the dearest at nearly ten minutes and five cents. The one failure was the
  reed field, which the same agent had passed the day before: this time it gave `Scatter.collide`
  0.1 on an entity scaled 0.1 across, so each collider came out a hundredth of a unit, since the
  radius is in the entity's own units. The field's documentation said so in four words; it now says
  what the scale does to it with an example, which is the kind of fix these runs are for.
- **After the agent tools.** The full set went to oh-my-pi again on 2026-10-01, after
  `project.apply`, `step {until}`, `project.brief`, the field checks on component writes and the
  release runtime for agent runs (ADR 0006) had landed: 29 of 29 in 24 minutes for 23 cents
  (`omp-deepseek-full-2.json`). On the twenty-five tasks both runs share, it took 1108 seconds
  against 1990, 17.7 cents against 21.6 and 439 tool calls against 514, and passed the one the
  earlier run failed; the coin that comes back took 203 seconds against 573. One run each, and runs
  of the same set have differed by up to twice, so this says the change did not cost anything rather
  than how much it saved. The run also showed how oh-my-pi reaches an extension's tool: as a device,
  a `write` to `xd://pocket` whose content is the tool's arguments (and a `read` of it for its
  description), so the runner's counts before this date list those calls as `write` and `read`; it
  now counts them as `pocket`, and `POCKET_AGENT_TRACES=<dir>` keeps every task's event stream. In
  the traces an agent that wanted two answers at once sent two commands through `curl` in one shell
  call; the `pocket` tool (and the MCP `runtime_command`) now takes `calls: [{method, params}, ...]`
  and answers each in order.
- **Forty-eight.** The full set with the eight tasks added since `full-4` went to oh-my-pi again
  late on 2026-10-01, after the press fix, the incremental world, the irradiance volumes and the
  ragdolls: 47 of 48 in 38 minutes for 44 cents, 765 calls (`omp-deepseek-full-5.json`). The failure
  was the checker's: that agent's merchant asks again after a sale, and the checker, taking the
  buying choice whenever it was offered, bought two potions in one talk. It now buys one a talk and
  leaves.
- **Forty-eight on opencode.** The full set went to opencode with GLM 5.3 Flash over MCP on
  2026-10-01 (the agent used for gameplay runs from that day): 48 of 48 at the first try in 112
  minutes, 549 tool calls against oh-my-pi's 765 on the same set, 16.7 million tokens, no call into
  the harness (`opencode-glm-full.json`; the coding plan reports no cost). It was slower than
  oh-my-pi with DeepSeek (38 minutes): `snake` alone took 22 minutes for 14 calls, the model's turns
  being long rather than many.
- **Fifty-five on opencode.** The full set went to opencode with GLM 5.3 Flash over MCP again late
  on 2026-10-01, with the iOS, Linux, compressed-model, voxel and diagnose work in: 52 of 55 in 122
  minutes, 22.3 million tokens, 753 tool calls (`opencode-glm-full55.json`). The three failures were
  one engine bug, one ambiguous brief and one checker bug. In `guard_view` the agent passed
  `tilemap.fov` a point as `[x, y]`; a JSON type error deep in the handler ended the runtime. A
  value of the wrong type is now a `bad_args` answer from any command, and sight and field of view
  take `[x, y]`. The same agent's `runtime_start` had built the engine from the checkout while it
  was being edited and failed to compile; the harness now hands its runtime copy to the agent's
  tools as `POCKET_RUNTIME`. In `snake` the agent laid the grid on XZ, where the brief gave cells as
  (x, y) without naming the plane; the brief now names it, as `key_door`'s does. In `voxel_house`
  the house stood at (10, 0, 0) as asked, but the checker read the local `Transform` of a part under
  the instantiated root; it reads the world position now. Run again after the fixes, the three
  passed: `guard_view` in 30 seconds and 9 calls against 660 and 39, `snake` in 661 and 22,
  `voxel_house` in 179 and 14 (`opencode-glm-rerun3.json`). The traces showed two more things worth
  an engine change: `merchant_talk` spent twenty-five calls pressing Space and clicking choices to
  see each branch of the conversation it wrote, which `dialogue.routes` now answers in one
  `script.eval` (`docs/design/dialogue.md`, Trying it); and `ui.click {id: "start"}`, a name where
  an id goes, was read as no element at all, though the dialogue documentation said it worked; names
  now stand for elements in every `ui.*` command.
- **Fifty-seven on opencode.** The set of fifty-seven (the fifty-five, `pause_menu` and `breakout`)
  went to opencode with GLM 5.3 Flash over MCP again on the night of 2026-10-01, after the fixes the
  run of fifty-five asked for, the friction fixes from its failed calls, `world.query` field paths,
  `Behavior` and the template's finished look: 57 of 57 at the first try in 131 minutes, 20.6
  million tokens, 713 tool calls (`opencode-glm-full57.json`). On the fifty-five both runs had, 52
  passed before and 55 now, in 115 minutes against 122, 632 calls against 753 and 17.2 million
  tokens against 22.3 (`tools/scripts/compare_runs.py`); `guard_view` took 42 seconds and 12 calls,
  `snake` 503 and 23, `voxel_house` 232 and 19, and `breakout` passed in 660 seconds and 55 calls at
  the edge of the runner's time. Failed calls fell from 48 to 15. Of those, six were opencode's own
  `edit` tool missing the text it was to replace; two were `world.spawn` refusing `Transform.pos`,
  which `world.spawn` and `world.set` now read as `position` and say so in `renamed`; two asked
  `tilemap.get` and `tilemap.text {entity}` to read a map, which now read a cell or the rows; and
  two were `pocket_scenario` calls cut off by the MCP client's minute because the tool, installed
  before it honoured `POCKET_RUNTIME`, was building the engine from the checkout being edited (the
  agent ran the same scenario through the shell in seconds).
- **Three new tasks.** `platformer`, `sokoban` and `watchman` went to opencode with GLM 5.3 Flash
  over MCP on 2026-10-02: 3 of 3 at the first try (`opencode-glm-new3.json`). The platformer took
  311 seconds and 41 calls: the agent drew its tiles as an SVG and laid the level with
  `tilemap.fromText`, having read the sprites sample's script after guessing its file's name wrong
  twice (the project guide now gives each sample's entry and scene). The puzzle took 482 seconds and
  27 calls, keeping its own grid as the brief allowed, and the watchman 453 and 48 with a `Behavior`
  of two states; its one failed call applied a `scene.json` that was not JSON, which `project.apply`
  answered without saying where (a scene or prefab that does not parse is now answered with the
  line, the column, the reason and the line itself).
- **The villagers.** `villagers` went to opencode with GLM 5.3 Flash the day it was added and failed
  twice, each time on the engine's account (`opencode-glm-villagers-1.json`, `-2.json`). The first
  agent gave its villagers a `Behavior` whose greeting state played a looping `wave` and whose
  wandering state named no clip, and `Animator.locomotion` would not take back a looping clip it had
  not chosen, so they waved as they walked away; locomotion now takes it back once the entity has
  moved a third of a second. The second agent, which found the humanoid and its clips through
  `docs_search` in three calls instead of reading the animation document, moved its villagers from
  its own script and played the wave on the tick a villager stopped, while the gait's smoothed speed
  still said walking, and the gait took it at once; it now waits for that third of a second of
  movement. Its `world.lint` had also called every humanoid a missing file, which it no longer does.
  A third agent (`-3.json`, 359 seconds, 29 calls, six `docs_search` calls and no whole document
  read) wrote what the reference does, a `Behavior` that wanders and greets with `face`, and set the
  villagers' `home` to the centre of the square with a radius of 6; the engine took a home of zero
  to mean "not given" and let each wander round where it began, up to 7 units out. The origin is now
  a home like any other. A fourth (`-4.json`) kept the Player's place in its script and wrote it
  back every tick, so the check's `world.set` never moved it: the brief had not said, as the other
  games' briefs do, that the game reads the Player's place from its Transform, and now does. The
  fifth (`-5.json`) passed in 337 seconds and 24 calls, 0.84 million tokens against the first's 1.2
  and the second's 1.4. `fireworks` passed at its first run the day it was added, in 529 seconds and
  40 calls (`opencode-glm-fireworks.json`): the agent found `Trail` in the particles document and
  the components' list, burst sparks from an emitter of their own at each rocket, and looked at its
  own captures three times to tune the glow.
- **Sixty-two on opencode.** The whole set of sixty-two (the fifty-seven and the five games added
  since: `platformer`, `sokoban`, `villagers`, `fireworks`, `watchman`) went to opencode with GLM
  5.3 Flash over MCP on 2026-10-02: 61 of 62 at the first try in 164 minutes, 27.8 million tokens,
  896 tool calls (`opencode-glm-full62.json`). On the fifty-seven both runs had, all passed again,
  in 129 minutes against 131, 754 calls against 713 and 22.7 million tokens against 20.6; the spread
  is the model's, not the engine's (`snake` and `coin_respawn` ran to the runner's 660 seconds
  testing their games over and over and passed, `breakout` took 31 calls against 55). `sokoban`
  failed: its agent bound `move_y` with the same signs as `move_z`, so Down moved the player up, and
  no part of the project said which way +y points on a pad. A binding now turns an axis over with a
  minus (`"-pad:lefty"`, `docs/design/input.md`) and the template's `project.toml` gives a 2D game's
  `move_y` beside the 3D `move_z`. Of the sixteen failed calls, seven were opencode's `edit`; three
  read a `scene.json` the project did not have; the rest each led to a change:
  `until {state, value}` and the other names agents give a comparison are now read as `equals`,
  `events.why {id}` takes the id as the event's `seq`, `world.set` takes a component under its name
  in any case (`transform`, `mesh_renderer`), and a key that is a path into a field
  (`"layers.1.height"`, `"position.y"`) changes that part only, through lists by index.
- **Three more.** `glade`, `fps_shotgun` and `guards_footsteps` went to opencode with GLM 5.3 Flash
  over MCP the same day: 3 of 3 at the first try in 531 seconds (`opencode-glm-new3b.json`). The
  shotgun took 143 seconds and 12 calls on the fps sample, the footsteps 83 and 12 on the guards
  sample, and the glade 305 and 35: the agent found the props, the particle presets and the sound
  presets through six `docs_search` calls without reading a whole document, and looked at two
  captures of its own. Two of its calls were refused for a parameter's name, and are now read as
  meant: `log.tail {limit}` (as `n`) and `events.why {event: "campfire.stoked"}` (the latest event
  of that type).
- **Snow.** `snowy_evening` went to opencode with GLM 5.3 Flash the day the weather landed: passed
  at its first run in 76 seconds and 13 calls (`opencode-glm-snowy.json`), the `Weather` found
  through `world.schema {search}` without a document read. Two calls were refused:
  `help {command: "world.add"}`, the second run to look for that name, and `world.get` given
  `quiet`. `world.add` is now `world.set` by another name (it adds the component when missing), and
  `quiet` is let pass on any command.
- **The farm.** `farm_rain` (on a copy of the farm sample: rain that answers the field, read from
  the project's own `Plot` component) went to opencode with GLM 5.3 Flash the day the sample landed:
  passed at its first run in 253 seconds and 20 calls (`opencode-glm-farm-rain.json`). The agent
  read `components.toml`, the script and the sample's scenarios, found the `Weather` fields with one
  `docs_search`, made the change in four edits, watched `Weather.rain`, `Weather.wet` and two plots'
  water through a 500-tick `step` with `watch`, and ran the sample's scenarios before answering.
- **Sixty-seven on opencode.** The sixty-two and the five added since (`glade`, `fps_shotgun`,
  `guards_footsteps`, `snowy_evening`, `farm_rain`) went to opencode with GLM 5.3 Flash over MCP on
  2026-10-02: 63 of 67 at the first try in 191 minutes, 30.8 million tokens, 993 tool calls
  (`opencode-glm-full67.json`). The five passed again. Of the sixty-two both runs had, 58 passed
  against 61, in 177 minutes against 164: three whole-game tasks (`snake`, `platformer`, `sokoban`)
  ran to the runner's 660 seconds and were stopped, `platformer` after 13 turns (some fifty seconds
  a turn, most of it the model's reasoning), `sokoban` after 17 and `snake` after
  46. The run shared the machine with builds of the engine, which slowed the agents' own builds and
checks, so its times are not the engine's alone. `reed_field` failed (a reed's collider a hundredth
of a unit across) on the runtime copied when the run began, before `Scatter.collide` became world
units (`docs/design/terrain.md`), the change that failure led to. Of the twenty-one failed calls,
`world.get {field}`, `world.get {components}` and `wind.at {position}` are read as meant since
changes made while the run went on, `world.schema` answers a name it does not have with the nearest
(`BoxCollider`: `Collider`), and two `project.apply` calls met scripts that used SDK names
(`onStart`) without importing them: a script error now says where such a name comes from.
- **Sea, grass, a sailboat.** `sea_around`, `meadow` and `sailboat` (the oceans, grass and boats
  added that day) went to opencode with GLM 5.3 Flash over MCP with `village_weather` and `wolves`
  on 2026-10-02: 4 of 5 at the first try in 17 minutes, 81 tool calls (`opencode-glm-new5.json`).
  The sea took 88 seconds and 10 calls, the meadow 61 and 9, both found through `world.schema` and
  `docs_search` without a document read; the sailboat took 353 seconds and 19 calls, none failed,
  the agent finding `Boat` by `world.schema {search}` (whose answer, 29 KB for `wind` over ten
  components, is now short: 7 KB) and looking at its boat with `look_around` before answering. The
  village kept its square and took the evening rain in 314 seconds and 30 calls. `wolves` failed on
  the check, not the game: its agent's script was right, and the check counted every `sheep.caught`
  in the log, the agent's own tries before the check among them; it now counts those the check's own
  run makes. Run again, `wolves` passed in 144 seconds (`opencode-glm-wolves2.json`).
- **Two games more.** `pause_menu` and `breakout` went to opencode with GLM 5.3 Flash the day they
  were added: 2 of 2 at the first try (`opencode-glm-games2.json`). The menu took 274 seconds and 33
  calls; the agent read the interface documentation, went looking for an example and read the UI
  sample's script, wrote the game, and clicked through it by the buttons' names. The breakout took
  661 seconds and 36 calls: the runner's eleven minutes ran out while the agent was still testing a
  finished game, having just destroyed all but one brick to see `level.clear` come.
- **The five games again.** After a new project's guide began listing the engine's samples
  (2026-10-01), the five whole-game tasks went to opencode with GLM 5.3 Flash once more: 3 of 5
  (`opencode-glm-games5.json`). `dodge` passed in 314 seconds and 21 calls (382 and 29 in the run of
  fifty-five), `snake` in 541 and 34, `pause_menu` in 503 and 49 (274 and 33 the day before: one run
  each says how much runs vary). The menu's agent opened the UI sample from the guide's list rather
  than guessing its path, then still read the SDK's sources to learn how its JSX works. `breakout`
  failed on the game: a ball sent up into a brick broke it and the one above, and the runner's
  eleven minutes ran out. `key_door` failed on the checker: the agent's player stopped at the door's
  center, x 8, and the check wanted it short of 7.9, which the brief never said (it gives the door's
  place, not its width); the check now allows up to 8.05.
- **opencode with GLM 5.3 Flash.** The first two tasks given to `opencode_agent.py` over MCP
  (2026-10-01): `spawn_named` in 35 seconds and 5 calls, `merchant_talk` (a dialogue script written
  and wired into the game) in 271 seconds and 26 calls, both at the first try.
- **Cloth.** `yard_flag` went to opencode with GLM 5.3 Flash the day cloth landed (2026-10-01):
  passed at the first try in 186 seconds and 14 calls, the flag streaming 1.51 past its pole
  (answered 1.53) (`opencode-glm-yard-flag.json`).
- **Going limp.** `knockout` (health, and a ragdoll when it runs out) went to oh-my-pi the day
  ragdolls landed (2026-10-01): passed at the first try in 78 seconds and 33 calls for a cent and a
  half (`omp-deepseek-knockout.json`).
- **Reading less.** The traces of the forty-eight on opencode showed `docs/generated/sdk.md` (75 KB)
  read whole in `dodge`, `key_door` and `snake`, at 53 KB a read, and every later turn carrying it.
  After a new project's AGENTS.md gave its size and said to search it (and `commands {text: true}`
  became an index), `key_door`, `dodge` and `merchant_talk` went to opencode with GLM 5.3 Flash
  again (2026-10-01): 3 of 3 (`opencode-glm-reading.json`). In both games the agent searched the
  reference with `grep` and read only parts of it (35 and 44 KB read in all, against 84 and 66), and
  the tasks took 0.61 and 0.65 million tokens against 1.67 and 1.56, in 16 and 17 turns against 28
  and 34. `merchant_talk`, which never opened those files, read the same 15 KB both times and took
  1.70 million tokens against 0.92, in 51 turns against 29: one run each says the reading changed
  and how much runs vary, not by how much the change saves.
- **A frozen game, and an engine bug.** `frozen_coins` went to opencode with GLM 5.3 Flash the day
  it landed (2026-10-01) and passed, but not cleanly: 660 seconds and 46 calls, the agent cut off at
  its ten minutes (`opencode-glm-frozen-coins.json`). It held `move_x`, stepped until
  `level.complete`, read the error that stopped the game (`glow@scripts/main.ts:20`, called from
  line 33) and fixed the line in its ninth call. Then `project.apply` failed with the same error,
  and the next 35 calls went on finding out why, until it deleted the script context from the
  registry by hand through `script.eval`. The engine was wrong: a script error stops the simulation,
  and nothing is dispatched to the scripts while one stands, including the unload a reload sends
  before loading the bundle again; the old handlers stayed, the new bundle's joined them, and the
  old tick threw again. A reload now clears the errors before the unload (`runtime_tests`
  `[reload]`). Run again after the change, `frozen_coins` took 60 seconds, 10 calls and 0.22 million
  tokens against 660, 46 and 1.78 (`opencode-glm-reload-voxels.json`). The same trace showed
  `script.eval` answering an error with its frames only (JavaScriptCore keeps the message out of
  `stack`) and without the SDK in reach (`Can't find variable: world`); it now answers the message
  too, and the SDK's exports are names there.
- **A house of voxels.** `voxel_house` went to opencode with GLM 5.3 Flash the day voxel models
  landed: passed at the first try in 186 seconds and 16 calls, a house 10 by 10 cells and 8 high in
  three colours (96 triangles), which it looked at with `asset_preview` before placing it
  (`opencode-glm-reload-voxels.json`).
- **The slow festival.** `festival_flags` went to opencode with GLM 5.3 Flash the day it landed
  (2026-10-01): passed at the first try in 155 seconds and 17 calls, a tick 5.99 ms before and 0.43
  after (`opencode-glm-festival.json`). It needed no measuring command: it read the scene, saw cloth
  woven 48 by 30 on every flag, searched the Cloth's documentation, lowered the weave, applied it
  and compared a capture before with one after.
- **Diagnosing.** The three diagnose-and-fix tasks went to opencode with GLM 5.3 Flash the day they
  and `script.profile` landed (2026-10-01): 3 of 3 at the first try in 12 minutes, 43 calls
  (`opencode-glm-diagnose.json`). On `slow_swarm` (468 seconds, 18 calls) the agent read the script,
  searched the command list for "profil", found `script.profile` and measured with it before its
  first edit and after each of the next four, ending at 0.23 ms of script a tick against the broken
  game's 20.14 (measured while other work loaded the machine) with the swarm the same tick for tick.
  `leaky_cannon` (192 seconds, 17 calls) it found by counting `Pellet_*` entities and reading the
  landing events, then changed the sweep's condition in one edit. `sinking_crate` (62 seconds, 8
  calls) it found by reading the scene and the physics documentation, and answered `Collider.group`.
- **Conversations as text.** `merchant_talk` went to oh-my-pi the day dialogue landed (2026-10-01):
  passed at the first try, but in 336 seconds and 85 calls for five cents
  (`omp-deepseek-merchant-talk-1.json`). The script and the wiring were done in a dozen calls; the
  rest went on playing the conversation to see it work. Pressing Space once a tick left the line
  where it was, because `input.press` of a key still down from the last press only held it longer,
  and `ui.key` did nothing, because the box looked for keys held and `ui.key` lets the key up before
  the next tick; the agent read the engine's input code to find out why. Presses a tick apart are
  now two presses and the box acts on keys that went down since the last tick; rerun, the task
  passed in 84 seconds and 38 calls for under a cent and a half
  (`omp-deepseek-merchant-talk-2.json`).
- **Levels as text.** `vault_level` went to oh-my-pi the day maps in characters landed: passed at
  the first try in 35 seconds and 15 calls for under a cent, the vault drawn in rows of characters
  through `tilemap.text` (`omp-deepseek-vault-level.json`).
- **Music as text.** `theme_song` went to oh-my-pi the day scores landed: passed at the first try in
  55 seconds and 26 calls for under a cent (`omp-deepseek-theme-song.json`), with a four-track tune
  close to the documentation's example. Its score gave one instrument a `reverb`, which the
  synthesizer then passed over; recipes now refuse a field they do not have, naming the ones they do
  (and where reverb lives).
- **Sound as text.** `coin_chime` (a `.sfx` recipe of the agent's own, played on every coin) went to
  oh-my-pi the day recipes landed (2026-10-01): passed at the first try in 158 seconds and 41 calls
  for under two cents (`omp-deepseek-coin-chime.json`).
- **Art as text.** `svg_coin` (an SVG file the agent writes, shown as a sprite) went to oh-my-pi
  with the extension the day SVG images landed (2026-10-01): passed at the first try in 30 seconds
  and 17 calls for half a cent (`omp-deepseek-svg-coin.json`).
- **A third game.** `snake` (a grid, segments named from the head, food the checker places by its
  Transform) went to oh-my-pi with the extension on 2026-10-01: passed at the first try in 273
  seconds and 73 calls for under five cents (`omp-deepseek-snake.json`).
- **Light and sight.** The two tasks on lit 2D and tile-map sight went to oh-my-pi with the
  extension the day those landed (2026-10-01): 2 of 2 at the first try, `night_level` in 68 seconds
  and 21 calls, `guard_view` in 37 seconds and 10 calls, two cents together
  (`omp-deepseek-light-sight.json`).
- **Forty-five.** The full set with the six tasks added since (light and sight, snake, the SVG coin,
  the sound recipe and the score) went to oh-my-pi again later on 2026-10-01: 44 of 45 in 43 minutes
  for 44 cents (`omp-deepseek-full-4.json`); on the thirty-nine tasks both runs share it took 1935
  seconds against 1863, 33.6 cents against 32.0 and 623 calls against 612: the same, one run each.
  The failure was `coin_chime`, and again the run's state was the cause: the agent had left `move_x`
  held for hundreds of ticks while testing, a hold outlives `project.reload`, and the restarted game
  walked into a coin before the check had pressed anything, so the chime "played before any coin". A
  restart now lets go of held actions and stops the last run's sounds, as a fresh run would start;
  rerun, the task passed (`omp-deepseek-full-4-reruns.json`). The run also caught an agent reading
  the harness: the copy of `walker_sprint` was named after the task, and the agent searched the
  repository for that name and read its check. Copies now have names that say nothing of their task;
  rerun, it passed without looking.
- **Thirty-nine, outside the repository.** The full set went to oh-my-pi again later on 2026-10-01,
  with the ten tasks added since (hitboxes, own components, waiting, meshes and maps by code, 2D
  physics, shaders and the two games) and every task on a copy outside the repository: 38 of 39 in
  31 minutes for 32 cents (`omp-deepseek-full-3.json`), no harness file read. On the twenty-nine
  tasks both full runs share it took 1189 seconds against 1461, 20.3 cents against 23.1 and 413 tool
  calls against 578 (the Persian locale and the double jump each in under half the time), one run
  each. The ten new tasks took eleven minutes and twelve cents. The one failure was `dodge`, and it
  was the check's: the game the agent wrote was right, but after the agent's own session the harness
  reloaded the project, which did not seed the random numbers again, so the rocks fell in other
  places than in a fresh run and one struck the Player before the check looked at a falling rock;
  the frozen game then read as a rock that did not fall. The same files, rebuilt from the trace and
  checked on a fresh runtime, pass. Two changes followed: a restart (`project.reload` of the scene
  and the scripts) now seeds `random()` and `Math.random` again, as a fresh run would, and the check
  moves the Player out of the rocks' columns (the brief has the game read its Transform) before it
  watches them.
- **Hits, own components, waiting.** The three tasks on hitboxes, a project's own components and
  `step {until}` went to oh-my-pi with the extension the day those landed: 3 of 3 at the first try
  in a minute for just over a cent (`omp-deepseek-combat-components.json`). In the trace of the coin
  task the agent read `project.brief` first, sent its questions three at a time through `calls`, and
  finished with one call holding the action and stepping until the coin event; the spike trap took
  ten calls to the engine and no file but the docs.
- **Things made by code and 2D physics.** Three tasks came with meshes and maps made by code and the
  2D rigid bodies, and went to oh-my-pi with the extension the day those landed, the first run with
  every task outside the repository (2026-10-01): 3 of 3 at the first try in four minutes for eight
  cents, none peeking at the harness (`omp-deepseek-made-physics2d.json`). The roof took 25 seconds
  and 14 calls: `mesh.create` with a base it chose to add (six triangles). The walled room took 57
  seconds and 17 calls: `tilemap.create`, four `tilemap.fill` strips and a pebble that came to rest
  on the floor at -6.75. The bridge took 152 seconds and 27 calls, most of them spent on the one
  thing the task does not say: an entity holds one `Joint2D`, and six hinges hold five planks
  between two points, so the agent spawned a sixth static body at the far bank to carry the last
  hinge, and followed the five planks' heights through two seconds with one `step {watch}`. It found
  the planks' ids by counting on from `world.roots`; entity fields now take names.
- **Shaders.** The two tasks on WGSL came with post effects and materials and went to oh-my-pi the
  same day: 2 of 2 at the first try in about a minute each for just over a cent each
  (`omp-deepseek-shaders.json`). The grey flashback took 62 seconds and 19 calls, one `render.post`
  with code. The orange ball took 68 seconds and 30 calls: a material file and the ball's spawn in
  the script given it, the colour coming out exactly #ff6600 on screen (255, 102, 0), so the agent
  wrote the orange in linear light, as the material's docs say its output is.
- **Whole games from a brief.** The two game tasks went to oh-my-pi with the extension on
  2026-10-01: 2 of 2 at the first try, `dodge` in 155 seconds and 43 calls for five and a half
  cents, `key_door` in 262 seconds and 54 calls for nine cents (`omp-deepseek-games.json`). Each
  wrote its `project.toml` actions, an empty scene and the script, and applied and played it through
  the engine (the dodge agent stepped the game and looked at a capture of it before answering). Both
  reached into the repository: the dodge agent read the tool's sources to see how `"pocket"`
  resolves for types, the door agent listed `tests/evidence/components` for an example of a
  project's components; neither touched the harness or its evidence (`pi_agent.py` now counts only
  those as `peeked`). The door agent applied its script nine times with `pocket_apply`, editing
  between them. Twice an agent passed a command's name as a key (`{"help": true}`); the pi tool now
  takes that as the command.
- **A guide in the project.** Since `pocket new` writes an `AGENTS.md` into every project (how to
  run, read, change and check the game), the copies the script tasks work on carry one, and oh-my-pi
  reads it on starting there. The eight script tasks went to it again the same day: 8 of 8 in 477
  seconds against 615 in the full run before (`omp-deepseek-script-guide.json`); every one that
  edited a script then applied it with `pocket_apply` rather than restarting anything. The double
  jump took 67 seconds against 217, the sprint 75 against 47: one run each, so the guide is not
  shown to save time, only to cost none.
- **Context is the bill.** Over nine tenths of every run's tokens are cache reads of the
  conversation so far, so what an agent carries into each turn decides the cost. Started at the
  repository root, oh-my-pi also took in its guidance files and wandered into the engine's source
  (it once wrote a stray file there); started outside, the same agent and model finished everything
  in under half the time for three quarters of the cost. pi with the two-tool extension carried the
  least and was the cheapest and fastest.

## Limits

Thirty-nine tasks (`grey_flashback` and `orange_ball` are shaders an agent writes; `dodge` and
`key_door` whole games from a brief; `roof_mesh`, `walled_room` and `plank_bridge` came with meshes
and maps made by code and the 2D rigid bodies; `spike_trap`, `brute_enemies` and `wait_for_coin`
with hitboxes, a project's own components and `step {until}`; the first three runs above were on the
first fifteen, the fourth on sixteen; `walker_sprint` came with the 3D characters, `hill_raise`,
`flowers` and `car_speed` with the terrain, scattering and vehicles, `raft` and `calm_lake` with
water, `sand_road` and `reed_field` with terrain painting and colliding, swaying copies,
`stormy_dusk` with the wind and the atmosphere, `dirt_patch` and `glass_window` with terrain layers,
glass and clear coats, `persian_locale` with right-to-left text and fallback fonts, `blender_level`
with components from Blender): nineteen edits or reads through commands, three one-line script
edits, two mechanics, four files made beside the script (a sound, a prefab, a language, and a
Blender level made by driving Blender) and one edit across `project.toml` and the script; none is
timed, and none needs art drawn, a level laid out or a scene composed by eye. A model scored here is
scored on reading the docs and making the right call or the right edit, which is the first thing an
agent must do and far from the last.
| `village_weather` | blank | a village square, then (a second agent, a follow-up) an evening in the rain: rain 0.8, 20:00, four lamps lit only after dark by the houses | the houses and well still there, the first `Weather`'s rain, the Sky's hour, each lamp's point light with `after_dark` |
