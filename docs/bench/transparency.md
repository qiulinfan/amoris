# Transparency: method and measurements

The transparent stage draws alpha-blended and transmissive instances sorted back to front after the
opaque pass and the sky, and with GTAO on after GTAO (charter 4.4, Pioneer note of 2026-10-16;
[transparency spec](../spec/transparency.md)). Related: [path tracing](path-tracing-nrc.md) (the
reference), [TAA and GTAO](taa-gtao.md), [depth prepass](prepass.md), [occlusion](occlusion.md).

Status: on branch `explore/transparency`. The charter note is dated 2026-10-16 (the wave's date);
the machine's clock read 2026-10-15 for the first round (17:30 to 23:10), 2026-10-15 to 2026-10-16
for the review round (23:35 to 00:40) and 2026-10-16 for the quiet re-measurement (00:57 to 01:42),
and the times below are the machine's. Sections 1, 3 and 4 are deterministic image comparisons,
re-run on the idle machine with the same results (section 4 adds what that found about the glass
sphere's pixels). The timings of sections 2 to 2.3 are quiet re-measurements of the review build
`47114c14` (its binaries, which `4113184a` does not change), 2026-10-16 00:57 to 01:31, with no
other agent's work on the machine (no builds, browsers or benchmarks; this session's Claude app and
the desktop compositor only: CPU load 3.5 to 9.5% at each session's start, mean over five samples;
the RTX 5060 at 52 to 66 C at each session's start and below 70 C before every run). They replace
provisional timings taken while five other agents worked on the machine (2026-10-15 21:12 to
2026-10-16 00:38); each section says what moved. Every GPU run held the machine-wide bench lock,
and each timing session recorded `tools/machine_conditions.py` first (`conditions-*.json` beside
its results). Raw: `out/transparency/quiet/` (`driver.sh` ran the re-measurement in order,
`driver.log` holds its output).

Machine: R1 (Ryzen 9 270, 16 threads, 15 GB; RTX 5060 Laptop 8 GB, driver 32.0.16.1714; Radeon
780M), Windows 11, AC power. Release builds.

## 1. pt-lab against the path tracer

`python tools/transparency_pt.py --build before=out/transparency/before --build after=. --out
out/transparency/pt-lab --backend dx12 --adapter 5060 [--pt-args "--emission-scale 0"] [--gtao
depth]` draws samples/pt-lab at 320x240 with `showcase_bench --hdr --ids` (4x MSAA, occlusion
culling off, time 0, GTAO off unless `--gtao`) from the build before the stage and from the build
under test, and compares each linear image with the path tracer's (`path_trace`, 256 frames of 4
samples, 1,024 paths a pixel, seed 1) over the whole image and over two entities' pixels in the
first build's id image: `BlendCard` (BLEND, alpha 0.45; 1,369 pixels) and `Glass` (inline sphere,
transmission 1, IOR 1.5; 3,612 pixels). The build before the stage is master `3fdc5b83`'s renderer
with the `--hdr` and `--ids` options added to the example only: its source is kept in
`out/transparency/src-before` (`b452f8b4`, which changes only the charter, and `8ad8d1b6`'s
`showcase_bench.rs`; `SOURCE.txt` there), its binaries in `out/transparency/before` (not rebuilt
from the kept source to check). The same path tracer with seed 2 against seed 1 is the reference's
own noise. MAE is the mean absolute error over RGB in linear radiance; RMSE is ruled by the
reference's fireflies (caustic paths through the glass and the mirror), so that the second seed's
RMSE in `Glass` exceeds the forward images'.

The forward renderer is not lit by emissive surfaces, and pt-lab's key light is an emissive panel
(`AreaEmitter`, emission 8, 7, 5), whose light, bounced, makes most of the path-traced image: the
forward image is darker everywhere (mean error -0.11, -0.06, -0.03 over the image). So the
comparison is made twice: as the scene is, and against the path tracer with `--emission-scale 0`
(emissive surfaces neither lit nor seen; the forward image is the same).

The `after` rows are those of `2a59b3d7` (2026-10-15 22:21), reproduced to every printed digit by
the review build `47114c14` (2026-10-16 00:14, and on the idle machine at 01:33 into
`out/transparency/quiet/pt-lab/` with copies of the references; the `before` images to the bit).
The stage's image itself is not reproducible to the bit: from run to run of one build up to 32
pixels of `Glass` and `CutoutCard`'s edge move by one half-float step (section 4):

| Reference | Image | Image MAE | BlendCard RMSE | BlendCard MAE | Glass RMSE | Glass MAE | Glass mean error (RGB) |
|---|---|---|---|---|---|---|---|
| as the scene is | before | 0.0713 | 0.0869 | 0.0649 | 0.598 | 0.1241 | -0.094, -0.048, +0.013 |
| | after | 0.0707 | 0.0850 | 0.0598 | 0.603 | 0.1185 | -0.124, -0.100, -0.065 |
| | path tracer, seed 2 | 0.0136 | 0.0313 | 0.0147 | 0.632 | 0.0484 | -0.009, -0.007, -0.007 |
| no emissive light | before | 0.0766 | 0.0432 | 0.0346 | 0.578 | 0.1115 | +0.040, +0.064, +0.093 |
| | after | 0.0742 | 0.0346 | 0.0248 | 0.571 | 0.0597 | +0.009, +0.012, +0.015 |
| | path tracer, seed 2 | 0.0090 | 0.0277 | 0.0092 | 0.630 | 0.0352 | -0.009, -0.008, -0.008 |

