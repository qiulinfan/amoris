# Scripting

Games are TypeScript (`entry` in `project.toml`, usually `scripts/main.ts`) compiled by `pocket` and
run inside the engine. The SDK is `sdk/runtime/`; `import ... from "pocket"` gets all of it,
`"pocket/test"` the test helpers. `docs/generated/sdk.md` lists every export on one line (its
signature and what it does, interfaces with their fields), generated from the sources; a running
game answers the same for one name with `help {"sdk": "timer.after"}`. Everything below is also a
runtime command an agent can call (`docs/mcp.md`), which is the point: a script and an agent see the
same world through the same verbs.

## Lifecycle

```ts
import { onStart, onTick, onFrame, onInput, onStop, expose, log } from "pocket";

onStart(() => log("ready"));
onTick((t) => { /* fixed step: t.dt seconds, t.tick, t.time, t.actions */ });
onFrame((f) => { /* every rendered frame, paused or not: interfaces and tools */ });
onInput((events) => { /* raw keyboard, mouse and gamepad events */ });
onStop(() => {});
expose("score", () => score);   // observable state: hashed, reported, in transcripts
```

Gameplay reads only `t.dt` and never the wall clock, which is what makes runs reproducible and
replays exact. The tick is 60 a second unless the runtime is started with `--tick-rate`
(`runtime().tickRate` says which), so `t.dt` is 1/60 and something that happens every 0.15 seconds
happens every 9 ticks; a paused runtime advances only when an agent or a test calls `step {ticks}`.

## World

