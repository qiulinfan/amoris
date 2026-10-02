# Working in Pocket3D

This file is read by AI coding agents (Claude Code through `CLAUDE.md`, Codex and others directly) and by humans. It states how work is done here. Project-wide decisions live in `docs/decisions/`; this file only points at them and lists the rules that apply to every change.

## What this project is

Pocket3D is an AI-agent-first game engine started from zero in 2026. Its human collaborator (the repository owner) and AI agents work as peers: agents write most of the code, humans set direction, review and decide. See `docs/agent-first.md`.

## Language boundaries (strict)

| Layer | Language | Rule |
|---|---|---|
| Engine runtime and editor core | C++26 (feature allowlist in ADR 0001) | Anything that runs per frame or ships in the runtime. |
| Build tool `pocket` | Rust (pinned in `rust-toolchain.toml`) | The only binary needed to bootstrap a checkout: toolchains, dependency fetch, build graph, codegen, TypeScript transform, asset cooking, tests, packaging, MCP server. |
| Gameplay, UI, editor extensions, SDK | TypeScript 7 on a JIT engine (JavaScriptCore now, V8 planned; ADR 0005) | The public face of the engine: game logic, all UI (editor and runtime, ADR 0004) and editor extensions. Type declarations are generated from engine metadata, never hand-written. |
| Offline tooling | Python 3.14 with uv | Analysis, data conversion, experiments. Never on the build or runtime critical path; the engine must build and run without Python. |

Lua is not used. CMake is not used for our own code (it may be invoked by `pocket` to build a third-party library as a cached foreign step).

## Rules for every change

