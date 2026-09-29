# The agent benchmark

`tools/scripts/agent_eval.py` measures the thing the engine is built for: whether a program can change a running project through the engine's own interface, given a task in words and the documentation. It is the "agent-first" claim (`docs/agent-first.md`) turned into a number that any model, tool or script can be scored on.

## What a task is

Each task starts one of the samples paused in a headless runtime with the JSON-RPC server (`docs/mcp.md`, the same interface `pocket mcp` exposes), runs the simulation to a point where the task is real (enemies about, a hit that needs explaining), hands the runner the task text, and then checks the world through the same commands. A task passes or fails; nothing is scored on style.

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
| `lamp_prefab` | hello (a copy) | write a prefab file and instantiate it three times from the script | `project.read`, `world.find`, `world.get`, `world.describe` (three yellow spheres where asked, a point light under each) |
| `hill_raise` | hills | raise the terrain at a point to a height without touching the ground 12 units away, and answer with the height | `terrain.height` (there, and around at 12 units, against the setup) |
| `flowers` | hills | strew red flowers over the terrain, only on ground no steeper than 20 degrees | `world.find`, `world.get`, `scatter.copies`, `terrain.height` (each flower's ground and its slope) |
| `car_speed` | drive | tune a car's top speed and power, drive it for five seconds through its action, and answer with its speed | `world.get` on the Vehicle (the settings, the speed) |
| `walker_sprint` | walker (a copy) | add an input action in `project.toml` and use it in the script: a sprint on LShift, 9 units a second instead of 5 | `input.describe`, `input.hold`, `state` (a second of walking with and without it) |
| `raft` | hills | spawn a dynamic box collider 2 by 0.2 by 1 and drop it into the lake so it floats (the mass is the runner's to choose), run 180 ticks, answer with the surface's height under it | `world.get` on its RigidBody and Collider, 120 more ticks, then `water.height` under it: within 0.3 of the surface, still over the lake |
| `calm_lake` | hills | make the lake still and clearer (8 units) and raise it half a unit, answer with its surface's height | `world.get` on the Water and its Transform, `water.height` |

Eleven tasks change the world through commands (`night_lamp` then answers from what the renderer reports about the frame it drew; `hill_raise` reshapes a terrain within a bound and answers with the height; `flowers` strews a Scatter within a slope limit; `car_speed` tunes a Vehicle and drives it through the game's own action, since the script sets the throttle every tick; `raft` floats a body on the lake, `calm_lake` changes the water), three ask a question about it, and eight ask for a change to the project's files: three a line or two of TypeScript, two a mechanic the runner has to design (where the jump counter lives and when it clears; what a coin is, where it was, how to bring it back later), with a check that plays the result rather than reads the source, and three that span files: a sprint that needs an action in `project.toml` and the speed it picks in the script, a sound the runner has to make and play and a prefab it has to write and instantiate, where the check reads the file through the runtime and then looks at what the world does with it. The question tasks take their truth from the same commands at check time, so a runner that reads the world correctly passes them and one that guesses does not. The script tasks run on a copy of the sample (`pocket new --from`, under `build/agent-eval/`): the runner gets `project_dir` and edits the project's entry script there (`scripts/main.ts`, or `scripts/main.tsx` for the sprites sample; the task text names it), and when it returns the harness bundles the copy again with the tool, reloads the project (`project.reload`: `project.toml`'s settings read again, a fresh world from the scene, the edited script started over it) and steps two ticks before the check. The copy and its bundle are removed afterwards.

## Runners

```bash
python3 tools/scripts/agent_eval.py --runner reference        # the solutions in the harness itself: 16/16
python3 tools/scripts/agent_eval.py --runner null             # does nothing: the floor, 0/16
python3 tools/scripts/agent_eval.py --runner "claude -p" --tasks spawn_named,recolor --json
```

`reference` is the harness checking itself: every solution is one or two commands, which is the point. `null` sets the floor. Any other value is a shell command run once per task with the task as JSON on stdin (`task`, `project`, `rpc_url`, `docs`, `notes`); it may print a JSON object with an `answer` as its last line, and `POCKET_RPC_URL` is in its environment. A runner that wraps a model gives it the docs listed and the RPC; what it does in between is its own business. `--json` prints the report (`runner`, `passed`, `total`, per task `ok`, `seconds`, `detail`, `answer`, `error`), and the exit code is 0 only for a full pass.

`tests/evidence/agent-eval/reference.json` and `null.json` are the two built-in runners' reports; coding agents with a model are below.

## Coding agents

`tools/scripts/runners/pi_agent.py` hands each task to a pi-family coding agent, oh-my-pi (`omp`) or pi, with the model it is set up for or the one `--model` names, and prints the answer with what the task cost: tokens, dollars, tool calls by tool, turns and seconds; the harness adds them up as `totals`. `--via` says how the agent reaches the engine: `mcp` through the `pocket` MCP server (`docs/mcp.md`), declared in a `.mcp.json` the runner writes where the agent starts; `shell` through `pocket rpc`; `extension` through pi's `pocket` and `pocket_look` tools (`integrations/pi/pocket.ts`). The agent starts in the project copy for a task that edits files and in an empty directory otherwise, never in the repository, so nothing it writes can land in the engine's sources. `--env-file` loads a provider's key file into the agent's environment without printing it, and `POCKET_EVAL_RUNTIME` names a copy of the runtime for the harness to start, so builds made while a long run goes on do not change what it measures.

```bash
POCKET_EVAL_RUNTIME=build/eval-runtime/pocket_runtime python3 tools/scripts/agent_eval.py --timeout 900 --json \
    --runner "python3 tools/scripts/runners/pi_agent.py --agent pi --via extension --model deepseek/deepseek-flash --env-file ~/.omp/agent/.env"
```

## Results

2026-09-29, the debug build, one run of each:

| Agent | Model | Reach | Passed | Wall time | Tokens | Cost | Tool calls | Report |
|---|---|---|---|---|---|---|---|---|
| oh-my-pi 18.4.3 | DeepSeek V4 Flash | MCP, started at the repository root | 14/15 | 27 min | 10.2 M | $0.30 | 309 | `omp-deepseek-mcp.json` |
| oh-my-pi 18.4.3 | DeepSeek V4 Flash | MCP, started outside the repository | 15/15 | 12.6 min | 8.2 M | $0.22 | 275 | `omp-deepseek-mcp-2.json` |
| pi 0.87.1 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension | 15/15 | 8.4 min | 3.3 M | $0.15 | 208 | `pi-deepseek-extension.json` |
| pi 0.87.1 | DeepSeek Flash (`deepseek/deepseek-flash`) | the pi extension, all sixteen tasks, after the rendering work | 16/16 | 8.8 min | 3.1 M | $0.15 | 195 | `pi-deepseek-extension-2.json` |

A cheap model passes every task through the engine's commands and docs, for a few cents a task: the claim of `docs/agent-first.md` measured instead of asserted. What the runs showed:

- **The cost is in design, not in commands.** In the last two runs every command task and one-line edit took between 4 and 35 seconds and at most three cents. The two mechanics and the two file tasks took most of the time and money (oh-my-pi 588 of 759 seconds and $0.16 of $0.22; pi 323 of 501 seconds and $0.09 of $0.15): the agent reads the game's script, decides where the state goes, edits, and plays the result, often more than once. The coin that comes back after three seconds was the most expensive task in all three runs.
- **The first failure was the engine's.** In the first run oh-my-pi destroyed the playground's enemies correctly, the game's own script then touched one, and the runtime exited under the harness. A served runtime now keeps serving after a script error (the simulation stops, `state` says why), and the task passes in every later run.
- **Watching the agents changed the interface.** Before these runs a first trace had an agent pass `world.set` its fields under the wrong key, be told `{"ok": true}`, and spend a dozen calls reading the engine's C++ to learn why nothing changed. Commands now refuse a parameter they do not take and name the ones they do, and every command explains itself (`help`, `commands {usage: true}`; `docs/mcp.md`). In these runs the agents still read files in the command tasks (25 to 45 reads, greps and shell calls beside 41 to 56 engine calls), mostly the docs the task lists.
- **Lighting through commands.** `night_lamp` came after these runs, with the spot lights, the clustered lights and their shadows; both agents passed it at the first try, pi in 13 seconds for half a cent without opening a file (all 13 of its calls went to the engine) and oh-my-pi in 18 seconds for a cent after reading two (`night-lamp-pi.json`, `night-lamp-omp.json`).
- **A bigger engine did not cost more.** The last run came after the renderer grew clustered lights, their shadows, volumetric fog, reflections, TAA and the rest, with their commands and docs (the agent's documentation set gained `docs/design/rendering.md`): pi passed all sixteen tasks in about the same time and for the same fifteen cents, with fewer tool calls. The two mechanics and the two file tasks were again most of it (395 of 529 seconds, $0.10 of $0.15).
- **New features from the docs alone.** The three tasks on the terrain, scattering and vehicles went to pi the day those features landed, with nothing but the documentation set (which gained `docs/design/terrain.md` and `docs/design/networking.md`): it passed all three at the first try in 75 seconds for under three cents (`pi-deepseek-new-features.json`). It raised the hill with repeated `terrain.sculpt` calls checked against `terrain.height`, strewed 1092 flowers with a `Scatter` within the slope limit, and drove the car through the game's `throttle` action rather than the Vehicle's field, which the script overwrites every tick.
- **Water, the same way.** The two tasks on water went to oh-my-pi with the extension the day water landed (the documentation set gained `docs/design/water.md`): 2 of 2 at the first try in 69 seconds for a cent and a half (`omp-deepseek-water.json`). The raft needed a mass the lake could hold up, which the task does not give: the default mass sinks a board of that size in water of the default density, and the agent chose a lighter one from the component's docs; the lake was stilled, cleared and raised with `world.set` and its surface read back with `water.height`.
- **Painting and planting.** The two tasks on terrain painting and scattered copies that sway and collide went to oh-my-pi with the extension the day those features landed (2026-09-29): 2 of 2 at the first try in two minutes for two cents (`omp-deepseek-terrain-paint.json`). The road took 26 seconds and eight calls: one stroke of `terrain.paint` through points along the line, checked with `terrain.height`. The reed field took 98 seconds and 32 calls, most of them reading how a copy's size and `collide` combine; it fixed the copies' size range so each collider is exactly 0.1 across, which the task asks for and does not spell out.
- **Context is the bill.** Over nine tenths of every run's tokens are cache reads of the conversation so far, so what an agent carries into each turn decides the cost. Started at the repository root, oh-my-pi also took in its guidance files and wandered into the engine's source (it once wrote a stray file there); started outside, the same agent and model finished everything in under half the time for three quarters of the cost. pi with the two-tool extension carried the least and was the cheapest and fastest.

## Limits

Twenty-four tasks (the first three runs above were on the first fifteen, the fourth on sixteen; `walker_sprint` came with the 3D characters, `hill_raise`, `flowers` and `car_speed` with the terrain, scattering and vehicles, `raft` and `calm_lake` with water, `sand_road` and `reed_field` with terrain painting and colliding, swaying copies): sixteen edits or reads through commands, three one-line script edits, two mechanics, two files made beside the script (a sound, a prefab) and one edit across `project.toml` and the script; none is timed, and none needs art drawn, a level laid out or a scene composed by eye. A model scored here is scored on reading the docs and making the right call or the right edit, which is the first thing an agent must do and far from the last.
