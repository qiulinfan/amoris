# Survey: is any known engine already agent-native? (September 2026)

- Status: research synthesis (2026-09-18). Source notes with URLs are in `docs/research/notes/`. Feeds `docs/positioning.md`, `docs/design/agent-perception.md` and the coming decision record on the agent interface.

## Question and method

The open question for Pocket3D is "AI agent as a first-class citizen": can an agent operate the engine, perceive gameplay without watching frames, and verify what it built? Before designing, we checked whether Unity, Unreal, Godot, Bevy, Roblox, the web engines, the "AI-native" creators and the research literature already have a good answer. Five desk-research passes were run on 2026-09-18 with a shared rubric:

- Operate: drive the editor and the runtime through a protocol; headless; official or community.
- World query: typed, filterable state (not screenshots, not raw dumps).
- Event log with causes: what happened and why, exportable.
- Deterministic replay and time travel.
- Semantic summaries of a session.
- Frame semantics: knowing what is in a picture (entity ids, visibility) rather than guessing.
- Verification loop: launch, inject input, assert on state, capture, report.
- Openness and license.

## Matrix

| Surface (owner, status) | Operate editor | Operate runtime | World query | Event log with causes | Deterministic replay | Semantic summary | Frame semantics | Verification loop | Open |
|---|---|---|---|---|---|---|---|---|---|
| Unreal 5.8 MCP + Remote Control (Epic, experimental, official) | yes, ~830 tools | partial: cooked builds can host MCP; Python is editor-only | partial: describe and property by path; no filtered world enumeration | partial: Trace channels, self-describing, headless CSV; no cause ids | no: replays rebuild replicated state only | no | no: screenshot tests, pixel hashes | yes: automation tests with JSON reports, Gauntlet | source under EULA |
| Unity CLI + MCP + Claude Code plugin (Unity, beta, official) | yes; permission levels, reversible checkpoints | partial: `eval` in a running build, not shipped games | partial: hierarchy and component dumps, find by name, tag, component | partial: Entities Journaling has system provenance, ECS only, not exportable | no: input traces only; physics same-machine | no | no | partial: in-process test framework; external drivers paid or instrumented | closed |
| Roblox Studio built-in MCP + Planning Mode (Roblox, official) | yes: tree as JSON, scripts, Luau per context, play, console, screenshots, simulated input | partial: play sessions; cloud Luau execution | partial: instance tree JSON with properties; change signals | no: changes without causes | no | no: LLM judgement over logs and screenshots | no | partial: Plan, Build, Test with a playtesting agent, no structured report | closed platform |
| Bevy Remote Protocol (+ community MCP) (Bevy, official protocol) | no editor shipped | yes: JSON-RPC in-process, headless | yes: typed ECS query with schema, watch streams | partial: watch gives diffs without causes | no | no | no | no built-in loop | MIT / Apache |
| Godot (community MCPs; Foundation refuses AI features) | partial: four incompatible servers, two paid | partial: headless `--script`, autoload sockets, RL Agents step loop | partial: remote tree, ObjectDB snapshot diffs, no published protocol | no | no (one paid server claims it, unverified) | no | no | partial: run, read errors, screenshot loops | MIT engine |
| PlayCanvas Editor MCP (PlayCanvas, official, MIT) | yes | yes: launch, capture frame, read logs, inject input, query runtime state | partial: per-entity state by name or GUID | no | no | no | partial: frame capture | yes: all four steps | MCP open, cloud editor commercial |
| three.js + Chrome DevTools MCP + three.js devtools MCP (community) | not applicable | yes: scene graph, materials, stats, console, input | partial: object dumps | no | no | no | partial: screenshots; canvas opaque to accessibility tools | partial: manual | open |
| AI game creators: Rosebud, Astrocade, SEELE, Nilo (closed) | they are the agent | preview | no | no | no | no | no | "play it yourself" plus a fix-error button | closed |
| AI playtesters: modl.ai, nunu.ai, GameDriver (commercial) | not applicable | pixels and OCR; optional SDK state; in-engine hierarchy paths | partial | no | no | reports rebuilt from pixels | no | reports with screenshots, logs, severity | closed |
| Rerun data layer (not an engine, open) | not applicable | not applicable | latest-at and range queries over entity paths and timelines | no gameplay semantics | not a simulator | no | no | not applicable | Apache / MIT |

## Findings

