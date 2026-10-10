# Surface PT and online NRC: Metal measurements, then Vulkan and Direct3D 12

Measured 2026-10-05, Apple M5, 32 GiB unified memory, wgpu 30.0.1 Metal ray queries. Release builds;
no concurrent agent GPU workloads. These are isolated static-renderer measurements, not interactive
application FPS. Transport contract and commands: [spec](../spec/path-tracing-nrc.md).

## Correctness and scope

The 320x240, 1,024 spp `pt-lab` image covers textured metallic-roughness, normal maps, rough/smooth
metal, closed dielectric glass, Mask/Blend coverage, emissive triangles and three punctual light
types. It records millions of transmitted, metallic and textured samples, alpha rejections and
Blend candidates, with zero nonfinite paths and no GPU errors. A separate atmosphere scene uses
the engine's baked sky radiance cube. Test textures and geometry are authored locally by
`tools/generate_pt_fixture.py`; there are no downloaded assets.

CPU f64 checks integrate PDFs/BSDFs, compare visible-normal Monte Carlo sampling to quadrature,
verify Snell/Fresnel/TIR and smooth-surface energy, and test roulette compensation. Actual Metal
`pt_bsdf_probe` evaluates 3,456 cases against that reference. All pass. Near-delta grazing PDFs can
reach millions: that explicitly isolated regime has 1% relative PDF tolerance and observed maximum
0.59% difference; other outputs retain 0.2% plus 2e-4 absolute tolerance. This avoids treating a
large absolute PDF error as a large path-weight error. The GGX denominator uses a cancellation-
resistant expression in both GPU and CPU calculations.

Native tests also verify material/coverage counters, bit-identical final-only vs per-frame readback,
training-only image parity, cold-cache warmup, real labels, cache queries and light-change image
reset. The earlier SHaRC and bounded ReSTIR Metal tests still pass. The rebuilt viewport loads the
actual host's baked GI in Chrome WebGPU without feed-format or GPU errors; its editor publishes no
frame samples, so the browser log's zero count is not an FPS result.

## PT optimization

`gi-room`, 320x240, 128 frames x 1 spp, 12-bounce horizon, RR from bounce 3, seed 42.
Three repeats with order reversal on the middle repeat. GPU means exclude the first 32 frames.
All variants produce bit-identical linear HDR and the same path samples in this opaque scene.

| Variant | Batch wall ms | Warm GPU ms/frame |
| --- | ---: | ---: |
| Nearest shadows, per-frame image readback | 281.457 | 1.715 |
| Nearest shadows, final-only readback | 231.171 | 1.729 |
| Any-hit shadows, final-only readback, WG64 | 249.161 | 1.841 |
| Any-hit shadows, final-only readback, WG128 | 237.149 | 1.761 |

Final-only readback reduces this measured batch wall time by **17.9%**. Its benefit is CPU/GPU
overlap and fewer readback fences, not a faster transport shader. Any-hit was valid but slower in
this run, so nearest shadows and WG64 remain defaults. NRC compile specialization and other
workgroup sizes were tested too; their small timing differences do not establish a reliable gain. No
broader speed claim is made from these short runs.

NEE/MIS and RR are also measured against a finite noisy 2,048 spp reference at 128x128:

| 128 spp variant | Linear HDR RMSE | Warm GPU ms/frame |
| --- | ---: | ---: |
| NEE/MIS + RR | 0.05870 | 0.568 |
| BSDF-only + RR | 0.10355 | 0.390 |
| NEE/MIS, no RR | 0.04872 | 0.906 |

NEE reduces noise at extra shadow-ray cost. RR saves work and raises fixed-spp noise; brightness
means agree closely here. These are quality/cost tradeoffs, not identical-noise timing comparisons.

## Online NRC toy

This is fresh path-label training, not the previous offline baked-field model. The 256-record batch,
14->32->32->3 network and Adam stay on the GPU. `nrc_online_probe` validates actual GPU gradients
against f64 CPU derivatives (max absolute error 9.67e-8) and GPU inference (4.60e-7). Its synthetic
2.5x light-change test moves pre-update log MSE from 0.10487 to 0.00005889 and changes mean
prediction from approximately (0.6065,0.3038,0.1449) to (1.5034,0.7480,0.3808). This synthetic
result establishes trainer correctness, not image quality in a traced scene.