`world.spawn(name, { parent, components })`, `world.lint()` (what is likely wrong with the world,
with fixes: `docs/design/world-model.md`), `world.instantiateMesh(path, { position })` (a glTF
file's nodes as entities, `docs/design/assets.md`), `world.get(entity, "Transform")`,
`world.set(entity, "Transform", { position: { x: 1 } })` (a partial patch), `world.has`,
`world.remove`, `world.destroy`, `world.find("/Player")`, `world.children`,
`world.reparent(entity, parent, { keepWorld })` (with `keepWorld` the entity stays where it stands),
`world.rename`, `world.describe`,
`world.query({ with: ["Health"], fields: ["Transform.position"] })`, `world.tree()` (the AI-native
text view), `world.summary()`. Prefabs:
`world.instantiate("prefabs/enemy.json", { parent, components })` and
`world.savePrefab(entity, path)`; scenes: `world.save()`, `world.load(scene)`,
`world.loadScene(path)`. Component names and field types come from
`sdk/runtime/generated/components.ts` (`docs/design/world-model.md`). For thousands of entities,
`world.pack` / `world.unpack` move numbers as typed arrays instead of JSON.

## Randomness and noise

`random()` is the run's stream: seeded by the run, so a replay, a scenario at many seeds and a
lockstep game draw the same numbers. `rng` has the helpers over it: `rng.int(1, 6)` (both ends
included), `rng.range(lo, hi)`, `rng.pick(items)`, `rng.shuffle(deck)` (in place),
`rng.chance(0.25)` and `rng.weighted({ common: 8, rare: 1 })`. `new Rng(seed)` is a stream of its
own with the same helpers (sfc32 on 32-bit integers, the same numbers on every engine), for
something made from a seed of its own, a level from its number, without moving the game's stream.
`noise.perlin2(x, y)` and `perlin3` are gradient noise (Perlin's improved noise) in about -1..1,
smooth, 0 on whole numbers; `noise.fbm2(x, y, { octaves, lacunarity, gain })` adds octaves (hills,
clouds, caves); `new Noise(seed)` is noise over another permutation. `tests/ts/rng.test.ts`: a
seeded stream repeats, helpers stay in bounds, a shuffle keeps every item, noise is smooth and the
same for a seed. Tests and scenarios have `expect(x).not.toBe(y)` (`not` before any matcher),
`toBeGreaterThanOrEqual`, `toBeLessThanOrEqual`, `toBeFalsy` and `toBeDefined` beside the others.

## Events

`events.emit("crate.dropped", { id }, { subject: id, cause })` and `events.recent()`,
`events.since(seq)`, `events.why(seq)` (the chain of causes as a story). Events carry causes, so a
transcript can say what led to what. `recorder.start(600)` keeps the last 600 ticks;
`recorder.track("/Ball", "Transform", "position.y")` is a jump profile,
`recorder.first("/Ball", "Transform", "position.y", "<", 0)` the tick it fell through,
`recorder.diff({ from, to })` what changed; `render.visible()` lists what the camera sees with
coverage and bounds; `render.debug({ colliders: true })` and `debug.line(a, b, { color, ticks })`
draw collision shapes and your own markers into captures.

## Input

`input.axis("move_x")`, `input.pressed("jump")`, `input.down`, `input.released`; the map comes from
`project.toml` `[input.actions]` or `input.map({...})` (`docs/design/input.md`).
`isKeyDown("Space")` reads the raw keyboard, `isKeyPressed("Space")` whether a key went down since
the last tick (once per press, a tap released between two ticks included).

## Players

In a lockstep network game (`docs/design/networking.md`) every player has their own action states:
`input.axis("move_x", player)`, `input.down(name, player)`, `input.pressed(name, player)`; `onInput`
events carry `player`; `net.info()` says which player this peer is (for its camera and interface,
never the game's logic). Without a network game only player 0 has input.

## Time

```ts
import { tween, timer, time, wait, ease } from "pocket";

tween.to(door, "Transform", { position: { y: 3 } }, { duration: 0.5, ease: "cubicOut" });
tween.value(0, 1, { duration: 2, onUpdate: (v) => setClearColor(v, v, v) });
timer.after(1.5, () => spawnWave());
timer.every(0.2, () => shoot(), 5);
time.scale(0.2, 1.5);   // slow motion for a second and a half of real time, then normal speed
async function openAndClose() { open(); await wait(2); close(); }
```

Tweens and timers advance with ticks, so they pause with the game, appear in transcripts and replay
exactly. A timer counts the ticks after the one that set it and fires during the n-th, n being its
seconds times the tick rate rounded up: 3 s at 60 ticks a second is the 180th. `tween.to`
interpolates the numeric fields you name (rotations along the shortest arc) and leaves the rest
alone; `repeat`, `yoyo`, `delay` and `cancel()` / `finish()` are there. `setTimeout` and
`setInterval` exist and count simulation milliseconds.

## Saving

```ts
import { saves, onSave, onLoad } from "pocket";

onSave(() => ({ score, wave }));                 // script-only values; the world saves itself
onLoad((d) => { score = d.score as number; wave = d.wave as number; });
saves.write("slot-1", { label: "Forest, wave 3" });
saves.list();                                   // [{ slot, modified, tick, entities, data }]
saves.load("slot-1");
```

A slot is one JSON file (the scene plus the `onSave` objects per script context) in the user's data
directory for the project (`saves.dir()`; `--save-dir` or `POCKET_SAVE_DIR` override it, tests point
it at a temp folder). Loading replaces the world and calls `onLoad`; the run's tick counter keeps
counting. `save.written` and `save.loaded` appear in the event log.

## Sprites

`sprites.defineClip(name, { texture, columns, rows, frames | first + count, fps, loop })`,
`sprites.play(entity, name, { speed, loop, fps, restart })`, `sprites.stop(entity, reset)`,
`sprites.clips()`; the engine plays clips through the `SpriteAnimation` component every tick
(`docs/design/sprites.md`).

## Tile maps

`tilemap.info(entity)`, `tilemap.cell(entity, x, y)`, `tilemap.tile(entity, at, layer?)`,
`tilemap.solid(entity, at)`, `tilemap.objects(entity, layer?)` for Tiled maps drawn by the `TileMap`
component; `tilemap.set(entity, at, tile, layer?)`, `tilemap.fill(entity, rect, tile, layer?)` and
`tilemap.save(entity, path?)` edit the map at runtime and write it back (`docs/design/tilemaps.md`).
`render.unproject(x, y, plane?, at?)` gives the world ray under a pixel and where it meets an axis
plane. `render.compare(path, options?)` holds the last frame against a reference PNG in the project
(writing it the first time) and reports how much differs and where (`docs/design/rendering.md`,
Comparing frames). `nav.bake(options)`, `nav.bakeTilemap(entity, options)`, `nav.path(from, to)`,
`nav.reachable`, `nav.nearest`, `nav.info`, `nav.clear` for walkability grids and A* paths
(`docs/design/navigation.md`).

## Animation

`animation.play(entity, clip?, { loop, speed, restart, time, fade })`,
`animation.stop(entity, reset)`, `animation.clips(entity | { mesh })`, `animation.pose(entity)` for
glTF skins and clips through the `Animator` component (`docs/design/animation.md`).

## Particles

`ParticleEmitter` on an entity does the work (`docs/design/particles.md`);
`particles.burst(entity, count, {at?, speed?})`, `particles.stats()`, `particles.clear()` reach the
simulation.

## Physics, audio, interface

`physics.raycast`, `physics.overlap`, `physics.joints`, `onContacts`, the `Joint` component and the
`Character` component for a 3D player (`docs/design/physics.md`); `RigidBody2D`, `Collider2D` and
`Joint2D` with `physics2d.raycast`, `physics2d.overlap` and `physics2d.impulse` for 2D bodies that
turn, stack and hinge (`docs/design/physics2d.md`); `t("menu.resume")`, `i18n.use("zh")` and
`i18n.number(1234.5)` for the game's words and numbers in several languages
(`docs/design/localization.md`); `camera.rig(camera, target, options)` for a camera that follows
(`docs/design/cameras.md`); `timeline.play(entity, path)` for keyed tracks and events written as a
file (`docs/design/timelines.md`); an `AnimationGraph` for a character's clips, driven by
`animation.param(entity, name, value)` and `animation.trigger(entity, name)`
(`docs/design/animation.md`, State machines); the `Terrain` component with `terrain.height(x, z)` to
find the ground, `terrain.sculpt` to reshape it and `terrain.paintPath(points, color)` or
`terrain.paintLayerPath(points, "dirt")` to paint a path on it (in a colour, or with one of its
textured layers; `docs/design/terrain.md`); the `Water` component with `water.height(x, z)` for the
moving surface and `water.under(point)` (`docs/design/water.md`); the `Wind` component and
`wind.at(x, z)` for the air that carries smoke, light bodies and swaying grass
(`docs/design/wind.md`); the `Body2D` component for platformers against tile maps and `TopDown2D`
for top-down movers on maps of any orientation (`docs/design/tilemaps.md`);
`audio.play("sounds/hit.wav", { volume, pitch, loop })` and the `AudioSource` component
(`docs/design/audio.md`); `mount(() => <Hud />)` with signals and components on Pocket UI
(`docs/design/pocket-ui.md`); `dialogue.start("dialogue/guard.dialogue.json")` and
`dialogue.show(conversation)` for conversations written as nodes of lines, choices, variables and
events, and `dialogue.check(path)` to find what is wrong with one before it runs
(`docs/design/dialogue.md`).

## Scenarios

`scenario("walking right collects a coin", (g) => { g.holdWhile("move_x", 1); g.until(() => g.state("score") >= 1, { timeout: 1 }); g.check(() => expect(g.count("coin.collected")).toBe(1)); })`
in `<project>/scenarios/*.ts`; `pocket scenario <project> --seeds 20` runs them
(`docs/design/scenarios.md`).

## Your own components

A game's own data goes on its entities as components it declares, not in script variables: a
`components.toml` beside `project.toml`, in the engine's format (ADR 0007):

```toml
[[component]]
name = "Enemy"
doc = "A chaser: what it does to the player on a hit and where it came from."
fields = [
    { name = "damage", type = "f32", default = 10.0, doc = "Health a hit takes from the player." },
    { name = "kind", type = "i32", default = 0, enum = ["grunt", "brute"], doc = "0 grunt, 1 brute." },
]
```

They are components like the engine's: `world.spawn` and `world.set` write them (`{kind: "brute"}`;
a misspelt field is refused with the nearest one), `world.query({with: ["Enemy"]})` finds them, the
world hash, saves, `recorder.track` and the editor's inspector see them, a scene file or a Blender
custom property (`pocket.Enemy = {"kind": "brute"}`) sets them, and `pocket check` types them
(`world.get(e, "Enemy").damage` is a number). Fields are scalars and vectors (`f32`, `f64`, `i32`,
`u32`, `i64`, `bool`, `string`, `entity`, `vec2`, `vec3`, `vec4`, `quat`, `color`), and an `i32` may
name its values. `project.apply` and `pocket run --watch` pick up an edited `components.toml`.
`samples/playground` keeps its enemies this way; `tests/evidence/components/` is an agent's session
with them.

## Packages

A package from npm is imported by its name, like the SDK: `npm install simplex-noise` in the project
directory, then `import { createNoise2D } from "simplex-noise"`. The bundler finds it in the nearest
`node_modules` (the project's or one above it) and enters it through its package.json the way a
browser bundler does: `exports` under the `import`, `module`, `browser`, `default` and `require`
conditions, in the package's order, subpaths and `*` patterns included (`inkjs/full`); without
`exports`, `module`, then `browser`, then `main`. ES modules go the same way as the project's own
files; CommonJS files are wrapped with `module`, `exports` and `require` (what they require by a
string literal is bundled, and an ES module importing one sees `module.exports` as its default and
its properties by name); JSON files are their value. Node's own modules (`fs`, `path`, `crypto`,
...) do not exist in a game: a static import of one is refused with a message, a `require` of one
throws only when it runs (packages often try one and fall back), and one a package's `"browser"` map
turns off (`"crypto": false`) is an empty object. `pocket check` reads the packages' own types or
their `@types/` package. Nothing else is needed at run time: the packages are in the bundle, so a
packed game carries them, web build included.

`Math.random` is seeded from the run's seed (sfc32, the same bits on every JavaScript engine, a
stream of its own beside `random()`), so a package that draws from it replays, runs a scenario at
many seeds and plays in lockstep like the rest of the game. A restart (`project.reload` of the scene
and the scripts, which `project.apply` does after an edit) seeds both streams again, starts the
scripts' clock over (`t.tick` and `t.time` from 0, and the world's time that the wind, the waves and
cloth move by), stops the last run's sounds and lets go of actions held for it, so a game edited and
reloaded starts as a fresh run would: one that fires on `t.tick % 6` fires on the same ticks. The
session's own tick carries on through restarts, so the event log and a recording stay one history;
`state` answers both (`tick`, and `run_tick` for the scripts' after a restart).
`tests/evidence/packages/` has a project using simplex-noise, an ink story compiled at run time with
inkjs and seedrandom, natively and in a browser.

## Types

The bundler strips types without reading them, so a misspelt field or a wrong callback would only
show when the game runs. `pocket check samples/ui` (or `pocket check` for the whole workspace: the
SDK, the editor, `tests/ts` and every sample) reads them with the TypeScript compiler, TypeScript
7's native one, which `pocket` fetches as the `typescript` dependency on first use (about 9 MB,
`.pocket/deps/typescript-7.0.2/`). The options are the ones the engine runs under: strict, the SDK
as `pocket`, JSX through `pocket/jsx-runtime`, ES2023 with the host's globals (`console`,
`performance`, the `setTimeout` family on the simulation clock) and no DOM or Node. It answers each
error with its file, line and column (`--json` for the structured report; the MCP tool
`pocket_check`), and the workspace takes a few hundredths of a second. `pocket test` runs it as the
`types` module, so the SDK and the samples stay free of type errors. Under `pocket run --watch` and
`pocket editor --watch` every rebundle is checked too: the errors go to the runtime as
`script.diagnostics` (the Console logs them, the editor's Script tab lists them under the text area,
an agent reads them with the command).

