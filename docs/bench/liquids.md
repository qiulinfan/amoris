# Liquids: method and measurements

Flowing liquids ([liquids.md](../spec/liquids.md)) and the splashes over them
([splashes.md](../spec/splashes.md)). The solver's own measurements, in a glass box, are in the
spike report [fluid-box.md](../spikes/fluid-box.md).

The cost of flow steps (a broken bank, a drained pool) and of the feed with states, which
liquids.md 9 names, is not measured yet. The water region's costs in blockcraft are in section 3.

## 1. Splashes

Machine: Apple M5 (10 GPU cores), macOS, Metal (and MoltenVK 1.4.2 for one row); release build,
2026-10-18. The machine was not checked for other load; one run per row.

Method: `cargo run --release -p amoris-app --example splashes -- --shots DIR --blocks
../amoris-samples/blockcraft --breach N [--off]`. The game runs for real: a lake on a 6-block cliff,
its rim broken 2N + 1 cells wide at tick 90 by a script, `physics.fluids` pouring it into a pool
below. Each frame steps the game one tick, applies its render feed and captures 1280 x 720 with
MSAA. The time is the whole capture, readback included, median over frames 30 to 89 (before the
break) and 150 to 299 (while it falls). "Particles" is the most particles alive at once, read back
every 30 frames outside the timing.

| Backend | Breach | Splashes | Before the break (ms) | While it falls (ms) | Particles |
|---|---|---|---|---|---|
| Metal | 3 cells | on | 1.42 | 2.38 | 1,697 |
| Metal | 3 cells | off | 1.32 | 1.46 | 0 |
| Metal | 9 cells | on | 1.45 | 2.76 | 7,696 |
| Metal | 9 cells | off | 1.33 | 1.47 | 0 |
| MoltenVK | 9 cells | on | 1.31 | 2.75 | 7,805 |
| MoltenVK | 9 cells | off | 1.27 | 1.32 | 0 |

While water falls, splashes add 0.9 ms a frame at 1.7k particles and 1.3 ms at 7.7k. Most of it
does not depend on the count: the grid passes run over the four regions' 1M cells every substep, the
particle passes over the whole ring of 32,768, and the surface is six screen passes. Before the
break, no region is open and no pass runs. The 0.04 to 0.13 ms difference there is within one run's
noise, and the renderer tests show those frames unchanged.

Not measured yet: a full ring, the RTX 5060 and the 780M, and Chrome.

## 2. Shallow water (the spike)

The height field of [water.md](../spec/water.md), as the spike built it
([shallow-water.md](../spikes/shallow-water.md), which has the M5's figures and method). Method:
`python tools/shallow_check.py` (defaults: solver sizes 128 to 1024, drawing 128 to 512), which
builds what it runs, compares each hash chain with the M5's and prints the medians below; raw lines
and traces stay in `out/shallow-check/`. The solver is the bench scene's dam break, single-threaded;
the drawing is `shallow_draw` at 1280 x 720 with MSAA, traced with the field and without it.

Machine: R1, AMD Ryzen 9 270 (x86-64), Windows 11, Rust 1.98.1, Node 25.8.1, Chrome 155, the RTX
5060 on Direct3D 12 for the drawing; commit `4dea38a0`, 2026-10-18. The machine was not checked for
other load; one run.

Hash chains: the M5's, for both schemes natively in release and debug, under Node and in Chrome (8
compared, none different).

Solver, ms a substep (ns a column):

| Scheme | Columns | Native | Node | Chrome |
|---|---|---|---|---|
| Advect | 128² | 0.43 (26.2) | 0.48 (29.3) | 0.49 (29.9) |
| Advect | 256² | 1.45 (22.1) | 1.74 (26.5) | 1.78 (27.2) |
| Advect | 512² | 5.16 (19.7) | 6.13 (23.4) | 6.19 (23.6) |
| Advect | 1024² | 19.4 (18.5) | 25.3 (24.1) | 25.7 (24.5) |
| Pipes | 128² | 0.10 (6.3) | 0.18 (10.9) | 0.18 (11.0) |
| Pipes | 256² | 0.37 (5.6) | 0.57 (8.7) | 0.56 (8.5) |
| Pipes | 512² | 1.63 (6.2) | 2.36 (9.0) | 2.36 (9.0) |
| Pipes | 1024² | 6.23 (5.9) | 9.75 (9.3) | 9.90 (9.4) |

Drawing, added GPU time (medians of 240 frames after 60):

| Columns | GPU busy | GPU frame span | Opaque pass (`opaque+sky`) |
|---|---|---|---|
| 128² | +0.20 ms | +0.21 ms | +0.21 ms |
| 256² | +0.33 ms | +0.33 ms | +0.33 ms |
| 512² | +0.59 ms | +0.58 ms | +0.58 ms |

Natively R1 is 1.3 to 1.9 times slower than the M5; its RTX 5060 draws the field with 5 to 8 times
less GPU busy time than the M5. Not measured yet: the 780M (`AMORIS_ADAPTER=780m`).

