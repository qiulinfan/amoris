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
| Renderer: primitive meshes, lights, camera, entity id buffer, pick/project/ids | `engine/renderer` | `renderer_tests` |
| Physics: boxes and spheres, sequential impulses, sleeping, triggers, raycast, contact events | `engine/physics` | `physics_tests` |
| Runtime: session with one command surface for scripts, HTTP JSON-RPC (`--serve`, `--paused`, `step`), input journal record/replay, reports with state hash, transcript, captures | `engine/app`, `engine/runtime` | `runtime_tests`, evidence transcripts |
| SDK: `pocket` module (lifecycle, expose, log), `world`, `events`, `render`, `physics`, `transcript`, in-engine `test` | `sdk/runtime` | `tests/ts` |
| Pocket UI: FreeType text (Noto Sans CJK), batched 2D painter, retained element tree with Yoga flexbox, hit testing, focus and text input, scrolling, text snapshots and synthetic input for agents, scene viewport; TSX with signals and a diffing reconciler; script contexts for editor + project | `engine/ui`, `sdk/runtime/ui.ts`, `docs/design/pocket-ui.md` | `ui_tests`, `tests/ts/ui.test.tsx`, `tests/evidence/ui/` |
| Samples: hello (bouncing ball), playground (scene, enemies, causal hits), physics (ramp, arena, trigger goal), ui (HUD, buttons, pause menu in TSX) | `samples/` | headless runs in evidence |
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

1. The editor shell on Pocket UI (`pocket editor <project>`): hierarchy, scene pane, inspector from `world.schema`, play/pause/step, console and transcript, save scene.
2. Typed-array component views for hot loops (scripts pay one JSON round trip per call today).
3. Agent eval suite and the perception-efficiency benchmark from `docs/design/agent-perception.md`.
4. Assets and packaging (M5), web export (M6).
