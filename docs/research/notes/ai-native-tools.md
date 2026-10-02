# Research note: products that call themselves AI-native game tools (September 2026)

Desk research by a Claude research agent on 2026-09-18 (all URLs opened that day; publication dates
in parentheses). Focus: what an agent can operate, perceive and verify.

## 1. Web-based AI game creators

- **Rosebud AI**: chat-to-game in the browser; generated JavaScript on three.js (3D) and Phaser
  (2D). The agent writes code, generates sprites, skyboxes and audio, hosts the game; the JavaScript
  is readable and editable; Windows export. Verification: live preview plus a "Fix Error" button
  that feeds the crash back; a May 2026 review reports roughly 7/10 success and a ceiling around
  2,500 lines. Subscription tiers; closed platform. https://rosebud.ai/ai-game-creator ;
  https://theaipicks.com/rosebud-ai-review-2026/
- **Astrocade**: free social platform; one-pass generation of 2D (some 3D) games with art and music;
  iteration is re-prompting; no code export; runtime undisclosed. https://www.astrocade.com/create
- **SEELE**: in-house multimodal models produce assets, code, animation and a three.js/WebGL build,
  with Unity 6 and Unreal export; verification is a runtime tab for humans. https://www.seeles.ai/
- **Nilo**: custom WASM/WebGPU engine with C++ physics; prompt-to-3D-world, natural-language
  scripting, Roblox, FBX and glTF export; verification is in-browser play.
  https://nilo.io/articles/vibe-code-game-engine-2026 (2026-05-10, vendor)
- **VibeGame** (Dylan Ebert): open-source declarative XML-style engine on three.js, rapier and
  bitecs so Claude Code needs little domain knowledge; covers only the mechanics it implements.
  https://huggingface.co/blog/vibegame (2025-09-29)
- **Bezi**: now a Unity agent workspace; Actions (2026-01-08) create GameObjects, prefabs and
  materials; claims Play-Mode debugging, console reading and "visual verification". Subscription;
  closed. https://www.bezi.com/
- Desktop wave: **Summer Engine** (Godot 4.6 base, hosted AI, a "write, play, read" loop reading
  runtime errors) https://www.summerengine.com/ ; **godot-ai** (MIT MCP server, 46 tools including
  test suites, 2.5k stars) https://github.com/hi-godot/godot-ai ; **nAIVE** (Rust, WebGPU, Lua, YAML
  scenes, JSON-RPC agent socket, headless fixed-timestep test runner; MIT; v0.1.18, 13 stars)
  https://naive.dev/ ; **GDevelop's AI agent** (2025-09-10) edits objects and events.
  https://gdevelop.io/blog/make-games-with-ai-agent-gdevelop-automated-prompt

## 2. Web engines with editors

- **PlayCanvas Editor MCP** (MIT, 137 stars): the most complete agent surface found: entities,
  assets, scripts, settings, version control, plus runtime tools `launch_start`, `capture_runtime`
  (frame), `read_runtime_logs`, `inject_input` and `query_runtime_state`, which returns per-entity
  transform, enabled flag, components, rigidbody velocity and UI text by name or GUID. Limits: one
  editor connection; edits are immediate and destructive.
  https://developer.playcanvas.com/user-manual/editor/mcp-server/
- **Babylon.js**: official MCP servers (2026-06-04) cover the authoring tools (node material,
  particle, geometry, render graph, GUI editors) with live sync, not a running game; live-scene
  control is community (babylon-mcp; babylonjs-inspector-mcp with scene JSON, screenshots, Spector
  capture, JS eval). https://forum.babylonjs.com/t/babylon-js-mcp-servers-are-out/63591
- **Cocos**: Creator 3.8.x has no official agent features; community MCP plugins add about 50 tools
  including build, preview and log reading. COCOS 4 (2026-01-04, MIT) splits engine from editor and
  promises CLI editing, an IDE with built-in agents and "MCPs or Agents over libraries"; announced,
  not yet visible in the repo. https://github.com/DaxianLee/cocos-mcp-server
- **Needle Inspector** (three.js, r3f, Spline): DevTools extension with MCP: query hierarchy,
  lights, materials, meshes, camera; live edits written back to source; paid tier.
  https://engine.needle.tools/docs/ai/needle-mcp-server.html
- **Spline V2** (2026-08-21): WebGPU rebuild "for the agentic era"; MCP creates objects, materials,
  lights, particles, variables, events, states and HTML/JS interactions; a design tool without play
  or verify tools. https://updates.spline.design/changelog/introducing-spline-v2