1. Keep the repository buildable end to end at every commit once milestone 0 exists. Add capabilities on top of a runnable baseline; never replace verified behavior with unfinished complexity.
2. Prefer the simplest coherent implementation of the current requirement. No speculative abstractions, configuration layers or compatibility shims.
3. Every change carries evidence: the commands run and their results (tests, captures, probes). Evidence directories live under `tests/evidence/<topic>/` with a README that says how each artifact was produced.
4. Decisions that shape the architecture get a decision record in `docs/decisions/` (next number, status Proposed until the human collaborator accepts).
5. Third-party code enters only through `third_party/` with a pinned version and license note, or as a prebuilt artifact fetched by `pocket` from a pinned manifest.
6. Reference sources are read-only. PocketEngine and aipocket may be ported from (they are ours). Unreal Engine source (Epic EULA) and Unity's C# reference source (Unity Reference-Only License) are for studying design only: never copy code, identifiers or comments from them. See `docs/ue5-lessons.md` and `docs/unity-lessons.md`.
7. User-facing explanations match the language the human uses (currently Chinese). Code, identifiers, commit messages and repository documentation are English.
8. Commit only when asked. AI-authored commits end with the agent's co-author trailer.
9. Code that changes the world during a tick (physics, water, characters, rigs, timelines, navigation, particles, the SDK's tweens) takes sin, cos, atan2, exp, pow and the like from `pocket::repro` (C++) or `repro` (SDK), not `<cmath>` or `Math`: a native and a web peer of a lockstep game must compute the same bits (`docs/design/networking.md`, Determinism). Drawing may use either.
10. Shaders are WGSL compiled by naga natively and by Tint in browsers, and Tint is stricter (it wants parentheses where `*` meets `^` or `&&` meets `||`, uniform control flow around `textureSample`): after changing a shader, pack a sample with `--web` and load it (`docs/web.md`) before calling it done; a module Tint rejects draws nothing.
11. Docs under `docs/` are prose wrapped at 100 columns (tables, code and generated files aside): agents read them by searching, and a match returns its line, so a paragraph on one line came back whole for every word in it. After writing a doc, run `python3 tools/scripts/dev/wrap_docs.py <files>` (it changes only where lines break).

## Daily commands

```bash
export DEVELOPER_DIR=/Library/Developer/CommandLineTools   # when Xcode itself is unusable
./scripts/bootstrap.sh                 # once: build tools/pocket, fetch dependencies
./.pocket/pocket build [--config release] [--json]
./.pocket/pocket run hello -- --headless --frames 120 --json --capture out.png   # stdout: the runtime's report alone (the tool's summary goes to stderr)
./.pocket/pocket test [--filter core] --json
./.pocket/pocket ts samples/hello      # bundle only
./.pocket/pocket check [samples/hello] # TypeScript types against the SDK (TypeScript 7; docs/sdk.md, Types)
./.pocket/pocket gen [--check]         # regenerate code from engine/*/meta/*.toml
./.pocket/pocket run playground -- --headless --serve 4711 --paused --json   # agent-driven session; enemies path around pillars (docs/design/navigation.md)
./.pocket/pocket mcp                   # MCP server over stdio (see docs/mcp.md)
./.pocket/pocket run physics -- --headless --frames 600 --json   # physics sample report
./.pocket/pocket run ui -- --headless --serve 4711 --paused --json   # UI sample; ui.snapshot / ui.click over /rpc
./.pocket/pocket editor physics                                      # the editor window (docs/editor.md)
./.pocket/pocket new mygame && ./.pocket/pocket run mygame           # scaffold a project under projects/ and run it
./.pocket/pocket run hello --watch                                   # window + hot reload when scripts change
./.pocket/pocket run assets -- --headless --frames 20 --json          # glTF meshes and textures (docs/design/assets.md)
./.pocket/pocket pack assets --zip                                   # dist/assets/ + dist/assets.zip (docs/packaging.md)
./.pocket/pocket setup --target wasm && ./.pocket/pocket pack hello --web   # dist/web/hello/ for a browser (docs/web.md); --editor ships the editor too
./.pocket/pocket run sprites -- --headless --frames 120 --json        # a 2D platformer: sprites, orthographic camera, tile map, Body2D (docs/design/sprites.md, docs/design/tilemaps.md)
./.pocket/pocket run audio -- --headless --frames 300 --json          # voices and audio events without a sound card (docs/design/audio.md)
./.pocket/pocket run swarm -- --headless --frames 120 --json          # 3000 entities per tick through typed arrays (tick.ms in state)
./.pocket/pocket scenario walker                                      # a 3D character (Character: stairs, slopes, a lift, crates) walked by a bot (docs/design/physics.md, Characters)
./.pocket/pocket run hills -- --headless --frames 60 --json            # noise hills (Terrain), a character on them, a lake (Water) things float on; terrain.height / water.height (docs/design/terrain.md, docs/design/water.md)
./.pocket/pocket scenario drive                                       # a car on raycast wheels (Vehicle) through four gates on a terrain (docs/design/physics.md, Vehicles)
./.pocket/pocket scenario fps                                         # first person: mouse look, hitscan shots, targets, raiders that hear and chase (docs/design/combat.md, Hitscan)
./.pocket/pocket scenario guards                                      # stealth: guards that patrol, see, hear a running player and search (docs/design/behavior.md)
./.pocket/pocket scenario village                                     # a 3D village without files: props, patterns, toon look, villagers, the elder's quest by dialogue
./.pocket/pocket run arena -- --net-host 7777   # and, elsewhere, -- --net-join HOST:7777: a lockstep two-player game (docs/design/networking.md)
# agents drive a game: input.hold {action, ticks} then step {ticks} then transcript (docs/design/input.md)
# time travel: run with --history 600, then recorder.at {tick} / recorder.diff {from, to} / recorder.track {entity, component, field}; events.why {seq} explains an event
# see the invisible: render.debug {colliders: true, joints: true} then capture; debug.line/box/sphere mark your own points
./.pocket/pocket scenario sprites --seeds 10                          # gameplay scenarios: play through actions, check outcomes, many seeds (docs/design/scenarios.md)
./.pocket/pocket bench physics                                        # perception benchmarks: what each gameplay answer costs in tokens against frame-by-frame vision
# where the time goes: the perf command (script, physics, world, state, render ms per phase)
./.pocket/pocket editor physics --watch                              # the same inside the editor
./.pocket/pocket graph                 # module graph
```

Rules that follow from the implementation:

- Engine modules declare `public_deps` and `private_deps` in `module.toml`; a header in `include/` must not include a private dependency's headers (SDL, JavaScriptCore stay inside their modules).
- Simulation code never reads the wall clock; use the tick's `dt`. Randomness comes from the seeded `Random` (C++) or `random()` (TypeScript).
- Anything observable by an agent goes through `expose()` in scripts or the report; do not print to stdout from the engine (stdout is the JSON channel).
- Golden values under `tests/evidence/` change only deliberately, with the reason in the commit message.
- Engine components are declared in `engine/world/meta/components.toml`; never hand-edit a `*.gen.*` file or `sdk/runtime/generated/`. A game's own components (an enemy's kind, a door's key) go in its project's `components.toml` instead (ADR 0007, `docs/sdk.md`, Your own components), never as state in script Maps that the world cannot see.
- Anything a script can ask the world is a command (`command(name, params)`), and every command is also reachable over `--serve`; do not add script-only or agent-only paths.

## Driving the engine from an agent

A running runtime is the engine's interface: every feature is a command on its control server (`--serve <port>`), the same one scripts, the editor and tests use.

- Start one: `./.pocket/pocket run <project> -- --headless --serve 4711 --paused --json`, or open the editor with `./.pocket/pocket editor <project> -- --serve 4711`.
- Call it: the `pocket` MCP server (registered in `.mcp.json`; `runtime_attach {url}`, or it attaches to `$POCKET_RPC_URL`), the pi extension `integrations/pi/pocket.ts`, or `./.pocket/pocket rpc <method> '<params>'` in a shell.
- After editing a project's scripts or `project.toml` while it runs: `project.apply` bundles, type-checks, reloads and steps it in one call (docs/mcp.md, Applying an edit).
- Ask it: `project.brief` first (the files, scene, components, actions, state and problems as one text; `runtime_start`/`runtime_attach` answer with it); `help {command}` gives a command's parameters and purpose; a command refuses keys it does not take and names the right ones. `world.tree`, `world.query`, `world.describe`, `transcript` and `events.why` read the game as text; `world.lint` lists what is likely wrong (with fixes) after building something; `render.views {path, entity?}` shows it from six sides in one picture; `capture` and `render.visible` show what the camera sees.
- The runtime is paused; `step {ticks}` advances it, drawing only the last tick when headless (`render: "each"` for effects that build up over frames: TAA, auto exposure, probe bounces), and `step {ticks, until}` stops when an event comes or a value crosses a line. Entities are ids, names or paths (`Player`, `/Level/Player`).

## Development helpers

`tools/scripts/dev/` holds the scripts a working session reuses (standard library only):

- `runtime.py`: `Runtime(sample, port, size, release=False)` starts a headless runtime serving a sample (bundle it first with `pocket ts`) and `rpc(method, params)` calls it; as a script, `runtime.py <sample> <method> '<json>' ...` runs calls in order and prints the answers. Run anything that talks to a runtime outside the sandbox (loopback connections are blocked inside it).
- `gpu_probe.py <sample> [--size WxH] [--frames N] [--rpc method '<json>']`: the release runtime's GPU time per frame, in all and per pass (`render.stats.gpu`), beside `perf`; for measuring a renderer change before and after. Close any browser page drawing a pack first (and do not measure while an agent benchmark runs): another workload on the GPU doubles every number.
- `suite.sh [name]`: the whole `pocket test --json` in the background into `build/test-reports/<name>.json`, with `<name>.done` when it ends; `test_summary.py <report>` prints the verdict and each failure. Run the module or tag a change touches directly (`./build/debug/bin/<module> "[tag]"`) while working, and the whole suite before a commit that changes shared code.
- `edits.py`: `patch(path, [(old, new), ...])`, exact replacements that fail unless each old text occurs once.
- `prose.py`: `replace(path, old, new)` and `append_after(path, old, new)` for wrapped documentation, matching the old text whatever its line breaks (then `wrap_docs.py`).

Image work (cropping or stitching captures for evidence) uses Pillow in the ignored `.venv` at the root: `python3 -m venv .venv && .venv/bin/pip install pillow`.

## Repository layout

```
pocket.toml        workspace manifest: targets, platforms, toolchain and dependency pins
tools/pocket/      Rust build tool (cargo workspace)
engine/            C++26 modules, one directory per module with a module.toml
sdk/               TypeScript SDK: generated .d.ts, runtime helpers, base tsconfig
tools/scripts/     Python tools: the agent benchmark and its runners, sample assets, evidence; dev/ (Development helpers)
editor/            the editor, a Pocket UI program in TypeScript
integrations/      agent integrations (pi extension)
samples/           sample projects written in TypeScript
tests/             cross-cutting integration tests, golden images, evidence
docs/              decisions/, design documents, lessons
third_party/       vendored source libraries and prebuilt manifests
.pocket/           local state, caches, fetched toolchains (ignored)
build/             build outputs (ignored)
```