Without the emissive light the stage removes 28% of the blended card's mean absolute error and 46%
of the glass sphere's, and the glass's mean error (a bright opaque ball before) falls from +0.04 to
+0.09 by channel to about +0.01: what is behind now shows through both. With the panel's light the
regions gain 4 to 8% only: the error there is the light the forward renderer does not have (the
panel, the room's interreflections seen through the glass and reflected in it), not the
transparency. What remains in the glass is refraction (the path tracer shows the room inverted
through a solid sphere, the stage shows it undisplaced), the caustic on the floor, and reflections
of the room (the stage reflects the sky's image-based light only). Images:
`out/transparency/pt-lab/` (each build's `.f32` and preview, the references, `summary*.json`).

### 1.1 With GTAO

The same with `--gtao depth` (2026-10-16 00:15, every figure reproduced on the idle machine at
01:33; `summary-gtao-depth.json`,
`summary-emission-scale-0-gtao-depth.json`), adding the review's predecessor `05716664` (`prefix`,
the stage drawn before GTAO, its binaries kept in `out/transparency/build-05716664`):

| Reference | Image | Image MAE | BlendCard RMSE | BlendCard MAE | Glass MAE |
|---|---|---|---|---|---|
| as the scene is | before | 0.0728 | 0.0879 | 0.0660 | 0.1257 |
| | prefix (GTAO after the stage) | 0.0717 | 0.0855 | 0.0602 | 0.1186 |
| | after (GTAO before the stage) | 0.0716 | 0.0854 | 0.0597 | 0.1186 |
| no emissive light | before | 0.0763 | 0.0435 | 0.0351 | 0.1086 |
| | prefix | 0.0738 | 0.0302 | 0.0214 | 0.0584 |
| | after | 0.0738 | 0.0298 | 0.0209 | 0.0583 |

pt-lab's blended card and glass stand mostly in front of open room, where GTAO darkens little, so
drawing them after GTAO moves their regions' error by 0.0005 at most. Where it matters, behind a
pane in front of a crease, the order is checked exactly by
`tests/transparency.rs::gtao_darkens_what_shows_through_a_pane_by_its_transmittance_only`: over
6,339 to 6,522 pixels of a pane of coverage 0.5 in front of a box's foot, the pane's pixels lose
1.0013 to 1.0014 times half of what GTAO takes from the same pixels without the pane (worst pixel
2.4e-4 off; 4x MSAA and one sample, occlusion culling off and on, RTX 5060 Direct3D 12). With GTAO
applied after the stage, the review measured 0.69 of the occlusion passing and the pane's own light
darkened by 3.8%; putting that order back under the test, with the stage no longer writing a share,
gives 1.48: the check fails either way.

## 2. The cost of 1,000 transparent instances

`python tools/transparency_bench.py --adapter 5060|780m --backend dx12|vulkan` runs `transparency
bench` (1600x900, 60 warm-up and 300 measured frames, `--settle 5`, the depth prepass and occlusion
culling off so that the kinds differ only by the spheres; the adapter's default anti-aliasing:
4x MSAA with GTAO on the RTX 5060, TAA without GTAO on the 780M) for five kinds of the same 1,000
spheres (a 10 x 10 x 10 lattice, 1.6 m apart, every one in view, in front of a row of opaque cubes
on a floor, under a sun with shadows and the atmosphere): `hidden` (invisible: the scene without
them), `opaque`, `blend` (a look alpha of 0.35), `glass` (transmission 1) and `double` (a
double-sided BLEND material of alpha 0.35), three rounds of the five kinds in turn, one process
each. Per run the p50 of the frames (the pass columns: the mean over the frames, the example's
`passes_ms`); per kind the median over the rounds. GPU: the timestamped passes (`gpu_ms`, the
display transform not included); the opaque pass (`opaque+sky`), in which the stage draws with GTAO
off; the pass after GTAO (`transparent`), in which it draws with GTAO on (the RTX 5060 here); the
shadow cascades. CPU: `Renderer::render`'s encoding time and the stage's own culling, sorting and
list write (`FrameStats::transparent_ms`). The spheres' levels of detail break the sorted order into
30 instanced draws (32 orbiting). Raw: `out/transparency/quiet/bench/` (with each session's
`conditions-*.json`; the provisional sessions' in `out/transparency/review/bench/`).

Quiet re-measurement, review build `47114c14`, 2026-10-16 00:57 to 01:14 (CPU load 3.5 to 7.8% at
the sessions' starts, the RTX 5060 at 52 to 63 C), one session per configuration:

| Adapter, backend | Kind | GPU ms | over hidden | opaque pass | transparent pass | shadows | CPU encode ms | over hidden | stage CPU ms |
|---|---|---|---|---|---|---|---|---|---|
| RTX 5060, Direct3D 12 | hidden | 0.404 | | 0.210 | | 0.052 | 0.54 | | |
| | opaque | 0.713 | +0.309 | 0.429 | | 0.154 | 0.68 | +0.14 | |
| | blend | 0.752 | +0.348 | 0.180 | 0.377 | 0.054 | 0.80 | +0.26 | 0.076 |
| | glass | 0.757 | +0.353 | 0.179 | 0.381 | 0.053 | 0.80 | +0.26 | 0.075 |
| | double | 1.093 | +0.689 | 0.180 | 0.722 | 0.053 | 0.82 | +0.28 | 0.082 |
| RTX 5060, Vulkan | hidden | 0.363 | | 0.177 | | 0.041 | 0.32 | | |
| | opaque | 0.606 | +0.243 | 0.376 | | 0.154 | 0.33 | +0.01 | |
| | blend | 0.609 | +0.246 | 0.165 | 0.312 | 0.049 | 0.42 | +0.10 | 0.077 |
| | glass | 0.612 | +0.249 | 0.150 | 0.295 | 0.045 | 0.42 | +0.10 | 0.077 |
| | double | 0.859 | +0.496 | 0.137 | 0.528 | 0.042 | 0.43 | +0.12 | 0.080 |
| Radeon 780M, Direct3D 12 | hidden | 2.102 | | 0.689 | | 0.749 | 0.77 | | |
| | opaque | 2.942 | +0.840 | 1.190 | | 1.149 | 0.87 | +0.10 | |
| | blend | 2.877 | +0.775 | 1.460 | | 0.772 | 0.99 | +0.23 | 0.110 |
| | glass | 2.810 | +0.708 | 1.455 | | 0.774 | 0.98 | +0.22 | 0.105 |
| | double | 3.583 | +1.481 | 2.274 | | 0.762 | 1.03 | +0.27 | 0.107 |
| Radeon 780M, Vulkan | hidden | 1.815 | | 0.539 | | 0.700 | 0.58 | | |
| | opaque | 2.531 | +0.716 | 0.894 | | 1.084 | 0.66 | +0.08 | |
| | blend | 2.351 | +0.536 | 1.050 | | 0.713 | 0.79 | +0.21 | 0.105 |
| | glass | 2.419 | +0.604 | 1.102 | | 0.706 | 0.79 | +0.21 | 0.107 |
| | double | 2.799 | +0.984 | 1.576 | | 0.689 | 0.76 | +0.18 | 0.099 |

Orbiting (`--extra=--orbit`: the camera turns 0.01 radian a frame about the lattice, so the order
and the list written change every frame; `summary-*-orbit.json`, the same sessions):

| Adapter, backend | Kind | GPU over hidden | CPU encode over hidden | stage CPU ms |
|---|---|---|---|---|
| RTX 5060, Direct3D 12 | blend | +0.321 | +0.30 | 0.106 |
| | double | +0.665 | +0.32 | 0.109 |
| RTX 5060, Vulkan | blend | +0.233 | +0.10 | 0.079 |
| | double | +0.471 | +0.14 | 0.101 |
| Radeon 780M, Direct3D 12 | blend | +0.730 | +0.24 | 0.109 |
| | double | +1.448 | +0.29 | 0.115 |
| Radeon 780M, Vulkan | blend | +0.504 | +0.21 | 0.108 |
| | double | +1.013 | +0.18 | 0.104 |

- **GPU.** Blended or glass, the 1,000 spheres cost 0.25 to 0.35 ms on the RTX 5060 over the scene
  without them, 1.0 to 1.1 times what the same spheres cost opaque (1.13 and 1.14 on Direct3D 12,
  1.01 and 1.02 on Vulkan). Their colour passes cost more than the opaque spheres' (the opaque and
  transparent passes against `hidden`'s opaque pass: 1.6 times on the RTX 5060's Direct3D 12, 1.4
  to 1.6 on the 780M): every covered sample of every sphere is shaded and blended, where opaque ones
  the depth test finds hidden are not shaded. Their shadows cost nothing (within 0.03 ms of
  `hidden`'s), where the opaque spheres' cost 0.10 to 0.11 ms on the RTX 5060 and 0.38 to 0.40 ms
  on the 780M: alpha 0.35 and clear glass never reach the cutoff (section 2.3). So on the 780M the
  transparent spheres cost less than the opaque ones: clearly on Vulkan (blend 0.54 and glass 0.60
  ms against 0.72 ms, every kind's rounds within 1.6%), and on Direct3D 12 by less than the rounds'
  spread (0.78 and 0.71 ms against 0.84 ms; the blend and glass rounds spread 13 and 15%, 2.67 to
  3.05 and 2.79 to 3.20 ms, the others up to 5.5%). The RTX 5060's rounds agree within 1.3%.
  Double-sided, they also shade the back faces: 1.8 to 2.0 times a single-sided sphere's extra
  cost.
- **CPU.** The stage's culling and sorting take 0.08 to 0.11 ms for the 1,000 instances with a
  still camera, where the list is written once; orbiting, where it is written every frame, 0.08 to
  0.12 ms (on the RTX 5060's Vulkan the blend runs read 0.07 to 0.08 or 0.10 ms from process to
  process, in this session and the provisional one). The render call's encoding time is 0.10 to
  0.28 ms longer than the scene's without them (0.01 to 0.14 ms for the same spheres opaque), the
  stage included; orbiting, 0.10 to 0.32 ms.
- **The RTX 5060's Vulkan passes.** In some runs there (any kind, `hidden` included, and the build
  before the review alike) a part of the frames take about 2 ms: p95 1.4 to 2.4 ms against a p50
  of 0.36 to 0.71 ms. The medians do not move, but the pass columns, means over the frames, do, so
  they do not split the total reliably on that configuration (the opaque spheres' opaque pass read
  0.38 ms here and 0.30 ms in the provisional session at the same total, `hidden`'s 0.18 and 0.21
  ms), and its colour-pass ratio is not given. Not investigated.
- **Against the provisional sessions** (review build, 2026-10-16 00:19 to 00:35): the RTX 5060's
  figures agree within 0.01 ms of GPU time and 0.03 ms of encoding. The 780M's moved by up to
  0.34 ms: on Direct3D 12, glass +1.048 to +0.708 ms (its provisional rounds spread 16%), blend
  +0.691 to +0.775, opaque +0.932 to +0.840; on Vulkan every kind 0.03 to 0.12 ms more over a
  `hidden` 0.05 ms slower (blend +0.464 to +0.536, glass +0.482 to +0.604, opaque +0.691 to +0.716,
  double +0.866 to +0.984). One conclusion changed: provisionally, glass on the 780M's Direct3D 12
  cost more than the opaque spheres (1.05 against 0.93 ms); quiet, it costs less, like blend,
  within the rounds' spread. Provisionally, double-sided spheres cost 1.9 to 2.1 times a
  single-sided one's extra cost (quiet 1.8 to 2.0), and orbiting the stage's CPU time read 0.10 to
  0.11 ms (quiet 0.08 to 0.12).

An earlier session (2026-10-15 21:12 to 21:21, CPU load 6 to 11%; the stage drawn before GTAO and
every transparent sphere drawn into the cascades; `out/transparency/bench/`) gave 0.35 to 0.47 ms on
the RTX 5060 and 0.87 to 0.97 ms on the 780M for blend and glass; section 2.3 separates the two
changes.

### 2.1 Double-sided: two-sided index lists, one draw discarding, two draws

A double-sided instance must draw its back faces before its front faces (spec 3). Three forms were
built: two draws per instance, back faces culled then front faces culled, with a pipeline switch
each (`440dd7f8`); one draw per run with two instances per record, nothing culled, each instance
discarding the other side (`b9eea658`); one draw per run from a two-sided index list, the back faces
as reversed triangles through the pipeline that culls back faces (`2a59b3d7`, kept). Each build's
`hidden` and `double` kinds, three rounds each, one build after the other on each configuration:
the lists of the review build (the worktree's), one draw, two draws, and last the lists before the
review (`05716664`, whose renderer is `2a59b3d7`'s), which like the two older builds draw every
transparent sphere into the cascades and, with GTAO, draw the stage before GTAO; the review
build's lists leave the spheres, which never cast, out of the cascades (section 2.3). Quiet
re-measurement, 2026-10-16 01:16 to 01:31 (CPU load 4.7 to 9.5% at the arms' starts, the RTX 5060
at 60 to 65 C; `out/transparency/quiet/ab3-*/`; the older builds' sources exported to
`out/transparency/src-*` with the first round's `transparency` example and bench script, and built
there). Each arm's figures are over its own `hidden` run:

| Adapter, backend | GPU over hidden: lists | lists before the review | one draw | two draws | CPU encode over hidden: lists | lists before the review | one draw | two draws |
|---|---|---|---|---|---|---|---|---|
| RTX 5060, Direct3D 12 | +0.70 ms | +0.83 ms | +1.31 ms | +0.84 ms | +0.24 ms | +0.23 ms | +0.22 ms | +1.31 ms |
| RTX 5060, Vulkan | +0.49 ms | +0.60 ms | +0.89 ms | +0.60 ms | +0.14 ms | +0.12 ms | +0.13 ms | +1.08 ms |
| Radeon 780M, Direct3D 12 | +1.28 ms | +1.49 ms | +3.38 ms | +2.59 ms | +0.19 ms | +0.20 ms | +0.46 ms | +0.97 ms |
| Radeon 780M, Vulkan | +1.09 ms | +1.23 ms | +1.86 ms | +2.25 ms | +0.18 ms | +0.17 ms | +0.20 ms | +1.33 ms |

(30 draws for the lists and one draw, 2,000 for two draws.) Against the lists before the review,
which differ from the other two forms only in how they draw the back faces: two draws cost the GPU
the same on the RTX 5060 (within 0.01 ms on both backends) and 1.1 and 1.0 ms more on the 780M (not
investigated), and the CPU 0.8 to 1.2 ms more; their 2,000 draws and pipeline switches cost more
still in a browser, where every call crosses to the GPU process (not measured). One draw that
discards rasterizes and shades both sides: 0.3 to 1.9 ms more GPU time than the lists (0.29 ms on
the RTX 5060's Vulkan, 1.88 ms on the 780M's Direct3D 12), at the lists' CPU time on the RTX 5060
(within 0.01 ms) and 0.03 and 0.26 ms more on the 780M. The review build's lists cost 0.11 to 0.13
ms less GPU time than the lists before the review on the RTX 5060 and 0.14 to 0.21 ms less on the
780M: their spheres' shadows are within 0.01 ms of `hidden`'s, where the lists before the review
spend 0.10 to 0.21 ms more there. The lists' cost is an index buffer of the stage's own, twice the
indices of each mesh row drawn double-sided, made once on the GPU.

The first round's comparison (2026-10-15 22:32 to 22:43, `2a59b3d7`'s lists;
`out/transparency/ab3-*/`) ran the RTX 5060's Direct3D 12 arms under 12 to 45% CPU load and gave
there +0.92, +1.51 and +0.84 ms of GPU time (lists, one draw, two draws) and on the other
configurations +0.61, +0.90, +0.61 ms (the RTX 5060's Vulkan), +1.77, +4.22, +2.36 ms and +1.23,
+2.16, +2.19 ms (the 780M's Direct3D 12 and Vulkan). The quiet comparison keeps its conclusions and
sharpens them: the lists' 0.08 ms over two draws on the RTX 5060's Direct3D 12 is gone (that round's
arms ran under different loads), the lists' advantage over two draws on the 780M is 1.0 to 1.1 ms
(provisionally 0.6 and 1.0), one draw's extra GPU time 0.3 to 1.9 ms (provisionally 0.3 to 2.5, the
2.45 ms on the 780M's Direct3D 12 now 1.88), and two draws' extra CPU time 0.8 to 1.2 ms
(provisionally 0.6 to 1.3).

### 2.2 The first frame, and start-up

The stage's pipeline was built on the render thread on the first frame with transparent instances
(and again after every change of anti-aliasing or GTAO); it is now built with the opaque pass's
other pipelines, concurrently, at start-up and in that change. Three processes per build, kind and
configuration of `transparency bench --frames 3 --warm 0`, the largest encoding time (the first
frame's), median (quiet re-measurement, 2026-10-16 01:14 to 01:16, CPU load 4.9 to 6.2%, the RTX
5060 at 63 to 66 C; `out/transparency/quiet/hitch/`, `before` the binaries of `05716664`):

| Adapter, backend | First frame, hidden (before, after) | blend, before | blend, after | Renderer::new (after) |
|---|---|---|---|---|
| RTX 5060, Direct3D 12 | 13.4, 12.7 ms | 110.0 ms | 14.1 ms | 224 to 227 ms |
| RTX 5060, Vulkan | 2.8, 3.0 ms | 4.7 ms | 2.7 ms | 53 ms |
| Radeon 780M, Direct3D 12 | 8.2, 8.0 ms | 108.8 ms | 8.5 ms | 252 to 255 ms |
| Radeon 780M, Vulkan | 7.2, 7.8 ms | 9.7 ms | 7.9 ms | 72 to 73 ms |

The first frame with transparent instances no longer stalls (about 100 ms on Direct3D 12 before,
97 and 101 ms over `hidden`'s first frame, where DXC compiles the pipeline). Start-up does not
visibly grow: each process's wall time (device, renderer, three frames) differs between the builds
by less than its own spread (`hidden`: 1,607 against 1,648 ms on the RTX 5060's Direct3D 12, its
runs 1,602 to 1,666 and 1,631 to 1,670 ms; 2,424 against 2,431 ms on the 780M's). The provisional
session (2026-10-16 00:36 to 00:38, CPU load 4 to 5%; `out/transparency/review/hitch/`) agreed:
stalls of 101.7 and 107.5 ms, the other first frames within 1.3 ms of these, `Renderer::new` 209 to
249 ms on Direct3D 12 and 48 to 72 ms on Vulkan (here 224 to 255 and 53 to 73 ms).

### 2.3 Before and after the review

The review changed two costs (spec 3 and 5): transparent instances that can never cast leave the
shadow passes, and with GTAO on the stage draws in a pass of its own after GTAO, the opaque pass
keeping its multisampled colour for it. `05716664`'s `transparency` example (the build before)
against the review build, the same kinds on the RTX 5060 (4x MSAA with GTAO), each session right
after the review build's session of section 2 (quiet re-measurement, 2026-10-16 01:00 and 01:04,
CPU load 4.2 to 4.5%, the RTX 5060 at 59 and 60 C; `out/transparency/quiet/ab-05716664/`):

| Backend | Kind | GPU over hidden: before | after | Colour passes over hidden: before | after | Shadows: before | after |
|---|---|---|---|---|---|---|---|
| Direct3D 12 | blend | +0.461 | +0.348 | +0.344 | +0.347 | 0.165 | 0.054 |
| | glass | +0.472 | +0.353 | +0.356 | +0.350 | 0.166 | 0.053 |
| | double | +0.826 | +0.689 | +0.707 | +0.692 | 0.168 | 0.053 |
| Vulkan | blend | +0.350 | +0.246 | +0.236 | +0.300 | 0.158 | 0.049 |
| | glass | +0.351 | +0.249 | +0.293 | +0.268 | 0.160 | 0.045 |
| | double | +0.602 | +0.496 | +0.484 | +0.488 | 0.155 | 0.042 |

(Colour passes: `opaque+sky` plus `transparent`, less the session's `hidden` `opaque+sky`; GTAO
itself 0.12 to 0.15 ms in every run. Both builds' `hidden` shadows: 0.052 ms on Direct3D 12, 0.041
ms on Vulkan.) Drawing the stage after GTAO costs nothing measurable here: the opaque pass no longer
resolves its colour, the pass after GTAO loads the samples and resolves them, and GTAO multiplies
the samples instead of the resolved image, and the GPU time the review saves is the shadows' saving
to within 0.03 ms (Direct3D 12: 0.113, 0.119 and 0.137 ms saved against 0.111, 0.113 and 0.115 ms of
shadows; Vulkan: 0.104, 0.102 and 0.106 against 0.109, 0.115 and 0.113 ms). The colour passes agree
within 0.02 ms on Direct3D 12; on Vulkan they move by up to 0.06 ms either way, which the pass
means, taken over frames that include section 2's slow frames, do not resolve. Dropping the casters
that cannot cast saves 0.11 ms on the RTX 5060 and about 0.2 ms on the 780M (section 2.1's sessions:
the double-sided spheres' shadows 0.21 ms over `hidden`'s before the review on both backends, within
0.01 ms of them after). The provisional sessions (2026-10-16 00:21 and 00:27;
`out/transparency/review/ab-05716664/`) gave the same GPU figures within 0.01 ms.

## 3. TAA without motion vectors

`transparency ghost --kind opaque|blend|glass --motion object|pan` (960x540;
`tools/transparency_bench.py --ghost` runs all six): vertical stripes of emissive white and
near-black cubes 6 m behind a sphere (2 m across) of one kind, lit by its emission only: `opaque`
(emission 0.6, 0.15, 0.1), `blend` (the same emission at alpha 0.35) or `glass` (transmission 1, no
emission). `object`: the sphere moves 0.1 m a tick across the stripes, the camera still (about 6.4
pixels a frame); `pan`: the sphere still, the camera moving 0.1 m a tick sideways (the stripes,
farther, move less on screen). Drawn with TAA (one sample) and with 4x MSAA on one device through
the same 48 frames at each tick's own pose, GTAO off; then each image's mean absolute difference in
linear luminance from a reference of the last moment held still (the mean of 64 frames jittered
over the pixel, aa_eval's kind of reference), where the sphere is now (8,680 pixels) and in its
trail (4,332 pixels it covered in the last eight frames and does not cover now). MSAA has no history
(its difference is the anti-aliasing floor); the opaque sphere writes motion vectors. The numbers
are images, not timings: identical on the RTX 5060 and the 780M (Direct3D 12) to within 0.0004, and
reproduced exactly by the review build (2026-10-16 00:18; `out/transparency/review/ghost/`) and
again on the idle machine (01:32; `out/transparency/quiet/ghost/`).

| Sphere | Motion | Sphere: TAA | Sphere: MSAA | Trail: TAA | Trail: MSAA | Image: TAA | Image: MSAA |
|---|---|---|---|---|---|---|---|
| opaque | object | 0.0119 | 0.0012 | 0.0236 | 0.0031 | 0.0046 | 0.0016 |
| blend | object | 0.0073 | 0.0032 | 0.0204 | 0.0025 | 0.0046 | 0.0016 |
| glass | object | 0.0135 | 0.0046 | 0.0123 | 0.0026 | 0.0046 | 0.0016 |
| opaque | pan | 0.0024 | 0.0010 | 0.0112 | 0.0060 | 0.0055 | 0.0017 |
| blend | pan | 0.0090 | 0.0015 | 0.0156 | 0.0051 | 0.0056 | 0.0017 |
| glass | pan | 0.0142 | 0.0023 | 0.0115 | 0.0053 | 0.0056 | 0.0017 |

(RTX 5060, Direct3D 12.) Where the object moves, TAA's trail behind the transparent spheres is no
larger in absolute terms than behind the opaque one (0.0204 and 0.0123 against 0.0236), whose motion
vectors do not help where it uncovers the stripes (history rejected by depth, then a few frames to
converge). This does not isolate the motion vectors: the spheres differ in contrast against the
stripes too (the blended one shows its emission at 0.35, the glass none), and against each one's
MSAA floor the trails are 7.6 (opaque), 8.2 (blend) and 4.7 (glass) times it. The missing motion
vectors show where the camera pans: the opaque sphere reprojects by its own depth and is within 2.4
times the MSAA floor, while the transparent ones reproject by the stripes' depth 6 m behind them and
are 6 times the floor (blend 0.0090, glass 0.0142 against 0.0015 and 0.0023): a blur of the stripes
seen through the sphere and of its rim, which the variance clip bounds. Accepted (spec 6); images:
`out/transparency/ghost/ghost-KIND-MOTION.png` (TAA, MSAA, reference side by side).

## 4. Images that must not change

Exact comparisons (deterministic: the machine's load does not enter them), on the review build
`47114c14`, its tree clean (2026-10-15 23:56 to 2026-10-16 00:16; the prepass and occlusion
comparisons, section 1 and the run-to-run check below again on the idle machine, 2026-10-16 01:33
to 01:42):

- **Scenes without a transparent material** (`tools/build_compare.py --before
  out/transparency/before --adapters nvidia|780m` with every case but `pt-lab` in `--cases`, the
  examples built from master `3fdc5b83`'s renderer): many_cubes with 200,000 shadow-casting
  cubes, the dense grid, the 1M-splat garden, gi-room drawn statically, `lod_field` with 40 skinned
  columns and many_cubes with all, every tenth and every fifth cube moving every frame, with 4x
  MSAA, 4x MSAA with GTAO, TAA, and TAA with GTAO's normal target (gi-room with MSAA only): 29 pairs
  on each of the RTX 5060 and the 780M, Direct3D 12 and Vulkan, 116 in all, every one identical to
  the bit (`out/transparency/build-compare-review/`; the same on `31934dd9` and `2a59b3d7` in the
  first round). pt-lab differs, as it should (section 1).
- **The prepass and occlusion culling** (`tools/prepass_compare.py`, `tools/occlusion_compare.py`,
  `--cases cubes-sphere,cubes-dense,cubes-shadows,splats,gi-room-static,pt-lab-static --scenes ''`:
  the renderer examples' captures with the adapter's defaults, so pt-lab with GTAO on the RTX 5060
  draws its stage after GTAO): forcing either on changes no pixel of pt-lab, whose blended card and
  glass sphere are drawn by the stage, nor of the other scenes on the four configurations, except
  many_cubes' sphere, where a few isolated pixels differ by one level with either feature (two cube
  faces at exactly equal depth, which `docs/bench/prepass.md` 2 records on master)
  (`out/transparency/{prepass,occlusion}-compare-review/`). On the idle machine the same
  (`out/transparency/quiet/{prepass,occlusion}-compare/`): pt-lab unchanged by either feature on
  the four configurations; which configurations show the sphere's one-level pixels changes from run
  to run (this time the RTX 5060's Direct3D 12 and both of the 780M's with the prepass, both
  Direct3D 12 configurations with occlusion culling). The `pocket serve` captures and the pocket-app
  `draw_paths` scenes were not run (pocket-app was not built on this branch).
- **GPU tests** (`tools/gpu_matrix.py --configs
  5060-dx12,5060-vulkan,780m-dx12,780m-vulkan,5060-dx12-minimal,780m-vulkan-minimal`, every
  `pocket-render` test binary): 189 passed on each of the four full configurations; 178 passed and
  11 skipped for missing features on each WebGPU-baseline device (no timestamps, no
  `indirect-first-instance`, no shader-f16, no ray queries)
  (`out/transparency/gpu-matrix/review.json`, commit `47114c146d3b`, no `-dirty`). The ray-traced
  parts of `rt_shadows` and `transparency` (a device of its own with ray queries) ran on the four
  full configurations. The first round's matrix runs were made from working trees with changes
  (`-dirty`) and are superseded.
- **In Chrome** (first round, `05716664`'s renderer, 2026-10-15 23:00 to 23:10; the viewport built
  locally with `tools/build_viewport.sh`, not committed, and not rebuilt for the review):
  `python tools/web_draw_paths.py "http://127.0.0.1:8193/viewport/?demo=transparent"` (the mixed
  scene with `demo::transparent_instances`: two blended spheres, a glass cube, a blended
  double-sided quad) draws the same pixels and the same id coverage on WebGPU's two draw paths
  (`first-instance` and `baseline`) on the 780M and on the RTX 5060
  (`--chrome-flags=--force_high_performance_gpu`), 4x MSAA, with the four transparent entities in
  the coverage on both paths; occlusion culling off and on give the same image and coverage on the
  RTX 5060. With TAA the two paths' screenshots differ at every object's edges by up to 51 levels
  (434 pixels over 8), the jitter phase at which each continuously running page was captured, not
  the transparent instances alone (`out/transparency/browser/`). The browser defaults to GTAO off,
  so the review's change of order does not enter these pages.
- **pt-lab across the double-sided changes**: `2a59b3d7`'s forward image differs from `440dd7f8`'s
  in 30 pixels by at most 1.2e-4 (one half-float step): 26 of the single-sided glass sphere and 4 at
  the edge of `CutoutCard` beside it, whose 4x MSAA pixels the glass partly covers; none of the
  blended card. The first round took it for the shader's code generation after `fs_transparent`
  became a function; the idle machine shows the same kind of difference between runs of one build,
  so it may be that variation instead (not settled). Eight images of pt-lab drawn by the review
  build's `showcase_bench` as section 1 draws them (2026-10-16 01:38 to 01:42,
  `out/transparency/quiet/determinism.py`'s six runs, the last of the idle machine's
  `transparency_pt.py` runs and the stored image of 00:18) are three different images: six alike, 14
  pixels off `2a59b3d7`'s; one 18 pixels off it; and one `2a59b3d7`'s to the bit. They differ only
  in 32 pixels (28 of `Glass`, 4 at `CutoutCard`'s edge), each by one half-float step, with the same
  id images; 16 of the 30 pixels by which `440dd7f8` differs are among them. Master's renderer
  (`before`) drew its stored image to the bit four times. So the stage's image of the glass sphere
  is not reproducible to the bit from run to run (cause not traced); section 1's figures do not move
  at their printed digits, and the 8-bit images of `prepass_compare.py` and `occlusion_compare.py`
  do not see it.

## 5. Re-measuring on a quiet machine

The quiet re-measurement (2026-10-16) ran `out/transparency/quiet/driver.sh`, which takes the
following steps in order, each inside the machine-wide bench lock, from the worktree with the
machine otherwise idle, waiting before each run for the RTX 5060 to read below 70 C (the paths
below are the main checkout's `out/`, seen from a worktree under `.claude/worktrees`; in Git Bash,
`export PATH="$HOME/.cargo/bin:$HOME/scoop/shims:$PATH"` first):

```sh
cargo build --release -p pocket-render --example transparency --example showcase_bench --example path_trace
L=../../../out/locks/bench_lock.py
T=../../../out/transparency
Q=$T/quiet
OLD=$T/build-05716664/target/release/examples/transparency.exe
for a in 5060 780m; do for b in dx12 vulkan; do              # section 2, and 2.3 on the RTX 5060
  python $L run --label quiet-transparency -- python tools/transparency_bench.py --adapter $a --backend $b --out $Q/bench
  [ $a = 5060 ] && python $L run --label quiet-transparency -- python tools/transparency_bench.py \
    --adapter $a --backend $b --kinds hidden,blend,glass,double --exe $OLD --out $Q/ab-05716664
  python $L run --label quiet-transparency -- python tools/transparency_bench.py --adapter $a --backend $b \
    --kinds hidden,blend,double --extra=--orbit --out $Q/bench
done; done
HITCH_OUT=$Q/hitch python $L run --label quiet-transparency -- python -I $T/scripts/hitch.py   # 2.2
for a in 5060 780m; do for b in dx12 vulkan; do              # 2.1
  for t in lists:. one-draw:$T/src-b9eea658 two-draws:$T/src-440dd7f8; do
    python $L run --label quiet-transparency -- python ${t#*:}/tools/transparency_bench.py --adapter $a \
      --backend $b --kinds hidden,double --out $Q/ab3-${t%%:*}
  done
  python $L run --label quiet-transparency -- python tools/transparency_bench.py --adapter $a \
    --backend $b --kinds hidden,double --exe $OLD --out $Q/ab3-lists-05716664
done; done
```

Then the deterministic checks: `transparency_bench.py --ghost` (section 3); `transparency_pt.py`
four times into `$Q/pt-lab` with copies of `$T/pt-lab/reference-*` (no tracing), the `before`,
`prefix` and `after` builds as section 1 and 1.1 name them; `compares.py prepass|occlusion` (the
script in `$T/scripts/`, writing into `$Q/`); and `$Q/determinism.py` (section 4). Section 2.2's
`hitch.py` runs, per configuration, build and kind (`hidden`, `blend`), three processes of
`transparency bench --frames 3 --warm 0` and takes the median of each one's `cpu_encode_ms.max`. The
two earlier builds' sources are exported (`git archive`) to `$T/src-440dd7f8` and `$T/src-b9eea658`
with the first round's `transparency` example and bench script; build each there with
`cargo build --release -p pocket-render --example transparency` into its own `target/` (the two
trees sharing one target directory is not enough: cargo takes the second tree's crates for fresh,
since their fingerprints do not depend on the tree's path; removing the `pocket-*` fingerprints
between the two builds also works). Section 1 alone:
`python tools/transparency_pt.py --build before=$T/before --build after=. --out $Q/pt-lab --adapter 5060`
(with and without `--pt-args "--emission-scale 0"`, and with `--build prefix=$T/build-05716664` and
`--gtao depth`) reproduces its figures; another `--out` without the references traces them again.
