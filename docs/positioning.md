# Positioning

- Status: working document (2026-09-17). The public manifesto will be derived from this once it is
  stable; it is not written yet, on purpose.

## The user

People who have never made a game and want to make their dream game, working with an AI coding
agent. They are not engineers and do not want to become build engineers. Today they land on three.js
because every model knows it, and they hit the ceiling when the game grows: performance, project
structure, tooling.

Secondary users: solo developers and small teams who want a lightweight engine with a modern stack;
and engine contributors, who are the only people who ever see the C++ and Rust toolchain.

## The job

"I describe, the AI builds, I look and tweak, we ship." In steps:

1. Start in minutes: one download, a template, the first playable result on screen the same hour.
2. Iterate on feel: "the jump is floaty", "add a boss", "make the forest bigger". The agent edits
   scripts and scenes, runs, captures, verifies. The human is the director, not the QA.
3. Grow without collapse: hundreds of assets, dozens of scripts, large scenes. The engine keeps
   performing and the project stays organized.
4. Tune visually: place, drag, adjust materials and lights in an editor. The agent can do the same
   edits through the same commands.
5. Ship: a desktop build for stores, and a link for friends. Sharing a dream game is a URL.
6. Understand: the human can see what the agent changed and why, in plain language, inside the
   editor.

## Why the current options fail this user (checked September 2026, see `docs/research/agent-native-survey.md`)

- **three.js**: a library, not an engine. No editor, no asset pipeline, no scene format, physics
  bolted on, browser ceilings. Its one superpower is that every model knows it. Projects rot at a
  few thousand lines; a 76k-line agent-written project drifted from ECS to OOP despite a long
  instruction file.
- **Unity and Unreal**: both shipped official agent protocols in 2026 (Unity's CLI and MCP with a
  Claude Code plugin, Epic's MCP server in UE 5.8), so operating them with an agent is no longer the
  gap. What remains is everything else: too much to learn, licensing, closed or EULA-bound source,
  agent APIs that are editor-only or beta, and perception limited to screenshots and hierarchy
  dumps.
- **Godot**: open and light, but the Foundation states it neither includes nor intends to add AI
  features and bans autonomous agent contributions; agent support is four incompatible community
  servers, two of them paid; three scripting stacks; GDScript has thin training data next to
  TypeScript.
- **Roblox**: the best existing agent experience (one instance tree, a built-in MCP server, a Plan,
  Build, Test agent), on a closed platform with its own runtime, no export, no determinism and no
  structured verification.
- **Babylon.js, PlayCanvas, Cocos Creator**: web engines with editors; PlayCanvas has the most
  complete agent surface of any web engine; all remain browser-bound or commercial, and none
  perceives beyond per-entity dumps and frames.
- **Bevy**: a typed ECS protocol and no editor; Rust only; made for engineers.

## What Pocket3D is

A lightweight 3D engine in the niche of three.js and Godot, designed so that an AI agent can build
and maintain a scalable game for a non-engineer.

- **three.js-easy for the AI**: a TypeScript API that models can guess, generated declarations and
  docs, templates.
- **Godot-light for the human**: one download, an editor, no toolchain.
- **Built to scale**: native core, ECS, JIT scripting, explicit GPU APIs, an asset pipeline,
  structure the tool enforces.
- **Agent-operable**: the agent drives the editor and the runtime through MCP and the CLI, verifies
  with headless runs and captures, and explains its changes.

## What "AI helps you make your game" requires from the engine

1. **Two audiences, two doors.** Game makers get binaries (editor, `pocket`, templates) and never
   see a compiler. Contributors get the C++26 and Rust toolchain. The build system's complexity is
   invisible to game makers.
2. **An API models can guess.** Use the vocabulary models already know (scene, entity, transform,
   mesh, material, camera, light, rigid body, input, audio, an update callback with delta time), one
   obvious way to do each thing, generated `.d.ts` with doc comments as the single source of truth.
   An **agent eval suite** measures it: given only the SDK types and docs, can a model complete
   sample tasks zero-shot? The score is tracked like a benchmark, and an API change that lowers it
   is a regression.
3. **An agent-operable editor and runtime.** MCP and CLI with JSON; headless runs, captures, probes,
   logs and replays so that the agent, not the human, is the QA; a human-readable change log of what
   the agent did (files, scene edits) inside the editor.
4. **Structure that does not rot.** Packages, scene and prefab boundaries, inspectors driven by
   metadata, formatting and lint enforced by `pocket`, conventions documented for agents in the
   project itself.
5. **Scale.** Published performance targets per milestone (entity counts, draw calls, script calls
   per frame) against three.js baselines; ECS storage, JIT scripts, explicit GPU APIs, asset
   streaming later.
6. **Ship by link.** Web export (a wasm32 core, WebGPU, scripts running on the browser's own
   JavaScript engine) is strategic because sharing a dream game is a URL. Consequence: the RHI
   follows the WebGPU model (wgpu-native or Dawn on desktop, WebGPU in the browser, WGSL or Slang
   compiled to WGSL) and the core stays portable to Emscripten. This is the strongest input to ADR 0002.
7. **Perception, not screenshots.** The agent must understand the game without looking at frames:
   the world as queryable data, a causal event log, time travel, geometric and visibility queries,
   gameplay analyzers, executable scenarios. See `docs/design/agent-perception.md`; this is the
   capability that a scripting language alone cannot provide and the core reason to choose Pocket3D
   over Godot.
8. **Templates for dream games.** Third-person adventure, top-down action, first-person, puzzle:
   each a TypeScript project with assets and a walkthrough the agent can extend.

## Non-goals

The AAA feature race; mobile and consoles before desktop and web are solid; being a library for
engineers only.

## Open questions to settle before the manifesto

1. Web export: a constraint from M0 (portable core, WebGPU-shaped RHI) with delivery after M3, or a
   later add-on? Recommendation: constraint from M0.
2. API vocabulary where three.js and Unity differ (Object3D versus entity and components). Proposal:
   entity-component vocabulary with three.js-familiar names for graphics objects.
3. The success metric: the agent-eval score plus time-to-first-playable for a non-engineer. Define
   the benchmark tasks.
4. License and, later, services.
