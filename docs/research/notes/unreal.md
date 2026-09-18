# Research note: Unreal Engine for AI coding agents

Desk research by a Claude research agent on 2026-09-18 (URLs opened that day). "Official" = published by Epic.

## 1. Epic's own AI features and agent API (official)

- **Epic Developer Assistant** (web chat, dev.epicgames.com/community/assistant): Verse/UEFN beta since June 2025; UE 5.6 docs Q&A and C++ generation announced by Epic staff on 2025-09-24, who also said 5.7 would bring the Assistant into the editor (https://forums.unrealengine.com/t/the-epic-developer-assistant-ai-powered-developer-assistant-for-unreal-engine-5-6/2659525). Chat only; no editor actions.
- **UE 5.7 (2025-11-12) "AI Assistant (Experimental)"**: slide-out panel plus F1 over any UI element; questions, code generation, step-by-step guidance; does not edit the scene (https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-5-7-release-notes?application_version=5.7).
- **UE 5.8 (2026-06-17) "MCP Server (Experimental)"**, the first official agent API (https://dev.epicgames.com/documentation/unreal-engine/unreal-mcp-in-unreal-editor): MCP server inside the editor process, HTTP + SSE at `http://127.0.0.1:8000/mcp`, loopback only, no authentication; tools come from a **Toolset Registry** (ActorTools, SceneTools, MaterialInstanceTools, ObjectTools, GAS AttributeSetToolset, animation and MetaHuman toolsets), authored in Python or C++; examples: spawning actors, configuring lighting, creating material instances, inspecting Slate widgets, running automation tests; no MCP Resources or Prompts; registry adapter is editor-only but cooked builds can host a server via `IModelContextProtocolModule::StartServer()`; new C++ tools need an editor restart; a Terminal plugin runs the agent CLI inside the editor. 5.8 is the last planned UE5 release; UE6 (early access targeted end of 2027) names "MCP with integrations for Claude, Gemini, and others" as one of three initiatives, alongside moving gameplay to Verse (https://www.unrealengine.com/news/state-of-unreal-2026-top-news-from-the-show).
- **UEFN, 2026-08-20, beta**: Unreal MCP in UEFN with toolsets to read/write/compile Verse, place devices, create Scene Graph entities, UMG, and start/stop/debug play sessions including pushing Verse changes into a running session and reading the client log (https://www.fortnite.com/news/unreal-mcp-is-now-available-in-uefn). Known issues: tool calls can hitch and hang the editor.

## 2. Remote Control API (official plugin, virtual-production heritage)

HTTP on 30010, WebSocket on 30020. Object routes: `/remote/object/property` (read/write, optional transaction), `/remote/object/call`, `/remote/object/describe`, `/remote/search/assets`, `/remote/object/thumbnail`, `/remote/batch`, `/remote/object/event` (experimental). Presets expose groups, properties, functions and push change events over WebSocket. Works in PIE; disabled by default in packaged builds (`-RCWebControlEnable`). Fit for an agent: an excellent property and function primitive with batching and describe-by-path; missing: a world enumeration route, auth, events beyond preset fields, frames, logs. (https://dev.epicgames.com/documentation/en-us/unreal-engine/remote-control-api-http-reference-for-unreal-engine)

## 3. Python editor scripting (official)

Python 3.11; editor only, never in PIE, standalone or cooked builds. Entry points: console, `-ExecutePythonScript`, the `pythonscript` commandlet (headless editor), `init_unreal.py`. Remote Execution (UDP discovery, then TCP) is an official setting and is what plugin-free MCP servers use. (https://dev.epicgames.com/documentation/en-us/unreal-engine/scripting-the-unreal-editor-using-python)

## 4. Community MCP servers (GitHub stats, 2026-09-18)

| Project | License | Stars | Last push | Notes |
|---|---|---|---|---|
| chongdashu/unreal-mcp | MIT per README | 2,081 | 2025-04-22 | Python server + C++ plugin, actors, Blueprints, UMG; dormant |
| flopperam/unreal-engine-mcp | none | 1,079 | 2026-06-26 | now owned by a commercial agent product |
| tumourlove/monolith | MIT | 310 | 2026-09-09 | native C++ proxy, ~1,400 actions, live PIE introspection and input injection, log capture |
| remiphilippe/mcp-unreal | Apache-2.0 | 69 | 2026-02-20 | headless build/cook/tests, PIE control, viewport capture |
| runreal/unreal-mcp | MIT | 115 | 2025-06-06 | plugin-free via Python remote execution |
| alexkenley/ue-mcp | MIT | 0 | 2026-09-17 | wraps Epic's 830 Toolset Registry tools on 5.8 |
| StraySpark Unreal MCP Server | commercial | – | 5.1.0-beta | 523 tools, Slate frame capture with pixel hash, automation spec scaffold/run/report |

Fragmented and mostly dormant; Epic's plugin is absorbing the space.

## 5. Automation and verification (official)

- Automation Test Framework: unit, feature, smoke, stress and screenshot tests; Functional Test actors in levels; headless via `UnrealEditor-Cmd <proj> -ExecCmds="Automation RunTests X;Quit" -unattended -NullRHI -ReportExportPath=<dir>` with JSON and HTML reports.
- Screenshot comparison with tolerances and per-platform ground truth.
- Gauntlet: orchestrates multi-client sessions on packaged builds; parses logs and crashes.
- Unreal Insights / Trace: `-trace=<channels>`; headless export of timers, counters and events to CSV; custom analyzers. Full tracing unsupported in Shipping.
- Rewind Debugger: scrubs recorded animation state; inspects, does not re-simulate; editor only.
- Chaos Visual Debugger: records particles, collisions, constraints and queries from packaged builds; GUI only, no documented export.
- Replays (DemoNetDriver): replication-based reconstruction, not deterministic re-simulation. No deterministic session replay exists.

## 6. Structured world-state and event export

- Trace channels (cpu, gpu, frame, log, bookmark, memory, net, object, animation, physics) in a self-describing `.utrace` with custom `UE_TRACE_EVENT` schemas: the only machine-readable event log.
- Visual Logger: text and shapes per actor with debug snapshots, reviewed in the editor; no documented JSON export.
- Gameplay Debugger: viewport overlay only.
- Remote Control `describe` gives a per-object schema; Epic's MCP tools answer context questions such as "what is selected".

## Lessons for a new engine (agent's assessment)

- Property-level remote access with reflection (describe, batch, transactions) is the right primitive; the gap is a first-class world query (filter actors by tag, distance, component).
- Make the agent API runtime-capable by design: Python, the Toolset adapter, Rewind Debugger and Gameplay Debugger are editor-only.
- Ship one structured, self-describing event log (Trace is the model) with headless export; VisLog, Gameplay Debugger and CVD are GUI-first.
- Provide deterministic, headless session record and replay with input capture.
- Return semantic verification, not pixels: screenshot tests and pixel hashes show demand, but an agent needs asserts on state, events and causality.
- Bake in auth and sandboxing; hot-register new tools without restart.
- Keep agent output editable by humans; expose build, test and report JSON as the closed loop.

Not verifiable: the underlying model of Epic's assistant; headless replay playback; Gauntlet's report format beyond logs and screenshots.
