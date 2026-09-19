# M3 evidence: agent interface (`pocket mcp`)

Produced on 2026-09-18 with the debug build.

| File | Command | What it shows |
|---|---|---|
| `mcp-session.txt` | a Python client speaking newline-delimited MCP JSON-RPC to `pocket --root . mcp` | `initialize`, `tools/list`, then `runtime_start playground`, `world_tree`, `step 120`, `world_query`, `events_since`, `capture` (color and ids), `render_pick`, a `world.set` through `runtime_command`, `world_describe`, `runtime_stop` with the final report. Every result is the same JSON the runtime's `/rpc` returns. |
| `mcp-capture.png`, `mcp-ids.png` | the `capture` call in that session | The frame and the id buffer at tick 120 as seen through MCP. |

The server needs no network: it launches `pocket_runtime --serve 0 --paused` as a child, reads the announced port, and forwards tool calls as JSON-RPC over loopback. Registration commands for Claude Code and Codex are in `docs/mcp.md`.
