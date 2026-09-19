# Scripting

Games are TypeScript (`entry` in `project.toml`, usually `scripts/main.ts`) compiled by `pocket` and run inside the engine. The SDK is `sdk/runtime/`; `import ... from "pocket"` gets all of it, `"pocket/test"` the test helpers. Everything below is also a runtime command an agent can call (`docs/mcp.md`), which is the point: a script and an agent see the same world through the same verbs.

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

Gameplay reads only `t.dt` and never the wall clock, which is what makes runs reproducible and replays exact.

## World

`world.spawn(name, { parent, components })`, `world.get(entity, "Transform")`, `world.set(entity, "Transform", { position: { x: 1 } })` (a partial patch), `world.has`, `world.remove`, `world.destroy`, `world.find("/Player")`, `world.children`, `world.reparent(entity, parent, { keepWorld })` (with `keepWorld` the entity stays where it stands), `world.rename`, `world.describe`, `world.query({ with: ["Health"], fields: ["Transform.position"] })`, `world.tree()` (the AI-native text view), `world.summary()`. Prefabs: `world.instantiate("prefabs/enemy.json", { parent, components })` and `world.savePrefab(entity, path)`; scenes: `world.save()`, `world.load(scene)`, `world.loadScene(path)`. Component names and field types come from `sdk/runtime/generated/components.ts` (`docs/design/world-model.md`). For thousands of entities, `world.pack` / `world.unpack` move numbers as typed arrays instead of JSON.

## Events

`events.emit("crate.dropped", { id }, { subject: id, cause })` and `events.recent()`, `events.since(seq)`, `events.why(seq)` (the chain of causes as a story). Events carry causes, so a transcript can say what led to what. `recorder.start(600)` keeps the last 600 ticks; `recorder.track("/Ball", "Transform", "position.y")` is a jump profile, `recorder.first("/Ball", "Transform", "position.y", "<", 0)` the tick it fell through, `recorder.diff({ from, to })` what changed; `render.visible()` lists what the camera sees with coverage and bounds; `render.debug({ colliders: true })` and `debug.line(a, b, { color, ticks })` draw collision shapes and your own markers into captures.

## Input

`input.axis("move_x")`, `input.pressed("jump")`, `input.down`, `input.released`; the map comes from `project.toml` `[input.actions]` or `input.map({...})` (`docs/design/input.md`). `isKeyDown("Space")` reads the raw keyboard.

## Time

```ts
import { tween, timer, wait, ease } from "pocket";

tween.to(door, "Transform", { position: { y: 3 } }, { duration: 0.5, ease: "cubicOut" });
tween.value(0, 1, { duration: 2, onUpdate: (v) => setClearColor(v, v, v) });
timer.after(1.5, () => spawnWave());
timer.every(0.2, () => shoot(), 5);
async function openAndClose() { open(); await wait(2); close(); }
```

Tweens and timers advance with ticks, so they pause with the game, appear in transcripts and replay exactly. `tween.to` interpolates the numeric fields you name (rotations along the shortest arc) and leaves the rest alone; `repeat`, `yoyo`, `delay` and `cancel()` / `finish()` are there. `setTimeout` and `setInterval` exist and count simulation milliseconds.

## Saving

```ts
import { saves, onSave, onLoad } from "pocket";

onSave(() => ({ score, wave }));                 // script-only values; the world saves itself
onLoad((d) => { score = d.score as number; wave = d.wave as number; });
saves.write("slot-1", { label: "Forest, wave 3" });
saves.list();                                   // [{ slot, modified, tick, entities, data }]
saves.load("slot-1");
```

A slot is one JSON file (the scene plus the `onSave` objects per script context) in the user's data directory for the project (`saves.dir()`; `--save-dir` or `POCKET_SAVE_DIR` override it, tests point it at a temp folder). Loading replaces the world and calls `onLoad`; the run's tick counter keeps counting. `save.written` and `save.loaded` appear in the event log.

## Sprites

`sprites.defineClip(name, { texture, columns, rows, frames | first + count, fps, loop })`, `sprites.play(entity, name, { speed, loop, fps, restart })`, `sprites.stop(entity, reset)`, `sprites.clips()`; the engine plays clips through the `SpriteAnimation` component every tick (`docs/design/sprites.md`).

## Tile maps

`tilemap.info(entity)`, `tilemap.cell(entity, x, y)`, `tilemap.tile(entity, at, layer?)`, `tilemap.solid(entity, at)`, `tilemap.objects(entity, layer?)` for Tiled maps drawn by the `TileMap` component; `tilemap.set(entity, at, tile, layer?)`, `tilemap.fill(entity, rect, tile, layer?)` and `tilemap.save(entity, path?)` edit the map at runtime and write it back (`docs/design/tilemaps.md`). `render.unproject(x, y, plane?, at?)` gives the world ray under a pixel and where it meets an axis plane. `render.compare(path, options?)` holds the last frame against a reference PNG in the project (writing it the first time) and reports how much differs and where (`docs/design/rendering.md`, Comparing frames). `nav.bake(options)`, `nav.bakeTilemap(entity, options)`, `nav.path(from, to)`, `nav.reachable`, `nav.nearest`, `nav.info`, `nav.clear` for walkability grids and A* paths (`docs/design/navigation.md`).

## Animation

`animation.play(entity, clip?, { loop, speed, restart, time, fade })`, `animation.stop(entity, reset)`, `animation.clips(entity | { mesh })`, `animation.pose(entity)` for glTF skins and clips through the `Animator` component (`docs/design/animation.md`).

## Particles

`ParticleEmitter` on an entity does the work (`docs/design/particles.md`); `particles.burst(entity, count)`, `particles.stats()`, `particles.clear()` reach the simulation.

## Physics, audio, interface

`physics.raycast`, `physics.overlap`, `physics.joints`, `onContacts`, the `Joint` component (`docs/design/physics.md`); the `Body2D` component for platformers against tile maps (`docs/design/tilemaps.md`); `audio.play("sounds/hit.wav", { volume, pitch, loop })` and the `AudioSource` component (`docs/design/audio.md`); `mount(() => <Hud />)` with signals and components on Pocket UI (`docs/design/pocket-ui.md`).

## Scenarios

`scenario("walking right collects a coin", (g) => { g.holdWhile("move_x", 1); g.until(() => g.state("score") >= 1, { timeout: 1 }); g.check(() => expect(g.count("coin.collected")).toBe(1)); })` in `<project>/scenarios/*.ts`; `pocket scenario <project> --seeds 20` runs them (`docs/design/scenarios.md`).

## Tests

`tests/ts/*.test.ts` run inside the engine with `pocket test`:

```ts
import { expect, test } from "pocket/test";
test("player starts at the origin", () => { expect(world.get("/Player", "Transform")!.position.x).toBe(0); });
```
