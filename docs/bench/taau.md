# TAA upscaling (render scale): quality, cost and method

A render scale below 1 draws the opaque pass, its depth prepass, Hi-Z and occlusion culling and
GTAO at that share of the output's width and height, and TAA reconstructs the output
(`AMORIS_RENDER_SCALE`, `Renderer::set_render_scale`, the viewport's `?scale=`; charter 4.4,
Pioneer 2026-10-15). Design: [TAA and GTAO](../spec/taa-gtao.md) 4.1. Related: the TAA and GTAO
measurements ([taa-gtao.md](taa-gtao.md)), the UE 5.7 comparison on branch `explore/ue5`
(`docs/bench/ue5-dense.md` there).

Status: the cost (section 3, native and Chrome) is a **quiet re-measurement**, taken on
2026-10-15 from 14:01 to 14:46 at commit c8128f1c (after review, so with the output-pixel filtering
of 3cbf3d25), when no other agent was working on the machine: every invocation held the
machine-wide GPU lock, the CPU load before and after each was 3 to 7%, the RTX 5060 started each
below 70 C (52 to 69 C), and the only other process on a GPU engine was the Claude desktop app (3
to 6%). It replaces the provisional cost measured the same morning while five other agents built
and tested on the machine; section 6 says what changed. The quality numbers (section 2) were taken
after review on the shared machine and again on the idle one (14:47 to 14:57): every value aa_eval
writes but its own run time, and every image, came out the same. Section 2 was run again on
2026-10-16, when the wider clip box where nothing moved became the upscaling's default (the owner's
decision, charter 4.4). The cost was not measured again: the change sets a value the pass already
read, and where nothing moved the pass now takes a branch of three assignments it skipped before.

## 1. Method

- Machine: R1, ASUS ROG Zephyrus G14: Ryzen 9 270 (16 threads), 15 GB, Windows 11 Pro 10.0.26200,
  RTX 5060 Laptop 8 GB (driver 32.0.16.1714, NVIDIA 617.14) and Radeon 780M (32.0.13062.3005); AC
  power, Balanced plan. Release build (wgpu 30.0.1). `tools/machine_conditions.py` recorded each
  run's start and end (`out/taau/`, not committed).
- **Scale 1 is unchanged.** `tools/build_compare.py --before <master b7be59a2's examples>` captured
  every case (many_cubes with shadows and dense, moving cubes, the splat garden, gi-room, pt-lab,
  the skinned crowd) under MSAA, MSAA with GTAO, TAA and TAA with GTAO's normal target on the RTX
  5060 and the Radeon 780M with Direct3D 12 and Vulkan: 120 of 120 pairs are equal pixel for pixel,
  before review (bffe488c), after it (2d6e1d83) and on the idle machine at c8128f1c.
