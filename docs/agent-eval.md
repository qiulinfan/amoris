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
| `ball_lower` | hello (a copy) | edit the script so the ball starts its fall at 1.0 | `state` |
| `spawn_stars` | hello (a copy) | edit the script to spawn five named spheres on start | `world.find`, `world.get` |
| `double_jump` | sprites (a copy) | design a mechanic: a second jump in the air, once per flight | `input.press`, `state` (the rise of a jump pressed again in the air) |
| `coin_respawn` | sprites (a copy) | design a mechanic: a collected coin returns after three seconds | `world.set`, `state` (the coin count once taken, at 1.7 s and at 3.2 s) |
| `jump_sound` | sprites (a copy) | make an asset and use it: write a WAV under `assets/` and play it on every jump from the ground | `assets.list`, `audio.list` (silent before a jump, a voice of the file after it, again after the next) |
| `lamp_prefab` | hello (a copy) | write a prefab file and instantiate it three times from the script | `project.read`, `world.find`, `world.get`, `world.describe` (three yellow spheres where asked, a point light under each) |

Five tasks change the world through commands, three ask a question about it, and seven ask for a change to the project's files: three a line or two of TypeScript, two a mechanic the runner has to design (where the jump counter lives and when it clears; what a coin is, where it was, how to bring it back later), with a check that plays the result rather than reads the source, and two that span files, a sound the runner has to make and play and a prefab it has to write and instantiate, where the check reads the file through the runtime and then looks at what the world does with it. The question tasks take their truth from the same commands at check time, so a runner that reads the world correctly passes them and one that guesses does not. The script tasks run on a copy of the sample (`pocket new --from`, under `build/agent-eval/`): the runner gets `project_dir` and edits the project's entry script there (`scripts/main.ts`, or `scripts/main.tsx` for the sprites sample; the task text names it), and when it returns the harness bundles the copy again with the tool, reloads the project (`project.reload`: a fresh world from the scene, the edited script started over it) and steps two ticks before the check. The copy and its bundle are removed afterwards.

## Runners

```bash
python3 tools/scripts/agent_eval.py --runner reference        # the solutions in the harness itself: 15/15
python3 tools/scripts/agent_eval.py --runner null             # does nothing: the floor, 0/15
python3 tools/scripts/agent_eval.py --runner "claude -p" --tasks spawn_named,recolor --json
```

`reference` is the harness checking itself: every solution is one or two commands, which is the point. `null` sets the floor. Any other value is a shell command run once per task with the task as JSON on stdin (`task`, `project`, `rpc_url`, `docs`, `notes`); it may print a JSON object with an `answer` as its last line, and `POCKET_RPC_URL` is in its environment. A runner that wraps a model gives it the docs listed and the RPC; what it does in between is its own business. `--json` prints the report (`runner`, `passed`, `total`, per task `ok`, `seconds`, `detail`, `answer`, `error`), and the exit code is 0 only for a full pass.

`tests/evidence/agent-eval/reference.json` and `null.json` are the two built-in runners' reports. No model has been scored yet: running one costs money, which is the user's call; the harness is ready for it.

## Limits

Fifteen tasks: eight one-step edits or reads through commands, three one-line script edits, two mechanics and two files made beside the script (a sound, a prefab); none is timed, and none needs art drawn, a level laid out or a scene composed by eye. A model scored here is scored on reading the docs and making the right call or the right edit, which is the first thing an agent must do and far from the last.
