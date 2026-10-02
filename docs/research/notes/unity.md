# Research note: Unity for AI coding agents

Desk research by a Claude research agent on 2026-09-18 (all pages observed that day). "Official" =
published by Unity.

## 1. Unity AI, Assistant, official MCP (official)

- Muse retired 2025-10-01, replaced by Unity AI in Unity 6.2+ [1]. Open beta since 2026-05-05 for
  Unity 6.0+: Assistant (Ask, Plan, Agent modes), MCP Server, AI Gateway (bring your own Claude, GPT
  or Gemini subscription), Generators. Personal tier gets a trial credit budget; Pro, Enterprise and
  Industry include it [2].
- Ask mode is read-only but answers scene queries ("what GameObjects use this material?"); Agent
  mode writes scripts, modifies components and creates prefabs under permission levels (read-only,
  scripts only, full autonomy) with checkpoints: "All changes are reversible" [3]. Users attach
  GameObjects, assets, scripts, Console messages and screenshots as context [4]; a C# `AssistantApi`
  runs it headless with JSON and image attachments [5]. No documented self-screenshot or Play-mode
  control.
- The official MCP server inside the assistant package (hierarchy read, GameObject CRUD, component
  values, script CRUD, console, build settings) was deprecated within months, replaced by
  `unity mcp` in the free Unity CLI [6][7][8].
- **Unity CLI + `com.unity.pipeline`** (experimental, free, Unity 6.0+): a running Editor becomes a
  local HTTP+JSON command server;
  `unity command editor_play | run_tests | list_build_targets | screenshot`; `eval` runs C# inside a
  live Unity instance (Editor or a running build); every command is available over CLI or MCP; "not
  for shipped games" [9][10]. An official Claude Code plugin (2026-09-09) bundles 29 skills, the CLI
  and MCP [11]. The Unity 7 roadmap promises a free MCP and a public API (beta December 2026) [12].
- Maturity: beta/experimental; the agent surface was rebuilt once within four months. Cannot:
  structured state queries beyond hierarchy and component dumps, event logs, replay; perception is
  screenshots.

## 2. Community MCP servers

