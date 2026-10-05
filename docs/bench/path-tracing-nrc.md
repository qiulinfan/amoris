# Metal surface PT and online NRC

Measured 2026-10-05, Apple M5, 32 GiB unified memory, wgpu 30.0.1 Metal ray queries.
Release builds; no concurrent agent GPU workloads. These are isolated static-renderer measurements,
not interactive application FPS. Transport contract and commands: [spec](../spec/path-tracing-nrc.md).

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
training-only image parity, cold-cache warmup, real labels, cache queries and light-change image reset.
The earlier SHaRC and bounded ReSTIR Metal tests still pass. The rebuilt viewport loads the actual
host's baked GI in Chrome WebGPU without feed-format or GPU errors; its editor publishes no frame
samples, so the browser log's zero count is not an FPS result.

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
this run, so nearest shadows and WG64 remain defaults. NRC compile specialization and other workgroup
sizes were tested too; their small timing differences do not establish a reliable gain. No broader
speed claim is made from these short runs.

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
2.5x light-change test moves pre-update log MSE from 0.10487 to 0.00005889 and changes mean prediction
from approximately (0.6065,0.3038,0.1449) to (1.5034,0.7480,0.3808). This synthetic result establishes
trainer correctness, not image quality in a traced scene.

For the real room: 128x128, 512 frames x 1 spp, warmup 64 nonempty updates, query depth 2,
12-bounce horizon, batch 256. Rough opaque vertices query; training pixels trace their full tails.
The cache records fresh labels every frame, and its queries remain zero during warmup. Default
active cache allocation is **1.64 MiB**, including the per-label gradient buffer.

Against the same finite 2,048 spp PT reference, the initial scene's NRC image has HDR RMSE **0.09195**
and mean RGB (0.38870,0.35755,0.25439), versus reference (0.41365,0.37869,0.26394). The cache is visibly
biased dark. A 2.5x lighting change at frame 256 retains the model and resets image accumulation;
the final 256-frame image rises to (0.94359,0.86921,0.62667), roughly 2.43x the initial mean, while
the matching reference is (1.03412,0.94671,0.65984). Changed-scene HDR RMSE is **0.27068**.
On the same 172 real path-feature records captured before the change, network prediction rises
from (0.19050,0.20260,0.12987) to (0.42930,0.41020,0.26552), **2.11x** by summed RGB. This diagnostic
uses the two GPU weight snapshots and the parity-validated CPU reference after rendering, so it
demonstrates network response independently of directly visible lights. It still under-responds
to the 2.5x source change; the records are diagnostic samples, not a held-out quality benchmark.

Single-path labels are noisy: pre-update log MSE averages 0.07091 before the change, 0.20061 just
after, and 0.20782 in the final window. It does not show loss recovery on this scene. Image brightness
and new traced labels establish response to changed light; synthetic smooth labels separately
establish learnability. Log-space regression, finite horizon, missing depth conditioning and limited
capacity remain deliberate toy approximations.

In the final single-run comparison, the last 64 frames trace about 71,420 total path/shadow rays
per frame for PT and 56,709 for NRC (**20.6% fewer**), with approximately 9,294 cache queries/frame.
However PT costs about **0.605 ms/frame**, versus **0.767 ms tracing/query + 0.325 ms training** for
NRC. This implementation currently slows rendering; no speedup is claimed. Counters distinguish
camera paths, all path-ray segments, continuation-only rays and shadow rays.

Committed evidence is under `docs/evidence/pt/`: material/atmosphere previews, room reference and
NRC previews, full reports, timing repeats, HDR comparisons, GPU parity and browser logs. Linear
RGBA outputs remain reproducible local build artifacts under `out/pt/`. `tools/compare_pt.py`
compares those outputs, not tone-mapped PNGs.
