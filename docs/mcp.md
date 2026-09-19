# Driving Pocket from an agent: `pocket mcp`

`pocket mcp` serves the Model Context Protocol over stdio. Any MCP client (Claude Code, Codex, an IDE, a CI harness) gets the same commands that scripts and tests use: build and test the engine, run a project headless, or start a paused session and step it while reading the world as text.

## Register

Claude Code:

```bash
claude mcp add pocket -- /Users/you/aipocket/.pocket/pocket --root /Users/you/aipocket mcp
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.pocket]
command = "/Users/you/aipocket/.pocket/pocket"
args = ["--root", "/Users/you/aipocket", "mcp"]
```

The binary comes from `scripts/bootstrap.sh`. On a machine where Xcode itself is unusable, set `DEVELOPER_DIR=/Library/Developer/CommandLineTools` in the client's environment for the server process.

## Tools

| Tool | What it does |
|---|---|
| `pocket_doctor`, `pocket_build`, `pocket_test`, `pocket_gen` | The build tool with structured results (diagnostics with file, line, column; per-suite test results). |
| `pocket_scenario` | Run a project's gameplay scenarios (`scenarios/*.ts`) at several seeds: pass counts, ticks to pass, each failure's step and reason (`docs/design/scenarios.md`). |
| `pocket_bench` | Run a project's perception benchmarks (`benches/*.ts`): per question, whether the instruments answered correctly, the tokens it cost, and what frame-by-frame vision would have cost (`docs/design/scenarios.md`, Perception benchmarks). |
| `pocket_run_headless` | Run a project for N frames and return the JSON report: exposed state, state hash, world summary, event histogram and tail, optional capture. |
| `runtime_start` | Build, bundle and launch a project paused with the control server. Headless by default; `headless: false` opens a window. |
| `step` | Advance N ticks; returns tick, exposed state and hashes. |
| `world_tree`, `world_query`, `world_describe`, `world_schema` | The observable world as text and JSON. |
| `events_since` | The causal event log after a sequence number, with `cause` links; `runtime_command {method: "events.why", params: {seq}}` turns one event into its chain of causes. |
| `capture`, `render_pick` | The last frame as PNG, the entity id buffer, the entity under a pixel. |
| `ui_snapshot`, `ui_query`, `ui_click`, `ui_type`, `ui_key` | The interface as text, element lookup by name/text/type, and synthetic input through the same path a player's input takes (see `docs/design/pocket-ui.md`). With `runtime_start {editor: true}` the same tools operate the editor (`docs/editor.md`). |
| `runtime_command` | Any other command (`world.spawn`, `world.set`, `events.emit`, `world.save`, `save.write`, `input.hold`, `perf`, `log.tail`, `render.visible`, `render.unproject`, `tilemap.set/fill/save`, `nav.bake/path/reachable/nearest`, `recorder.at/diff/track/first` when the session started with `history`, ...); `runtime_commands` lists them. |
| `runtime_stop` | Quit the session and return its final report. |

## A session

1. `runtime_start { project: "playground" }` builds the runtime, bundles the TypeScript, starts `pocket_runtime --serve 0 --paused --headless` and reports the initial state.
2. `world_tree { depth: 2 }` shows the scene loaded from `scene.json`.
3. `step { ticks: 120 }` runs two simulated seconds; the exposed state says how many enemies exist and the player's health.
4. `events_since { seq: 0, type: "player." }` explains what hit the player; each `player.hit` carries the `cause` of the enemy's spawn event.
5. `capture { path: "out.png", ids: "ids.png" }` writes the picture and the id buffer and lists the visible entities with pixel counts.
6. `runtime_stop` returns the report with the deterministic state hash, which `pocket_run_headless` with the same seed and frame count reproduces.

Everything the session did is also reachable without MCP: the runtime's `--serve` port speaks JSON-RPC 2.0 on `POST /rpc` and serves `GET /tree`, `/state`, `/summary`, `/events?since=N`, `/schema`, `/commands`.
