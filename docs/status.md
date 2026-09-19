# Status (2026-09-18)

What exists on the `agent-first` branch, how to verify it, and what is next. Everything listed builds and passes `pocket test` on macOS (Apple silicon) in the sanitized debug configuration.

## Built

| Area | Where | Verified by |
|---|---|---|
| Build tool `pocket` (Rust): manifests, toolchain discovery, Ninja graph, `compile_commands.json`, dependency fetch with sha256 and CMake foreign builds, oxc TypeScript bundler, codegen, `--json` everywhere, MCP server | `tools/pocket` | every command below |
| Core: `Result`/`Error` on `std::expected`, JSONL logging with a ring, fixed tick clock, PCG32, state hasher, math, filesystem | `engine/core` | `core_tests` |
| Platform: SDL3 window, normalized input events, headless mode | `engine/platform` | runtime tests, windowed runs |
| RHI: wgpu-native device, offscreen target, blit present, captures | `engine/rhi` | `renderer_tests`, captures in evidence |
| Script host: JavaScriptCore backend behind `ScriptHost` (ADR 0005), JSON dispatch, typed-array sharing | `engine/script` | every sample and TypeScript test |
| World: flecs entities with ordered hierarchy, generated components (`pocket gen`), JSON access with merge, tree/describe/query/summary/schema, deterministic hash, scenes, motion/lifetime/transform/bounds systems, causal event log, gameplay transcript | `engine/world` | `world_tests`, `tests/ts/world.test.ts` |
| Renderer: primitive meshes with texture coordinates, glTF meshes with per-material draws, base color textures, lights, camera, entity id buffer, pick/project/ids, scene viewport | `engine/renderer` | `renderer_tests` |
| Assets: glTF 2.0 reader (.glb/.gltf, baked nodes, materials), stb_image decoding, AssetStore with project-relative paths, `assets.list/describe/reload/stats`, missing assets drawn magenta and reported | `engine/assets`, `docs/design/assets.md` | `assets_tests`, `renderer_tests` (`[assets]`) |
| Physics: boxes and spheres, sequential impulses, sleeping, triggers, raycast, contact events | `engine/physics` | `physics_tests` |
| Runtime: session with one command surface for scripts, HTTP JSON-RPC (`--serve`, `--paused`, `step`), input journal record/replay, reports with state hash, transcript, captures | `engine/app`, `engine/runtime` | `runtime_tests`, evidence transcripts |
| SDK: `pocket` module (lifecycle, expose, log, onFrame), `world` (with prefabs: instantiate/savePrefab/loadScene), `events`, `render`, `physics`, `audio`, `ui`, `transcript`, in-engine `test` | `sdk/runtime` | `tests/ts` |
| Typed arrays: `world.pack` / `world.unpack` share Float32/Float64 buffers with scripts for per-tick updates of thousands of entities; `samples/swarm` reports the cost of both paths | `engine/world`, `sdk/runtime/world.ts` | `tests/ts/world.test.ts`, `docs/evidence/swarm.md` |
| Authoring: `pocket new <name>` scaffolds a runnable project (scene, crate prefab, WASD script with exposed state); prefabs and scene files by path through `world.instantiate` / `world.save_prefab` / `world.load {path}` | `tools/pocket/src/commands.rs`, `engine/world` | `runtime_tests` (`[prefab]`), `tests/ts/world.test.ts` |
| Pocket UI: FreeType text (Noto Sans CJK), batched 2D painter, retained element tree with Yoga flexbox, hit testing, focus and text input, scrolling, text snapshots and synthetic input for agents, scene viewport; TSX with signals and a diffing reconciler; script contexts for editor + project | `engine/ui`, `sdk/runtime/ui.ts`, `docs/design/pocket-ui.md` | `ui_tests`, `tests/ts/ui.test.tsx`, `tests/evidence/ui/` |
| Hot reload: `pocket run <project> --watch` and `pocket editor <project> --watch` rebundle on source changes and call `project.reload` over the control server; a bundle error keeps the previous scripts | `tools/pocket/src/watch.rs`, `project.reload` in `engine/app` | manual session in `tests/evidence/editor/watch.txt` |
| Editor: `pocket editor <project>`; a TSX program in its own script context with toolbar (play/pause/step/stop, save, spawn, delete), hierarchy, scene pane (pick, orbit, zoom), schema-driven inspector, console/events/transcript; operable headless by agents | `editor/`, `docs/editor.md` | `runtime_tests` (`[editor]`), `tests/evidence/editor/` |
| Audio: WAV clips via SDL, tick-driven voices (deterministic positions, loop/finish events), software mixer into an SDL3 stream, `AudioSource` component with autoplay, `audio.*` commands and SDK | `engine/audio`, `docs/design/audio.md` | `audio_tests` |
| Packaging: `pocket pack <project> [--zip]` builds a self-contained folder (runtime, bundle, config, scene, assets, font, launcher) that runs and serves agents without the repository | `tools/pocket/src/pack.rs`, `docs/packaging.md` | CI packs `assets` and runs it headless |
| Samples: hello (bouncing ball), playground (scene, enemies, causal hits), physics (ramp, arena, trigger goal), ui (HUD, buttons, pause menu in TSX), assets (glTF crate and pyramid, checker texture, a missing asset), audio (looping hum, beeps, landing clicks) | `samples/` | headless runs in evidence |
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
- CI workflow exists (`.github/workflows/ci.yml`) but has not run yet.

## Next

1. Editor depth: undo/redo, gizmos, multi-selection, layout persistence; HarfBuzz shaping for the UI.
2. Input mapping (actions, gamepads), tweening helpers, editor undo and gizmos.
3. Agent eval suite and the perception-efficiency benchmark from `docs/design/agent-perception.md`.
4. Web export (M6). Asset loading and packaging are done; skins, animations and PBR maps are not.