- **CoplayDev/unity-mcp** (formerly justinpbarnett/unity-mcp): MIT, 14.3k stars, v10.0.0
  (2026-06-30), Unity 2021.3 to 6.x. About 48 tools and 25 resources: scene, GameObject and
  component CRUD, `find_gameobjects` (name, tag, layer, component, path), `execute_code` (arbitrary
  C# in the Editor), `execute_menu_item`, `read_console`, `run_tests`, `manage_profiler`,
  `manage_build`, `manage_ui`, `batch_execute`; resources `editor_state`, `gameobject_components`
  (full property serialization), `rendering_stats`. No screenshot tool [13][14].
- **IvanMurzak/Unity-MCP**: Apache-2.0, 4.3k stars, 70+ tools: Roslyn script execution, reflection
  method calls, play/pause/stop, screenshots of game, scene and isolated views, console, tests,
  profiler; also runs inside compiled games [15].
- **CoderGamester/mcp-unity**: MIT, 1.9k stars; menu items, GameObject and component updates,
  play-mode get/set, tests, console, a scenes-hierarchy resource; no screenshots [16].
- **AnkleBreaker unity-mcp-server**: custom licence, 465 stars, 330+ tools, editor-window
  screenshots, per-action undo [17].
- Maturity: active with breaking majors; all lean on code-execution escape hatches.

## 3. ML-Agents (official)

Apache-2.0; Release 23 (package 4.0.0, 2025-08-28), minimum Unity 6000.0; no release in the
following twelve months [18]. Observations: `VectorSensor`, camera and RenderTexture sensors,
`RayPerceptionSensor` (2D and 3D) and `GridSensor` that classify hits by detectable tags,
variable-length `BufferSensor`, goal signals [19]. Feeds tensors to RL policies; not an editor or
agent query API.

## 4. External test drivers

- **AltTester Unity SDK**: GPL-3.0 SDK; paid tiers add UI Toolkit support; 2.3.2 (June 2026)
  supports Unity 6. Selectors by name, path (XPath-like), id, text, component, tag, layer; get and
  set component properties, call component methods; input injection, PNG screenshots, scene loading,
  time scale. The instrumented app must be running on a device; accounts required on all tiers
  [20][21][22].
- **GameDriver** (commercial; free for solo developers and educators): HierarchyPath addressing such
  as `//*[@name='HiddenCube']/fn:component('UnityEngine.Behaviour')/@isActiveAndEnabled`; field get
  and set, method calls, screenshots, wait-for-object, play mode control [23][24]. Headless use not
  verified.
- **Unity Automated QA** (official, preview): uGUI-only click and drag recording; development on
  hold since 2021-12-06 [25].
- **Unity Test Framework** (official): in-process NUnit,
  `-runTests -batchmode -testPlatform EditMode|PlayMode`; no external hierarchy access [26].

## 5. Record and replay, determinism, event logs

- **Input System `InputEventTrace`** (official): records events, writes and reads disk, replays with
  frame markers. Inputs only; no state snapshot [27].
- **Entities Journaling** (official, ECS only): world, entity and system lifecycle plus component
  add, remove and set, each with the executing system, the origin system and the frame index; window
  and C# API; no export documented [28].
- **Determinism**: Unity Physics claims determinism, yet Unity states physics is deterministic on
  the same machine only [29][30]; Netcode for Entities: the simulation need not be fully
  deterministic, "although this is something you should aim for" [31]. No official time-travel
  debugger.
- **Unity Behavior**: play-mode graph debugging with node breakpoints; no replay or log [32].
- **Recorder**: image sequences, video and AOVs in Editor Play mode; scripting API and command line
  [33].

## 6. UI Toolkit tree

The UI Toolkit Debugger shows the live hierarchy, picks elements and lists matching USS selectors;
edits are not saved; no export or programmatic API documented [34]. `UQuery` lets a script walk and
serialise `rootVisualElement`, but Unity ships no tree-snapshot API [35].

## Lessons for a new engine (agent's assessment)

1. Put the agent interface in the engine, not in an AI add-on. Unity dropped its assistant-bundled
   MCP within four months for a free HTTP command server (`eval`, `editor_play`, `run_tests`,
   `screenshot`, attribute-registered typed commands) exposed as CLI and MCP. Keep `eval` as the
   escape hatch, typed commands as the default.
2. Structured world-state queries exist only ad hoc (path selectors, HierarchyPath, instance ids,
   `find_gameobjects`); own a stable entity address scheme and typed property read and write.
3. Causal event logs: only Entities Journaling, ECS-only, breakpoint-inspected, not exportable. Emit
   an exportable (frame, system, entity, change, cause) record per mutation.
4. Deterministic replay is absent: input traces replay inputs, physics is same-machine only, Netcode
   only aims for determinism. Fixed-step determinism plus state snapshots turns an input trace into
   a reproducible bug report and a verification primitive.
5. Perception is screenshots everywhere; nothing emits semantic summaries (visible objects, screen
   positions, states). ML-Agents' tagged ray and grid sensors show the idea but output RL tensors,
   not text.
6. Permissions and reversible checkpoints are right; most MCP servers mutate without undo. Make
   every agent mutation transactional.
7. Verification hooks are gated: the test framework is in-process, Automated QA is dead, external
   drivers need instrumented builds and paid tiers. Build hierarchy access and input injection in,
   headless and free.

Sources (observed 2026-09-18): [1]
https://docs.unity3d.com/Packages/com.unity.muse.chat@1.1/manual/index.html [2]
https://unity.com/blog/unity-ai-how-to-get-started [3]
https://unity.com/blog/unity-ai-assistant-ask-plan-agent-mode-explained [4]
https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.0/manual/assistant-interface.html [5]
https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.9/manual/integration/assistant-api.html
[6] https://unity.com/blog/unity-ai-mcp-how-to-get-started [7]
https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.0/manual/unity-mcp-get-started.html [8]
https://docs.unity.com/en-us/unity-cli/replace-mcp-server-unity-cli [9]
https://unity.com/blog/meet-the-unity-cli [10]
https://unity.com/resources/unity-pipeline-cli-technical-walkthrough [11]
https://unity.com/blog/unity-plugin-for-claude-code [12]
https://unity.com/news/unity-7-roadmap-revealed-at-unite-seoul [13]
https://github.com/CoplayDev/unity-mcp [14] https://coplaydev.github.io/unity-mcp/reference/tools
[15] https://github.com/IvanMurzak/Unity-MCP [16] https://github.com/CoderGamester/mcp-unity [17]
https://github.com/anklebreaker-studio/unity-mcp-server [18]
https://github.com/Unity-Technologies/ml-agents/releases [19]
https://unity-technologies.github.io/ml-agents/Learning-Environment-Design-Agents/ [20]
https://github.com/alttester/AltTester-Unity-SDK [21]
https://alttester.com/docs/sdk/latest/pages/commands.html [22]
https://alttester.com/alttester-2-2-release-ui-toolkit-support-for-unreal-engine/ [23]
https://www2.gamedriver.io/en-us/pricing [24]
https://github.com/GameDriver-io/gdio.unity_api.doc/blob/main/gdio.unity_api.v2/ApiClient/GetObjectFieldValue.md
[25] https://docs.unity3d.com/Packages/com.unity.automated-testing@0.8/manual/index.html [26]
https://docs.unity3d.com/Packages/com.unity.test-framework@1.4/manual/reference-command-line.html
[27]
https://docs.unity3d.com/Packages/com.unity.inputsystem@1.14/api/UnityEngine.InputSystem.LowLevel.InputEventTrace.html
[28] https://docs.unity3d.com/Packages/com.unity.entities@1.3/manual/entities-journaling.html [29]
https://github.com/needle-mirror/com.unity.physics/blob/master/Documentation~/index.md [30]
https://support.unity.com/hc/en-us/articles/360015178512-Determinism-with-2D-Physics [31]
https://docs.unity3d.com/Packages/com.unity.netcode@1.4/manual/intro-to-prediction.html [32]
https://docs.unity3d.com/Packages/com.unity.behavior@1.0/changelog/CHANGELOG.html [33]
https://docs.unity3d.com/Packages/com.unity.recorder@5.1/manual/index.html [34]
https://docs.unity3d.com/6000.0/Documentation/Manual/UIE-ui-debugger.html [35]
https://docs.unity3d.com/Manual/UIE-UQuery.html