## 3. three.js plus a coding agent

The 2025 Vibe Coding Game Jam (over 1,000 entries; three.js recommended, web-only) and the 2026
edition codified the workflow. https://levels.io/winners-of-the-2025-vibe-code-game-jam ;
https://vibejam.com/

Ceilings, first-hand: a 76k-line, 735-commit, roughly 95 percent Claude-written Mario Galaxy clone
(Hacker News, April 2026): the model drifted to OOP against an ECS design, needing a custom review
tool and a 164-line CLAUDE.md; "level design failed completely" (placing objects in 3D with
arbitrary gravity); camera and gravity edge cases stayed human work.
https://news.ycombinator.com/item?id=47600002. Guides name draw calls, undisposed resources and
device pixel ratio as recurring performance failures. GameDevBench measures the blind spot: visual
feedback lifts GPT-5.4 from 41.1 to 52.0 percent task success. https://arxiv.org/abs/2602.11103

Feedback beyond a screenshot: Chrome DevTools MCP (Google, Apache 2.0: screenshots, source-mapped
console, network, performance traces, input) https://github.com/ChromeDevTools/chrome-devtools-mcp ;
Playwright MCP (accessibility-tree snapshots; a WebGL canvas is opaque to it) ; threejs-devtools-mcp
(MIT, 59 tools: scene graph, materials, shaders, renderer stats, memory)
https://github.com/DmitriyGolub/threejs-devtools-mcp. All expose objects; none exposes game meaning.

## 4. World models (a different category)

They render frames from learned dynamics: no authored scene, no queryable state, no determinism, no
code to edit. Genie 3 (DeepMind, 2025-08-05; Project Genie 2026-01-29), Muse/WHAM and WHAMM
(Microsoft, 2025), Oasis (Decart, 2024), Hunyuan-GameCraft-2 (Tencent, 2025-11-28). Marble (World
Labs, 2025-11-12) exports splats, meshes and colliders into engines: asset generation, not a
runtime.

## 5. Agent playtesting and semantic state

- **modl.ai**: black-box vision and OCR agents, no SDK; reports carry descriptions, screenshots,
  video, logs, tags and a severity score; weak on timing-critical play. https://modl.ai/
- **nunu.ai**: agents on rendered frames; an optional SDK adds error logs and app state; outputs
  recordings, step reasoning, exact inputs, verdicts. https://nunu.ai/blog/how-nunu-works
  (2026-06-11)
- **GameDriver** (Unity and Unreal): in-engine agent; HierarchyPath queries any object, property or
  method at runtime; sold as QA-as-a-service with AI-maintained tests.
- Research: Play2Code / PlaytestArena has a GUI agent play generated browser games against rubrics
  and feed failures to the coder (66.8 percent pass, +14.6 over agentic coding).
  https://arxiv.org/abs/2605.28258 (2026-05-27). Perception benchmarks: VideoGameBench (pixels only;
  best 0.48 percent real-time, 1.6 percent paused) https://arxiv.org/abs/2505.18134 ; lmgame-Bench
  ("brittle vision perception") https://arxiv.org/abs/2505.15146 ; OmniGameArena (12 UE5 games)
  https://arxiv.org/abs/2606.09826

## Lessons for a new engine (agent's assessment)

1. Onboarding that converts non-engineers: one prompt box, the game running beside it within
   seconds, one-click "fix this error". Hide the toolchain; show the running game first.
2. Declarative, human-readable scene files cut what the model must know; the OOP-versus-ECS drift
   shows the runtime, not a CLAUDE.md, should enforce architecture.
3. Structured state exists only as per-object dumps (PlayCanvas runtime state, GameDriver
   HierarchyPath, three.js inspectors). Nobody offers queries over game semantics: goals, rules,
   affordances, legal actions.
4. No product emits a causal event log; that log is what turns a screenshot into a bug report.
5. No web engine offers deterministic replay from input traces.
6. Semantic session summaries are absent everywhere; modl.ai and nunu.ai rebuild them from pixels
   and OCR because engines emit nothing.
7. Perception is the measured bottleneck (0.48 percent on VideoGameBench; +11 points from visual
   feedback in GameDevBench). An engine that publishes its own perception benchmark can credibly
   claim agent-native.
8. Verification must be a built-in loop: launch, inject input, assert on state and logs, capture
   frames. PlayCanvas has all four; prompt-to-game products offer only "play it yourself".
9. Openness attracts agent tooling fast; closed platforms cap project size and forbid export.