1. **Operating an engine through an agent protocol is commoditized in 2026.** Epic shipped an MCP server in UE 5.8 and made "MCP plus Verse" a UE6 pillar; Unity replaced its assistant-bundled MCP with a free CLI command server exposed as CLI and MCP and published a Claude Code plugin; Roblox built MCP and a Plan, Build, Test agent into Studio; PlayCanvas ships an editor MCP with runtime tools; Bevy has a typed ECS protocol. "The agent can drive the editor" no longer differentiates anyone. It is table stakes, and the incumbents also show how not to do it: Unity's surface was rebuilt within four months, Godot has four incompatible community servers, Unreal's richest tools are editor-only.
2. **Perception is screenshots and raw dumps everywhere.** Not one engine or product offers a typed world query with semantic filters, an event log with cause ids, deterministic replay or time travel for agents, semantic session summaries, frame semantics such as an entity id buffer, or a perception benchmark. The nearest pieces are fragments: Unity's Entities Journaling (provenance, not exportable), Unreal's Trace (self-describing, no causes), Bevy's watch streams (diffs, no causes), Tomorrow Corporation's in-house time travel, Unreal's Rewind Debugger (animation only). Playtesting vendors rebuild summaries from pixels and OCR because engines emit nothing.
3. **The research record says structured state wins on reliability and cost.** Text observations beat images in BALROG (images hurt GPT-4o); Voyager and OpenAI Five ran on structured state; the Gemini Pokémon harness found the screen "almost redundant" once RAM state was exposed; TITAN's state API reached 95 percent task completion against 82 percent for deep RL; OSWorld's accessibility tree more than doubled screenshot success; practitioner token math shows structured calls about 45 times cheaper than vision steps and a scoped subtree costing about 50 tokens. The honest caveat: vision-only play became possible with the strongest 2026 models (Claude Fable 5 beat Pokémon FireRed from raw screenshots), so the claim is efficiency, reliability, auditability and reproducibility, not impossibility.
4. **Determinism and replay are solved engineering in specific games** (Factorio's per-tick checksum and first-divergent-tick diagnosis, Overwatch's command frames) and absent from every general engine's agent surface.
5. **Rerun's data model matches "tree plus timeline"** (entity paths, multiple timelines, columnar chunks, latest-at and range queries, a viewer, a C++ SDK, permissive licenses) and can serve as the storage and viewer backbone. Gameplay semantics, causality, segmentation, summaries and re-simulation are not in it and must be ours.
6. **Godot will not become agent-native by policy.** The Foundation states it neither includes nor intends to add AI features and bans autonomous agent contributions. The niche of a lightweight, open, TypeScript engine that is agent-native is open by the incumbent's own choice.
7. **Onboarding lessons from the creators**: one prompt box with the game running beside it within seconds, a one-click fix-error loop, declarative scene files, and architecture enforced by the runtime rather than by a prompt file (a 76k-line agent-written project drifted from ECS to OOP despite a 164-line CLAUDE.md).

## What Pocket3D adopts (to be recorded in the agent-interface decision record)

- **Protocol shape**: JSON-RPC 2.0 with a self-describing schema and discovery, plus watch streams (Bevy BRP); typed commands as the default with `eval` as the escape hatch (Unity CLI); batch, transactions and describe-by-path (Unreal Remote Control); permission levels and reversible checkpoints (Unity Assistant). One official server, inside the engine, identical for the editor, the runtime and headless runs, free, from milestone 0 (Roblox and PlayCanvas got this right; Unity and Godot show the cost of not doing it).
- **One tree as the document** (Roblox's DataModel, our ECS-backed tree): paths, stable ids, per-node properties, derived annotations, and scoped queries (radius, visibility, relevance) under a token budget.
- **Observation tiers**: player-knowable versus omniscient. Omniscient access by default taught Gemini to ignore the screen; play bots get the player tier, debugging gets everything.
- **Simulation control**: step-gated loop with pause, step N and speedup (Godot RL Agents); the language model is never inside the tick (every pixel agent had to pause the game); pathfinding and macro actions as tools (the Pokémon harness navigators).
- **Event log**: append-only, tick-indexed, self-describing with headless export (Unreal Trace), with system and origin provenance (Entities Journaling) and cause ids (ours).
- **Determinism package**: fixed tick, seeded randomness, input journal, per-tick state hash and first-divergent-tick diagnosis (Factorio), snapshots at exponentially spaced intervals for time travel (Tomorrow Corporation).
- **Data layer**: a Rerun-compatible model (entity paths, timelines, Arrow chunks); evaluate embedding Rerun's C++ SDK and viewer at milestone 3 while keeping in-engine queries.
- **Segmentation and summaries**: Bayesian online change-point detection on a few channels, surprise-triggered summaries (Event Segmentation Theory, Kumar 2023, EM-LLM), the gameplay transcript.
- **Reports designers already want**: coverage heatmaps, stuck areas, exploit lists, pass or fail per natural-language task (EA SEED, TITAN, modl.ai).
- **Verification loop built in**: launch, inject input, assert on state and log, capture (PlayCanvas has all four).
- **Perception benchmark**: reference scenes with ground-truth state; agent success and tokens with the state API versus pixels; external baselines GameDevBench, VideoGameBench, BALROG.
- **Onboarding**: prompt to running game in seconds, a fix-error loop, declarative scenes, runtime-enforced architecture.

## What Pocket3D avoids

- Agent APIs bundled with an assistant or bolted on as side plugins.
- Editor-only agent APIs.
- Pixel hashes as the primary verification.
- Omniscient observations by default for play bots.
- Closed platforms that cap project size or forbid export.

## Risks, stated plainly

- Vision-only agents keep improving. If frontier models play from pixels cheaply by 2027, the efficiency argument weakens; determinism, auditability and reproducibility remain, and humans want the time-travel debugger regardless.
- The large engines will keep growing their MCP surfaces; UE6 makes MCP core. Pocket3D wins on lightweight, TypeScript, web reach and perception, never on tool count.
- The perception stack is milestones 1 to 4 of real work; the benchmark must exist early so the claim is measured, not asserted.
