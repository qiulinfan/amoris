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
it to the sea's level (280 m³): 99% of it 15.7 s after the break (14.7 s before calm damping,
water.md 4).

Machine: Apple M5 (10 GPU cores), macOS 27.0.1, Metal, release build, 2026-10-18. The machine was
not checked for other load; one run per table.

CPU, back to back (`cargo run --release -p amoris-runtime --features transpile --example
water_bench -- --ticks 9000 ../amoris-samples/blockcraft`). After each tick: `physics.water` inside
the tick (a step hook), the snapshot through `SnapshotCache` with the size of the `WaterRegion`
section, and the feed's extraction for one subscriber with its frame's encoded size. Medians over
the windows of ticks:

| Window (ticks) | Tiles stepped | Solver (ms) | Snapshot (ms) | `WaterRegion` section | Feed (ms) | Frame |
|---|---|---|---|---|---|---|
| Every tile awake: the field liquids just converted (2 to 60) | 256 | 2.24 | 1.06 | 2.0 MiB | 0.24 | 1.0 MiB |
| Asleep, after the dig (70 to 120) | 0 | 0.002 | 0.006 | 2.0 MiB | 0.001 | 1 KiB |
| The flood, 0 to 10 s after the break (121 to 720) | 56 | 0.64 | 1.02 | 2.0 MiB | 0.05 | 225 KiB |
| 10 to 30 s after it (721 to 1920) | 164 | 1.82 | 1.01 | 2.0 MiB | 0.15 | 658 KiB |
| 30 to 90 s after it (1921 to 5520) | 165 | 2.02 | 1.00 | 2.0 MiB | 0.15 | 662 KiB |
| 90 to 148 s after it (5521 to 9000) | 0 | 0.002 | 0.006 | 2.0 MiB | 0.001 | 1 KiB |

- Every tile awake, the solver takes 1.12 ms a substep, 17 ns a column (the spike's flat grid:
  12.5 ns at 256²); the rest is the window's gather and write-back, drying and the sleep test.
- The snapshot encodes the whole section whenever any tile changed: about 1 ms a tick natively
  whatever the number of awake tiles, the same with 56 tiles as with 256 (water.md 15, question 5).
- **Sleep after the break.** The flood's pressure wave wakes every tile that holds sea: 65 tiles
  are awake 10 s after the break, 130 at 30 s and 130 at 60 s, and every tile sleeps again 112 s
  after it. Most of that time is the sea re-levelling around the island through a channel 1.8 m deep
  between the island and the region's wall, about 14 s per e-fold.
- Before calm damping (water.md 4; still test 1e-6 m and 1e-4 m/s) the disturbed sea did not sleep
  again: 165 tiles stayed awake for the 3 minutes measured, at 1.9 ms of solver and 1 ms of snapshot
  a tick. Bed friction damps 12 m of water at 0.002 a second, a time constant of about 10 minutes.
  Variants measured while choosing the rule (all with the 99% fill within 15 to 19 s): calm damping
  with no cap above 0.5 m/s, or proportional to depth, over-damped the re-levelling and slept later
  or not within 2 minutes; a region's awake tiles sleeping only together slept in 75 to 92 s but
  would keep every tile a continuous source disturbed awake; a still test of 1 cm/s slept in 66 s
  but can freeze tilts of about 2 cm. The owner chose the per-tile rule.
- Not measured: the browser worker (WebAssembly) and R1.

