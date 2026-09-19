# Evidence: the agent benchmark

Produced on 2026-09-19 with the debug (sanitized) build by `tools/scripts/agent_eval.py` (`docs/agent-eval.md`):

```
python3 tools/scripts/agent_eval.py --runner reference --json > tests/evidence/agent-eval/reference.json
python3 tools/scripts/agent_eval.py --runner null --json > tests/evidence/agent-eval/null.json
```

`reference.json`: the harness's own solutions pass all eleven tasks in about ten seconds of wall time (each task starts its sample in a fresh runtime; the playground tasks run up to 1500 ticks of setup first; the three script tasks copy and bundle the hello sample, edit its script, bundle it again and reload). `null.json`: a runner that does nothing fails all eleven, which shows the checks demand the change, the edit or the answer rather than the setup. The question tasks report the truth beside the answer in `detail` (the playground path is 22.882 units, five bodies lie within four units of the physics arena's origin, the root cause of the last hit is the enemy's `entity.spawned`).