- **Quality**: `aa_eval OUT --only upscale-tune --scale 0.5,0.67,0.77,1.0` at 960x540 on the RTX
  5060 with Vulkan (`--only upscale` on the Radeon 780M with Direct3D 12): the check scene of
  [taa-gtao.md](taa-gtao.md) against the output-size reference (the mean of 256 jittered
  one-sample frames, a box-filtered supersampled image), RGB PSNR after exposure and AgX. The still
  scene's convergence over 128 frames; its converged value and its regions as the mean PSNR over
  the last full jitter cycle up to frame 128 (16 frames at 1 and 0.77, 18 at 0.67, 32 at 0.5: a
  single frame lands anywhere in its cycle, frame 128 about 0.2 dB above the mean at 0.67 and 0.4
  at 1); the panning and still-camera motion checks (whole image / the moving cube's region /
  frame-to-frame change against the reference's) at frames 30, 60 and 90; the upscaling's options
  varied one at a time.
- **Cost**: `tools/aa_bench.py` (section 5 has the commands): the check scene through aa_eval's
  bench (150 frames after 30 of warm-up, each submitted and waited for; the median frame's
  timestamped GPU time and the 10th percentile of each pass) and many_cubes' dense grid (the UE5
  comparison's scene: 1.6M cubes, 482,000 in the frustum, 443 visible; `--headless-bench 150
  --settle 5`, the mean of each pass), at 1600x900 and 2560x1440, on both GPUs and both APIs, two
  rounds (the dense grid in reverse order in the second), medians over the rounds. The dense grid
  pins the depth prepass on (`--prepass on`): its auto mode decided differently from one
  configuration to the next, and with occlusion culling off a frame with the prepass and one
  without differ by up to 2.4 times. GTAO off unless named. Bloom and the display transform are
  outside the timestamped total (docs/bench/taa-gtao.md). Both workloads wait 5 s after each
  configuration's renderer is created (`--settle 5`): it compiles its pipelines on every core, a
  burst that lowers R1's GPU clocks for a few seconds (AGENTS.md). The provisional check-scene runs
  did not wait (section 6).

## 2. Quality

Since 2026-10-16 the upscaling resolve widens its clip box where nothing moved (3 standard
deviations, not intersected with the neighbourhood's extremes; `TaaTuning::upscale_still_gamma`),
the owner's decision (charter 4.4); TAA at the output's size keeps it off. This section is the run
of 2026-10-16 (05:52 to 06:04, branch `explore/decisions`, in the bench lock) with it on. aa_eval is
deterministic: the sweep's row without the wider box gave every value of the 2026-10-15 run again,
digit for digit, and the panning camera, under which every pixel moves, is unchanged.

RTX 5060, Vulkan, 960x540. Still camera and scene, PSNR in dB after each frame count, and the mean
over the last jitter cycle (its length in frames):

| Mode | Opaque pass | 1 | 4 | 16 | 64 | 128 | Last cycle |
|---|---|---:|---:|---:|---:|---:|---:|
| msaa | 960x540 | 34.34 | 34.34 | 34.34 | 34.34 | 34.34 | 34.34 |
| taa | 960x540 | 29.70 | 40.21 | 39.12 | 39.12 | 39.17 | 38.77 (16) |
| taa x0.77 | 739x416 | 28.56 | 37.27 | 39.83 | 39.53 | 39.54 | 39.97 (16) |
| taa x0.67 | 643x362 | 26.80 | 35.13 | 38.36 | 38.71 | 39.42 | 38.97 (18) |
| taa x0.5 | 480x270 | 24.18 | 29.30 | 35.92 | 36.49 | 36.50 | 36.27 (32) |
| msaa+taa x0.67 | 643x362, 4 samples | 26.97 | 35.24 | 38.64 | 39.07 | 39.72 | 39.28 (18) |

Converged, the still image at 0.77 is 5.6 dB closer to the supersampled reference than MSAA at the
output's size, at 0.67 4.6 dB, at 0.5 1.9 dB. Against TAA at the output's size, the mode it would
replace if an integrated GPU upscaled (the owner kept those at scale 1.0 on 2026-10-16), it gains
1.2 dB at 0.77, ties at 0.67 (+0.2) and loses 2.5 dB at 0.5; that compares the defaults, since TAA
at the output's size keeps the tighter box where nothing moved. Without the wider box (2026-10-15)
the upscaled still image lost 2.2 (0.77), 3.7 (0.67) and 6.9 dB (0.5) to it: the wider box gains
3.4, 3.9 and 4.4 dB. Four samples at the smaller size add 0.3 dB at 0.67.

Regions, mean over the last jitter cycle (dB):

| Region | msaa | taa | taa x0.77 | taa x0.67 | taa x0.5 |
|---|---:|---:|---:|---:|---:|
| the alpha-tested chain link | 26.48 | 33.80 | 33.85 | 32.27 | 30.47 |
| the fence and wires | 46.36 | 38.82 | 38.25 | 36.59 | 34.48 |
| the chrome spheres | 34.71 | 41.44 | 41.05 | 38.01 | 35.36 |

At 0.67 the regions lose 1.5 (chain link), 2.2 (fence) and 3.4 dB (spheres) against TAA at the
output's size (2.3, 2.5 and 4.7 without the wider box); against MSAA the chain link and the spheres
gain 5.8 and 3.3 dB and the fence loses 9.8. The wider box gains 0.55 to 0.76 dB on the chain link,
0.22 to 0.24 on the fence and 0.57 to 1.54 on the spheres. Before review the specular anti-aliasing
and the sea filtered for the opaque pass's larger pixel (only material textures took the output's
mip level): at frame 128 the spheres were then 4.4 to 8.4 dB lower (31.39 against 37.49 dB at 0.67,
without the wider box), the fence and the chain link 1.6 to 4.5 dB, the whole image 0.25 to 0.73 dB
(`out/taau/eval5/` against `out/taau/eval7/`).

Motion, frame 60 (whole image / moving cube's region / frame-to-frame change, dB; frames 30 and 90
in `out/decisions/quality/`):

| Mode | Panning camera | Still camera, moving cube |
|---|---|---|
| msaa | 35.27 / 35.97 / 37.51 | 34.98 / 35.17 / 50.84 |
| taa | 34.61 / 34.34 / 35.96 | 38.11 / 35.57 / 41.65 |
| taa x0.77 | 32.52 / 31.87 / 34.52 | 36.46 / 31.24 / 40.21 |
| taa x0.67 | 31.79 / 31.45 / 33.88 | 35.71 / 30.92 / 39.50 |
| taa x0.5 | 29.81 / 29.23 / 31.81 | 33.98 / 29.72 / 38.55 |

Under a panning camera upscaling loses to MSAA at the output's size by 2.3 to 6.1 dB over frames
30 to 90, and to TAA at the output's size by 1.6 to 5.0 dB; its frame-to-frame changes are 1.4 to
4.8 dB further from the reference's than TAA's at the output's size. The moving cube's region
drops about as much as the whole image does (at frame 60, against TAA at the output's size, 2.5,
2.9 and 5.1 dB at 0.77, 0.67 and 0.5, against 2.1, 2.8 and 4.8 for the image), so upscaling adds
no ghosting of its own there. Under the still camera the wider box raises the whole image by 0.8 to
2.8 dB over frames 30 to 90 and lowers the moving cube's region, which holds its shadow too, by
0.2 to 1.5 dB at 0.77 and 0.67 (at 0.5 by 0.7 at frame 30, then level and 0.4 dB higher): where
the cube and its shadow have just left, the wider box keeps what was there a little longer. There
the region is 4.3, 4.7 and 5.9 dB below TAA at the output's size at frame 60 (3.2, 3.9 and 5.9
without the wider box).

On the Radeon 780M with Direct3D 12 (`out/decisions/quality780/`) the order is the same, the
absolute values lower as in [taa-gtao.md](taa-gtao.md): still, last cycle, msaa 32.19, taa 37.43,
x0.77 38.49, x0.67 37.42, x0.5 34.98 (1.1 dB above, level with and 2.5 dB below TAA at the
output's size; 2.3, 3.8 and 6.6 dB below it without the wider box); panning at frame 60, msaa
33.01, taa 33.68, x0.77 31.69, x0.67 31.00, x0.5 29.19 (2.0, 2.7 and 4.5 below); still camera,
moving cube's region at frame 60, x0.77 31.09, x0.67 30.75, x0.5 29.52 (0.9 and 0.5 dB lower and
0.3 higher than without the wider box); at 0.67 the regions lose 1.6 (chain link), 2.2 (fence)
and 2.8 dB (spheres) against TAA at the output's size.

**Options, one at a time** (scale 0.67; still: the last cycle's mean; panning and still-camera rows
at frame 60 as above):

| Variant | Still | Panning camera | Still camera, moving cube |
|---|---:|---|---|
| default (kernel 4, box 1.5 sigmas and 3 where nothing moved, weighted share, filtering for the output's pixel) | 38.97 | 31.79 / 31.45 / 33.88 | 35.71 / 30.92 / 39.50 |
| no wider box where nothing moved (`upscale_still_gamma` 0, the default until 2026-10-16) | 35.05 | 31.79 / 31.45 / 33.88 | 34.04 / 31.72 / 39.40 |
| box 1.25 sigmas (TAA's at the output's size) | 38.97 | 31.52 / 31.11 / 33.69 | 35.76 / 30.93 / 39.55 |
| box 2 sigmas | 38.97 | 31.80 / 31.39 / 33.85 | 35.66 / 30.90 / 39.45 |
| kernel 2.29 (close to Blackman-Harris) | 35.96 | 31.20 / 30.87 / 33.77 | 34.11 / 30.24 / 39.90 |
| kernel 6 | 38.96 | 31.93 / 31.60 / 33.80 | 35.76 / 30.99 / 39.24 |
| kernel 8 | 38.24 | 31.90 / 31.54 / 33.68 | 35.44 / 30.90 / 39.08 |
| share not weighted by the samples | 35.66 | 31.24 / 30.89 / 33.65 | 33.99 / 30.26 / 39.67 |
| filtering for the opaque pass's pixel (no texture mip bias, unscaled specular anti-aliasing and sea) | 30.94 | 28.64 / 29.67 / 32.42 | 30.68 / 29.57 / 39.89 |

At 0.5 and 0.77 the same options move the still image by: no wider box, -4.4 and -3.4 dB; kernel
2.29, -1.9 and -3.6; kernel 6, +0.1 and +0.1; kernel 8, -0.4 and -0.7; the share not weighted,
-5.3 and -2.2; filtering for the opaque pass's pixel, -9.5 and -6.5. The box's size no longer moves
the still image at all, every pixel of which takes the wider box; it acts where something moved,
where 1.25 sigmas lose 0.1 to 0.5 dB under the panning camera at frame 60 and 2 sigmas move it by
-0.1 to +0.2 dB.

The wider box where nothing moved gains the most on still views and costs up to 1.5 dB around the
moving cube and its shadow under a still camera at 0.67 and 0.77 (between -0.7 and +0.4 dB at
0.5): the lag it has at the output's size, where it stays off for that reason. The owner turned it
on for upscaling on 2026-10-16 (charter 4.4), where it recovers most of what the coarser
neighbourhood clips. The other defaults were chosen on 2026-10-15 without it
(`out/taau/quiet/quality/`): filtering for the output's pixel and the weighted share were worth the
most; the kernel `k` 4 (a standard deviation of 0.35 output pixels) was 0.7 to 1.9 dB better than
2.29 for up to 0.6 dB of still-camera stability, and sharper kernels gained at most 0.3 dB while
losing 0.1 to 0.5 dB of stability. A box of 1.5 sigmas gained 0.9 to 1.2 dB over 1.25 on the still
image, which also lost up to 1 dB under motion; 2 sigmas gained another 0.3 to 0.6 dB on the still
image, but lost 0.16 to 0.31 dB of still-camera stability at 0.67 and 0.77 (up to 0.18 at 0.5) and
0.05 to 0.28 dB around the moving cube there, so 1.5 stayed. With the wider box on they still hold
(kernel 6 within 0.15 dB of 4, kernel 8 0.4 to 0.7 dB lower). The tuning was chosen on this scene
and renderer, against a box-filtered reference.

## 3. Cost

Cells: the frame's median timestamped GPU time in ms (opaque passes, that is the depth prepass
and the one or two opaque passes / the TAA pass, and for the check scene with GTAO / GTAO). The
check scene's pass times are 10th percentiles, the dense grid's means. Quiet re-measurement of
2026-10-15 (Status). `*` marks a cell whose two rounds differ by more than 10% of their median:
only the RTX 5060's with Vulkan, on the dense grid without occlusion culling (up to 21%) and in one
disturbed check-scene run (section 6 names the rows to distrust); every other cell's rounds agree
within 9%.

**The check scene** (light geometry, shading-bound):

1600x900:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 0.32 (0.23 / 0.00) | 0.29 (0.21 / 0.00) | 2.74 (1.69 / 0.00) | 3.56 (2.67 / 0.00) |
| taa x1 | 0.47 (0.19 / 0.19) | 0.38 (0.16 / 0.14) | 2.65 (0.93 / 0.56) | 2.11 (0.67 / 0.47) |
| taa x0.77 | 0.40 (0.12 / 0.19) | 0.37 (0.10 / 0.20) | 2.29 (0.60 / 0.64) | 1.94 (0.44 / 0.60) |
| taa x0.67 | 0.37 (0.09 / 0.19) | 0.35 (0.08 / 0.19) | 2.20 (0.49 / 0.64) | 1.86 (0.34 / 0.57) |
| taa x0.5 | 0.34 (0.06 / 0.19) | 0.42* (0.05 / 0.20) | 2.01 (0.32 / 0.64) | 1.73 (0.22 / 0.56) |
| taa+gtao x1 | 0.60 (0.19 / 0.19 / 0.13) | 0.51 (0.16 / 0.14 / 0.13) | 3.20 (1.01 / 0.58 / 0.41) | 2.58 (0.75 / 0.47 / 0.37) |
| taa+gtao x0.67 | 0.44 (0.10 / 0.19 / 0.06) | 0.42 (0.08 / 0.19 / 0.07) | 2.48 (0.53 / 0.65 / 0.18) | 2.09 (0.39 / 0.57 / 0.17) |

2560x1440:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 0.66 (0.57 / 0.00) | 0.60 (0.52 / 0.00) | 5.14 (3.94 / 0.00) | 7.64 (6.61 / 0.00) |
| taa x1 | 1.03 (0.46 / 0.48) | 0.83 (0.38 / 0.37) | 4.87 (2.09 / 1.35) | 3.84 (1.56 / 1.20) |
| taa x0.77 | 0.86 (0.29 / 0.48) | 0.83 (0.24 / 0.50) | 4.19 (1.34 / 1.57) | 3.36 (0.98 / 1.39) |
| taa x0.67 | 0.79 (0.22 / 0.48) | 0.77 (0.19 / 0.50) | 4.03 (1.06 / 1.69) | 3.19 (0.79 / 1.37) |
| taa x0.5 | 0.70 (0.13 / 0.48) | 0.69 (0.11 / 0.49) | 3.57 (0.66 / 1.56) | 2.79 (0.48 / 1.37) |
| taa+gtao x1 | 1.40 (0.47 / 0.48 / 0.36) | 1.21 (0.39 / 0.37 / 0.35) | 5.68 (2.25 / 1.32 / 1.04) | 5.07 (1.77 / 1.21 / 0.93) |
| taa+gtao x0.67 | 0.94 (0.22 / 0.47 / 0.15) | 0.92 (0.19 / 0.49 / 0.15) | 4.70 (1.16 / 1.57 / 0.47) | 3.74 (0.88 / 1.39 / 0.43) |

**The dense grid, occlusion culling on** (`auto`, the default; 443 cubes visible):

1600x900:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 0.85 (0.25 / 0.00) | 0.82 (0.24 / 0.00) | 4.27 (1.56 / 0.00) | 5.36 (2.90 / 0.00) |
| taa x1 | 0.94 (0.19 / 0.20) | 0.86 (0.17 / 0.15) | 3.68 (0.73 / 0.68) | 3.48 (0.81 / 0.58) |
| taa x0.77 | 0.87 (0.12 / 0.20) | 0.86 (0.11 / 0.21) | 3.57 (0.46 / 0.81) | 3.38 (0.42 / 0.72) |
| taa x0.67 | 0.86 (0.10 / 0.20) | 0.83 (0.09 / 0.20) | 3.44 (0.38 / 0.82) | 3.16 (0.35 / 0.68) |
| taa x0.5 | 0.81 (0.07 / 0.20) | 0.79 (0.06 / 0.21) | 3.26 (0.23 / 0.83) | 2.94 (0.34 / 0.65) |

2560x1440:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 1.39 (0.61 / 0.00) | 1.35 (0.58 / 0.00) | 7.30 (3.77 / 0.00) | 10.08 (7.00 / 0.00) |
| taa x1 | 1.55 (0.46 / 0.52) | 1.38 (0.41 / 0.39) | 5.66 (1.72 / 1.57) | 5.34 (1.70 / 1.33) |
| taa x0.77 | 1.40 (0.28 / 0.51) | 1.36 (0.26 / 0.54) | 5.17 (1.06 / 1.87) | 4.80 (1.10 / 1.60) |
| taa x0.67 | 1.32 (0.22 / 0.51) | 1.29 (0.20 / 0.54) | 5.29 (0.85 / 2.09) | 4.49 (0.98 / 1.56) |
| taa x0.5 | 1.27 (0.13 / 0.50) | 1.20 (0.12 / 0.54) | 4.63 (0.47 / 1.94) | 4.15 (0.39 / 1.58) |

**The dense grid, occlusion culling off** (482,000 cubes in the frustum, geometry-bound):

1600x900:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 10.17 (9.81 / 0.00) | 9.75 (9.38 / 0.00) | 39.88 (38.13 / 0.00) | 39.58 (37.92 / 0.00) |
| taa x1 | 7.56 (7.01 / 0.20) | 7.40 (6.90 / 0.15) | 16.43 (14.28 / 0.61) | 15.56 (13.68 / 0.52) |
| taa x0.77 | 7.20 (6.66 / 0.20) | 6.51* (5.96 / 0.20) | 13.46 (11.21 / 0.74) | 12.65 (10.75 / 0.65) |
| taa x0.67 | 7.08 (6.53 / 0.20) | 6.40* (5.85 / 0.20) | 12.39 (10.30 / 0.75) | 11.65 (9.67 / 0.66) |
| taa x0.5 | 6.92 (6.37 / 0.20) | 6.82 (6.27 / 0.20) | 11.11 (9.02 / 0.79) | 10.21 (8.44 / 0.65) |

2560x1440:

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| msaa x1 | 16.16 (15.68 / 0.00) | 16.23 (15.75 / 0.00) | 86.02 (84.24 / 0.00) | 87.39 (85.75 / 0.00) |
| taa x1 | 9.36 (8.50 / 0.50) | 8.93* (8.20 / 0.38) | 30.50 (27.28 / 1.50) | 29.59 (26.63 / 1.31) |
| taa x0.77 | 8.35 (7.50 / 0.50) | 8.27 (7.39 / 0.52) | 22.26 (18.70 / 1.82) | 21.16 (18.45 / 1.56) |
| taa x0.67 | 8.02 (7.16 / 0.50) | 7.38* (6.51 / 0.51) | 18.89 (15.46 / 1.84) | 17.90 (15.40 / 1.54) |
| taa x0.5 | 7.54 (6.70 / 0.50) | 7.08* (6.22 / 0.51) | 14.87 (11.51 / 1.88) | 13.88 (10.86 / 1.58) |

**Fixed passes and wall time** (the same runs; cells: the passes that do not depend on the render
scale / the wall time per frame, submitted and waited for including encoding, median, ms):

The check scene (cull and shadows):

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| 1600x900, msaa x1 | 0.08 / 1.16 | 0.07 / 0.78 | 0.87 / 4.26 | 0.80 / 5.05 |
| 1600x900, taa x1 | 0.08 / 1.45 | 0.07 / 1.00 | 0.87 / 4.24 | 0.79 / 3.54 |
| 1600x900, taa x0.77 | 0.08 / 1.40 | 0.07 / 0.96 | 0.88 / 3.96 | 0.79 / 3.41 |
| 1600x900, taa x0.67 | 0.08 / 1.24 | 0.07 / 0.94 | 0.88 / 3.80 | 0.79 / 3.28 |
| 1600x900, taa x0.5 | 0.08 / 1.20 | 0.07 / 1.12 | 0.88 / 3.61 | 0.80 / 3.16 |
| 1600x900, taa+gtao x1 | 0.08 / 1.67 | 0.07 / 1.18 | 0.87 / 4.96 | 0.80 / 4.33 |
| 1600x900, taa+gtao x0.67 | 0.08 / 1.44 | 0.07 / 1.25 | 0.88 / 4.18 | 0.79 / 3.66 |
| 2560x1440, msaa x1 | 0.09 / 1.66 | 0.07 / 1.45 | 0.87 / 7.57 | 0.79 / 10.12 |
| 2560x1440, taa x1 | 0.08 / 2.24 | 0.07 / 1.79 | 0.87 / 7.73 | 0.80 / 6.35 |
| 2560x1440, taa x0.77 | 0.08 / 2.05 | 0.07 / 1.82 | 0.88 / 6.86 | 0.80 / 5.77 |
| 2560x1440, taa x0.67 | 0.08 / 1.98 | 0.07 / 1.73 | 0.88 / 6.57 | 0.80 / 5.58 |
| 2560x1440, taa x0.5 | 0.08 / 1.93 | 0.07 / 1.65 | 0.87 / 6.28 | 0.80 / 5.24 |
| 2560x1440, taa+gtao x1 | 0.09 / 2.81 | 0.08 / 2.34 | 0.87 / 8.46 | 0.80 / 7.87 |
| 2560x1440, taa+gtao x0.67 | 0.08 / 2.19 | 0.07 / 2.00 | 0.88 / 7.64 | 0.79 / 6.44 |

The dense grid with occlusion culling (cull, Hi-Z and the late cull):

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| 1600x900, msaa x1 | 0.59 / 1.74 | 0.58 / 1.37 | 2.68 / 5.92 | 2.64 / 6.96 |
| 1600x900, taa x1 | 0.56 / 2.09 | 0.54 / 1.53 | 2.28 / 5.57 | 2.23 / 5.06 |
| 1600x900, taa x0.77 | 0.55 / 2.01 | 0.54 / 1.52 | 2.26 / 5.56 | 2.23 / 5.08 |
| 1600x900, taa x0.67 | 0.55 / 1.98 | 0.53 / 1.50 | 2.23 / 5.45 | 2.17 / 4.69 |
| 1600x900, taa x0.5 | 0.55 / 1.91 | 0.52 / 1.45 | 2.19 / 5.11 | 2.12 / 4.30 |
| 2560x1440, msaa x1 | 0.77 / 2.47 | 0.76 / 2.08 | 3.53 / 9.83 | 3.25 / 12.51 |
| 2560x1440, taa x1 | 0.58 / 2.84 | 0.57 / 2.25 | 2.35 / 8.32 | 2.36 / 8.06 |
| 2560x1440, taa x0.77 | 0.60 / 2.73 | 0.57 / 2.24 | 2.29 / 8.13 | 2.27 / 7.12 |
| 2560x1440, taa x0.67 | 0.59 / 2.65 | 0.56 / 2.15 | 2.33 / 8.20 | 2.22 / 6.92 |
| 2560x1440, taa x0.5 | 0.62 / 2.57 | 0.54 / 2.04 | 2.23 / 7.39 | 2.18 / 6.55 |

The dense grid without occlusion culling (cull):

| Mode, scale | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---:|---:|---:|---:|
| 1600x900, msaa x1 | 0.37 / 11.40 | 0.36 / 10.48 | 1.74 / 42.56 | 1.73 / 41.34 |
| 1600x900, taa x1 | 0.36 / 9.03 | 0.35 / 8.23 | 1.77 / 18.67 | 1.72 / 17.44 |
| 1600x900, taa x0.77 | 0.35 / 8.56 | 0.35 / 7.30 | 1.75 / 15.52 | 1.71 / 14.41 |
| 1600x900, taa x0.67 | 0.35 / 8.43 | 0.35 / 7.18 | 1.77 / 14.59 | 1.71 / 13.46 |
| 1600x900, taa x0.5 | 0.35 / 8.39 | 0.35 / 7.59 | 1.75 / 13.12 | 1.72 / 12.01 |
| 2560x1440, msaa x1 | 0.48 / 17.59 | 0.48 / 17.26 | 1.86 / 89.53 | 1.71 / 89.92 |
| 2560x1440, taa x1 | 0.36 / 11.08 | 0.36 / 9.93 | 1.74 / 33.54 | 1.72 / 32.31 |
| 2560x1440, taa x0.77 | 0.36 / 10.00 | 0.36 / 9.26 | 1.77 / 25.16 | 1.72 / 23.96 |
| 2560x1440, taa x0.67 | 0.36 / 9.64 | 0.36 / 8.34 | 1.76 / 21.76 | 1.73 / 20.51 |
| 2560x1440, taa x0.5 | 0.35 / 9.16 | 0.35 / 8.01 | 1.76 / 17.60 | 1.73 / 16.46 |

**Chrome** (155, WebGPU through Direct3D 12, the RTX 5060 with `--force_high_performance_gpu`; the
viewport's check scene, canvas 1574x744; medians of three rounds of 600 timestamped frames; a
viewport built locally from c8128f1c and not committed; quiet, 14:44 to 14:46). The opaque pass is
the one every frame draws (`opaque+sky`): the passes the auto modes' probes draw on a few frames
(the depth prepass, the early opaque pass, Hi-Z) report medians over those frames only, so they
are left out.

| Mode, scale | GPU frame | Opaque pass | TAA pass |
|---|---:|---:|---:|
| msaa x1 | 0.287 | 0.194 | — |
| taa x1 | 0.441 | 0.178 | 0.166 |
| taa x0.67 | 0.413 | 0.099 | 0.216 |
| taa x0.5 | 0.381 | 0.067 | 0.216 |

Every pass took 10 to 42% longer than in the provisional run on the shared machine (0.234, 0.392,
0.322 and 0.294 ms frames), the passes this branch does not touch too (culling 23 to 42%, the
shadows 10 to 21%), so the difference between the two runs is not the code; its cause was not
measured (the GPU's clocks were not recorded). Within the quiet run the shadow pass takes the same
time in every configuration (0.073 to 0.080 ms), and the upscaling TAA pass costs 30% more than
TAA at the output's size, where the provisional run showed 12% (0.166 against 0.148 ms).

What it buys:

- Where shading bounds them (the check scene, the dense grid with occlusion culling) the opaque
  passes scale with the pixels drawn (45% of them at 0.67): at 2560x1440, 0.67 draws them in 47 to
  49% of their time at the output's size on the RTX 5060 and 50 to 58% on the Radeon 780M; GTAO at
  0.67 takes 43 to 46% of its time. The culling passes do not depend on the scale.
- The TAA pass stays the output's size and costs at least what TAA there does: the same with
  Direct3D 12 on the RTX 5060 (-0.01 to +0.00 ms), 0.05 to 0.06 ms more at 1600x900 and 0.12 to
  0.14 ms more at 2560x1440 with Vulkan (34 to 38%), 0.08 to 0.52 ms more on the 780M with
  Direct3D 12 (14 to 33%) and 0.07 to 0.27 ms more with Vulkan (12 to 28%), over every workload and
  scale here; in Chrome 0.216 against 0.166 ms (30%). On the 780M with Direct3D 12 at 2560x1440 it
  is dearest at 0.67 (the dense grid: 2.09 ms against 1.57 at 1, 1.87 at 0.77 and 1.94 at 0.5; the
  check scene: 1.69 against 1.35, 1.57 and 1.56).
- A light frame therefore saves little: the check scene at 2560x1440 with TAA at 0.67 is 23% (RTX
  5060, Direct3D 12) to 8% (Vulkan) cheaper than TAA at the output's size, and still dearer than
  MSAA there (0.79 against 0.66 ms with Direct3D 12). On the 780M with Vulkan, where MSAA is dear,
  0.67 costs 42% (2560x1440) to 52% (1600x900) of MSAA's frame and 83 to 88% of TAA's at the
  output's size; with Direct3D 12 83% of TAA's at both sizes (81 to 84% within each round). With
  GTAO the saving grows (1.40 to 0.94 ms on the RTX 5060 with Direct3D 12).
- The dense grid with occlusion culling is mostly fixed costs at these sizes (culling and Hi-Z,
  0.5 to 0.8 ms on the RTX 5060 and 2.1 to 3.5 ms on the 780M, and the TAA pass): at 2560x1440, 0.67
  saves 7 to 16% against TAA at the output's size and 0.5 13 to 22%, on both GPUs; at 1600x900
  0.67 saves 3 to 9%.
- Without occlusion culling the dense grid is bound by its 5.8M triangles and their overdraw: on
  the RTX 5060 the opaque passes shrink by only 21% from 1 to 0.5 at 2560x1440 with Direct3D 12
  (the Vulkan row jumps between two levels, section 6), while on the 780M, whose frame is
  fill-bound, the frame falls from 30.5 to 18.9 ms at 0.67 and 14.9 ms at 0.5 with Direct3D 12
  (MSAA at the output's size: 86.0 ms).
- Wall time per frame follows the GPU time on the 780M and moves less on the RTX 5060, whose light
  frames are bound by submission (the check scene at 2560x1440 with Direct3D 12: 2.24 ms at 1,
  1.98 at 0.67, MSAA 1.66).

## 4. Context: UE 5.7 on the same scene

Branch `explore/ue5` measured Unreal Engine 5.7.4 on the dense grid at 2560x1440 on this RTX 5060
(2026-10-10, quiet, `docs/bench/ue5-dense.md` there; a different engine and frame, so context, not
a match). A new project's defaults render 1552x873 (61%) and reconstruct with TSR: TSR alone took
3.55 ms with Direct3D 12 and 3.95 ms with Vulkan of a 16.40 and 21.23 ms frame (Lumen, virtual
shadow maps, the ray-tracing scene). With Nanite at 100% TSR took 5.17 and 6.58 ms of 9.21 and
12.09 ms; with UE's TAA instead the Nanite frame was 4.23 and 5.88 ms. The matched forward
configuration with MSAA and per-instance HZB occlusion drew it in 3.49 and 4.59 ms against our
2.98 and 3.08 ms then (in the window, with the post chain timed; the numbers below are headless,
without it, on a later master).

Here the same grid at 2560x1440 with occlusion culling, TAA upscaling from 0.67 (the measured
scale nearest 61%) costs 1.32 ms with Direct3D 12 and 1.29 ms with Vulkan on the RTX 5060 (from 0.5,
1.27 and 1.20), of which the upscaling resolve is 0.50 to 0.54 ms; UE's TSR pass from 61% alone took
3.55 to 3.95 ms. TSR reconstructs far more carefully (its own history rejection and thin-feature
handling among others), and section 2 shows what the cheaper resolve loses under motion; our
timestamped total also leaves out bloom and the display transform, which UE's includes.

On the RTX 5060, whose default is MSAA at the output's size (1.39 ms with Direct3D 12, 1.35 with
Vulkan), upscaling barely pays on this scene: 0.67 saves 5 and 4%, 0.5 9 and 11%, for 3.5 and 5.5
dB under the panning camera (section 2); the provisional runs had 0.67 level with MSAA with
Direct3D 12 (1.37 against 1.36 ms). On the Radeon 780M the default is TAA at the output's size
(5.66 ms with Direct3D 12, 5.34 with Vulkan here). Against it 0.77 saves 9% with Direct3D 12 and
10% with Vulkan (6 and 11%, and 10 and 10%, within each round), 0.67 7% with Direct3D 12 (5.29 ms;
0 and 13% within the two rounds) and 16% with Vulkan (4.49 ms; 16% in both rounds), 0.5 18 and
22%; for +1.1, 0.0 and -2.5 dB of the still image (2.3, 3.8 and 6.6 dB lost before the wider box
where nothing moved, 2026-10-16), 2.0, 2.7 and 4.5 dB under the panning camera, and at 0.67 1.6 to
2.8 dB in the regions (section 2). With Direct3D 12, whose resolve is dearest at
0.67, 0.67 is no cheaper than 0.77 there (5.29 against 5.17 ms). MSAA at the output's size (7.30
and 10.08 ms on the 780M) is not the mode upscaling would replace there.

## 5. Re-measuring on a quiet machine

From the worktree (or master after the merge), with `cargo build --release -p amoris-render
--example aa_eval --example many_cubes --example splats --example lod_field --example
showcase_bench --example image_diff` done and nothing else running. Each line is one lock hold of
at most about 10 minutes (`python out/locks/bench_lock.py run --label NAME -- ...` on the shared
machine; drop the wrapper on a quiet one). Sections 1 to 3 and 6 report these steps as run on
2026-10-15 at c8128f1c on the idle machine, each wrapped in the lock anyway (step 1: 4 to 6 minutes
an invocation, 42 minutes in all, the RTX 5060 below 70 C before each; raw output in
`out/taau/quiet/`, not committed):

1. Cost, two rounds, every adapter and API (updates section 3; both workloads wait 5 s after each
   renderer is created):

       for r in 0 1; do for c in nvidia:dx12 nvidia:vulkan 780m:dx12 780m:vulkan; do
         python tools/aa_bench.py --adapters ${c%%:*} --backends ${c##*:} --rounds 1 \
           --first-round $r --workloads check,dense --scales 1,0.77,0.67,0.5 \
           --sizes 1600x900,2560x1440 --occlusion off,auto --prepass on --frames 150 \
           --configs msaa:off,taa:off,taa:off:0.77,taa:off:0.67,taa:off:0.5,taa:depth,taa:depth:0.67 \
           --conditions out/taau/quiet/r$r-${c%%:*}-${c##*:} \
           --out out/taau/quiet/r$r-${c%%:*}-${c##*:}.json
       done; done

2. Chrome (updates the browser row of section 3): `tools/build_viewport.sh` (not committed), `python
   -m http.server 8231 -d web`, then `python tools/aa_web_bench.py --url http://127.0.0.1:8231 --gpu
   nvidia --rounds 3 --seconds 8 --size 1600x900 --scales 1,0.67,0.5 --configs
   msaa:off,taa:off,taa:off:0.67,taa:off:0.5 --out out/taau/quiet/web-nvidia.json`; restore
   `web/viewport/pkg` afterwards (`git checkout web/viewport/pkg`).
3. Quality does not depend on the load (section 2): `AMORIS_BACKEND=vulkan AMORIS_ADAPTER=nvidia
   target/release/examples/aa_eval out/taau/quiet/quality --only upscale-tune --scale
   0.5,0.67,0.77,1.0` repeats it (`AMORIS_BACKEND=dx12 AMORIS_ADAPTER=780m ... --only upscale` the
   780M's).
4. Scale 1 against a master build: `python tools/build_compare.py --before <checkout with master's
   release examples> --adapters nvidia,780m --backends dx12,vulkan` (120 pairs, 9 minutes). A master
   from before the 2026-10-16 rename, as `b7be59a2` was, is built from its exported tree after
   `python tools/amoris_rename.py --tree <dir>` (AGENTS.md), or its examples ignore the `AMORIS_*`
   variables the tool sets.
5. The depth prepass below scale 1, for each adapter and API: `AMORIS_AA=msaa+taa
   AMORIS_RENDER_SCALE=0.5 python tools/prepass_compare.py --backend <dx12|vulkan> --adapter
   <nvidia|780m> --cases cubes-sphere,cubes-dense,cubes-shadows,splats,gi-room-static,pt-lab-static
   --scenes ""` (only the ties of section 6 may differ).

## 6. Limits

- Noise (section 3, quiet re-measurement). The provisional rounds, taken while other agents built
  and tested, differed by up to 26% on the 780M and 41% on the RTX 5060; on the idle machine every
  cell's rounds agree within 9% except two kinds. The RTX 5060's Vulkan rows of the dense grid
  without occlusion culling still jump between two levels about 1 to 1.5 ms apart from one run to
  the next (1600x900 at 0.77: 7.05 and 5.97 ms, at 0.67 5.83 and 6.97; 2560x1440 at 0.5: 6.34 and
  7.82; scale 1 at 1600x900 took 7.40 ms in both quiet rounds and 5.78 and 6.10 in the provisional
  ones), so their scales are not ordered (0.5 above 0.67 at 1600x900): that spread is not other
  agents' load, as the provisional section implied, but this GPU and API's own on a geometry-bound
  frame, and its cause is open. Distrust that row. One check-scene run was disturbed (RTX 5060,
  Vulkan, 1600x900, 0.5, the second round: its median frame 0.524 ms, its 10th percentile 0.321),
  which lifts that cell's median to 0.42 ms. Against the quiet runs the provisional check-scene
  frames on the 780M with Direct3D 12 were 14 to 36% slower (they did not wait out the start-up
  burst, and the machine was loaded); for every other adapter, API and workload the median cell
  moved by at most 7%.
- The upscaling resolve costs more than TAA at the output's size with Vulkan, on the Radeon 780M and
  in Chrome (30% more there, on the RTX 5060 through Direct3D 12, where native Direct3D 12 shows no
  difference; section 3). It issues as many loads as TAA there (the opaque pass's 10x10 pixels for
  each group of 8x8 output pixels, as many groups) plus the nine samples' weights; why it costs more
  on those is open (no shader profiler for the 780M here). Removing the loop's divisions and `exp`
  did not change it on the RTX 5060 (bffe488c).
- Upscaling loses to MSAA at the output's size under camera motion on this scene (section 2) and
  keeps TAA's limits: no thin-feature locks, no reactive mask, no history confidence of its own.
- Levels of detail and the shadow cascades are chosen as for the output's size: a geometry-bound
  frame (the dense grid without occlusion culling) saves only its per-pixel share.
- The editor's grid is drawn inside the opaque pass without the jitter, so it keeps the opaque
  pass's pixel (its lines are filtered for it) and is upsampled with the scene.
- With the depth prepass, surfaces that touch tie in depth along their contact line, and the longer
  jitter cycle below 1 puts samples there where scale 1 does not: `tools/prepass_compare.py` at 0.5
  with MSAA and TAA changes 2 pixels of pt-lab (the red wall on the floor) on the RTX 5060 and 3 of
  gi-room (the walls under the ceiling) on the 780M by up to 4 levels, and none once the touching
  box is lifted by 1 cm (docs/spec/prepass.md 4, ties). The idle machine's run at c8128f1c (section
  5, step 5) found the same pixels and nothing else beyond the many_cubes sphere. With the wider box
  where nothing moved (2026-10-16, `AMORIS_AA=msaa+taa` and `taa` at 0.5 and 0.67 on both GPUs and
  APIs) the tool finds the same 2 pixels of pt-lab, 1 of gi-room's 3, and the sphere's 1 to 37
  pixels by up to 17 levels (1 to 52 by up to 15 before; it differs at scale 1 too), nothing else.
