# Swarm: per-tick updates of 3000 entities (2026-09-19)

`samples/swarm` spawns 3000 cubes and moves every one of them each tick, alternating between two paths that do the same arithmetic:

- **typed arrays**: `world.pack("Transform", ["position"])`, edit the shared `Float32Array`, `world.unpack()` — two commands per tick;
- **JSON**: `world.get` + `world.set` per entity — 6000 commands per tick.

Measured inside the script with `performance.now()` around the update, averaged over 60 ticks of each path, headless at 320x180. Apple M5, macOS, single run:

| Build | typed arrays (ms/tick) | JSON (ms/tick) | ratio |
|---|---:|---:|---:|
| release (`pocket pack swarm`) | 0.37 | 16.3 | 45x |
| debug with ASan+UBSan (`pocket run swarm`) | 10.7 | 402 | 37x |

The release run of 120 frames (with rendering: 3000 draw calls per frame at 960x540) took 1.34 s wall clock, about 11 ms per frame including the JSON ticks.

What the numbers mean: the JSON path costs about 2.7 microseconds per command in release, which is fine for gameplay code that touches tens of entities and wrong for thousands; the typed-array path makes the per-entity cost a float multiply. Both paths produce the same world (the same `world.set` semantics apply; `unpack` marks components modified so `WorldTransform` and `Bounds` update the same tick).

Reproduce:

```bash
./.pocket/pocket run swarm -- --headless --frames 120 --json      # debug numbers in state
./.pocket/pocket pack swarm && ./dist/swarm/swarm --headless --frames 120 --json
```

`tests/evidence/swarm/swarm.png` is the release capture at 960x540.

## Rendering the swarm

The same 3000 cubes drawn two ways (release build, headless 960x540, Apple M5, `perf` command averages over 300 frames):

| Renderer | Draw calls | Render ms/frame |
|---|---|---|
| One draw per entity, per-object uniform with a dynamic offset | 3000 | 0.90 |
| Instanced: one storage buffer of objects, `instance_index` per row, one draw per (mesh, submesh, material) run | 1 | 0.35 |

The script side of this sample (8.4 ms, half of it the deliberately slow JSON path) is what the frame is made of; the renderer is not the cost at this size. `perf` reports every phase (`frame`, `poll`, `tick`, `script`, `physics`, `world`, `state`, `render`) so the next bottleneck is a number too.

## Hashing the swarm (2026-10-01)

Every tick folds the exposed state and the whole world into the run's tick hash (what replays, `recorder` and lockstep peers compare). At 3002 entities the `state` phase was 1.35 ms a tick (release, 300 headless frames, `timings.state.avg_ms`), against 0.008 ms with `--no-tick-hash`; timing its parts put 1.32 ms in `World::hash` and the rest at a few microseconds. The bytes hashed (about 260 an entity: names, component names, every field) went through FNV-1a a byte at a time, a multiply each, one after another.

| Change | `state` ms/tick |
|---|---:|
| before | 1.35 |
| the world walked once, which components an entity has asked once per flecs table | 1.19 |
| `StateHasher` mixing one step per value (a string: its length, then eight bytes a step) | 0.42 |
| the entity-order map built only when a component refers to an entity | 0.38 to 0.43 |

The digest is new (`hello-golden.json` and the reproducible-math sweep pin were regenerated); what is hashed is unchanged, and the mixing uses 64-bit integer operations only, so native and web builds agree as before.

## The JSON path's answers (2026-10-01)

`world.set` came to answer with the component as it now is (`{ok, value}`), which a script never reads: the swarm's JSON path went from 16 to 26 ms a tick for its 6000 commands. The SDK's `world.set` now passes `quiet: true` (answered `{ok}` alone), and the path is 17.7 ms again (three runs: 17.9, 17.8, 17.5); an agent's `world.set` still gets the value. Looking every command's help up by hash instead of along the table, and its parameter names once instead of every call, changed nothing measurable: the cost is in the JSON each call makes and reads on both sides.

## Numbers without JSON (2026-10-01)

A script's `world.get` and `world.set` of a component whose fields are all numbers (Transform, Velocity, Health, RigidBody2D and the rest `pocket gen` finds; `numericLayouts` in the generated SDK) no longer go through JSON: the engine writes the component's fields into a shared `Float64Array` in the generated order (`read_numbers`), the SDK builds the object from it, and a write reads the component, lays the patch over the numbers and hands them back (`write_numbers`). A patch the numbers cannot hold (a value's name, an array, a field the component lacks, a number that is not finite), a missing component and a `cause` take the command as before, so the rules and the error messages are the command's. Before that, the copies on the command path were cut (the binding no longer copies the parameters, a quiet script `world.set` answers nothing to convert): 17.7 to 16.2 ms.

| Release, Apple M5, `samples/swarm` (3000 `world.get` + `world.set` a tick) | ms a tick, three runs |
|---|---|
| JSON commands (before) | 17.8, 17.9, 17.6 |
| fewer copies | 16.5, 16.2, 16.1 |
| numbers without JSON | 5.80, 5.86, 5.84 |

The typed-array path (`world.pack`) is still 0.35 ms: the per-entity calls are three native calls each now rather than two JSON round trips, about a third of a microsecond each. The world after the run is the same (the full suite's goldens and `tests/ts/world.test.ts`, which compares the two paths' answers, pass).

## Natives that take numbers, readers made per component (2026-10-01)

Timing the pieces inside the runtime (`script.eval`, 30000 calls each, release) put a numeric read at 0.09 µs and a write at 0.13 µs by component index, 0.23 µs by name: the natives were a fifth of the 1.9 µs an entity cost. The rest was the SDK's own JavaScript: a loop over the layout building the object field by field, another walking the patch with `Object.keys` arrays, and a second read before every write. Three changes, each measured on its own (one run each, then three of the last):

| Release, Apple M5, `samples/swarm` (3000 `world.get` + `world.set` a tick) | ms a tick |
|---|---|
| before | 5.9 |
| numbers-only natives (`bind_numbers`: arguments read as doubles, no JSON either way) taking a component index the SDK asks once per name | 5.25 |
| a reader and a patcher made once per component (`new Function` over its fields: one object shape, no loops, keys counted with `for in`) | 3.29 |
| the write as one call: the patch's numbers and a mask of which ones, laid over the component in the engine | 2.64, 2.64, 2.63 |

The script phase of the swarm (both paths, alternating) went from 3.16 to 1.53 ms a tick. An entity given by name or path, a patch the numbers cannot hold and a component of more than 31 numbers take the earlier paths. The web build binds the numbers-only natives through its JSON bridge (the same answers, without the gain).