For the real room: 128x128, 512 frames x 1 spp, warmup 64 nonempty updates, query depth 2,
12-bounce horizon, batch 256. Rough opaque vertices query; training pixels trace their full tails.
The cache records fresh labels every frame, and its queries remain zero during warmup. Default
active cache allocation is **1.64 MiB**, including the per-label gradient buffer.

Against the same finite 2,048 spp PT reference, the initial scene's NRC image has HDR RMSE
**0.09195** and mean RGB (0.38870,0.35755,0.25439), versus reference (0.41365,0.37869,0.26394). The
cache is visibly biased dark. A 2.5x lighting change at frame 256 retains the model and resets image
accumulation; the final 256-frame image rises to (0.94359,0.86921,0.62667), roughly 2.43x the
initial mean, while the matching reference is (1.03412,0.94671,0.65984). Changed-scene HDR RMSE is
**0.27068**. On the same 172 real path-feature records captured before the change, network
prediction rises from (0.19050,0.20260,0.12987) to (0.42930,0.41020,0.26552), **2.11x** by summed
RGB. This diagnostic uses the two GPU weight snapshots and the parity-validated CPU reference after
rendering, so it demonstrates network response independently of directly visible lights. It still
under-responds to the 2.5x source change; the records are diagnostic samples, not a held-out quality
benchmark.

Single-path labels are noisy: pre-update log MSE averages 0.07091 before the change, 0.20061 just
after, and 0.20782 in the final window. It does not show loss recovery on this scene. Image
brightness and new traced labels establish response to changed light; synthetic smooth labels
separately establish learnability. Log-space regression, finite horizon, missing depth conditioning
and limited capacity remain deliberate toy approximations.

In the final single-run comparison, the last 64 frames trace about 71,420 total path/shadow rays
per frame for PT and 56,709 for NRC (**20.6% fewer**), with approximately 9,294 cache queries/frame.
However PT costs about **0.605 ms/frame**, versus **0.767 ms tracing/query + 0.325 ms training** for
NRC. This implementation currently slows rendering; no speedup is claimed. Counters distinguish
camera paths, all path-ray segments, continuation-only rays and shadow rays.

Local output is written under `out/bench-runs/pt/`: material/atmosphere previews, room reference and
NRC previews, full reports, timing repeats, HDR comparisons, GPU parity and browser logs. Linear
RGBA outputs remain reproducible local build artifacts under `out/pt/`. `tools/compare_pt.py`
compares those outputs, not tone-mapped PNGs.

## Windows: Vulkan and Direct3D 12 (Pioneer, 2026-10-09)

