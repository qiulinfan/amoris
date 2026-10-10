# Hi-Z occlusion: method, default and measured limits

Two-phase Hi-Z culling is enabled in `auto` mode. `POCKET_OCCLUSION=off|on|auto` overrides it;
the browser uses `?occlusion=`. The decision is based on work saved, so a lightly occluded scene
can remain on the ordinary frustum-culling path.

Design and correctness contract: [occlusion](../spec/occlusion.md),
[rendering architecture](../spec/architecture.md). Related measurements:
[LOD](lod.md), [backend cost](dx12.md), [shared conditions](quiet-2026-10-09.md).

## How it works

The early pass draws previously visible instances at the current interpolated pose. Its depth
builds the Hi-Z pyramid; conservative projected bounds then test the remaining instances. Visible
late instances are drawn in a second pass, including entity IDs. A cold frame, camera cut, resize,
new occluder or newly exposed object must not inherit an invalid visibility decision.

The auto model charges `200,000 + 0.5 * frustum_instances` triangle-equivalents for the second
phase. A probe activates it only when saved triangles exceed 1.5 times that charge. LOD-aware
triangle counts matter: hiding an already coarse instance saves less work than hiding its full
mesh. The margin avoids flipping modes around a small predicted gain.

## Workloads and timing

R1: Windows 11 Pro 10.0.26200, Ryzen 9 270 (16 threads), 15 GiB, RTX 5060 Laptop 8 GB and
Radeon 780M, wgpu 30.0.1, AC/Balanced. NVIDIA 617.14; AMD 32.0.13062.3005 / Vulkan 24.30.62.03.
No concurrent builds or benchmark agents; desktop UI load was still present.

`many_cubes` draws at 1280x720, 4x MSAA, GTAO off, without sun shadows: 60 warm-up frames and
200 measured frames, each submitted and waited for. GPU time is the sum of timestamped pass
means; wall time additionally includes encoding/submission and waiting.

| Layout | Geometry and visibility |
|---|---|
| Sphere | 1.6M cubes; camera inside; about 187,500 in the frustum and visible |
| Dense | 117-cube-side grid; camera inside; 481,933 in the frustum, 443 visible |
| Orbit | Camera outside the dense grid; about 102,000 visible; newly exposed objects need late draws |

## Last reliable large-layout measurements

These 2026-10-09 quiet-build results are medians of three interleaved repeats, GPU ms. The full
large layouts were not repeated on the final Windows build; the decisive sphere/crossover checks
below were. The large-layout measurements had no start-up settling or per-repeat cooling.

| Configuration | Layout | Off | On | Auto |
|---|---|---:|---:|---:|
| RTX 5060, Vulkan | dense | 14.77 (14.73; 0%) | **0.85** (0.85; 1%) | 0.88 (0.88; 8%) |
| RTX 5060, Vulkan | orbit | 20.05 (20.02; 0%) | **2.51** (2.51; 1%) | 2.56 (2.52; 2%) |
| RTX 5060, Direct3D 12 | dense | 18.15 (18.13; 0%) | **0.98** (0.97; 2%) | 0.98 (0.96; 10%) |
| RTX 5060, Direct3D 12 | orbit | 21.09 (21.09; 0%) | **2.64** (2.60; 2%) | 2.62 (2.62; 0%) |
| Radeon 780M, Vulkan | dense | 86.64 (85.98; 5%) | **4.85** (4.84; 3%) | 5.11 (4.96; 7%) |
| Radeon 780M, Vulkan | orbit | 47.27 (42.25; 13%) | **8.17** (8.07; 5%) | 8.26 (8.11; 2%) |
| Radeon 780M, Direct3D 12 | dense | 75.47 (75.37; 2%) | **4.34** (4.31; 1%) | 4.24 (4.17; 4%) |
| Radeon 780M, Direct3D 12 | orbit | 41.02 (40.95; 1%) | **7.94** (7.88; 1%) | 8.03 (7.92; 3%) |

The dense layout gains 17–19 times and the orbit about 5–8 times on these workloads. Where
nothing is occluded, forced two-phase rendering instead adds work. These are visibility-heavy
synthetic scenes, not a general multiplier for games.

