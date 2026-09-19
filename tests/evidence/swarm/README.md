# Evidence: typed arrays against JSON round trips

`samples/swarm` moves 3000 cubes every tick, alternating between the two ways a script can touch that many entities (`docs/design/world-model.md`, Typed arrays): `world.pack` / `world.unpack` (two commands per tick over shared Float32 and Float64 buffers) and per-entity `world.get` / `world.set` (6000 JSON round trips per tick). The sample exposes the average milliseconds of each path; the ratio is the number the design rests on.

Measured on 2026-09-19 on an Apple M5 (32 GB, macOS 27.0), 240 headless frames:

```
./.pocket/pocket run swarm --config release -- --headless --frames 240 --json
./.pocket/pocket run swarm -- --headless --frames 240 --json      # debug: sanitizers on
```

| Build | `pack.ms.avg` (typed arrays) | `json.ms.avg` (6000 round trips) | Speedup |
|---|---|---|---|
| release | 0.353 | 16.026 | 45.4 |
| debug (ASan, UBSan) | 10.88 | 404.4 | 37.2 |

A JSON round trip costs about 2.7 microseconds in the release build (16 ms over 6000), which is fine for the hundreds of entities a script usually touches and wrong for thousands, where the typed arrays cost a tenth of a microsecond per entity. `swarm.png` is the sample's window.