Drawing (`cargo run --release -p amoris-app --example blockcraft_water -- --project
../amoris-samples/blockcraft --frames 1030 --bench`, at `3e25f5b4`, before calm damping, which
does not change the drawing's work). Three renderers fed the same frames, one
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

## 4. Full-readback GPU benchmark (2026-10-19)

The native GPU execution slice ([water.md](../spec/water.md) 17). Apple M5, 10 GPU cores,
macOS 27.0.1, Metal, release, local working tree. No concurrent builds or test processes during
these three timing runs; the machine was not otherwise isolated (load average about 13 after
compilation). These are local measurements, not a comparison with another engine.

Method:

```sh
cargo build --release -p amoris-app --example gpu_water
target/release/examples/gpu_water --ticks 40
```

The harness builds the same game twice with `solver` set to `cpu` or `gpu`. A region of 1 m
columns starts with 2 m depth on its left half and dry columns on the right, with the sample's
additional pour at tick 2. Every tile is awake throughout the measured window (ticks 11 to 50,
less than the 60 ticks needed to sleep). Two substeps per tick. The paths alternate execution
order, with 10 warmup ticks and 40 measured ticks per run; compilation, first device/pipeline
creation, snapshots, feed extraction and drawing are outside the timer. The `physics.water`
hook includes gathering, GPU packing/upload/submission/wait/readback/unpacking where selected,
drying and sleep bookkeeping.

The resident measurement is separate: the same grid size and two-substep kernel, initialized as a
2 m dam, uploaded once and stepped without per-tick CPU water-state transfer. It sums the six
GPU pass timestamps, with the existing profiler's deferred resolve on Metal. It excludes queue
submission, CPU bookkeeping and inter-pass gaps; it is not the full tick's cost.

Each cell below is the median of the three runs' medians, with the range of those medians in
parentheses. Times are milliseconds.

| Columns | CPU `physics.water` | GPU `physics.water` including transfers | Resident GPU passes |
|---|---|---|---|
| 128² | 0.612 (0.610 to 0.621) | 0.558 (0.544 to 0.564) | 0.030 (0.026 to 0.036) |
| 256² | 2.520 (2.513 to 2.578) | 1.428 (1.402 to 1.543) | 0.073 (0.073 to 0.073) |
| 512² | 12.087 (12.026 to 12.596) | 7.759 (5.890 to 7.995) | 0.217 (0.215 to 0.226) |

The complete path improves by about 1.8x at 256² and 1.6x at 512²; 128² gains little. The 512²
range is wide. Most of the GPU path's time is outside the kernels: this slice still uploads and
reads every column in the active window each tick, then writes the f64 components back. Resident
state, sparse queries, asynchronous game-worker integration and direct GPU drawing are covered
separately in section 5; the small kernel number alone does not establish their performance.

Correctness checks on this machine:

- Metal, MoltenVK, and the Metal device restricted to WebGPU baseline features/limits: still lake
  over an uneven bed, dam transport, nonnegative depths, conservation within f32 tolerance,
  inactive boundaries and repeating a restored GPU field.
- Actual CPU/GPU games, after 60 ticks: maximum water-depth difference 9.3e-7 m; GPU total volume
  1023.999975 m³ versus 1024 m³ initially. Two-axis advection over uneven beds at world height
  1,000,000 m also agrees within 2e-4 m or m/s, leaving persisted bed values unchanged.
- Same-device game forks and restores continue with equal hashes. A host without the GPU service
  stops with `water.compute_missing` and poisons the tick, rather than changing solvers.
- `amoris check samples/water-dam`: types, same/cross-process determinism, fork, replay and reload
  pass on Metal. Replay perturbation localization is not checked by this sample because it records
  no external writes. The check crate's negative-control tests still reject unrecorded writes.
- `amoris-render` and the game/check crates build for wasm32. This verifies the compute core's
  build, not browser worker execution; no browser GPU timing or cross-vendor comparison was run.

Raw CSV files and logs stay in ignored `out/water-gpu/`. R1, Direct3D 12 and browser runtime
measurements remain unmeasured. GPU mode does not promise cross-device bitwise replay.

## 5. Resident execution and interaction checks

The published resident path is specified in water.md 19--45. Native and browser gameplay share
complete checkpoints, bounded replay programs, GPU field digests and current-tick sparse queries.
Full field transfers occur for explicit checkpoints rather than every gameplay observation.
The renderer reconstructs its own visual replica; its solve submissions are outside per-frame
GPU pass totals, so those totals must not be reported as the whole game's GPU solving cost.

Methods:

```sh
cargo test -p amoris-render --test water_compute --test water_resident --test water_summary
cargo test -p amoris-render --test water_samples --test water_reactions --test water_boundaries
cargo test -p amoris-physics --test field_buoyancy --test field_hull --test resident_swimming
cargo test -p amoris-app --test gpu_water --test gpu_floaters --test gpu_feedback --test gpu_swimming
cargo test -p amoris-runtime --features transpile --test water_resident_async --test water_feedback
cargo run --release -p amoris-app --example resident_water -- --frames 180
```

Run native tests on each available backend and with WebGPU baseline features/limits, using
AMORIS_GPU_MINIMAL=features,limits,timestamps. Apple M5 Metal and baseline MoltenVK were verified
locally. GPU determinism is scoped to the same execution environment; no cross-vendor replay
claim follows from these checks. Performance remains a measured result, never a pass condition.

The actual-browser checks use rebuilt paired game/viewport modules and tools/water_worker_check.mjs,
water_resident_worker_check.mjs, water_probe_script_check.mjs and water_floater_check.mjs.
The exported swimming validation game uses tools/water_pool_check.mjs: actual keyboard/mouse input,
entry, underwater marker, resurfacing, dry return and explicit retry. Its instrumentation sums
active step + external GPU wait + publication at the same retained tick index. Frame-delivery
wall intervals and viewport rolling means are separate quantities, neither isolated GPU time.

The pool's sleeping-column data rebind was checked against a fresh-host CPU control: 80 actor
states remained identical while requests dropped from 80 to two. A fixed-input native comparison
retained all 765 actor states and complete world hashes byte for byte. One historical M5 Chrome
route measured complete-tick p50/p95 7.2/11.3 ms before rebind and 0.6/5.1 ms after; its real-time
routes were not paired trajectories and do not establish a portable speedup. Captures retain
start-up outliers. Raw evidence stays in ignored local output storage.

## 6. Scope of the retained foundation

The native/browser integration covers bounded advection and nonnegative depth transport, complete
MAC outside faces, actual inlet/outlet receipts, characteristic/Sea boundary controls, checked
fixed-collider beds, exact checkpoint reconstruction, cancellation, field ownership and dry masks,
shared observation budgets, and force/reaction delivery before body integration. Existing tests
use independent f64 flux graphs, analytic fields and force expectations rather than self-comparison
alone. Resource reuse cannot skip individual historical steps or bypass digest verification.

Field rendering retains mesh bounds/culling/LOD/morph, scene-aware refraction, camera medium,
surface foam and dilute visual spray checks. These do not establish final beach quality, rolling
breakers or dense waterfall physics. Floaters, Hull and Walker contact remain one-way except for
explicit queued horizontal reactions. Occupied volume, vertical impacts, added mass and global
fluid/body energy conservation are not implemented by this delivery. Unadopted 3D fluid/rigid
pressure experiments and the unfinished surfing game are excluded.
