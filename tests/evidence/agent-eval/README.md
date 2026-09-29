# Evidence: the agent benchmark

Produced on 2026-09-19 with the debug (sanitized) build by `tools/scripts/agent_eval.py` (`docs/agent-eval.md`):

```
python3 tools/scripts/agent_eval.py --runner reference --json > tests/evidence/agent-eval/reference.json
python3 tools/scripts/agent_eval.py --runner null --json > tests/evidence/agent-eval/null.json
```

`reference.json`: the harness's own solutions pass all seventeen tasks (regenerated 2026-09-29 with `walker_sprint`) in about twenty seconds of wall time (each task starts its sample in a fresh runtime; the playground tasks run up to 1500 ticks of setup first; the eight script tasks copy and bundle their sample, edit its files, bundle it again and reload the project; the two mechanic tasks then play the result: a jump pressed again in the air, a coin stood on and waited for; the two file tasks read the file back through the runtime and then play or look: a jump that is heard, three lamps from a prefab). `null.json`: a runner that does nothing fails all seventeen, which shows the checks demand the change, the edit or the answer rather than the setup. The question tasks report the truth beside the answer in `detail` (the playground path is 22.882 units, five bodies lie within four units of the physics arena's origin, the root cause of the last hit is the enemy's `entity.spawned`).

Coding agents with a model (2026-09-29, `docs/agent-eval.md`, Results), through `tools/scripts/runners/pi_agent.py` with the runtime frozen at `build/eval-runtime/pocket_runtime` (`POCKET_EVAL_RUNTIME`) and the DeepSeek key loaded from `~/.omp/agent/.env` (`--env-file`; never printed):

- `omp-deepseek-mcp.json`: oh-my-pi 18.4.3, its default DeepSeek V4 Flash, over MCP, started at the repository root: 14 of 15 in 27 minutes for $0.30. `clear_enemies` failed because the runtime exited on a script error after the enemies were destroyed; the served runtime has kept serving since, and the task passes alone (21 s, 18 calls).
- `omp-deepseek-mcp-2.json`: the same agent and model over MCP, started outside the repository (the runner writes a `.mcp.json` into the project copy or an empty directory): 15 of 15 in 12.6 minutes for $0.22.
- `pi-deepseek-extension.json`: pi 0.87.1 with `deepseek/deepseek-flash` and `integrations/pi/pocket.ts`: 15 of 15 in 8.4 minutes for $0.15.
- `pi-deepseek-extension-2.json` (2026-09-29): the same agent, model and extension on all sixteen tasks against the runtime after the rendering milestones: 16 of 16 in 8.8 minutes for $0.15, 195 tool calls.

- `night-lamp-pi.json`, `night-lamp-omp.json`: the task added afterwards (`--tasks night_lamp`, the runtime refrozen with spot lights and light shadows): pi with the extension passed in 13 s for $0.006, oh-my-pi over MCP in 18 s for $0.010.

Each task's `metrics` holds its tokens (input, output, cache reads), cost, tool calls by tool, turns and seconds.