## Final no-occlusion calibration

2026-10-10 final Windows build, sphere layout: six repeats, reversed mode order on alternate
repeats, five-second start-up settling, NVIDIA at 70 C or below before each repeat. Cells show
median (minimum; spread), GPU ms. The cost difference is calculated before display rounding.

|  Configuration  |  off  |  on  |  auto  |  on - off  |  Triangles (of the bar)  |
| --- | --- | --- | --- | --- | --- |
|  RTX 5060, Direct3D 12  |  2.26 (2.24; 1%)  |  2.54 (2.53; 1%)  |  2.26 (2.23; 2%)  |  0.29  |  346,000 (0.79)  |
|  RTX 5060, Vulkan  |  2.24 (1.78; 22%)  |  2.50 (2.22; 12%)  |  2.18 (1.98; 17%)  |  0.27  |  324,000 (0.74)  |
|  Radeon 780M, Direct3D 12  |  6.45 (6.38; 1%)  |  7.34 (7.19; 5%)  |  6.45 (6.32; 4%)  |  0.89  |  407,000 (0.92)  |
|  Radeon 780M, Vulkan  |  7.39 (7.24; 6%)  |  8.29 (8.18; 11%)  |  7.30 (7.17; 6%)  |  0.90  |  345,000 (0.78)  |

The 440,634-triangle probe bar exceeds the median second-phase cost by 8–26% across these four
configurations. The 780M with D3D12 has the least margin. NVIDIA Vulkan remains bimodal: individual
processes can select different opaque-pass timing clusters, so the median comparison is more
useful than one pair of starts. Some 780M Vulkan timestamp sums also exceed frame wall time.

## Final threshold check

Three repeats on the final build, GPU ms; the low-margin mode choice is intentionally conservative.
These threshold runs did not use the later settling/cooling protocol of the sphere calibration.

| Configuration | 27,000 off / on / auto | 64,000 off / on / auto |
|---|---|---|
| RTX 5060, Direct3D 12 | 0.35 / **0.22** / 0.34 | 0.79 / 0.30 / 0.30 |
| RTX 5060, Vulkan | 0.34 / **0.21** / 0.34 | 0.86 / 0.29 / 0.29 |
| Radeon 780M, Direct3D 12 | 1.41 / **1.33** / 1.39 | 2.94 / 1.64 / 1.62 |
| Radeon 780M, Vulkan | 1.94 / 1.92 / 1.97 | 3.45 / 2.23 / 2.24 |

Auto stays off at 27,000 cubes and on at 64,000. The constants remain unchanged. A change that
raises second-phase cost, particularly on 780M D3D12, needs the sphere calibration repeated.
Apple comparative costs and a final-build browser re-measurement are absent from this dataset.

## Correctness and reproduction

The native checks compare exact images and ID-pass coverage through movement, interpolation,
camera cuts, resize, slits and orthographic views, with all draw paths and minimal capabilities.
The pyramid and box tests are also checked against brute force. In-process scene comparisons on
both Windows GPUs/APIs had identical pixels and coverage; browser occluder/dense comparisons also
agreed. Separate-process animations are unsuitable as an exact pixel oracle because their time
can differ. Pixel picking and coverage readback must include late draws.

```sh
cargo build --release -p pocket-render --examples
cargo build --release -p pocket-app --example draw_paths
POCKET_AA=msaa POCKET_GTAO=off python tools/occlusion_bench.py \
  --configs vulkan:nvidia,dx12:nvidia,vulkan:780m,dx12:780m --layouts sphere --frames 200 \
  --repeats 6 --alternate --settle 5 --cool 70 --out out/bench-runs/hiz.json
python tools/quiet_tables.py hiz out/bench-runs/hiz.json
cargo test --release -p pocket-render --test occlusion --test hiz
```

`tools/occlusion_compare.py --backend <backend>` checks captures and in-process scenes. Run mutation
checks only in an isolated clean worktree; keep their detailed results in ignored `out/`. The [fixed
historical
report](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/quiet/hiz)
records the original measurements.
