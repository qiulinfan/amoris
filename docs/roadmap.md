# Roadmap

Work proceeds in vertical slices; each milestone ends with something that builds, runs and is
verified by `pocket test` on CI. A milestone starts only when the previous one's evidence is in
`tests/evidence/`.

## M0: hello (build tool v0)

Status: done on macOS (2026-09-18), evidence in `tests/evidence/m0/`. Deviations from the plan
below: the script engine is JavaScriptCore instead of V8 (ADR 0005), the executor is Ninja itself
rather than n2 (n2 embedding is a tool milestone), and CI is not set up yet.

Goal: `pocket setup && pocket build && pocket run hello` opens an SDL3 window on macOS; a TypeScript
script compiled by `pocket` runs in V8 and calls an engine API that logs; `pocket test` runs a
Catch2 suite and a headless golden-image test without a display; CI builds macOS and Ubuntu.

- `tools/pocket` v0: workspace and module manifests, clang toolchain discovery, Ninja-compatible
  graph executed by n2, `compile_commands.json`, prebuilt fetch by content hash (V8, tsgo),
  oxc-based TypeScript transform, `--json` output for build and test.
- V8 monolith build recipe and the first prebuilt artifact (macOS arm64).
- `engine/core`: log, time, fixed step, seeded RNG, the `std::expected` error convention, no
  globals.
- `engine/platform`: SDL3 window, input, main loop, headless mode.
- `engine/script`: isolate, context, module loader, one hello binding.

## M1: world model and metadata

Status: core done on macOS (2026-09-18), evidence in `tests/evidence/m1/`, design in
`docs/design/world-model.md`. Delivered: flecs world with ordered hierarchy, component metadata and
`pocket gen`, generic JSON component access, tree/describe/query/summary/schema views, causal event
log, scene files, input journal record/replay, HTTP JSON-RPC control server with paused stepping,
in-engine TypeScript test runner. Not yet: zero-copy typed-array component views, the agent eval
suite, ports of the PocketEngine Phase 1 specs.

ECS decision (ADR 0003), component metadata DSL, code generation for bindings, `.d.ts` and
serialization, the TypeScript component API with zero-copy typed-array views, the in-engine
TypeScript test runner, the scene format (text, schema, stable order), and the first version of the
agent eval suite (models given only the SDK types and docs must complete sample tasks). Ports the
Phase 1 config and component specs from PocketEngine as tests.

## M2: graphics

Status: first slice done on macOS (2026-09-18), evidence in `tests/evidence/m2/`. Delivered:
`engine/renderer` forward renderer on the WebGPU-shaped RHI (primitive meshes, one directional and
up to eight point lights, first active Camera, per-object uniforms with dynamic offsets), an entity
id buffer written with every frame plus `render.pick`, `render.project` and `render.ids` commands,
`Bounds` computed by the world, captures of color and ids. Since then: textures and glTF meshes (M5
first half), instanced draws, sprites and orthographic cameras, a directional shadow map
(`docs/design/rendering.md`). Not yet: MSAA, golden-image comparisons across GPUs.

RHI decision (ADR 0002), mesh, material, light and camera, offscreen render targets, frame capture
and golden images. Ports the Phase 1 math conventions spec (right-handed, +Y up, depth [0,1],
counter-clockwise front faces, column-major matrices) as tests.

## M3: UI system and editor shell

Status: the agent interface part is done (2026-09-18): `pocket mcp` serves build, test, headless
runs and live paused sessions (tree, query, describe, schema, step, events, capture, pick, any
runtime command) over the Model Context Protocol; evidence in `tests/evidence/m3/`, usage in
`docs/mcp.md`. Pocket UI is done (2026-09-18): `engine/ui` (FreeType text, batched painter, Yoga
element tree, hit testing, focus, text input, scrolling, `ui.*` commands with text snapshots and
synthetic input), TSX with signals and a diffing reconciler in `sdk/runtime/ui.ts`, `samples/ui`,
`docs/design/pocket-ui.md`, evidence in `tests/evidence/ui/`. HarfBuzz is not integrated yet
(FreeType advances only). The editor shell is done (2026-09-18): `pocket editor <project>` runs
`editor/` (TSX) beside the project in its own script context with play/pause/step/stop, hierarchy,
scene pane with picking and orbit, a schema-driven inspector, console/events/transcript, and scene
saving; agents drive it headless through the same `ui.*` commands (`docs/editor.md`,
`tests/evidence/editor/`). Docking, undo and gizmos are not started. Assets (M5, first half,
2026-09-18): `engine/assets` reads glTF 2.0 and images, the renderer draws textured glTF meshes with
per-material submeshes, `samples/assets` and `docs/design/assets.md`; `pocket pack` produces a
self-contained folder or zip (`docs/packaging.md`), checked in CI by running the packed game
headless. Audio (2026-09-19): `engine/audio` with tick-driven voices, an SDL3 mixer, the
`AudioSource` component and `audio.*` commands (`docs/design/audio.md`).

