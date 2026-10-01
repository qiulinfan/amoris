# After the first list: reach, agents and the CPU (research, 2026-10-01)

A second read-only study, at 8bc7c75, once the plan of `2026-09-30-next-phase.md` had landed but for its item 11. Three passes, each told to check its claims against the code and to say what already exists: the games the engine cannot make yet, where coding agents still spend their turns (from the traces of the 2026-10-01 benchmark runs), and where a tick's CPU time goes. Nothing here was measured unless it says so; the gains are estimates to be checked with `perf` and `gpu_probe.py` before and after each change. Items are struck through in `docs/status.md` as they land.

## Agents

What the traces showed (twelve oh-my-pi runs with DeepSeek Flash and the pi extension):

- Money is mostly the model's output (about half), then new tool results (about a third); cache reads are under a fifth of the dollars though over nine tenths of the tokens. Tool execution was at most 3% of wall time: the time is the model's turns.
- Engine answers average about 500 bytes. Project files (25%) and docs (22%) are most of what agents read; the SDK's sources come next (12%), read to learn what a function returns.
- Two agents read the harness's checks, since script tasks were copied under the repository: landed in 029628a (copies and docs outside the repository, `peeked` in the runner's report).
- Agents write their own loops of one-tick steps to watch a value (seventeen `eval` calls in one double-jump trace), and a batch of fifty answers was cut mid-JSON by the pi tool's limit (landed in 029628a, with compact MCP answers, `events.last_seq` as `{seq}` and a project guide whose commands work outside `samples/`).

Next, by value for effort:

1. ~~**`step {watch}` and `state {keys}`.**~~ Landed in f93df0b. A step that follows named exposed values or component fields and answers each one's first, last, least and greatest value and the ticks they came at (with the series on request); `state` and `step` that answer only the keys asked for. Replaces the step-by-step loops. M.
2. ~~**The SDK's API as a generated index.**~~ Landed in f93df0b. One line per export (signature, what it returns, the first sentence of its doc comment) in `docs/generated/sdk.md`, and `help {sdk: "timer.after"}`; timer rounding said in `docs/sdk.md`. Agents read `sdk/runtime/*.ts` to learn this. M.
3. **`world.mark` and `world.diff {since}`.** What was spawned, destroyed and changed since a mark, without the recorder's per-tick buffer; useful to a checker and to an agent verifying an edit. M.
4. **Whole games from a brief.** A tier of tasks that starts from an empty project outside the repository, a brief of at most 150 words and a contract (entity names, actions, exposed state, events), checked by a hidden scenario that plays it through the contract with several seeds, a fuzz bot that must cause no script error, the type check and `world.lint`. Candidates: dodge (rocks fall, the player moves, game over on a touch), breakout, snake on a grid, a key and a door, a menu that starts, pauses and restarts. The benchmark has had nothing judged by play. L.

## Reach

Already there, though a quick look could miss it: save slots, path finding on 2D maps of every kind, gamepad rumble and per-player pads, dialogue through inkjs from npm, UI drag events and wrapping rows, hit stop through `time.scale`, additive scene loading, timelines for cutscenes.

Missing, by what it unlocks against its size:

1. ~~**A project's own fonts and richer text**~~ (landed) (`[ui] font`, named fonts, weight, outline and shadow, spans of another colour, a glyph count for typewriter text). Visual novels, RPG dialogue, card games, a pixel-art game's own letters. Today the tool always sets the bundled Noto Sans CJK as the font (`tools/pocket/src/commands.rs`). S to M.
2. **Things made by code**: `mesh.create {positions, indices, uvs}` as a mesh a `MeshRenderer` and a `Collider` can name, `tilemap.create`, and saves that keep a map's and a terrain's edits; line of sight on a tile map. Procedural dungeons, sandboxes, builders. M.
3. **General 2D physics**: rotating boxes, circles and polygons that collide with each other, joints, 2D ray casts; `Body2D` is an axis-aligned box and movers pass through each other. Physics puzzles, pinball, survivors-likes with solid crowds. A vendored Box2D v3 (MIT; deterministic) is the likely route. M to L.
4. **Materials written as shaders**: a WGSL snippet with typed parameters, named by `MeshRenderer` and `Sprite`, and a chain of post effects; lit 2D. Toon shading, outlines, dissolves, hit flashes, CRT looks. Coding agents write shaders well. L.
5. **Several cameras**: a viewport per camera, layers, a camera drawing into a texture the UI can show. Split screen, minimaps, mirrors, a first-person weapon drawn apart. L, or M without TAA, reflections and fog in the second view.
6. **Paths**: a `Path` (points, closed, Catmull-Rom), a `PathFollower`, Tiled polylines as paths. Tracks, lanes, patrols, rails. M.
7. **The window and touch**: fullscreen and size commands and settings; on-screen sticks and buttons that feed actions; a web manifest. S each.
8. **A humanoid animation library**: clips from separate files mapped by joint name (Mixamo), two-parameter blend spaces, a crouching character; ragdolls later. M, ragdolls L.
9. Windows and Linux builds, and play over the internet (relays, rollback, `wss://`): large, and the first cannot be tested on this machine.

## CPU

The script engine is JavaScriptCore natively (ADR 0005); scripts are the largest phase in the swarm (8.4 of about 11 ms a frame), at about 3 µs a JSON command.

1. **A cheaper command path from scripts.** Each call stringifies its params in JavaScriptCore, copies them, parses them, copies them again in the binding, and the answer is dumped, copied and parsed back. Moving instead of copying and answering quiet writes with nothing: an estimated quarter to a third per call. Numeric fast paths for Transform, Velocity and Bounds through a shared `Float64Array`: several times on those calls. M.
2. **Physics queries without a broad phase.** Ray casts, sweeps and overlaps test every body, and colliding particles, wheels, camera rigs and audio occlusion all cast: queries times bodies. A snapshot of the bodies' boxes kept while nothing moves, then a grid. M; ties must keep today's order.
3. **Transforms and bounds only where something changed.** Every tick propagates every transform and rebuilds every bound; flecs's change detection can skip tables nothing wrote. M.
4. **A name index for `World::find`**, which walks the tree per lookup and is called per tick by joints, attachments, camera rigs, 2D bodies on a named map and every string reference from a script. S to M.
5. **Physics bookkeeping and animation allocations**: a map of sleep timers refilled every step, characters gathered with no characters, a dozen allocations per animated entity per tick. S to M.
6. **The recorder** hashes every component of every entity through a fresh `std::function` per component each tick. M.