## Errors and cost

The game runs as one bundle (`build/ts/<name>.js`), but what it says names the files that were
written. `pocket ts` writes `<bundle>.lines.json` beside the bundle, the file and line each of its
lines came from (oxc's source map of each module, its lines kept through the rewriting of imports
and exports), and the runtime rewrites every `<bundle>:<line>:<column>` it reports with it: a script
error's message and stack, `console.*` and `log` output, a `script.eval` error. A file of the
project is named from the project directory (`advance@scripts/main.ts:9`), one of the SDK or a
package by its full path. A bundle without the file (made by hand or by an older tool) shows its own
lines.

A command given `entity: undefined` (a `world.find` or a query that found nothing, an index past the
end of a list) throws at the call with that said, instead of crossing to the engine as a missing
`entity`. A parameter whose JSON type a command cannot use (a list where it reads `{x, y}`, a string
where it reads a number) is answered as `bad_args`, naming the command, rather than ending the
runtime.

`perf` says how long the scripts took a tick; `script.profile` says where:

```json
{"ticks": 30, "script_ms_per_tick": 119.107,
 "handlers": [{"kind": "tick", "context": "project", "name": "steer", "at": "scripts/main.ts:16", "calls": 30, "ms": 3569.862, "max_ms": 126.28, "ms_per_call": 118.995, "share": 1.0}, ...],
 "commands": [{"method": "world.query", "calls": 6030, "ms": 2416.115, "calls_per_tick": 201.0}, ...],
 "component_reads": 0, "component_writes": 0}
```

Each handler (`onTick`, `onFrame`, `onInput`, `onContacts`, `onStart`, `onStop`, and the SDK's own,
such as the timers' and the tweens' ticks) is timed as it runs and named by its function's name and
the line that registered it, and so is each `expose` getter (kind `expose`, named by its key: the
engine reads them all every tick, which `perf` counts under `state`); each command a script calls is
counted and timed (a handler's time includes its commands'); the numbers-only `world.get` and
`world.set` of a component are only counted, being too quick to time one by one. The counts run from
the start or from the last `{"reset": true}`, so an agent resets, steps the stretch it cares about,
and reads it. The example (release, Apple M5) is a script that queried its 200 entities once per
entity each tick: the profile names the handler, the line that registered it, and the 201 queries a
tick it made, two thirds of its time.

## Tests

`tests/ts/*.test.ts` run inside the engine with `pocket test`:

```ts
import { expect, test } from "pocket/test";
test("player starts at the origin", () => { expect(world.get("/Player", "Transform")!.position.x).toBe(0); });
```
