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
