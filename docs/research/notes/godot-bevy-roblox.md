# Research note: Godot, Bevy and Roblox for AI coding agents

Desk research by a Claude research agent on 2026-09-18 (all pages opened that day; parenthesised
dates are publication dates). "Official" = shipped by the engine's maintainers.

## Godot

**Official stance and engine features.** The Godot Foundation's contribution policy (30 Jun 2026)
bans "autonomous AI agent use or vibe coding" and AI-written substantial code in pull requests; it
is silent on product features. Rémi Verschelde (Game Developer, 22 Jun 2026): "Godot doesn't include
AI features nor intends to add any." "Godot Copilot" is a community plugin, last commit April 2024.

Agent-usable facilities in 4.4 to 4.7 (latest 4.7.2): CLI `--headless`, `-s/--script`,
`--check-only`, `--remote-debug <uri>`, `--doctool`, `--write-movie`; 4.4 embedded game window with
click-to-select of live nodes and an expression evaluator at breakpoints; 4.5 script backtraces and
custom loggers; 4.6 (26 Jan 2026) ObjectDB snapshots with diffing, game-speed controls, LibGodot
(embed the engine and drive its loop), a JSON-described GDExtension API. The Scene dock's Remote tab
reads and edits the running tree; `EditorDebuggerPlugin` can send arbitrary messages over the
debugger session (TCP 6007) but there is no published wire spec. Cannot: external RPC or MCP,
structured state export, frame-capture API, replay. Sources:
godotengine.org/article/contribution-policy-2026/; godotengine.org/releases/4.4/ and /4.6/;
docs.godotengine.org (command_line_tutorial, class_editordebuggerplugin,
overview_of_debugging_tools).

**Community MCP servers.**
- Coding-Solo/godot-mcp: MIT, TypeScript, 5.7k stars, no tagged releases; 14 tools (launch editor,
  run project, debug output, create scene, add node, save scene, UID tools) driving the CLI with a
  bundled GDScript; no plugin, screenshots or runtime state.
- GDAI MCP (3ddelano): proprietary, paid, Godot 4.2+; editor plugin with about 32 tools: scene tree,
  add node, update property, play scene, errors, editor and running-scene screenshots, arbitrary
  GDScript execution.
- tugcantopaloglu/godot-mcp: MIT, 461 stars, 157 tools, tested to 4.7; headless CLI for scene files
  plus a TCP autoload in the running game: screenshot, key press, mouse move, pause, eval, errors.
- youichi-uda/godot-mcp-pro: proprietary, paid, 600 stars, 175 tools over a WebSocket plugin; claims
  input sequences, property monitoring, recording and replay, assertions, screenshot comparison
  (untested claims).
All re-implement the same tree, run, error and screenshot tools behind incompatible plugins; hobby
to micro-vendor maturity.

**Godot RL Agents** (community, MIT, 1.6k stars): `AIController` defines observations and action
spaces; sensors: 2D and 3D raycast "LIDAR", camera, grid. A Sync node speaks JSON over TCP:
handshake, env_info, reset, step {obs, reward, done, info}, action. Physics pauses during the
exchange; `--speedup` scales physics ticks and time scale; `--action_repeat` batches frames.
Wrappers for SB3, Sample Factory, CleanRL, RLlib. A clean step-gated observation loop; no editor
control or semantic summaries. The CA2 paper (arXiv 2605.13918, May 2026) adds call-stack traces to
state for test agents. Sources: github.com/edbeeching/godot_rl_agents; arxiv.org/html/2112.03636v1.

## Bevy

**Bevy Remote Protocol** (official; since 0.15, 29 Nov 2024; docs 0.19.1, 13 Aug 2026). JSON-RPC
2.0; reference transport `RemoteHttpPlugin` (HTTP POST, port 15702); transport-agnostic, "simplicity
over efficiency". Methods: `world.query` (components, option, has, with, without, strict), get,
insert, mutate and remove components, spawn and despawn entities, reparent, list components,
resource equivalents, trigger event, `registry.schema` (JSON-Schema-style map keyed by type path),
`rpc.discover` (OpenRPC), streaming `get_components+watch` and `list_components+watch`. Entities are
numeric ids; components are fully-qualified type paths; only `Reflect`-registered types are visible.
Stated purpose: inspectors and an editor in a separate process. Cannot: screenshots, input, time
control, frame stepping, history. MCP wrapper (community): natepiano/bevy_brp (bevy_brp_mcp 0.22.6,
10 Sep 2026, Bevy 0.19): launch, world query, find by name, watch components, mutate, trigger event,
read log, type guide, plus screenshot, send keys and diagnostics through an extras plugin. Sources:
docs.rs/bevy/latest/bevy/remote/; bevy.org/news/bevy-0-15/; docs.rs/crate/bevy_brp_mcp/latest.