## 3. The water region in blockcraft (slice A)

The engine's water ([water.md](../spec/water.md) 12 and 13) in blockcraft at two columns a block:
one region of 256 x 256 columns of 0.5 m (256 tiles) over the island, two substeps a tick at 60 Hz,
`dry_below` 0.002. The scenario is amoris-app's `blockcraft_water/breach.rs`: a copy of the project
whose script digs a pit 8 x 6 blocks and 6 below the sea beside the shore at tick 2 and breaks the
bank 4 blocks wide at tick 120. The water floods the pit through the breach at about 5 m/s and fills
it to the sea's level in about 15 s (282 m³).

Machine: Apple M5 (10 GPU cores), macOS 27.0.1, Metal, release build, 2026-10-18. The machine was
not checked for other load; one run per table.

CPU, back to back (`cargo run --release -p amoris-runtime --features transpile --example
water_bench -- --ticks 10920 ../amoris-samples/blockcraft`). After each tick: `physics.water`
inside the tick (a step hook), the snapshot through `SnapshotCache` with the size of the
`WaterRegion` section, and the feed's extraction for one subscriber with its frame's encoded size.
Medians over the windows of ticks:

| Window (ticks) | Tiles stepped | Solver (ms) | Snapshot (ms) | `WaterRegion` section | Feed (ms) | Frame |
|---|---|---|---|---|---|---|
| Every tile awake: the field liquids just converted (2 to 60) | 256 | 2.19 | 1.00 | 2.0 MiB | 0.24 | 1.0 MiB |
| Asleep, after the dig (70 to 120) | 0 | 0.002 | 0.006 | 2.0 MiB | 0.001 | 1 KiB |
| The flood (121 to 720) | 56 | 0.65 | 1.03 | 2.0 MiB | 0.06 | 225 KiB |
| After it (721 to 1920) | 165 | 1.83 | 1.03 | 2.0 MiB | 0.15 | 662 KiB |
| Later (1921 to 5520) | 165 | 1.92 | 1.02 | 2.0 MiB | 0.16 | 662 KiB |
| Latest (5521 to 10920) | 165 | 1.91 | 1.02 | 2.0 MiB | 0.16 | 662 KiB |

- Every tile awake, the solver takes 1.09 ms a substep, 16.7 ns a column (the spike's flat grid:
  12.5 ns at 256²); the rest is the window's gather and write-back, drying and the sleep test.
- The snapshot encodes the whole section whenever any tile changed: about 1 ms a tick natively
  whatever the number of awake tiles, the same with 56 tiles as with 256 (water.md 15, question 5).
- **The disturbed sea does not go back to sleep.** The flood draws 282 m³ from the sea inside the
  region, a level drop of a few centimetres that rings between its walls. The sea there is up to
  12 m deep, and the bed friction damps a column's velocity at `friction / depth` per second, 0.002
  for 12 m: a time constant of about 10 minutes. 165 tiles stayed above the sleep test's 1e-4 m/s
  for the 3 minutes measured, at 1.9 ms of solver and 1 ms of snapshot a tick. Shallow water settles
  faster but slowly too: in a unit test, a 4 cm film over 24 m relaxed with a time constant of about
  55 s, and a 2 m dam break was still moving at 4e-4 m/s after 380 s.
- Not measured: the browser worker (WebAssembly) and R1.

Drawing (`cargo run --release -p amoris-app --example blockcraft_water -- --project
../amoris-samples/blockcraft --frames 1030 --bench`). Three renderers fed the same frames, one
without the water, one drawing it Smooth and one Blocky, render in turn at 1280 x 720 with MSAA from
the far camera (high over the sea, the shore and the pit filling about two thirds of the view).
With no transparent instance in view the water is drawn at the end of the opaque pass, so the
table gives that pass (`opaque early` and `opaque+sky`) and the GPU busy time, medians of 240
frames after 60:

| Ticks after the break | Opaque, no water | Opaque, Smooth | Opaque, Blocky | Busy, no water | Busy, Smooth | Busy, Blocky |
|---|---|---|---|---|---|---|
| -10 | 1.35 | 1.78 | 1.49 | 3.83 | 5.06 | 5.10 |
| +15 | 1.28 | 1.83 | 1.70 | 3.90 | 4.08 | 4.16 |
| +60 | 1.24 | 1.89 | 1.74 | 4.48 | 4.19 | 4.24 |
| +240 | 1.28 | 1.84 | 1.72 | 3.95 | 4.20 | 5.17 |
| +900 | 1.29 | 1.86 | 1.69 | 4.82 | 4.38 | 4.47 |

The water adds about 0.55 ms to the opaque pass drawn Smooth and 0.4 ms drawn Blocky. The busy time
varies by more than that from frame window to frame window (the shadow pass alone moves by 1 to 2
ms), so it does not resolve the water's cost here. The feed's tiles upload is not timed apart.
