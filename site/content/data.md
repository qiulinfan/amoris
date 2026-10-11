# Data and validation

Updated **10 October 2026**, through [Amoris
`3204f242`](https://github.com/qiulinfan/amoris/commit/3204f24281d0c1bcb6ae9df7df1e9126d50b8f8f).
The Windows prepass results, Metal profiling checks, and older showcase measurements use different
workloads and metrics. They do not establish a general engine ranking. [Profiling](profiling.md)
explains how to record and compare your own scene.

## Windows: depth prepass

Recorded **10 October 2026** on a Ryzen 9 270 / Windows 11 laptop with an
**RTX 5060 Laptop 8 GB** and **Radeon 780M**, wgpu 30.0.1. The workload is a
**1.6 million cube dense grid**, headless at **2560 × 1440**, **4× MSAA**,
GTAO off and **occlusion culling off**. These figures compare the prepass forced
off with forced on; they are the median of three runs' mean timestamped pass sums.

| GPU / backend | Prepass off | Prepass on | Reduction in measured GPU pass sum |
| --- | --- | --- | --- |
| RTX 5060 Laptop / Direct3D 12 | 51.330 ms | 16.216 ms | 68.4% |
| RTX 5060 Laptop / Vulkan | 42.505 ms | 16.591 ms | 61.0% |
| Radeon 780M / Direct3D 12 | 297.574 ms | 96.256 ms | 67.7% |

Each run settles for five seconds, warms up for 60 frames, then measures 200 frames
on the RTX 5060 or 100 on the 780M. These results are **provisional**: another agent
shared the machine, CPU load was not excluded, and clocks were not locked.
The timestamped sum excludes the post chain in this experiment.

The default is adaptive `auto`. On the less-overlapping sphere workload, forcing the
prepass on instead costs 19–35% more GPU time on the RTX 5060 and 8–10% on the 780M.
With occlusion culling at its default `auto`, the RTX 5060 / Direct3D 12 dense case
measured **2.33 → 1.44 ms** from prepass off to auto. These are separate configurations;
the 51.330 → 16.216 ms result is not the default scene's cost.

See the [prepass method and full
tables](https://github.com/qiulinfan/amoris/blob/main/docs/bench/prepass.md), [Windows profiling
study](https://github.com/qiulinfan/amoris/blob/main/docs/bench/dx12.md), and [published prepass
runs](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-prepass-20261010).

## Metal: frame attribution and trace export

Recorded **10 October 2026** on **Apple M5**, macOS 27.0.1 (26A434), wgpu 30.0.1.
Metal and MoltenVK now resolve timestamp queries after the sampling submission
completes, with separate resources for each in-flight slot. Normal rendering reads
back asynchronously; clock calibration and final export may wait for completion.
This fixes counters being attributed to the wrong frame.

Three interleaved rounds used **200,000 cubes**, **1280 × 720**, **4× MSAA**,
GTAO and occlusion off, a two-second settle, 60 warm-up frames and 300 measured
frames per run. The second round reverses the case order.

| Case | Measured frames | GPU frame median, median of runs | Full range of run medians |
| --- | --- | --- | --- |
| Dense, prepass off | 900 | 6.368 ms | 6.278–29.601 ms |
| Dense, prepass auto | 900 | 6.478 ms | 6.191–6.637 ms |
| Sphere, prepass auto | 900 | 0.901 ms | 0.849–13.877 ms |

All **2,700 GPU frame spans** fit their own CPU submit/wait bounds within the reported
clock uncertainty plus a 20 µs tolerance, with **zero missing or dropped records**.
Clock-placement uncertainty was **53.5–79.9 µs**. Auto kept the
prepass off for 297–299 of each run's 300 frames; the others were probes.

These captures validate frame attribution and export. The active desktop and unlocked
clocks produced large variation, so they do **not** establish a stable speedup.
A GPU frame span runs from the first timestamped pass's start to the last one's end;
it includes gaps between those passes. It differs from the Windows table's summed pass
durations and from CPU wall time, so the two tables are not a backend comparison.

The six prepass comparison scenes preserve every pixel and entity's coverage. The matching occlusion
comparisons preserve all entity coverage; one channel of one pixel in `pt-lab` differs by one 8-bit
level, with the other five scenes identical. See the [Metal
method](https://github.com/qiulinfan/amoris/blob/main/docs/bench/metal-profiling.md) and [all raw
runs and
controls](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/amoris-metal-profiling-20261010).

## High-detail native rendering · 5 October archive

The complete Bistro exterior contains **2,829,226 triangles**, **1,296 mesh objects**
and **132 authored materials**. Its 405 embedded source images decode to 3.76 GB of
RGBA8 before the renderer's 1K conversion. The imported runtime has 1,591 primitive
instances and 133 material slots including its default material. At the recorded
street-side camera, the same 1080p render-and-wait method measured **12.02 ms mean**,
**11.94 ms p50**, **13.00 ms p95**, and **1.827 ms CPU encode mean** over 300 frames.

Recorded **5 October 2026**, **Apple M5 / Metal**, **1920 × 1080**. Each CC0
Flight Helmet contributes 94,722 triangles and six materials. Geometry and textures
are shared; the scene repeats the complete model rather than substituting a primitive.
Directional shadows and normal PBR/postprocessing passes remain enabled.

| Helmets | Scene triangles before culling | Primitive instances | Completed frame mean | p50 | p95 | CPU encode mean |
| --- | --- | --- | --- | --- | --- | --- |
| 64 | 6,062,210 | 385 | 10.52 ms | 10.48 ms | 11.48 ms | 0.317 ms |
| 256 | 24,248,834 | 1,537 | 28.81 ms | 28.83 ms | 29.65 ms | 0.407 ms |
| 1,024 | 96,995,330 | 6,145 | 82.43 ms | 82.41 ms | 83.66 ms | 0.454 ms |

The benchmark waits for **zero pending assets**, completes **60 loaded warm-up frames**,
then measures **300 frames**. Each frame renders offscreen and waits for GPU completion.
This wall time includes the complete static rendering workload and command submission;
it excludes game simulation, window presentation, pixel readback and PNG encoding.
It is not an interactive game's FPS. The scene triangle counts exclude shadow-pass
multiplication and precede culling. An untimed ID pass confirmed visible pixels from all
64 and 256 helmets, and 1,021 of 1,024 helmets (others were occluded), plus the ground.

This is the original completed-frame wall-time study, not a rerun of the current build.
It does not use the historical Metal pass timestamps affected by the later profiling fix.
The profiler at capture time covered selected passes and omitted postprocessing; that
sum was not a whole-frame measurement. The capture used maps downsampled into
**1024 × 1024** layers. Raw measurements belong in ignored output or the
[results archive](https://github.com/qiulinfan/amoris-benchmarks-results).

Reproduce the workload:

```sh
python3 tools/showcase_assets.py --projects
cargo run --release -p pocket-render --example showcase_bench -- \
  site/demos/stress-1024 --width 1920 --height 1080 --frames 300 --warm 60
```

See [asset credits](credits.md) for source URLs, licenses, authors, conversion details,
and material limitations. The fetch prepares the recorded source assets.

## Native and browser validation · 10 October

The Metal integration at `3204f242` was checked as follows:

| Check | Result |
| --- | --- |
| Workspace tests | 713 passed, 0 failed, 0 ignored across 117 suites; normally ignored tests included |
| Implemented `xtask` stages | Passed across the full run and subsequent formatting, documentation and native/wasm Clippy rerun |
| Python profiling and benchmark-runner tests | 7 passed |
| Metal traces | 2,700 measured frames; no missing or dropped GPU records |
| Prepass image and entity comparisons | Six scenes identical |
| Rebuilt WebGPU viewport | Chrome smoke: 300 frames, 10,000 instances, all 300 GPU-timed, first-instance draw path |

The full `cargo xtask check` run passed its test and other implemented stages but
reported a Clippy diagnostic. After that correction, the formatting, documentation,
and native/wasm Clippy stages passed on rerun. This was **not one all-green full run**.
The contract conformance and performance-budget stages remain **unimplemented** and
report **Skipped**. The Chrome smoke validates the rebuilt viewport; it is not a
new browser performance comparison.

## Web rendering study · 4 October archive

Recorded **4 October 2026** on Apple M5, Chrome 152 headless, with a 1280 × 577 canvas
at DPR 1. Results are the means of the last 300 frames after warm-up, without a frame-rate cap.
These predate later viewport and shader changes and have not been remeasured on the M5.

The sphere layout follows Bevy's `many_cubes`: one mesh, one material, a directional light,
and 4× MSAA. three.js r186 uses `WebGPURenderer` and an `InstancedMesh`. The Amoris renderer
uses per-instance GPU culling, indirect draws, clustered PBR, atmosphere lighting, bloom,
and AgX. The two paths perform different work.

| Cubes | three.js frame interval | three.js GPU | Amoris frame interval | Amoris GPU |
| --- | --- | --- | --- | --- |
| 100,000 | 1.00 ms | 2.45 ms | 0.95 ms | 0.71 ms |
| 400,000 | 4.17 ms | 2.36 ms | 2.36 ms | 1.81 ms |
| 1,600,000 | 24.8 ms | 21.9 ms | 15.0 ms | 14.7 ms |

The frame interval measures submission pace; timestamp queries measure GPU work.
The original [web study](https://github.com/qiulinfan/amoris/blob/main/docs/bench/web.md)
records the original methodology and later Windows browser measurements. Its early
native Metal timestamp comparison predates the frame-attribution fix and does not
establish a current browser/native performance gap.

## Physics is a separate study

The current physics backend is Rapier 3D with enhanced determinism. A separate benchmark
compares its feature configurations with Jolt 5.6 on equivalent Jolt PerformanceTest scenes,
and studies snapshots and forks. The source explicitly distinguishes results from a future
backend recommendation; the engine has not switched to Jolt.

See the [physics study](https://github.com/qiulinfan/amoris/blob/main/docs/bench/physics.md)
for native, wasm, SIMD, and deterministic configurations.

## Reproduce the checks

```sh
cargo xtask check --json
./target/release/pocket check samples/sailing --json
cd editor && bun run typecheck && bun run build
```

Keep `CARGO_TARGET_DIR` inside the checkout. Never set `POCKET_BLESS` while validating.
Media capture is separate from these performance measurements: the demonstration clips
use recorded frames and fixed simulation steps.