Decision: charter 4.4 (Pioneer, 2026-10-09), ray-query research on every backend that exposes it.
Branch `explore/rt`. Machine: ASUS ROG Zephyrus G14 (budgets.md R1), Ryzen 9 270, 15 GiB, Windows 11
Pro 10.0.26200; NVIDIA GeForce RTX 5060 Laptop GPU (driver 617.14) and AMD Radeon 780M (driver
32.0.13062.3005, Vulkan 24.30.62.03 LLPC); wgpu 30.0.1, DXC 1.8.2502 from Windows SDK 10.0.26100.
Contract changes: [spec, Backends](../spec/path-tracing-nrc.md#backends-pioneer-2026-10-09).

**The timings below are provisional and superseded by "Timing on a quiet machine"** (the same
day, which repeats them and adds the Radeon 780M): three other agents built and ran GPU work on
the same machine throughout. Runs were interleaved and order-reversed (`tools/rt_bench.py`), and
the spreads are given. Correctness results do not depend on load.

### What runs where

All four GPU/backend pairs expose `EXPERIMENTAL_RAY_QUERY` (RTX 5060 and Radeon 780M, each with
Vulkan and Direct3D 12), and `ray_query_probe` passes on all four: hits, misses, masks, instance
transforms, opaque traversal, shader-confirmed non-opaque candidates and the facing convention
(`out/bench-runs/rt/probe-*.json`). `pt_bsdf_probe` passes all 3,456 cases on all four, with the
M5's margins: worst error/tolerance ratio 0.590 (M5 0.590, the isolated near-delta PDF regime) and
0.071 to 0.093 for ordinary outputs (M5 0.071) (`bsdf-parity-*.json`). `nrc_online_probe` passes on
all four: GPU gradients within 9.7e-8 to 1.6e-7 of the f64 reference (M5 9.67e-8), inference within
5.5e-7 to 7.4e-7 (M5 4.60e-7), and the synthetic light change reproduces the M5's loss trajectory to
seven digits (0.0991549, ..., 0.1048720 at the change, 5.889e-5 at the end; `nrc-parity-*.json`).
The three GPU tests pass on all four pairs.

### Failures found and fixed

- **AMD Direct3D 12 culled every front face.** On the Radeon 780M with Direct3D 12 every camera ray
  of the path tracer missed (gi-room: 12,288 of 12,288 camera paths reached the environment, mean
  radiance 0), and pt-lab kept only double-sided and glass surfaces; the PT GPU test caught it ("no
  metallic samples"), but only because so much went missing: with the culling merely inverted (front
  faces culled, mean radiance 0.105 against 0.695) it still passed. It now also checks pt-lab's mean
  radiance and that every committed hit's `front_face` matches its triangle's winding (spec,
  Backends). Bisection: opaque geometry, constant ray flags or a shorter `t_max` did not matter;
  removing the single-sided test did; counting `candidate.front_face` alone gave the right numbers;
  `!shadow && !candidate.front_face` alone worked; the same test combined with the two material
  comparisons (`emission.w < 0.5 && surface.z <= 0.0`) broke it whatever the order. DXC's DXIL for
  it is correct (`CandidateTriangleFrontFace`, zero-extended, compared with 0, and-ed with the two
  float compares), Vulkan on the same GPU and both backends of the RTX 5060 are right, and the
  probe's simpler candidate loop passes on that pair: a driver miscompile. The fix compares the
  triangle's winding with the ray direction instead, equal by the convention the probe checks.
  Afterwards gi-room and pt-lab give identical 16-counter totals and mean radiance on all four pairs
  (64x48, 4 frames). Strip `out/bench-runs/rt/amd-dx12-facing-bug-pt-lab.png` (780M, pt-lab,
  160x120, 16 frames): Vulkan, Direct3D 12 with the old test (mean radiance 0.061 against 0.274),
  Direct3D 12 with the fix (equal to Vulkan).
- **naga panics on a ray query passed to a function** (`not a cached ray query` in the SPIR-V
  writer, `unreachable` in the HLSL writer): a process abort, not a validation error. The research
  shaders never did it; the new probe shader hands its helper the intersection instead.
- **Vulkan adapters sometimes vanish under parallel tests**: one of the first test runs found no
  Vulkan adapter at all in one test thread (`vulkan drivers/libraries could not be loaded`) while
  the others had one. The tests now open their GPUs one at a time; not seen again in about ten runs.

### Correctness against the M5 references

`python tools/rt_bench.py correctness --adapter 5060` renders every configuration whose M5 report is
committed, at the same settings and seed (`out/bench-runs/rt/correctness-5060.json`; a subset on the
780M in `correctness-780m.json` gives the same picture). The M5 runs committed no linear HDR, so the
comparison with them uses what they did commit: mean HDR radiance, per-frame path counters and the
8-bit preview. HDR RMSE is computed against each backend's own finite reference at the M5's
settings, next to the RMSE the M5 recorded against its reference. Pairs are Vulkan / Direct3D 12 on
the RTX 5060.

| Configuration | Mean vs M5 (max rel.) | Frames with M5's counters | Preview pixels equal to M5 | Max 8-bit diff | Vulkan vs D3D12 rel. HDR RMSE | HDR RMSE vs reference (Vulkan / D3D12 / M5) |
|---|---:|---:|---:|---:|---:|---:|
| gi-room reference, 2,048 spp | 3.1e-6 / 2.7e-6 | 449 / 448 of 512 | 99.79% / 99.79% | 5 / 5 | 1.7e-5 | reference |
| 128 spp | 1.4e-6 / 2.0e-6 | 127 / 127 of 128 | 99.90% / 99.88% | 1 / 1 | 4.5e-5 | 0.05870 / 0.05870 / 0.05870 |
| 512 spp | 3.0e-6 / 1.3e-6 | 499 / 500 of 512 | 99.90% / 99.88% | 2 / 2 | 3.0e-5 | 0.02545 / 0.02545 / 0.02545 |
| BSDF-only | 1.9e-5 / 1.9e-5 | 124 / 124 of 128 | 99.99% / 99.99% | 13 / 13 | 2.5e-8 | 0.10355 / 0.10355 / 0.10355 |
| no RR | 4.0e-6 / 3.5e-6 | 111 / 110 of 128 | 99.91% / 99.90% | 10 / 10 | 5.6e-5 | 0.04872 / 0.04872 / 0.04872 |
| NRC | 3.8e-5 / 1.4e-5 | 507 / 507 of 512 | 97.08% / 96.44% | 1 / 1 | 5.1e-5 | 0.09194 / 0.09195 / 0.09195 |
| NRC, 2.5x light change | 3.4e-5 / 3.3e-5 | 507 / 507 of 512 | 95.43% / 95.72% | 1 / 1 | 9.9e-5 | 0.27069 / 0.27067 / 0.27068 |
| pt-lab materials, 1,024 spp | 4.7e-4 / 4.7e-4 | 0 / 0 of 256 | 92.63% / 92.64% | 88 / 88 | 4.9e-4 | (no M5 RMSE) |
| pt-lab atmosphere | 9.9e-5 / 1.0e-4 | 0 / 0 of 128 | 97.71% / 97.70% | 42 / 42 | 1.1e-4 | (no M5 RMSE) |

Reading it:

- In the opaque gi-room the Windows backends trace **the same paths as the M5**: the same seed and
  integer RNG give identical path decisions, so most frames have exactly the M5's 16 counters and
  the mean radiance agrees to a few parts per million. The frames that differ do so by a handful of
  events (largest relative counter difference 0.06% to 0.16%): a float rounded differently flips a
  roulette, BSDF or NEE decision now and then and that path continues differently. Vulkan and
  Direct3D 12 differ from each other in the same way and no more (relative HDR RMSE 1.7e-5 to
  5.6e-5). Sources of the rounding: transcendental functions (`pow`, `exp`, `sqrt`) of different
  precision per compiler and vendor, fused multiply-add contraction that MSL, SPIR-V (driver) and
  DXIL compilers apply differently, and the hardware intersectors' barycentrics.
- The NRC runs agree to the same degree although they train a network on the GPU: the Adam
  updates use no float atomics, so training is reproducible to rounding.
- pt-lab diverges from the first frame (no frame equal to the M5's) but agrees in the mean to 5e-4
  and in the preview to 92.6% of pixels exactly, 0.2% off by more than 2. The counters that move
  are the candidate ones (`alpha_rejections` -2.0%, `transparent_candidates` -3.7%): Blend coverage
  draws a random number per candidate hit, and how many candidates a traversal visits before it
  commits the closest depends on the vendor's BVH. Once one draw is consumed differently the
  sequence differs. Texture filtering precision also differs per vendor. The atmosphere scene
  samples the baked sky cube with hardware filtering. Both are expected and statistically
  consistent (the RTX 5060 and the 780M visit the same candidates on the small runs; Apple does
  not).
- Approximate HDR from the M5 previews (inverse Reinhard/sRGB of the 8-bit PNG, saturated pixels
  left out) is in the JSON, but 8-bit quantization alone gives 2.3% to 3.4% relative RMSE even for
  the identical gi-room runs, so it is not used for conclusions.

Strips (M5 left, this machine middle, 8x difference right): gi-room reference
`out/bench-runs/rt/strip-room-m5-vulkan.png`, pt-lab `strip-lab-m5-vulkan.png`, and pt-lab Vulkan vs
Direct3D 12 `strip-lab-vulkan-dx12.png` (largest 8-bit difference 1).

### Timing (provisional, superseded below), RTX 5060

`python tools/rt_bench.py timing --adapter 5060 --rounds 5` (`out/bench-runs/rt/timing-5060.json`):
median GPU ms per frame over five interleaved rounds (min to max in brackets), the documented
settings of the M5 tables above. "Lean" builds the shaders without naga's loop bounding and
ray-query initialization tracking (`--lean-shaders true`, off by default).

gi-room, 320x240, 128 frames x 1 spp, 12 bounces, RR from 3, seed 42; GPU mean from frame 32:

| Variant | Vulkan | Direct3D 12 | M5 (Metal) |
|---|---:|---:|---:|
| Nearest shadows, per-frame readback | 0.316 [0.315, 0.346]; wall 92 ms | 0.378 [0.372, 0.389]; wall 112 ms | 1.715; wall 281 ms |
| Nearest shadows, final-only readback | 0.320 [0.314, 0.331]; wall 45 ms | 0.410 [0.372, 0.412]; wall 59 ms | 1.729; wall 231 ms |
| Any-hit shadows, WG64 | 0.312 [0.307, 0.317] | 0.368 [0.364, 0.403] | 1.841 |
| Any-hit shadows, WG128 | 0.331 [0.325, 0.334] | 0.402 [0.386, 0.427] | 1.761 |
| Nearest, final-only, **lean shaders** | **0.247** [0.245, 0.250] | **0.287** [0.287, 0.321] | not measured |

- The RTX 5060 traces this workload about 5.4x faster than the M5 on Vulkan and 4.2x on
  Direct3D 12. Direct3D 12 is 16% to 28% slower than Vulkan on the same GPU with the same WGSL.
- Final-only readback halves the batch wall time here (92 to 45 ms on Vulkan, 112 to 59 on
  Direct3D 12; M5 -18%): with a faster GPU the per-frame fence and copy are a larger share.
- Any-hit shadow rays are slightly faster on both backends (unlike on the M5); WG128 is slower.
  Neither difference is large enough to change the defaults on provisional numbers.
- naga's ray-query initialization tracking costs about a quarter of the trace pass: lean shaders
  run it 23% faster on Vulkan and 30% faster on Direct3D 12, with the same paths (the GPU tests
  compare them). A single-variable run (`PT_UNCHECKED` experiment, not kept in code) attributed
  0.325 to 0.246 ms on Vulkan and 0.394 to 0.291 ms on Direct3D 12 to the tracker alone; loop
  bounding cost 6% to 9%; bounds and integer-division checks changed it by less than that
  two-round experiment's noise.

Online NRC, gi-room 128x128, 512 frames x 1 spp, warmup 64, batch 256; last 64 frames:

| | Vulkan | Direct3D 12 | M5 |
|---|---:|---:|---:|
| PT, GPU ms/frame | 0.117 | 0.129 | 0.605 |
| NRC trace/query + training | 0.192 + 0.204 = 0.396 | 0.495 + 0.381 = 0.876 | 0.767 + 0.325 = 1.092 |
| PT, lean | 0.080 | 0.124 | |
| NRC, lean | 0.124 + 0.085 = 0.209 | 0.118 + 0.061 = 0.179 | |
| Path + shadow rays/frame, PT vs NRC | 71,420 vs 56,709 (-20.6%) | the same | the same |

- The ray counts equal the M5's exactly (71,420 and 56,709, 9,294 cache queries per frame): the
  same paths again.
- With default shaders the NRC costs 3.4x PT on Vulkan and 6.8x on Direct3D 12. The Direct3D 12
  excess is naga's loop bounding: with bounded loops DXC does not optimize the network's fixed
  32x32 and 14x32 loops (trace pass 0.495 ms; 0.126 ms with loop bounding alone removed, the
  experiment above), and the trainer's backward pass suffers the same. Lean shaders bring both
  backends to about 0.2 ms, still 2.6x (Vulkan) and 1.4x (Direct3D 12) the uncached PT: as on the
  M5, the toy cache saves 20.6% of the rays but costs more than it saves. No speedup is claimed.

### Timing on a quiet machine (2026-10-09)

The same tool and settings with no other agent running, on AC power (conditions:
[quiet-2026-10-09.md](quiet-2026-10-09.md)), master `ef35e1af`:
`python tools/rt_bench.py timing --adapter 5060 --rounds 5` and `--adapter 780m --rounds 3`, rounds
interleaved and order-reversed
([timing-5060.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/rt/timing-5060.json),
[timing-780m.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/rt/timing-780m.json);
`python tools/quiet_tables.py rt`). GPU ms per frame, median of the rounds (minimum; spread); the
provisional RTX 5060 medians follow the slash.

gi-room, 320x240, 128 frames x 1 spp, 12 bounces:

| Variant | RTX 5060 Vulkan | RTX 5060 D3D12 | 780M Vulkan | 780M D3D12 |
|---|---:|---:|---:|---:|
| Nearest, per-frame readback | 0.315 (0.314; 2%) / 0.316 | 0.376 (0.372; 9%) / 0.378 | 1.255 (1.232; 6%) | 1.249 (1.237; 4%) |
| Nearest, final-only readback | 0.315 (0.314; 2%) / 0.320 | 0.405 (0.388; 6%) / 0.410 | 1.224 (1.223; 3%) | 1.209 (1.192; 4%) |
| Any-hit shadows, WG64 | 0.307 (0.307; 2%) / 0.312 | 0.402 (0.396; 2%) / 0.368 | 1.193 (1.184; 4%) | 1.194 (1.182; 3%) |
| Any-hit shadows, WG128 | 0.325 (0.324; 2%) / 0.331 | 0.426 (0.401; 6%) / 0.402 | 1.388 (1.385; 1%) | 1.387 (1.370; 1%) |
| Nearest, final-only, lean | 0.245 (0.244; 1%) / 0.247 | 0.316 (0.295; 7%) / 0.287 | 0.877 (0.852; 3%) | 0.896 (0.890; 1%) |
| Batch wall, final-only (ms) | 44 / 45 | 56 / 59 | 241 | 234 |

Online NRC, gi-room 128x128, 512 frames; trace or query + training, last 64 frames:

| | RTX 5060 Vulkan | RTX 5060 D3D12 | 780M Vulkan | 780M D3D12 |
|---|---:|---:|---:|---:|
| PT | 0.117 / 0.117 | 0.142 / 0.129 | 0.341 | 0.313 |
| NRC | 0.181 + 0.185 = 0.366 / 0.396 | 0.494 + 0.380 = 0.874 / 0.876 | 0.443 + 0.192 = 0.635 | 1.075 + 0.299 = 1.374 |
| PT, lean | 0.080 / 0.080 | 0.124 / 0.124 | 0.225 | 0.251 |
| NRC, lean | 0.115 + 0.075 = 0.191 / 0.209 | 0.118 + 0.061 = 0.178 / 0.179 | 0.215 + 0.095 = 0.310 | 0.270 + 0.082 = 0.352 |

- **The RTX 5060's provisional numbers stand** (they were taken in a quieter stretch): every quiet
  median is within 3% of its provisional one except Direct3D 12's any-hit, lean and NRC-run PT
  cells (6 to 10% slower now) and Vulkan's NRC runs (8 to 9% faster), and the rounds agree within
  0 to 10%. Direct3D 12
  stays 19 to 31% slower than Vulkan on the same GPU with the same WGSL.
- **The Radeon 780M** (new): the two APIs tie on the path tracer (within 3%), about 4x slower than
  the RTX 5060. Lean shaders save 28% (Vulkan) and 26% (Direct3D 12) of the trace, as on the RTX
  5060 (22%).
- **NRC still costs more than it saves**: 3.1x the uncached PT on the RTX 5060 with Vulkan (2.4x
  lean), 1.9x on the 780M (1.4x lean); with default shaders Direct3D 12 pays naga's loop bounding
  again (6.2x on the RTX 5060, 4.4x on the 780M). No speedup is claimed.
- **Defaults of the research tool unchanged**: nearest-hit shadow rays and 64-wide workgroups.
  Any-hit shadows are 2.5% faster on Vulkan and 1% on Direct3D 12, less than the rounds' spread
  there, and were slower on the M5; 128-wide workgroups are 6 to 16% slower everywhere.
  Lean shaders stay opt-in: they drop naga's loop bounding and ray-query initialization checks,
  a safety choice, not one waiting on speed.

### Reproduce (Windows)

```sh
cargo build --release -p pocket-render --examples
POCKET_BACKEND=vulkan cargo run --release -p pocket-render --example ray_query_probe
POCKET_BACKEND=dx12 POCKET_ADAPTER=780m cargo run --release -p pocket-render --example pt_bsdf_probe
cargo test --release -p pocket-render --lib ray_query        # both backends; POCKET_ADAPTER=780m too
python tools/rt_bench.py correctness --adapter 5060 --summary out/bench-runs/rt/correctness-5060.json
python tools/rt_bench.py timing --adapter 5060 --rounds 5 --summary out/bench-runs/rt/timing-5060.json
```
