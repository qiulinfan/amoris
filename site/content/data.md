# Data and validation

These results describe particular workloads and builds. They are not promises about
every scene or a general ranking against other engines.

## High-detail native rendering

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

Raw frame measurements stay in ignored local output; the table retains the completed-frame summary.
The existing GPU profiler covers selected passes and omits postprocessing; Metal pass
boundaries can omit tile work. Its sum is **not** a whole-frame GPU time and is not used
for the chart above. The renderer currently downsamples maps into **1024 × 1024** layers.

Reproduce the workload:

```sh
python3 tools/showcase_assets.py --projects
cargo run --release -p pocket-render --example showcase_bench -- \
  site/demos/stress-1024 --width 1920 --height 1080 --frames 300 --warm 60
```

The asset fetch verifies pinned hashes. See [asset credits](credits.md) for licenses,
source authors, conversion details, and current material limitations.

## Native and browser validation

The imported implementation was checked on **5 October 2026**:

| Check | Result |
| --- | --- |
| Native release build | Passed |
| Native tests | 383 passed, 0 failed, 0 ignored in 78 suites; ignored tests included |
| Dependency boundaries | Passed across 19 workspace crates and 2 targets |
| wasm compilation | Game crates and pocket-web passed |
| Script types | Passed for 2 projects |
| Determinism | Passed, including a native debug/release comparison |
| Fork, replay, and reload | Passed on the configured sample workloads |
| Browser/native hash chain | Passed in headless Chrome, isolated and non-isolated, and after wasm-opt -O3 |
| Editor | Typecheck and production build passed |

The full `cargo xtask check` verdict remains **Fail**: the source has Rust formatting,
Markdown wrapping, and Clippy diagnostics. Its contract conformance and performance
budget steps are still unimplemented and report **Skipped**. The test count above does
not mean the full check is green.

## Web rendering study

Recorded **4 October 2026** on Apple M5, Chrome 152 headless, with a 1280 × 577 canvas
at DPR 1. Results are the means of the last 300 frames after warm-up, without a frame-rate cap.

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
records methodology, module sizes, and the open gap between native and web GPU timing.

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
