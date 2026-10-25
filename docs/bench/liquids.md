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
pressure experiments and the unfinished surfing game are excluded. Amoris Pioneer's frontier work
(section 7) adds occupied volume, ballistic falls, breaking, dispersion and three played samples.

## 7. Frontier water (Pioneer, 2026-10-22)

Amoris Pioneer's `explore/water` and `explore/water-nh` (water.md 46 to 56) on the M5 (Metal),
release builds. Raw logs and captures stay in ignored `out/`.

**The dispersive models' cost.** `cargo test --release -p amoris-render --test water_frontier
time_dispersive_steps -- --ignored --nocapture` times a resident step of 4 substeps in shallow water
alone, with section 49's dispersive split and with section 55's non-hydrostatic pressure, wall time
with its 64-byte readback:

| Region | Shallow water | Dispersive split | Non-hydrostatic |
|---|---|---|---|
| 128² | 1.4 ms | 2.0 ms | 1.6 ms |
| 256² | 1.3 ms | 3.4 ms | 2.9 ms |
| 512 x 256 | 1.7 ms | 5.7 ms | 6.6 ms |
| 512² | 2.7 ms | 11.1 ms | 12.4 ms |

The split's stationary pyramid keeps eight full-resolution levels and its mirrored transforms run
over twice each side, once a step. The non-hydrostatic solve runs 48 Chebyshev iterations in every
substep, in one compute pass (a pass for each dispatch took 13.4 ms at 512 x 256, about 30 µs a
pass); the surf sample (512 x 256) runs 2 substeps, about 3.5 ms a tick.

**Accuracy** (`water_frontier`, checked). The split: standing waves in 2 m of water, 2, 4 and 8 m
long, run within 2 percent of Airy's periods (shallow water alone 1.3 to 2.4 times too fast); a
10 s, 5 cm swell into a 256 m channel 6 m deep keeps 5.25 cm within 1 percent from 30 to 150 m and
runs at 7.57 m/s against Airy's 7.36 (a long wave's 7.67). The non-hydrostatic pressure: standing
waves 8 and 16 m long in 2 m run at 2.3002 and 3.8882 s against the model's 2.2965 and 3.8807 s;
a solitary wave keeps 0.996 of its crest at 1.002 of its speed after 10 s (shallow water 1.045 at
1.097).

**Energy** (checked). A closed basin 8.8 m deep with a hump, non-hydrostatic, keeps 1.0001 of its
energy (the vertical motion's included) after 5 s and 0.92 after 20 s; one Chebyshev cycle of 64
iterations broke it apart within 4 s. Four floats dropped 0.6 m into a closed 32 m lake with no
friction or calm damping, pushing its water aside and dragging it: the floats and the lake together
hold 0.198, 0.179 and 0.161 of the drop's energy over the quarter minutes from 15 to 60 s
(`amoris-app` tests/gpu_energy.rs). Before a float felt its own push in the same step they held a
quarter at 20 s and half at 150 s. The heave of a 250 kg float runs at 1.0014, 1.2263 and 1.4157 s
with added mass 0, 0.5 and 1 against 1.0030, 1.2285 and 1.4185 s (`amoris-physics`
tests/added_mass.rs).

**The surf bay** (`water_frontier`'s ignored `surf_bay_research`, the sample's bay and swell
without bodies): non-hydrostatic, with breaking at 2 substeps, the fastest flow stays between 3 and
7.3 m/s over 300 s and the kinetic energy at 124 to 141 MJ; at 4 substeps over 600 s it follows the
two swell trains' 17-minute beat between 90 and 160 MJ (hydrostatic: 95 to 122 MJ). With the
dispersive split (`SURF_DISPERSION=1`) the bay's energy grows after about two minutes (water.md
49.5).

**Caustics** cost about 1 ms of GPU time a frame for the pool sample's 512² map at 2 x 2 photons
a texel. `AMORIS_CAUSTICS=off` removes the pass.

**The samples** (`cargo test --release -p amoris-app --test gpu_frontier_scenes`, under a minute in
all): the pool party collects all eight ducks and raises a 3 to 4 cm wave at the corner gauges; the
river run passes two gates, its tube drops about 7 m over the falls and lands at 12 to 13 m/s, and
the run reaches the beach in 17 to 19 s with the river's volume ledger exact; the surfer rides 30.5
m at up to 7.0 m/s before the next crest throws them. Exported with `cargo xtask export-web`, each
loads and plays in headless Chrome on the WebGPU baseline device (`tools/web_play_check.mjs`; its
block-placing checks do not apply to these samples) with no validation errors or script exceptions.

## 8. Sub-grid ripples (Pioneer, 2026-10-22)

The ripple layer of [water.md](../spec/water.md) 57 on the M5 (10 GPU cores), macOS, Metal, release
builds, 2026-10-22. The machine was not checked for other load. Raw outputs, traces and captures
stay in ignored `out/ripples/`.

**The step alone.**
`cargo test --release -p amoris-render --test water_ripples time_ripple_steps -- --ignored --nocapture`
steps a window over a 16 m pool on a slow current (0.05 m/s), stirred by 74 duck-sized points
bobbing, as many as the pool party sends. GPU time of the step's compute pass (timestamps), median
over 200 steps:

| Water under the window | Depth levels run | GPU time a step |
|---|---|---|
| 3 m everywhere | 1 | 0.36 ms |
| 3 m, with a strip 5 cm deep | 4 | 0.66 ms |

Timed one stage at a time over deep water: the rows' forward transform with the water sampled under
each cell 0.07 ms, the column pairs' forward and inverse transforms 0.105 ms, the rows' inverse
transforms and blend 0.065 ms, transport and masks 0.09 ms, the stirring points 0.016 ms, the slopes
0.016 ms and the mips 0.019 ms. Each further depth level adds about 0.05 ms to each of the two
turning passes. Fusing the levels into those two passes (one pair of columns a workgroup, the blend
kept in registers) took the four-level step from 0.77 to 0.66 ms; a table of twiddle factors in
workgroup memory gained under 3 percent and was left out.

**In the samples.**
`cargo run --release -p amoris-app --example ripple_shots -- --sample pool-party --only off` (then
`on`) with `AMORIS_TRACE=frames=400,skip=200,out=PATH`, then
`python tools/frame_trace.py diff OFF ON`: the actual game, played through the player's actions (the
pool party's swimmer runs off the deck, jumps in and swims for the ducks while the guests
cannonball; the surf beach's rider lies on the board and paddles), drawn by one renderer from the
player's eye at 1280 x 720 with MSAA. Medians over the traced frames:

| Sample | GPU frame span, off and on | Union of the GPU passes, off and on | `water ripples` pass |
|---|---|---|---|
| pool-party | 5.27, 6.45 ms (+1.18) | 4.79, 5.96 ms (+1.17) | 0.90 ms |
| surf-beach | 8.30, 8.78 ms (+0.48) | 8.28, 8.73 ms (+0.46) | 0.56 ms |

The pool runs three depth levels (its entry steps stand 0.25 m deep under the window) and, with the
ripples on, its refraction and reflection rays and its caustics' photons work through a rougher
surface; the surf beach's window over deep water runs one. The M5 overlaps the ripple step with the
frame's other passes, so the passes' times are no sum of costs: with the ripples on, the opaque
passes measured 0.4 to 0.7 ms longer each. The frame span and the passes' union are what the layer
costs. The feed carries 74 points, 1.6 KB a tick, in the pool party and 17, 375 bytes, in the surf
beach. In the river falls, its rider paddling the tube down the fast river (`--sample river-falls`),
the ripples stay finite and the pass takes 0.80 ms (95th percentile 1.26) among four renderers, 25
points and 551 bytes a tick.

**Captures.** `ripple_shots` without `--only` draws the same frames four ways, from the player's eye
and from beside the player, with the ripples on and off, at every `--every` ticks:
`out/ripples/<sample>/{eye,side}-{on,off}-<tick>.png`.

**In Chrome.** Exported with `cargo xtask export-web` and played with
`node tools/web_play_check.mjs` in headless Chrome 155 on the M5's WebGPU (the check's block-placing
steps do not apply to these samples), pool-party and surf-beach load, take the player's input and
draw at 16.7 ms a frame with no WebGPU validation error or script exception;
`node tools/water_pool_check.mjs` passes all twelve of its checks on samples/water-pool, whose
swimmer stirs the water too. The ripple pass's time in Chrome was not measured apart.

Not measured: R1 with the RTX 5060 and the 780M, Direct3D 12, and the browser's ripple pass alone.

## 9. The waterline and the sun under water (Pioneer, 2026-10-22)

The port and the shafts of [water.md](../spec/water.md) 58 on the M5 (10 GPU cores), macOS, Metal,
release builds, 2026-10-22. The machine was not checked for other load. Raw traces and captures
stay in ignored `out/`.

`cargo run --release -p amoris-render --example water_lab -- --case pool --view under|line
--frames 300 --trace --out DIR` with this build and with the build before it (fcbd821b), then
`python tools/frame_trace.py stats DIR/pool-foam-trace.json`: the lab's analytic pool, its water
scattering 0.01 to 0.04 per metre under a sun 53 degrees up, drawn at 1280 x 720 with MSAA, the eye
0.7 m under the surface (`under`) or at it (`line`). Medians and 95th percentiles over 240 frames:

| View | `water medium integrate`, before | after |
|---|---|---|
| under the surface | 0.335 ms (p95 0.421) | 1.426 ms (p95 1.489) |
| at the waterline | 0.394 ms (p95 0.805) | 1.527 ms (p95 2.030) |

The shafts take the difference. Sixteen samples a pixel took 2.95 ms; taking the surface over a
sample from the caustics map's texel, and the focus from the nearest one, 2.40 ms; interleaving the
sixteen over a 2 x 2 block, four a pixel, 1.43 ms. Above the water, away from the surface, the pass
still dispatches no image work.

## 10. The water's sound (Pioneer, 2026-10-22)

The tally and the synthesizer of [water.md](../spec/water.md) 59 on the M5 (10 GPU cores), macOS,
Metal, release builds, 2026-10-22. The machine was not checked for other load. Raw recordings,
tallies and traces stay in ignored `out/`.

**The tally.**
`cargo run --release -p amoris-app --example water_sound -- --sample surf-beach --ticks 1800` with
`AMORIS_TRACE=frames=300,skip=1500,out=PATH`, again with `--deaf` (the renderer not listening),
then `python tools/frame_trace.py diff DEAF LISTEN`: the surf developed (its waves reach the beach
after about 15 s), the board paddling among the breakers, about 26 000 breaking events a second.
The same for `--sample river-falls --ticks 600` with `skip=300`, the tube in the gorge above the
falls. Medians over 300 frames:

| Sample | `water spray emit`, deaf and listening | `water spray simulate`, deaf and listening |
|---|---|---|
| surf beach | 0.096, 0.094 ms | 0.017, 0.016 ms |
| river falls | 0.008, 0.009 ms | 0.013, 0.013 ms |

Listening costs nothing measurable: a few thousand sources a frame add three atomics each to 18
words. The readback is 128 bytes a frame, through two staging buffers.

**The synthesizer.** `water_sound` sounds each run's tallies offline, 800 samples a tick at 48 kHz,
on one core:

| Sample | Sound | Synthesis | Share of real time | Most bubbles ringing |
|---|---|---|---|---|
| pool party | 20 s | 0.62 s | 3.1% | 526 |
| surf beach | 60 s | 2.41 s | 4.0% | 712 |
| river falls | 20 s | 0.78 s | 3.9% | 587 |

Each ringing bubble costs a multiply-add pair a sample, its pitch updated every 64 samples; the
budget of 1024 at once bounds the synthesizer at about 6% of a core. The recordings' levels and
spectra are in water.md 59.3.

Not measured: R1 (the RTX 5060 and the 780M) and Direct3D 12; the synthesizer on kira's audio
thread in a live window (it was timed offline); the browser, which does not play the water's sound.

## 11. The non-hydrostatic solve in blocks (Pioneer, 2026-10-22)

The temporal blocking of [water.md](../spec/water.md) 55.2 on the M5 (10 GPU cores), macOS, Metal,
release builds, 2026-10-22. The machine was not checked for other load.

**The step.** `cargo test --release -p amoris-render --test water_frontier time_dispersive_steps --
--ignored --nocapture`, two runs, wall time a step of 4 substeps with its 64-byte readback; one
dispatch an iteration is the build before (b3473e6b), run the same day:

| Region | Shallow water | Non-hydrostatic, one dispatch an iteration | Four iterations a dispatch |
|---|---|---|---|
| 128² | 1.0 to 1.7 ms | 1.53 ms | 1.28 to 1.31 ms |
| 256² | 1.20 ms | 2.88 ms | 2.34 to 2.35 ms |
| 512 x 256 | 1.62 ms | 6.56 ms | 3.91 to 3.94 ms |
| 512² | 2.56 to 2.59 ms | 12.32 ms | 6.80 ms |

Over shallow water, the pressure costs 1.15 ms a step at 256² (1.65 before), 2.31 at 512 x 256
(4.85) and 4.22 at 512² (9.62). At 512², with 8, 48 and 96 iterations a substep (`CYCLE` 4, 24 and
48), a step took 3.96, 6.80 and 10.13 ms: 17.5 µs an iteration a substep, and 0.21 ms a substep for
the system, its bounds, the coefficients and the correction.

**Shapes tried**, a step at 512² (every one the same field bit for bit):

| Tile, iterations a dispatch | A step |
|---|---|
| 24 x 24, 4 (kept) | 6.80 to 6.95 ms |
| 26 x 26, 3 | 6.94 ms |
| 28 x 28, 2 | 8.02 to 8.05 ms |
| 16 x 16, 8 | 8.21 to 8.22 ms |
| 16 x 16, 8, a thread's four columns in arrays indexed in a loop | 12.67 ms |

Arrays of a thread's columns indexed in a loop lived in its memory, not its registers, and the
blocks ran no faster than one dispatch an iteration. Sharing the couplings through workgroup memory
too (all 16 KB of it) gained 3 percent at 512² and nothing at the other sizes; a reciprocal in
place of the division gained nothing and broke the bit-for-bit match. Both were left out.

**The surf bay** (`SURF_SUBSTEPS=2 SURF_TICKS=600|1800 cargo test --release -p amoris-render --test
water_frontier surf_bay_research -- --ignored`): the run's time for 1800 ticks less its time for
600, over the 1200 ticks between, is 2.30 ms a tick, against 3.36 ms one dispatch an iteration
(`water_nh::REFERENCE` set for the run).

**The same field.** `the_blocked_non_hydrostatic_solve_matches_one_dispatch_an_iteration` finds no
column differing after 3 s of a deep basin and of a 100 x 36 beach (partial tiles, dry land,
breaking, an open side), on Metal, on MoltenVK and as the WebGPU baseline. Against the build before,
both runs' fields after 600 ticks are the same byte for byte, on Metal and on MoltenVK.

**In Chrome.** With the viewport rebuilt, the surf beach's export, whose worker runs the
non-hydrostatic solve, plays in Chrome 155 on the M5's WebGPU at 16.7 ms a frame with no WebGPU
validation error or script exception (`web_play_check`, whose block-placing steps do not apply),
and `water_pool_check.mjs` passes its twelve checks.

Not measured: R1 (the RTX 5060 and the 780M), Direct3D 12, and the pass alone in the browser.

## 12. The wind sea (Pioneer, 2026-10-23)

The wind sea of [water.md](../spec/water.md) 60 on the M5 (10 GPU cores), macOS, Metal, release
builds, 2026-10-23. The machine was not checked for other load; a set during which other work took
the CPU (a frame's CPU span 2.8 ms against 0.9) was dropped and run again. Raw traces and captures
stay in ignored `out/`.

`AMORIS_TRACE=frames=300,skip=1200,out=PATH cargo run --release -p amoris-app --example ocean_shots -- --out DIR --at 1700 --view mouth`,
then `python tools/frame_trace.py diff CALM WIND`: the surf beach at 1280 x 720 with MSAA, from
inside the bay's mouth looking out to sea, the bay's water in front and the sea to the horizon. Calm
is the same scene with the sea's fetch at 0, the sea then drawn with its wavelets. Two runs of each,
alternated; medians over 300 frames:

| | Calm | Wind |
|---|---|---|
| frame interval | 12.65, 12.67 ms | 13.15, 13.23 ms |
| `ocean waves` | none | 0.297, 0.296 ms (p95 0.37, 0.31) |
| `opaque+sky` (the sea) | 1.35, 1.35 ms | 1.33, 1.33 ms |
| `water+foreground` (the bay) | 3.77, 3.77 ms | 4.06, 4.05 ms |
| `post` | 3.73, 3.72 ms | 4.00, 4.00 ms |
| CPU `uniforms` | 0.282, 0.284 ms | 0.284, 0.286 ms |

The wind sea makes a frame about 0.5 ms longer: the synthesis, 0.30 ms, and the bay's surface
reading the waves, 0.29 ms. The sea's own pixels cost what its wavelets did. `post`'s interval grows
with the frame's work although post-processing does not change: a pass's interval can include
waiting for the work before it.

**The synthesis.** Four dispatches take 0.297 ms. The thirteen it took at first (the spectrum, the
rows, the columns, the texels and a dispatch a mip level) took 0.565 ms; timed in passes of their
own, 0.043 ms for the spectrum, 0.189 for the rows, 0.148 for the columns, 0.085 for the texels and
0.130 for the eight levels. Making the spectrum in the rows' workgroups and the texels in the
columns' saves writing and reading the four fields once each, and the eight levels' dispatches, each
waiting on the last, became two. The spectrum's constants, made every frame at first, took 0.05 ms
of the CPU's `uniforms` span; made once a wind, nothing.

**Anisotropy.** 16 samples against 8 made the frame 0.39 to 0.40 ms longer, `opaque+sky` 0.13 to
0.14 ms and `water+foreground` 0.19 ms, and changed the six views of `ocean_shots` by 0.006 to 0.23
levels on average. 8 is kept.

Not measured: R1 (the RTX 5060 and the 780M), Direct3D 12, Vulkan, and the browser's pass alone.

## 13. The shore (Pioneer, 2026-10-23)

The shore of [water.md](../spec/water.md) 61 on the M5 (10 GPU cores), macOS, Metal, release
builds, 2026-10-23. The machine was not checked for other load. Raw traces and captures stay in
ignored `out/`.

**The kernel's step.** `cargo test --release -p amoris-render --test water_frontier
time_dispersive_steps -- --ignored --nocapture`, two runs, wall time a step of 4 substeps with its
64-byte readback, the field 1.5 m deep and smooth, the face velocities transported by each scheme
(`ResidentWater::set_advection`):

| Region | MacCormack | Momentum-conserving | Hybrid (regions) |
|---|---|---|---|
| 128² | 1.06, 1.55 ms | 1.07, 1.27 ms | 1.08, 1.12 ms |
| 256² | 1.39, 1.54 ms | 1.52, 1.52 ms | 1.28, 1.36 ms |
| 512 x 256 | 1.67, 1.68 ms | 1.89, 1.94 ms | 1.69, 1.70 ms |
| 512² | 2.64, 2.67 ms | 3.11, 3.11 ms | 2.63, 2.63 ms |

In deep, smooth water the hybrid transports as MacCormack does and costs what it did; the
momentum-conserving form everywhere costs 0.47 ms more a step at 512², an upper bound for a region
whose every face is shallow or steep. The smaller regions' runs spread more than the schemes
differ.

**The wet bed.** `AMORIS_TRACE=frames=300,skip=2400,out=PATH cargo run --release -p amoris-app
--example shore_shots -- --out DIR --at 2710`, then `python tools/frame_trace.py diff DRY WET`: the
surf beach at 1280 x 720 with MSAA from the player's camera, its region's `wet_s` 90 against 0, two
runs of each, alternated; medians over 300 frames:

| | Dry | Wet |
|---|---|---|
| frame interval | 18.72, 18.80 ms | 19.01, 18.84 ms |
| `wet bed depth` | none | 0.094, 0.094 ms |
| `water background resolve` (with the blend) | 1.66, 1.66 ms | 1.90, 1.92 ms |

The wet bed costs about 0.35 ms a frame: 0.24 to 0.26 ms for its blend, a full-screen triangle
whose pixels outside the region or untouched by water are discarded, and 0.094 ms for resolving the
scene's depth before it. Drawn first as two blends, a darkening and then the film, the pass took
0.50 ms more. The foam pass keeps the wetness at no measurable cost.

Not measured: R1 (the RTX 5060 and the 780M), Direct3D 12, Vulkan, and the browser's pass alone.

## 14. Vessels (Pioneer, 2026-10-23)

The vessels of [water.md](../spec/water.md) 62 on the M5, macOS, release builds, 2026-10-23. The
machine was not checked for other load.

`cargo test --release -p amoris-physics --test vessel time_vessels -- --ignored --nocapture`, two
runs: a tick of a world with a CPU region of still water, 64 x 64 m, and eight of the pool party's
60 kg dinghies afloat in it, four with a hole of 20 cm^2 filling, against the same world without
them, 600 ticks each:

| | Without | With eight dinghies | A dinghy |
|---|---|---|---|
| a tick | 0.019, 0.008 ms | 0.759, 0.755 ms | 0.093 ms |

A dinghy's hull is 2.2 m long: its triangles are split to 0.275 m, and its lattice is 9 x 9 nodes.
The cost covers its rigid body as well. Built first with triangles a twelfth of the hull's extent,
the water inside levelled by 40 bisections over every triangle and the hull's volume under the
water computed again for coupling the region did not ask for, a dinghy took 0.515 ms a tick.

Not measured: a resident region, whose nodes add to its samples (water.md 47), and R1.

## 15. Plunging lips and falling sheets (Pioneer, 2026-10-23)

The lips of [water.md](../spec/water.md) 63 on the M5, macOS, Metal, release builds, 2026-10-23.
The machine was not checked for other load.

`AMORIS_TRACE=frames=300,skip=2000,out=PATH cargo run --release -p amoris-app --example shore_shots
-- --out DIR --at 2310`, then `python tools/frame_trace.py diff OFF ON`: the surf beach at 1280 x
720 with MSAA from the player's camera at the lineup while the break peels, `plunging` off against
on, two runs of each, alternated; medians over 300 frames:

| | Plunging off | Plunging on |
|---|---|---|
| frame interval | 14.25, 14.22 ms | 14.74, 14.66 ms |
| `water lips` (the finding pass) | none | 0.016, 0.018 ms |
| `water+foreground` | 4.82, 4.85 ms | 4.53, 4.55 ms |

The pass that finds the lips costs under 0.02 ms over the bay's 512 x 256 columns. The ribbons draw
within `water+foreground`, whose time does not separate them from the different sea plunging makes:
its breaking leaves other foam and shallows, and the pass took less time with it. A frame took about
0.5 ms longer.

The river falls (`scene_shots samples/river-falls --eye 46,2.5,8 --at 34.6,3.5,14.7`, 300 frames
from tick 300, facing the falls): `water lips` 0.006 ms, `water+foreground` 5.48 and 5.49 ms, a
frame 14.38 and 14.29 ms.

Not measured: the ribbons' draw alone, R1, and the browser.

## 16. Water at scale (Pioneer, 2026-10-23)

Baking, focus and joined regions ([water.md](../spec/water.md) 64) on the M5, macOS, Metal,
release builds, 2026-10-23. The machine was not checked for other load.

`cargo test --release -p amoris-app --test gpu_scale time_scale -- --ignored --nocapture`, two
runs: the wall time of a tick, its GPU work and readbacks waited for, over 600 ticks:

| | A tick |
|---|---|
| the river (`samples/water-river`, 64 x 32 columns) stepping, from tick 300 | 0.766, 0.772 ms |
| the same river baked | 0.014, 0.014 ms |
| two pools of 32 x 16 columns joined along a side | 1.448, 1.453 ms |
| the two walled apart | 0.692, 0.685 ms |
| one pool of 64 x 16 columns over both | 0.717, 0.696 ms |

A baked region costs its tick no GPU work and no readback. Two regions cost a tick no more than one
region of both while they are closed: one batch submits and reads back both. Their open sides cost
the rest: the same two pools open on an outflow instead of joined took 1.377 ms, so joining them
costs 0.04 ms more than the boundary passes every open side runs each substep.

Not measured: large joined bodies of water, a region resting beyond a focus (as a baked one, it
submits nothing), R1 and the browser.

## 17. Terrain and authored water (Pioneer, 2026-10-23)

The terrain and pours of [water.md](../spec/water.md) 65 on the M5, macOS, release builds,
2026-10-23. The machine was not checked for other load.

`cargo test --release -p amoris-runtime --features transpile --test terrain time_terrain --
--ignored --nocapture`, two runs, each the mean of ten: the valley's terrain, 384 x 192 cells of
half a metre with noise, five hills and a channel of twelve points:

| | Once |
|---|---|
| its heights | 11.01, 8.70 ms |
| its collider (heights and triangle mesh) | 8.91, 8.94 ms |
| a 192 x 192 region's beds and pour (heights included) | 8.84, 8.84 ms |

None of it runs on a tick: the collider is made when the terrain changes, the beds and pours when
a region's tiles come from its bed, and the edit world's previews when the terrain or the region
is edited. A tick of the valley's two regions runs their resident steps as any regions' do
(sections 4 to 16).

Not measured: the renderer's meshing, the browser and R1.
