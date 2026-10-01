# Next phase: performance, reach and agents (research, 2026-09-30)

A read-only study of the engine at 6bc9395 from four angles (CPU per tick, GPU per frame, the games
it can and cannot make, where coding agents spend their turns), each claim checked against the code
by a fifth pass, ranked by value for effort. It is the working plan; items are struck through in
docs/status.md as they land. Line numbers are at 6bc9395. The four full reports are in
`2026-09-30-reports.json` beside this file.

**Where it stands (2026-10-01).** Items 1 to 10 and 12 have landed: a56d2a6 and 245edf0 (5), bc6299e (1), 01bfacd (2), e93cb33 (3, ADR 0006), 6bd2f6e (4), d64c95b (6), 2821e44 (7), b3c809a (8: the digest changed after all, since the bytes, not the lookups, were the cost), 3d71900 (9), 787182d (10), 82fcfe2 (12, ADR 0007). Item 11 waits: the id pass draws translucent and glass surfaces into the prepass depth, so an `Equal` scene pass would drop the solids behind them unless the id pass first draws its solids alone; and on the Apple GPU this was measured on, the solid pipelines without a discard (item 9) already leave hidden-surface removal to shade a pixel about once, so the saving is for immediate-mode GPUs (most desktop and many browser ones), which should be measured there.

