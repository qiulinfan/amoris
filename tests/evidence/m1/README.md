# M1 evidence: world model

Produced on 2026-09-18 with the debug (sanitized) build.

| File | Command | What it shows |
|---|---|---|
| `playground-240.json` | `pocket_runtime --project samples/playground --bundle build/ts/playground.js --headless --frames 240 --json --size 320x180` | A scene loaded from `scene.json`, enemies spawned and driven from TypeScript, the causal event log (`events.histogram`, `events.tail` with `cause` links), the world summary and the deterministic state hash. |
| `playground-tree.txt` | `world.tree` at tick 240 through the control server (`GET /tree`) | The AI-native tree: one line per entity with only the fields that differ from defaults. |
| `control-session.txt` | `--serve 4711 --paused --headless`, then JSON-RPC `step`, `world.query`, `events.since`, `quit` | An external process driving the simulation tick by tick. |
| `replay.txt` | `--record` then `--replay --headless` | The replayed run reproduces the recorded state hash. |

`pocket test --json` runs `core_tests`, `world_tests`, `runtime_tests` (Catch2) and `tests/ts/world.test.ts` (in-engine TypeScript).