**Editor** (official status). Sixth birthday post (10 Aug 2026): no shipped editor; prerequisites
done (BSN scenes in 0.19, Feathers widgets, BRP, dynamic plugin loading); the editor is "the next
and overriding priority", planned as "a normal Bevy app built on Bevy UI + BSN" (in-process). The
editor prototypes repo was archived 16 Apr 2026, pointing to the community Jackdaw editor (very
early). BRP's editor role is unsettled. bevy-inspector-egui is in-process egui reflection, not BRP.
Sources: bevy.org/news/bevys-sixth-birthday/; github.com/bevyengine/bevy_editor_prototypes;
github.com/jbuehler23/jackdaw.

## Roblox

**Studio MCP server** (official). The reference repo Roblox/studio-rust-mcp-server (MIT, 488 stars;
six tools: run code, insert model, console output, start and stop play, run script in play mode,
studio mode) was archived 3 Apr 2026 in favour of the built-in server (Assistant, "Manage MCP
Servers"; proprietary part of Studio). Documented tools of the built-in server: the instance
hierarchy as JSON with per-instance properties and attributes; read, batch-edit and search scripts;
run Luau in Edit, Client or Server context; start and stop play; console output; viewport
screenshots with camera placement; simulated character navigation, keyboard and mouse; mesh,
material and procedural model generation; Creator Store insert; docs lookup; subagents. Clients:
Claude Code and Desktop, Cursor, Codex, Gemini CLI, VS Code. Cannot: run headless, replay, transact
or undo; the docs warn that clients can read and modify content in open places. Community superset
Chrrxs/robloxstudio-mcp (MIT, 238 stars): breakpoints, multiplayer sessions, per-peer eval,
profiler. Sources: create.roblox.com/docs/studio/mcp; github.com/Roblox/studio-rust-mcp-server;
github.com/Chrrxs/robloxstudio-mcp.

**Assistant and generative features** (official). The Assistant creates, edits and deletes instances
and scripts, bulk-edits properties and inserts assets; its context is the Explorer selection, the
open script and the viewport via a screen-capture subagent. Planning Mode (16 Apr 2026): Plan,
Build, Test with an editable step list; a playtesting agent (beta) runs the game, reads logs,
captures screenshots, drives keyboard and mouse and judges the result against the plan. Cube 3D
(March 2025, open-sourced, mesh generation API in Studio), CubePart (May 2026), Text Generation API
(runtime NPC LLM). Cannot: no determinism or structured test report; verification is LLM judgement
over logs and screenshots. Sources: create.roblox.com/docs/assistant/guide;
about.roblox.com/newsroom/2026/04/roblox-studio-going-agentic.

**DataModel and Open Cloud** (official). One tree: every object is an Instance with Name, Parent,
ClassName, reflected properties and attributes; services are roots (Workspace, ReplicatedStorage,
ServerScriptService, StarterGui); scripts are instances with Source. The edit-time tree is the
runtime tree; server-to-client replication is automatic per container. Legibility: GetChildren and
GetDescendants, ChildAdded, DescendantAdded, AncestryChanged, Changed and GetPropertyChangedSignal,
AttributeChanged; the tree serialises to rbxl and rbxm. Open Cloud: an Engine Instance API (beta)
and a Luau Execution API (a cloud server loads a place version, runs Luau with full DataModel
access, returns values and logs; limits grew to five minutes and ten tasks per place). A CI demo
(Rojo build, upload, cloud tests) exists. Sources: create.roblox.com/docs/projects/data-model;
create.roblox.com/docs/cloud/reference/LuauExecutionSessionTask.

## Lessons for a new engine (agent's assessment)

- Bevy's engine-owned, typed ECS query over JSON-RPC with a self-describing schema and change
  streams is the right protocol shape; add frames, input and time control so no side plugin is
  needed.
- Roblox's single observable tree (scene, code and replication in one) plus a first-party MCP that
  reads it as JSON, runs code per context, plays, screenshots and injects input is the right world
  model. Ship the official server on day one, or inherit Godot's four incompatible community
  servers, two of them paid.
- Godot's headless `--script`, remote-tree editing and ObjectDB snapshot diffs are the right
  primitives; expose them over the network, not only inside the editor.
- Godot RL Agents' step-gated JSON loop with speedup is the right simulation control: pause, step N,
  fast-forward from outside; observations as versioned typed messages.
- Nobody has a causal event log: BRP watch and Roblox `Changed` give diffs without cause. Keep an
  append-only, tick-indexed mutation log with provenance.
- Nobody has deterministic replay or time travel: fixed tick, seeded RNG, input journal, snapshot
  and restore.
- Nobody summarises: every tool returns raw trees. Offer engine-side semantic queries (region,
  proximity, contacts since t, diff since last call) and tool budgeting; Roblox subagents and
  "adaptive modes" exist because clients cap tool counts.
- No perception benchmark exists: ship reference scenes with ground-truth state so perception is
  scorable.
- Verification is a loop (read errors, fix, run, screenshot; Roblox's Plan, Build, Test), so make
  assertions first-class: a headless runner, structured pass and fail, artifacts per tick range.