**After the list (2026-10-01).** From "Next after these": `EventLog::since` reads from the sequence's place (831a8c6), contacts become JSON only for a listener (831a8c6), `project.brief` and `step {until}` landed (86b59ff, 831a8c6), batched calls in the tools and the server (660ac39, efdf6f6), the combat kit's first part (hitboxes and health, 8042bce) and 2D sensors with additive sprites (03c0b33, 4292d12). On the GPU, measured on the showcase at 1920x1080 with nothing else drawing: volumetric fog blends its frames (2.55 to 1.03 ms, 748e8c9), screen-space reflections halve their steps under TAA (1.43 to 0.80 ms, d819b07), the AO blur reads the depth its pass left (0.32 to 0.21 ms, e4b3a52); the whole frame went from 8.0 to 4.95 ms. Two items did not pay on this machine and were left: the HDR ping-pong (removing the TAA copy-back changed the frame by under 0.02 ms: Apple's copies are nearly free here), and 16-bit sun cascades (kept for half the memory, f84ad7f; the shadow pass is bound by vertices, not by depth writes). A navigation crowd grid can wait: 400 agents following the player cost 1.5 ms a tick, close to linear. Measure with the built-in browser's pages closed: a visible page drawing a pack doubles every GPU time.

Roadmap: next phase of aipocket (read-only research, checked against the code)

**Context**
- **Checkout.** The repo is on `master` at HEAD 6bc9395 ("...master branch"), not `agent-first`.
- **Another session is editing the tree.** The working tree has its uncommitted changes in `engine/app/src/session.cpp` (+27 lines), `engine/world/src/world.cpp` (+10) and `engine/app/src/command_help.cpp:12`. They make `step {render: "last"|"each"}` draw only the last tick when headless, and add a `mesh_bounds_source` so Bounds come from the asset store. That is the first half of item 5.
- **Line numbers.** Every line number below is at HEAD.

**Measurements the ranking rests on (recomputed from the repo, nothing run)**
- **Test suite.** The last full `pocket test` (`build/test-reports/tangents.json`) took 806 s. Scenarios took 467 s and benches 20 s, all run one after another on the debug ASan build:
  - drive 121.5 s (9 runs, 32.8 ms per tick)
  - physics 103.1 s
  - hills 101.7 s
  - sprites 81.8 s (13.5 ms per tick)
  - walker 37.1 s (11.3 ms per tick)
  - That is 108 scenario runs on a 10-core M5.
- **Debug against release** (`tests/evidence/swarm/README.md` table): typed arrays take 10.88 against 0.353 ms (30.8x) and JSON 404.4 against 16.026 ms (25.2x).
- **Agent benchmark** (100 runs in `tests/evidence/agent-eval/*.json`):
  - Tokens grow as turns^1.62, wall time as turns^1.51 and cost as turns^1.10. I reproduced all three exponents.
  - In `pi-deepseek-extension-2.json` the 7 script tasks used 2.75M of 3.12M tokens (88%). They made 98 file or shell calls against 36 engine calls.
  - The 9 command tasks made 5 file or shell calls against 56 engine calls.

---

### 1. One call that applies an edit: `pocket apply`, an MCP `project_apply` tool and a pi `pocket_apply` tool, plus messages that stop pointing at a stale reload
- **Serves:** agent-first, the largest single lever. Script and file tasks take 51-88% of each full run's tokens, and coin_respawn alone takes 24-58%.
- **Evidence:**
  - `project.reload` re-reads only `options_.bundle` and the `<bundle>.project.json` the bundler wrote (`session.cpp:4165-4207`; written at `commands.rs:458`).
  - Its help says scripts restart "after edits" (`command_help.cpp:198`).
  - The stopped-simulation hints say "fix the script and project.reload" (`session.cpp:5379, 5384`, and the resume hint).
  - No MCP tool rebundles a running session (`mcp.rs:39-138`, 30 tools). The pi extension has only `pocket` and `pocket_look` (`integrations/pi/pocket.ts:35-83`).
  - The harness note says the harness bundles "when you are done" (`agent_eval.py:1062`).
- **Plan:**
  - **The command.** `pocket apply [--url]`:
    1. Read `dir` and `bundle` from `project.info` (`session.cpp:4152-4163`).
    2. Bundle with `bundle_project(ws, dir, Some(bundle))` (`commands.rs:429`) and type-check with `check::check`.
    3. Send the type errors as `script.diagnostics`, reusing `send_types` and `rpc` from `watch.rs:89-120`.
    4. Call `project.reload`, then `step {ticks:1}`.
    5. Print one JSON result: `{bundle_ms, type_errors, script_errors, state}`.
    6. If the bundle fails, do not reload.
  - **The tools.** Add MCP `project_apply` and pi `pocket_apply`; the pi tool spawns `pocket apply`.
  - **The messages.** Reword `command_help.cpp:198` and the hints to "rebundle (pocket apply), then project.reload". Have `project.reload` list files under `scripts/` or `project.toml` that are newer than the bundle, as `stale`.
  - **The harness.** Add one line to the harness note and to the `pi_agent.py` prompt.
- **Effort:** small (about 150 lines of Rust, about 20 of C++).
- **Worked if:**
  - The 7 script tasks with pi and the extension, run 3 times before and after, show file and shell calls cut in half (baseline 98) and script-task tokens down at least 30% (baseline 2.75M). Run-to-run variance is about 2x, which is why each side needs 3 runs.
  - omp over MCP alone passes coin_respawn without a bash call.

### 2. Run scenario and bench runs in parallel
- **Serves:** agent-loop speed and the owner's suite; measured at 487 of the suite's 806 s.
- **Evidence:**
  - The loops are file × scenario × seed, one runtime process each, one after another (`commands.rs:582-597` `run_one`, `:609` probe, `:616`/`:620` loops).
  - `pocket test` runs every sample at 3 seeds (`commands.rs:827`).
- **Plan:**
  - After the probes, collect `(file, name, seed)` jobs and run them on a `std::thread::scope` pool. Add `--jobs` with a default of `available_parallelism/2`.
  - Put results back in their original order, so rows and output stay the same.
  - Flatten all samples' jobs in `pocket test`, keeping one row per module. Do the same for bench.
- **Effort:** small (about 60 lines of Rust).
- **Worked if:**
  - `pocket test --filter scenarios` drops from 467 s to under 100 s with the same pass rows.
  - `pocket scenario sprites --seeds 10` is faster in wall time, and peak memory stays acceptable (check with `/usr/bin/time -l`).

### 3. Use the release runtime for agent-facing runs
- **Serves:** agent-loop speed. Measured 25-31x on script and world CPU.
- **Evidence:**
  - `mcp.rs:180,185`: `runtime_start` builds and runs debug.
  - `:315,:320,:329`: scenario, bench and run_headless hard-code debug and take no config.
  - `main.rs:65,78,94` default to debug.
  - `sdk/python/pocket_env.py:61` (the RL pool) defaults to `POCKET_CONFIG=debug`.
  - `pocket.toml:20-24`: debug means -O0 with ASan and UBSan.
- **Plan:**
  - MCP `runtime_start`, `pocket_scenario`, `pocket_bench` and `pocket_run_headless` take `config`, default release. CLI scenario and bench default to release, and so does `pocket_env`.
  - `pocket test`, `build`, `run` and `editor` stay on debug, so sanitizers still cover the suite and human iteration.
  - Reuse `[configs.release]`, so there is no third build tree.
  - Write an ADR (Proposed): `status.md:3` names sanitized debug as the verified configuration.
- **Effort:** small, plus the ADR.
- **Worked if:** `pocket scenario drive --seeds 3` runs at least 5x faster than the debug baseline (121 s for 9 runs), and a served `step {ticks:600}` on playground is timed both ways.

### 4. Make discovery fit the agents' tools
- **Serves:** agent-first, medium-high. The cheapest measured setup (pi with the extension: 3.1M tokens for 16 tasks, against 8.2-10.2M for omp over MCP) cannot see 46 of the 204 commands.
- **Evidence:**
  - **The cut.** `pocket.ts:12` cuts results at 24,000 characters and `:31` pretty-prints. `commands {usage:true}` comes to 32,466 characters (recomputed from `kHelp`). The cut hides 46 of the 204 commands, among them `terrain.height`, `save.*`, `script.*`, `project.reload/read/write` and `ui.*`.
  - **The other big answers.** `help {}` is about 56 KB.
  - **No filters.** `world.schema` takes no parameters (`command_help.cpp:40`) and returns every component with its docs and defaults (`world.cpp:602-624`). My estimate is 80-100 KB, so the pi tool shows about a quarter of it. `commands` takes only `usage?` (`:20`). Unknown keys are refused (`session.cpp:5203-5216`).
  - **The cut note points nowhere.** It suggests asking for "a depth, a limit or one entity" (`pocket.ts:32`), which neither command accepts.
- **Plan:**
  - Add filters: `commands {family?, search?, usage?}`, `help {command?|family?|search?}` and `world.schema {component?|components?}`.
  - Return usage as one `{text}` block with one line per command, which comes to about 20 KB.
  - Send compact JSON from `pocket.ts`, and make the cut note name the filter for the command it cut.
  - Give MCP `runtime_commands` the same `family` and `search` filters.
- **Effort:** small.
- **Worked if:** all 204 commands are visible through the pi tool, and `world.schema {component:"Light"}` is under 2 KB. Add that size bound to `runtime_tests [help]`.

### 5. Draw only when someone looks: finish what the working tree started
- **Serves:** agent-loop and RL speed. Unmeasured; I estimate 1.5-3x more ticks per second.
- **Already in the working tree:**
  - `step` skips drawing every tick but the last when headless, so `env.step` is covered too: it calls `step` (`session.cpp:2228`).
  - Skipped ticks still pump audio, update terrains and lay out the UI.
  - Bounds no longer wait for `take_new_bounds` (`session.cpp:1694`).
- **What remains:**
  - `--frames` runs (scenario, bench, run_headless) still draw every frame through `frame()` (`session.cpp:4235-4288`). At 320x180 (`commands.rs:585`) that still means 4 shadow cascades at 2048² (`renderer.cpp:8209-8245`) plus the id and scene passes.
  - Add `--render each|last` with `each` as the default, so captures and goldens don't change, and have scenario and bench pass `last`.
  - On a skipped frame, mark the frame stale and draw once before any command that reads it: `capture` and `render.pick`, `ids`, `visible`, `compare`, `stats` and `views`.
  - Reset TAA history on the first frame drawn after a gap.
- **Effort:** small.
- **Worked if:**
  - `perf.render.samples` equals the number of steps, not ticks.
  - `step {ticks:600}` takes less time on drive and on showcase.
  - The scenario wall time for drive drops.
  - On a scene without TAA, a capture after `step {ticks:120}` matches the `"each"` capture under `render.compare`.

### 6. Cursor lock, and mouse buttons as input actions
- **Serves:** reach, high for the effort: first-person, mouse-orbit third-person, twin-stick aim, click-to-move.
- **Evidence:**
  - `platform.cpp:278-282` reads `xrel`, but nothing in `engine/` or `tools/` calls `SDL_SetWindowRelativeMouseMode`.
  - `InputMap::apply` drops MouseDown and MouseUp at `default: return` (`input_map.cpp:111`), even though the platform emits them (`platform.cpp:283-290`).
  - `web_shell.html` has no pointer lock and no `touch-action`.
- **Plan:**
  - Add `Platform::set_cursor` using `SDL_SetWindowRelativeMouseMode` and `SDL_HideCursor`. SDL3's Emscripten port turns relative mode into Pointer Lock on a click.
  - Add the `input.cursor {locked?, visible?}` command, SDK `input.lockCursor()` and `[input] cursor`. Escape releases the lock; it does nothing headless, and the editor never locks.
  - Map MouseDown and MouseUp to the sources `mouse:left/right/middle/x1/x2`, skipping presses the UI took.
  - Set `touch-action:none` on the canvas in the web shell.
- **Effort:** small.
- **Worked if:** `runtime_tests [input][mouse]` shows `fire=["mouse:left"]` pressed for one tick, and walker in a window has mouse look with a locked cursor.

### 7. Seeded `Math.random` first, then npm packages by name
- **Serves:** reach (inkjs, rot-js, simplex-noise) and determinism for code agents write (replays, scenarios at many seeds, lockstep).
- **Evidence:**
  - Nothing overrides `Math.random` in `sdk/runtime` or `engine/script`.
  - `random()` is a native call (`session.cpp:622`).
  - `ts.rs:245` refuses bare imports.
  - No sample uses `Math.random`, so seeding it changes no golden.
- **Plan:**
  - **(a) Seeded Math.random, on its own.** Before the bundle is evaluated, run a prelude that replaces `Math.random` with sfc32 or xoshiro128** built on `Math.imul`. That gives the same bits on JavaScriptCore, V8 and the browser. Seed it from the run seed on a stream separate from `random()`, and do the same on the web host.
  - **(b) Packages by name.** For bare imports, `ts.rs` resolves through `node_modules`: package.json `exports` (import or default), then `module`, then `main`.
    - ES modules use the existing path; CommonJS is wrapped with `module`, `exports` and `require`.
    - Node built-ins are refused with a message.
    - `pocket check` sees the packages' types.
- **Effort:** (a) tiny; (b) small to medium (about 200 lines of Rust).
- **Worked if:** the same seed gives the same `Math.random` values in two native runs and in a web pack, with the hello golden unchanged; and a playground copy using simplex-noise and an inkjs two-choice dialogue bundles, type-checks and runs.

### 8. A cheaper per-tick world hash with the same digest, hashed less often in served sessions
- **Serves:** performance; per-tick CPU that grows with entity count.
- **The bound:** swarm in release took about 11 ms per frame, with 8.4 ms in scripts and 0.35 ms rendering (`docs/evidence/swarm.md:15,37`). Everything else, the hash included, is therefore at most about 2.3 ms at 3000 entities.
- **Evidence:**
  - `runtime.hpp:35` sets `hash_every_tick = true`.
  - `session.cpp:765-774` hashes `s.dump()`, `world_->hash()` and the particles every tick.
  - `World::hash` builds an order map, walks the tree twice and calls `has()` for all 40 component ops per entity (`world.cpp:880-908`).
  - MCP `runtime_start` does not pass `--no-tick-hash`; scenarios do (`commands.rs:585`).
- **Plan:**
  1. Measure `perf` `state.avg_ms` on swarm in release, with and without `--no-tick-hash`.
  2. Cache, per flecs table, which ops it carries, and cache the tree-order map under a structural version. That gives the same digest, so no golden changes.
  3. Served sessions (MCP and the editor) use `--tick-hash 30`, the network check's cadence (`session.cpp:815`). `pocket run` keeps hashing every tick.
- **Effort:** small.
- **Worked if:** `hello-golden.json` and the m-series state hashes are unchanged after step 2, and swarm's `perf.state` drops.

### 9. Opaque pipelines without `discard`
- **Serves:** GPU performance. Every opaque draw in the id pass, and in the scene pass until item 11, loses Apple's hidden-surface removal and early depth. My estimate is 0.2-1 ms at 1080p.
- **Evidence:**
  - The discard sits in `renderer.cpp:898` and `:1100` and is compiled into the only opaque camera pipelines (`fs_color` at 5594, `fs_id` at 5716).
  - The camera passes pass `nullptr` as `cut` (8544, 8589).
  - Opaque draws are sorted by texture and mesh strings, not distance (7901-7909).
- **Plan:**
  - Add `fs_id_cut` and `fs_color_cut` entry points that keep the discard; the plain entry points drop it.
  - Create `id_cut`, `id_cut_skinned` and `color_cut` pipelines and pass them as `cut` in both camera passes.
  - Draw cut-outs after the solid meshes, and order the solid runs front to back using `Draw.depth`.
  - After item 11 the scene pass needs no discard at all; the id-pass half of this change still matters.
- **Effort:** small.
- **Worked if:**
  - `tools/scripts/dev/gpu_probe.py showcase --size 1920x1080` shows lower `pocket.ids` and `pocket.scene` times.
  - `renderer_tests [cutout]` passes and `render.compare` shows the same image.
  - A `--web` pack loads (AGENTS.md rule 10).

### 10. Refuse unknown component fields and wrong types, accept enum names, and have `world.set` answer with the value
- **Serves:** agent-first. This closes the last place where a mistake fails silently in spawn, set and instantiate.
- **Evidence:**
  - The generated `from_json` reads only the keys it knows (`gen.rs:282-290`).
  - A value of the wrong type is ignored (`gen.rs:339`).
  - `World::set` merges and succeeds (`world.cpp:418-430`), and `world.set` answers `{"ok":true}` (`session.cpp:4627-4632`).
  - So `Light {kind:"point"}` stays directional (`components.toml:99`). The same integer-code style is used by `RigidBody.kind` (:462), `Joint.kind` (:478), `Collider.shape` and `Sky.mode`.
- **Plan:**
  - `gen.rs` emits each component's and record's field names and kinds.
  - At the command boundary (set, spawn, instantiate), check keys and types recursively. Answer `bad_args` with the nearest field name (the edit distance in `command_help.cpp`).
  - Scene, prefab and Blender loads stay lenient and only warn.
  - Add `enum=[...]` to the integer-code fields in `components.toml`. Accept either the name or the number; show names in the schema and the `.d.ts`; scenes keep storing numbers.
  - `world.set` answers `{ok, value}`.
- **Effort:** medium.
- **Worked if:** `{kind:"point"}` gives 1, `{colour:...}` fails with "did you mean color", `pocket test` stays green, and the 9 command tasks show the same pass rate.

### 11. Shade the scene pass against the prepass depth
- **Serves:** GPU performance; the largest structural waste in the frame. My estimate is 0.5-2 ms at 720p and 1-4 ms at 1080p.
- **Evidence:**
  - The id pass writes Depth32Float `prepass_view` (`renderer.cpp:209`, 8526).
  - The scene pass clears and stores a separate `frame.depth` (8152-8156) and draws with `Less` and depth writes (5597-5600).
  - The sky is drawn first, full screen, with compare `Always` (8577-8582).
  - Water draws a second time into the prepass (4798-4811).
- **Plan (split mode, no MSAA):**
  - Build the scene pipelines with `kPrepassDepth`. Opaque and glass use `Equal` with no depth write; blend, sprite and line pipelines use `LessEqual` with no write.
  - Draw the sky after the opaque meshes, with `LessEqual` at z = 1.
  - Mark the vertex position `@invariant` in the plain, skinned and sway paths, so both passes rasterize the same depth.
  - Attach `ds.view = prepass_view` with `Load`, read-only where nothing writes depth.
  - Draw water as the MSAA path does (8715) and delete the `water.depth` pass.
  - Leave the MSAA path as it is.
- **Effort:** medium.
- **Worked if:**
  - `gpu_probe` on showcase and hills at 1920x1080 shows a lower `pocket.scene` time, and `render.compare` against the old captures is within tolerance.
  - `renderer_tests [taa] [ssr] [cutout] [oit]` and the water tests pass, and a web pack loads.

### 12. Gameplay components declared by the project
- **Serves:** reach in every genre (enemy kinds, loot, teams and tuning values coming from Blender and Tiled) and agent-first (gameplay state visible to `world.query`, the hash, saves, the recorder and the inspector, instead of living in script Maps).
- **Evidence:**
  - AGENTS.md says components are declared only in the engine's `components.toml`.
  - `ops_table()` is a static list with a linear `find_ops` (`world.cpp:47-66`).
  - All 26 uses of `ops_table`/`find_ops` are in `world.cpp`, so the change stays contained.
  - Blender properties that name no component are skipped (`docs/design/assets.md:58`).
  - The samples keep game state in script Maps (`playground/scripts/main.ts:9`, `sprites/scripts/main.tsx:11`).
- **Plan:**
  - An ADR, since this changes an AGENTS.md rule.
  - A project `components.toml` in the engine's schema, read at start and on reload.
  - A registry of component ops per world. Each project component is a flecs runtime component holding a JSON value, with ctor, dtor, copy and move hooks, checked by item 10's validator.
  - Hash the canonical JSON. Save, load, schema and the recorder already iterate the ops, so they pick these up.
  - `pocket gen` and `pocket check` emit a project `.d.ts`. Typed-array pack for these comes later.
- **Effort:** medium (about 400 lines, mostly in `world.cpp`, plus the generator).
- **Worked if:** an `Enemy {hp, speed, kind}` in a playground copy can be set from the scene or a Blender property, queried, saved and reloaded, tracked with `recorder.track`, and edited in the inspector, with a transcript saved under `tests/evidence/components/`.

---

**Next after these, roughly in order**
- **HDR ping-pong:** remove the full-frame copy-backs (`renderer.cpp:5290-5297`, 4947-4956), fold the bloom add into post (3322), and limit full-screen passes to the editor pane.
- **`EventLog::since`:** jump to the sequence instead of scanning from the oldest event (`events.cpp:31-40`; `find()` already indexes at 46-52). `splash_water` calls it every tick (`session.cpp:879`). About 10 lines.
- **Benchmark traces and repeats:** keep traces and add `--repeat` (`pi_agent.py:124-147` keeps only counts). Needed to tell the effects of items 1, 4 and 10 apart from the 2x noise.
- **Contacts:** turn them into JSON only when a script listens (`session.cpp:734-747`).
- **Name index:** index names for `World::find` (`world.cpp:311-331`).
- **Shadows:** Depth16Unorm and per-face frustum culling (6020, 6097, 7066, 8466-8469), then cached faces.
- **Environment rebuild:** quantize the rebuild key (3925).
- **Agent tools:** `project.brief`; `watch`/`until` on `step`; batched calls in the tools (the server already batches, `server.cpp:110-129`).
- **Larger reach work:** 2D sensors and additive sprites; the combat kit; persistent pack views; a crowd grid; Windows and Linux.

---

**Claims in the reports that are wrong or overstated**
1. **CPU: the "37-45x" figures from `docs/evidence/swarm.md` are not debug-against-release ratios.** They are typed-array against JSON speedups (`swarm.md:12-13`; the README's Speedup column, 45.4 and 37.2).
   - The debug-to-release ratios are 25-31x: 404.4/16.026 = 25.2x and 10.88/0.353 = 30.8x in the README; 402/16.3 = 24.7x and 10.7/0.37 = 29x in `swarm.md`.
   - Those ratios were measured on script and world CPU only, so "every tick costs 25-37x" overgeneralizes.
2. **CPU: a hash of "1.5-3 ms at 3000 entities" is too high at the top end.** Swarm in release ran about 11 ms per frame, with 8.4 ms in scripts and 0.35 ms rendering (`swarm.md:15,37`). That leaves at most about 2.3 ms for all other work, startup included, so the hash is likely 1-1.5 ms at most.
3. **Agent: blaming jump_sound's 600.2 s on the missing apply loop is unsupported.** That run had only 16 turns and 21 calls (`omp-deepseek-mcp.json`), which points to a tool call that blocked. Without traces it cannot be attributed.
4. **Agent: "no MCP tool bundles a project" is not quite right.** `runtime_start` bundles (`mcp.rs:184`), and so do scenario and bench. An agent can apply an edit today with `runtime_stop` then `runtime_start` on a session it started, losing the world. The gap is real for attached sessions, such as the benchmark's.
5. **Reach: "first-person is blocked" is overstated.** Clicks already reach scripts as raw `mouse_down`/`mouse_up` events in `onInput` (`platform.cpp:283-290`; `docs/design/input.md:27`). What is missing is binding them to actions (`input_map.cpp:111`) and cursor lock.
6. **Reach: "inkjs is unavailable" is overstated.** Only bare package names are refused (`ts.rs:245`). Relative `.js` files resolve (`ts.rs:247`), so a single-file ES-module build copied into the project imports today. CommonJS-only files do not work.
7. **GPU: the millisecond figures assume an M1/M2-class GPU.** The evidence machine is an Apple M5 (`tests/evidence/swarm/README.md:5`). The ranking should still hold, but each change needs measuring with `gpu_probe.py`.
8. **All reports: the branch is master, not agent-first,** and another session has uncommitted changes in the working tree (item 5).
