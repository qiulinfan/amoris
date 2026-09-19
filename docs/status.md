# Status (2026-09-18)

What exists on the `agent-first` branch, how to verify it, and what is next. Everything listed builds and passes `pocket test` on macOS (Apple silicon) in the sanitized debug configuration.

## Built

| Area | Where | Verified by |
|---|---|---|
| Build tool `pocket` (Rust): manifests, toolchain discovery, Ninja graph, `compile_commands.json`, dependency fetch with sha256 and CMake foreign builds, oxc TypeScript bundler, codegen, `--json` everywhere, MCP server | `tools/pocket` | every command below |
| Web build (M6): `[configs.wasm]` with Emscripten (6.0.9), SDL3 and WebGPU from its ports, CMake dependencies rebuilt per target (`pocket setup --target wasm`); the runtime on the browser's frame callback, the RHI on browser WebGPU (Asyncify for its asynchronous waits), scripts on the browser's JavaScript engine (`web_host.cpp`), `pocket pack <project> --web [--editor]` producing a static folder whose page exposes every command as `window.pocket.command`, with the UI font subset to the project's text (16.4 MB to 44 KB), saves in IndexedDB, text input (IME included) through the page, and the editor when asked | `engine/script/src/web_host.cpp`, `engine/app/src/runtime.cpp`, `tools/pocket/src/pack.rs`, `docs/web.md` | `tests/evidence/web/` (captures the runtime made inside Chromium, keyboard-driven movement, resize) |
| Core: `Result`/`Error` on `std::expected`, JSONL logging with a ring, fixed tick clock, PCG32, state hasher, math, filesystem | `engine/core` | `core_tests` |
| Platform: SDL3 window, normalized keyboard/mouse/gamepad events, OS text input, headless mode | `engine/platform` | runtime tests, windowed runs |
| Shadows: a 2048x2048 directional shadow map fitted to the scene's bounds, 3x3 comparison filtering with slope-scaled bias, `render.shadows` settings and `[render] shadows` in project.toml | `engine/renderer`, `docs/design/rendering.md` | `renderer_tests` (`[shadows]`: shaded ground is darker than lit ground, and equal with shadows off) |
| Text shaping: HarfBuzz (OpenType contextual forms, ligatures, kerning, marks, RTL visual order) feeding the FreeType atlas by glyph index; measurement cache | `engine/ui/src/font.cpp`, `docs/design/pocket-ui.md` | `ui_tests` (`[shaping]`) |
| Save slots: `save.write/read/list/delete/dir` (scene + `onSave` script state per context + caller data, one JSON per slot in the OS user data directory or `--save-dir`), `saves` SDK, `save.written/loaded` events | `Session::save_command`, `sdk/runtime/pocket.ts`, `docs/sdk.md` | `runtime_tests` (`[saves]`) |
| 2D: the `Sprite` component (textured unit quad in XY, size, anchor, tint, layer, sheet sub-rectangle, flips), unlit and alpha blended after meshes in layer order through the instanced path, orthographic cameras, bounds and picking by shape; sheet clips (`[sprite_clips]`, `sprite.clip/play/stop`, `SpriteAnimation`) advanced by the world tick with `sprite.finished` events | `engine/renderer` (sprite pipeline), `engine/world` (Sprite bounds, clips), `samples/sprites`, `docs/design/sprites.md` | `runtime_tests` (`[sprites]`), `world_tests` (`[sprites]`), `tests/ts/sprites.test.ts` |
| Tile maps: Tiled JSON (`.tmj`) read by the asset store (tile and object layers, embedded tilesets, flips, properties, groups), the `TileMap` component drawing each visible layer as one static mesh per tileset through the sprite path, `tilemap.info/cell/tile/solid/objects` in world units | `engine/assets` (`parse_tilemap`), `engine/renderer` (tile layer meshes), `docs/design/tilemaps.md`, `samples/sprites` (level.tmj) | `assets_tests` (`[tilemap]`), `runtime_tests` (`[sprites]`), `tests/ts/tilemap.test.ts` |
| Skeletal animation: glTF skins, node hierarchies and clips read by the asset loader; `Animator` advanced on the fixed tick, clips sampled into joint matrices, cross-fades between clips (`animation.play {fade}`), skinned draws (scene and shadow passes) through a joint storage buffer; `animation.clips/play/stop/pose` and `animation.finished` | `engine/assets`, `engine/renderer/animation.cpp`, `docs/design/animation.md`, `samples/assets` (a generated two-joint arm) | `assets_tests` (`[animation]`), `runtime_tests` (`[animation]`: the pose turns and a pick lands on the bent mesh), `tests/ts/animation.test.ts` |
| Particles: `ParticleEmitter` (cone, gravity, drag, lifetime, size and color over age, billboard or XY, world or local space) simulated on the fixed tick with a seeded stream per emitter, drawn as instanced unlit quads through the sprite path, in the state hash and the id buffer; `particles.stats/burst/clear` | `engine/renderer/particles.cpp`, `docs/design/particles.md`, `samples/playground` (fountain, sparks on hits) | `runtime_tests` (`[particles]`), `tests/ts/particles.test.ts` |
| Instanced rendering: per-object data in one storage buffer indexed by `instance_index`, one draw per run of equal mesh/submesh/material (3000 cubes: 3000 draws to 1, 0.90 ms to 0.35 ms); `perf` command and `report.timings` with wall-clock averages per phase | `engine/renderer`, `Session::perf` | `renderer_tests`, `docs/evidence/swarm.md` |
| Editor depth: undo/redo of every edit, multi-selection, move/rotate/scale gizmo handles agents can drag, resizable panes with the layout saved per project, keyboard shortcuts, modifier keys on events | `editor/history.ts`, `editor/gizmo.ts`, `editor/main.tsx`, `docs/editor.md` | `runtime_tests` (`[editor]`) |
| Scripting helpers: tweens (numbers and component fields, easing, repeat/yoyo/delay) and timers (`after`, `every`, `wait`, the setTimeout family) on the simulation clock | `sdk/runtime/tween.ts`, `sdk/runtime/timer.ts`, `docs/sdk.md` | `tests/ts/tween.test.ts` |
| Input actions: action maps (keys, pad buttons, axes with dead zones) from project.toml or scripts, per-frame edges in every tick, synthetic holds for agents that the journal records | `engine/app/input_map`, `sdk/runtime/input.ts`, `docs/design/input.md` | `runtime_tests` (`[input]`), `tests/ts/world.test.ts` |
| RHI: wgpu-native device, offscreen target, blit present, captures | `engine/rhi` | `renderer_tests`, captures in evidence |
| Script host: JavaScriptCore backend behind `ScriptHost` (ADR 0005), JSON dispatch, typed-array sharing | `engine/script` | every sample and TypeScript test |
| World: flecs entities with ordered hierarchy, generated components (`pocket gen`), JSON access with merge, tree/describe/query/summary/schema, deterministic hash, scenes, motion/lifetime/transform/bounds systems, causal event log, gameplay transcript | `engine/world` | `world_tests`, `tests/ts/world.test.ts` |
| Renderer: primitive meshes with texture coordinates, glTF meshes with per-material draws, metallic-roughness materials with base color, metallic-roughness, normal and emissive maps (GGX lighting), lights, camera, entity id buffer, pick/project/ids, scene viewport, debug lines (`render.debug` overlays for colliders, joints, bounds, axes; `debug.line/box/sphere` for scripts) | `engine/renderer`, `docs/design/rendering.md` | `renderer_tests` (`[debug]`), `tests/evidence/rendering/overlays.png` |
| Assets: glTF 2.0 reader (.glb/.gltf, baked nodes, materials), stb_image decoding, AssetStore with project-relative paths, `assets.list/describe/reload/stats`, missing assets drawn magenta and reported | `engine/assets`, `docs/design/assets.md` | `assets_tests`, `renderer_tests` (`[assets]`) |
| Physics: boxes, spheres and capsules, sequential impulses, sleeping, rotation locks, triggers, raycast and overlap, contact events, distance/rope/ball joints with break forces and `joint.broken` | `engine/physics`, `docs/design/physics.md` | `physics_tests`, `runtime_tests` (`[joints]`) |
| Runtime: session with one command surface for scripts, HTTP JSON-RPC (`--serve`, `--paused`, `step`), input journal record/replay, reports with state hash, transcript, captures | `engine/app`, `engine/runtime` | `runtime_tests`, evidence transcripts |
| Perception instruments: `events.why` (cause chains as a story), the frame recorder (`--history N`, `recorder.at/diff/track/first`: time travel over every entity's components), `render.visible` and `render.ids` with per-entity coverage, bounds and centers | `engine/world/recorder.hpp`, `engine/app/src/session.cpp`, `docs/design/agent-perception.md` | `world_tests` (`[recorder]`, `[events]`), `runtime_tests` (`[recorder]`) |
| SDK: `pocket` module (lifecycle, expose, log, onFrame), `world` (with prefabs: instantiate/savePrefab/loadScene), `events`, `render`, `physics`, `audio`, `ui`, `transcript`, in-engine `test` | `sdk/runtime` | `tests/ts` |
| Typed arrays: `world.pack` / `world.unpack` share Float32/Float64 buffers with scripts for per-tick updates of thousands of entities; `samples/swarm` reports the cost of both paths | `engine/world`, `sdk/runtime/world.ts` | `tests/ts/world.test.ts`, `docs/evidence/swarm.md` |
| Authoring: `pocket new <name>` scaffolds a runnable project (scene, crate prefab, WASD script with exposed state); prefabs and scene files by path through `world.instantiate` / `world.save_prefab` / `world.load {path}` | `tools/pocket/src/commands.rs`, `engine/world` | `runtime_tests` (`[prefab]`), `tests/ts/world.test.ts` |
| Pocket UI: FreeType text (Noto Sans CJK), batched 2D painter, retained element tree with Yoga flexbox, hit testing, focus and text input, scrolling, text snapshots and synthetic input for agents, scene viewport; TSX with signals and a diffing reconciler; script contexts for editor + project | `engine/ui`, `sdk/runtime/ui.ts`, `docs/design/pocket-ui.md` | `ui_tests`, `tests/ts/ui.test.tsx`, `tests/evidence/ui/` |
| Hot reload: `pocket run <project> --watch` and `pocket editor <project> --watch` rebundle on source changes and call `project.reload` over the control server; a bundle error keeps the previous scripts | `tools/pocket/src/watch.rs`, `project.reload` in `engine/app` | manual session in `tests/evidence/editor/watch.txt` |
| Editor: `pocket editor <project>`; a TSX program in its own script context with toolbar (play/pause/step/stop, save, spawn, delete), hierarchy, scene pane (pick, orbit, zoom), schema-driven inspector, console/events/transcript; operable headless by agents | `editor/`, `docs/editor.md` | `runtime_tests` (`[editor]`), `tests/evidence/editor/` |
| Audio: WAV clips via SDL, tick-driven voices (deterministic positions, loop/finish events), software mixer into an SDL3 stream, `AudioSource` component with autoplay, `audio.*` commands and SDK | `engine/audio`, `docs/design/audio.md` | `audio_tests` |
| Packaging: `pocket pack <project> [--zip]` builds a self-contained folder (runtime, bundle, config, scene, assets, font, launcher) that runs and serves agents without the repository | `tools/pocket/src/pack.rs`, `docs/packaging.md` | CI packs `assets` and runs it headless |
| Samples: hello (bouncing ball), playground (scene, enemies, causal hits), physics (ramp, arena, trigger goal, pendulum chain, snapping rope, capsule log), ui (HUD, buttons, pause menu in TSX), assets (glTF crate and pyramid, checker texture, a plate with normal, metallic-roughness and emissive maps, a gold orb, a missing asset), audio (looping hum, beeps, landing clicks) | `samples/` | headless runs in evidence |
| Agent interface: `pocket mcp` with build/test/run tools and live sessions | `tools/pocket/src/mcp.rs`, `docs/mcp.md` | `tests/evidence/m3/mcp-session.txt` |

## Verify

```bash
export DEVELOPER_DIR=/Library/Developer/CommandLineTools   # only when Xcode itself is unusable
./scripts/bootstrap.sh
./.pocket/pocket test --json
./.pocket/pocket run physics -- --headless --frames 600 --json --capture out.png
./.pocket/pocket run ui -- --headless --serve 4711 --paused --json   # then POST ui.snapshot / ui.click to /rpc
```

The `transcript` field of any report, `GET /tree` on a served runtime, and the `world_tree`/`transcript` MCP tools are the quickest way to see what "agent-first" means in practice.

## Deviations from the plan

- JavaScriptCore instead of V8 on macOS (ADR 0005); V8 stays the target for Linux and Windows.
- Ninja as a subprocess instead of embedded n2.
- Physics is a new implementation, not a port of the aipocket reference branch.
- CI runs on GitHub (macOS job green through the HarfBuzz commit); from the shadow-map commit on, GitHub declines to start jobs on this account until its spending limit is raised, so later runs are unverified there. The `web` job (Ubuntu + Emscripten) was added while that block was in place and has not run yet.

## Next

1. Web build follow-ups: JSPI instead of Asyncify where the browser has it, a smaller runtime (`-Oz`, modules a project does not use left out), a download button for what the browser editor saves.
2. Renderer: MSAA (needs a separate id pass), layered animation blending (masks, additive clips); physics: hinge limits and motors, mesh colliders.
3. Agent eval suite and the perception-efficiency benchmark from `docs/design/agent-perception.md`; executable scenarios with bots; gameplay analyzers on top of `recorder.track`.
4. Editing tiles at runtime and 2D physics against tile maps.