Pocket UI (ADR 0004): element tree with a DOM-like API, Yoga flexbox layout, CSS subset, FreeType
and HarfBuzz text, the 2D batch renderer on the RHI, TSX components with signals, tree snapshots and
selector queries, golden-image UI tests. Then the editor shell on top of it: docking, menus,
document and command model, scene view, inspector generated from metadata, play and stop, an MCP
server exposing the same commands as the CLI, evidence capture hooks as a product feature. No Dear
ImGui at any milestone; before M3, verification is headless.

## M4: physics

Status: done as a new implementation rather than a port (2026-09-18), evidence in
`tests/evidence/m4/`. `engine/physics`: boxes and spheres, static/dynamic/kinematic bodies from
`RigidBody` + `Collider` metadata, gravity, sort-and-sweep broadphase ordered by entity id, SAT and
sphere tests, sequential impulses with accumulated clamped normal and friction impulses, Baumgarte
correction, restitution, sleeping, triggers, `collision.begin/end` and `trigger.enter/exit` events
with causes, raycast and sphere overlap, `physics.*` commands and the `onContacts` SDK hook.
Capsules, rotation locks and joints (distance, rope, ball, break forces, `physics.joints`) followed
on 2026-09-19 (`docs/design/physics.md`). Not yet: mesh colliders, hinge limits and motors,
continuous collision, per-body layers.

The original plan was a port of the aipocket `Physics3D` engine (bodies, shapes, SAT manifolds,
sequential impulses, sleeping, triggers, raycasts) with its unit tests, bridged to the ECS and
TypeScript.

## M5: assets, cook, package

Asset registry, content-addressed derived-data cache, cooking, packaging for the three desktop
platforms, the first dream-game templates.

Status (2026-09-19): partly there. Packaging exists for macOS (`pocket pack`) and the web (`--web`);
Windows and Linux packs wait on their platforms (ADR 0005). Assets load from the project's files
through the asset store (`assets.list`, `assets.describe`, `assets.reload`), with no registry file
and no content-addressed derived-data cache: the only cook step is the font subset of a web pack,
and glTF, images, sounds and maps are read as they are. Templates are the samples:
`pocket new <name> --from <sample>` copies one (`docs/status.md`, Authoring).

## M6: ship by link

Web export: the core compiled to wasm32 with Emscripten and SDL3, the RHI on browser WebGPU, scripts
running on the browser's JavaScript engine. Portability of the core and the WebGPU-shaped RHI are
constraints from M0 so that this milestone is a port, not a rewrite.

Status: done on 2026-09-19 as `pocket pack <project> --web` (`docs/web.md`), with the hello and
sprites samples verified in Chromium: rendering, shadows, Pocket UI text, keyboard input, captures
through the runtime's own command, resizing. Fonts are subset per project (a pack is 6 MB, the
runtime itself), saves persist in IndexedDB, text input goes through the page (IMEs work), and
`--editor` ships the editor in the page.

## Reuse of existing tests

| Source | Kept as | Notes |
|---|---|---|
| PocketEngine `tests/pocket3d/phase1_math_spec_tests.cpp` | M2 convention spec | Rewritten against the new RHI; the conventions stay. |
| PocketEngine `phase1_config_tests`, `phase1_components_tests`, `phase1_test_spec_tests` | M1 specs | Config and scene formats change; the behaviors are the spec. |
| PocketEngine editor bridge and AI service tests | M3 reference | The protocol becomes MCP-first; tests are re-derived. |
| aipocket `physics3d_tests.cpp` (14 cases) | M4, ported nearly as is | Pure C++, no engine dependency. |
| aipocket OpenGL and Metal smoke tests | M2 reference | The backend API changes; the checks (clear, draw, readback, pixel identity across backends) carry over. |
| aipocket runtime physics smoke tests and the `Pocket3DPhysics` Lua project | M4 sample | Rewritten in TypeScript as the first sample project. |
