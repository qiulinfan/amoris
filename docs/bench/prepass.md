# Depth prepass: method, default and provisional measurements

The opaque pass draws a depth prepass in `auto` mode by default: GPU timestamps compare the opaque
passes with and without it and the cheaper way is kept (docs/spec/prepass.md 5).
`POCKET_PREPASS=off|on|auto` overrides it; the browser uses `?prepass=`. Design and correctness
contract: [prepass](../spec/prepass.md). Related: [occlusion](occlusion.md), [backend
cost](dx12.md), the UE5 comparison on branch `explore/ue5` (`docs/bench/ue5-dense.md` there).

Status: **provisional**. Measured on 2026-10-10 while another agent shared the machine: every
timing run held the shared GPU lock (no other GPU timing at the same time), but the other agent's
builds and tests could load the CPU and the GPU between and during runs.

## 1. Method

- Machine: R1, ASUS ROG Zephyrus G14: Ryzen 9 270 (16 threads), 15 GB, Windows 11 Pro 10.0.26200,
  RTX 5060 Laptop 8 GB (driver 32.0.16.1714) and Radeon 780M; on AC power (battery status 2, 100%).
  Release build of `explore/prepass` (wgpu 30.0.1); `old` is master `c90c414c` built the same
  way (before the prepass and before `@invariant`).
- `python tools/prepass_bench.py` (one round per call, every configuration once, the order
  reversed every other round; each round held the GPU lock for its whole duration):
  `many_cubes --dense` and the sphere (1.6M cubes), `POCKET_AA=msaa POCKET_GTAO=off` as in the UE5
  comparison, the prepass `off`, `on` and `auto` against occlusion culling `off` and `auto`.
  Headless: `--headless-bench 200 --size 2560x1440 --settle 5` (60 warm-up frames, then 200
  measured, each submitted and waited for; 100 measured on the 780M); the frame is submit to GPU
  idle (p50), the GPU time the mean of the timestamped passes' sum. Window: `--bench 300` (60
  skipped, 300 measured; 1280x720 logical, 2560x1440 pixels on this 200% display, no vsync); the
  frame is the wall time between presented frames (p50), the GPU time the mean timestamped sum.
- Tables give the minimum and the median over the rounds. nvidia-smi's temperature before each
  run is kept with the run (61 to 76 C); `tools/machine_conditions.py` recorded each round's start
  and end. Raw runs: `out/prepass/bench/*.jsonl` (not committed).
- The auto mode's probes after the review (section 3.1): the same tool against the build before the
  fix, `--table ... --tails` for the frame time's tail and the probe frames.

## 2. The image does not change

`python tools/prepass_compare.py` (every capture of tools/backend_compare.py with the prepass off
and forced on, each its own process; `draw_paths --compare prepass` over the mixed scene, the
many_cubes sphere and four samples, both shots in one process at one moment), on the RTX 5060 with
Direct3D 12 and Vulkan, on the Radeon 780M with Direct3D 12, and with
`POCKET_GPU_MINIMAL=features,limits,timestamps` (WebGPU's baseline device; Direct3D 12 on the RTX
5060; the `draw_paths` scenes run on the full device either way):

| Pairs | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | Baseline device |
|---|---|---|---|---|
| many_cubes dense, orbit, 200k with shadows; splats; gi-room and pt-lab static | identical | identical | identical | identical |
| many_cubes sphere | 2 pixels 1 level apart | identical | 2 pixels 1 level apart | 1 pixel 1 level apart |
| `draw_paths`: mixed, cubes, samples/anim, sailing, gi-room, pt-lab (pixels and id coverage) | identical | identical | identical | identical |

The sphere's pixels are isolated dots (heat maps), at other places on each GPU, and one level
apart: a hole (a depth the forward pass misses) would show the cleared target, far darker. They are
read as ties, samples where two cubes' faces store the same depth, where the prepass's equal test
shows the last drawn and the test without it the first (docs/spec/prepass.md 4); occlusion
culling's change of draw order showed the same kind of difference in this scene (occlusion.md).
Not checked sample by sample. The `pocket serve` captures (sailing, anim and gi-room run
by the host) did not start in this session (`backend_compare.py` refused the ports it could not
probe); `draw_paths` covers the same samples at a fixed moment.

`cargo test -p pocket-render --test prepass` (identical images and coverage in every scene, three
draw paths) passes on both GPUs with both backends and on the baseline device. The whole crate's
tests pass in the default auto mode and with `POCKET_PREPASS=on` (Direct3D 12; on Vulkan the
occlusion, hiz, prepass, forward_variants, draw_paths, taa, lod and neural_material tests).
tests/occlusion.rs, tests/hiz.rs and the occlusion comparison's scenes pass with the prepass on:
culling is unchanged.


## 3. Timings

GPU time in milliseconds, the median of the rounds with the minimum in parentheses, then the frame's
median (headless: submit to GPU idle; window: between presented frames). RTX 5060 (`nvidia`) and
Radeon 780M (`780m`, 100 frames a run) three rounds each, the old build two rounds. `Old build`:
master before the prepass and before `@invariant`. `tools/prepass_bench.py --table ... --markdown`
prints these tables from the raw runs.

Dense grid, headless:

| Backend, adapter | Occlusion | Off | On | Auto | Old build |
|---|---|---|---|---|---|
| dx12, nvidia | off | 51.33 (51.20); 52.93 | 16.22 (16.21); 17.67 | 17.03 (17.03); 17.61 | 51.39 (51.35); 53.05 |
| dx12, nvidia | auto | 2.33 (2.30); 3.53 | 1.49 (1.40); 2.82 | 1.44 (1.42); 2.58 | 2.37 (2.33); 3.74 |
| vulkan, nvidia | off | 42.51 (42.35); 43.77 | 16.59 (16.21); 17.66 | 17.12 (17.10); 17.78 | 42.76 (42.40); 44.04 |
| vulkan, nvidia | auto | 1.98 (1.96); 2.88 | 1.40 (1.34); 2.33 | 1.36 (1.36); 2.12 | 2.16 (1.98); 3.67 |
| dx12, 780m | off | 297.57 (245.61); 303.27 | 96.26 (86.39); 101.67 | 92.30 (86.79); 99.58 | |
| dx12, 780m | auto | 10.50 (9.52); 14.62 | 7.80 (7.04); 10.95 | 7.88 (7.12); 11.64 | |

Dense grid, window (2560x1440 pixels):

| Backend, adapter | Occlusion | Off | On | Auto |
|---|---|---|---|---|
| dx12, nvidia | off | 52.04 (50.59); 52.48 | 17.02 (16.30); 17.27 | 18.24 (16.88); 17.90 |
| dx12, nvidia | auto | 2.51 (2.35); 3.26 | 1.40 (1.31); 2.69 | 1.35 (1.34); 2.56 |
| vulkan, nvidia | off | 38.80 (38.48); 38.24 | 18.21 (17.05); 18.62 | 18.42 (17.43); 18.19 |
| vulkan, nvidia | auto | 2.61 (2.58); 3.28 | 1.88 (1.65); 2.35 | 1.78 (1.69); 2.44 |

Sphere, headless:

| Backend, adapter | Occlusion | Off | On | Auto | Old build |
|---|---|---|---|---|---|
| dx12, nvidia | off | 3.49 (3.41); 4.91 | 4.47 (4.39); 6.37 | 3.53 (3.51); 5.23 | 3.58 (3.57); 5.04 |
| dx12, nvidia | auto | 3.42 (3.29); 4.54 | 4.61 (4.19); 5.96 | 3.64 (3.26); 4.72 | 3.53 (3.51); 4.89 |
| vulkan, nvidia | off | 3.61 (3.54); 4.82 | 4.39 (4.27); 5.36 | 3.63 (3.62); 4.57 | 3.52 (3.30); 4.76 |
| vulkan, nvidia | auto | 3.77 (3.69); 4.58 | 4.50 (4.48); 5.79 | 3.72 (3.23); 4.55 | 3.68 (3.46); 4.87 |
| dx12, 780m | off | 18.91 (16.25); 22.64 | 20.41 (18.11); 24.81 | 19.27 (16.36); 23.47 | |
| dx12, 780m | auto | 18.61 (16.35); 22.56 | 20.44 (18.17); 24.62 | 18.39 (16.54); 21.91 | |

- **Dense grid, occlusion culling off** (UE's matched configuration): the prepass cuts the GPU time
  to a third on the RTX 5060 (Direct3D 12 51.3 to 16.2 ms headless; Vulkan 42.5 to 16.6 ms) and on
  the 780M (298 to 96 ms). The prepass pass itself is 11.2 ms on Direct3D 12 and 11.7 ms on Vulkan,
  the shading pass after it 4.4 to 4.5 ms: it still rasterizes and tests all 482,000 cubes in the
  frustum, and shades the samples of the 443 visible ones.
- **Dense grid, occlusion culling auto** (on here): the early phase draws the 470 cubes visible the
  frame before, which overlap: its opaque pass costs 1.45 ms on Direct3D 12 and 1.07 ms on Vulkan
  without the prepass, 0.15 + 0.38 and 0.15 + 0.32 ms with it (round 0's pass means). The frame's
  GPU time falls by a third here too (2.33 to 1.44 ms auto; 1.98 to 1.36; 780M 10.5 to 7.9).
- **Sphere** (1.6M cubes on a sphere around the camera, about 187,500 in the frustum and nearly all
  visible, hardly overlapping): the prepass is a second geometry pass for little: 1.2 ms of
  prepass, the shading pass 0.1 to 0.4 ms cheaper; 19 to 35% more GPU time on the RTX 5060, 8 to 10%
  on the 780M.
- **Auto** (as built before the review of section 3.1) draws it on the dense grid (195 of 200
  measured frames headless, 289 to 295 of 300 in the window) and not on the sphere (195 to 198 of
  200 without), within 0 to 1.2 ms of the better fixed mode. The gap is the probes: the first
  decision is made in the warm-up, the re-probe 240 frames later falls in the measured window and
  drew five frames in a row the slower way; since the review it draws one (section 3.1).
- `@invariant` costs nothing measurable: the old build and this one without the prepass are within
  the rounds' spread (dense 51.39 and 51.33 ms on Direct3D 12, 42.76 and 42.51 on Vulkan; sphere
  3.58 and 3.49, 3.52 and 3.61). `Renderer::new` did not grow either (Direct3D 12 minimum 184 ms,
  median 201, against the old build's 190 and 256; the equal-depth twins reuse the compiled
  shaders).
- The 780M's runs spread widely (its dense grid without the prepass 246 to 303 ms): it drives the
  display, and the other agent's work shared the machine.

### 3.1 The auto mode's probes, after the review

A review of the auto mode found two defects (the rules that replaced it: docs/spec/prepass.md 5).
Each reading counted for the running probe whatever frame it measured: frames without instances
(the sky alone, 0.1 ms) and frames from before a resize; ten empty frames at start made the dense
grid's first decision `off` for 240 frames. That is a question of which readings count, and
prepass.rs' unit tests check it. And every re-probe drew both ways by turns until it had four
readings of each, so with readings two or three frames late it drew five frames in a row the slower
way, which shows in the frame times:

- A/B in the same rounds: `before` is commit `82a6ab10` (the probes section 3 measured, with these
  frame time tails), `after` this build; the dense grid and the sphere, occlusion culling off,
  `POCKET_AA=msaa POCKET_GTAO=off`, 200 measured frames after 60, so each run holds the first
  re-probe (240 frames after the first decision, which falls in the warm-up). Headless 2560x1440
  with `--settle 5`; the window gave 1600x900 pixels this time (the display's scaling had changed
  since section 3's runs), so its numbers compare only with each other.
- Three rounds on 2026-10-10 between 16:37 and 16:53, each in one GPU lock, the order reversed in
  round 1; on AC power, the RTX 5060 at 68 to 77 C before the runs; the other agent's work could
  load the machine (31% CPU at the first round's start). Medians of the three rounds; the probe
  frames per round. Raw runs: `out/prepass/bench/probes-*.jsonl`.

Dense grid, headless: frames drawn without the prepass in each run, the slowest of them, the
frame's p99 (the second slowest of 200) and the GPU time's mean, median (minimum):

| Backend, adapter | Frames without | Their slowest, ms | Frame p99, ms | GPU mean, ms |
|---|---|---|---|---|
| dx12, nvidia, before | 5, 5, 5 | 54.1 to 60.5 | 58.69 | 18.48 (18.14) |
| dx12, nvidia, after | 1, 1, 1 | 43.6 to 49.1 | 28.67 | 17.48 (17.34) |
| dx12, nvidia, prepass on | 0 | | 21.59 | 17.35 (17.29) |
| vulkan, nvidia, before | 5, 5, 5 | 42.3 to 61.3 | 53.36 | 18.54 (18.34) |
| vulkan, nvidia, after | 1, 1, 1 | 35.5 to 40.0 | 30.96 | 18.45 (17.82) |
| vulkan, nvidia, prepass on | 0 | | 21.18 | 17.75 (17.69) |
| dx12, 780m, before | 5, 5, 5 | 245.4 to 258.3 | 245.37 | 90.92 (90.45) |
| dx12, 780m, after | 1, 1, 1 | 243.8 to 278.4 | 96.33 | 87.40 (87.36) |
| dx12, 780m, prepass on | 0 | | 99.47 | 87.31 (86.95) |

Dense grid, window (1600x900 pixels; wall time between presented frames):

| Backend | Frames without | Frame p99, ms | Frame max, ms (range) |
|---|---|---|---|
| dx12, before | 5, 5, 5 | 31.15 | 31.99 (24.28 to 32.74) |
| dx12, after | 1, 1, 1 | 15.17 | 21.17 (17.91 to 22.41) |
| dx12, prepass on | 0 | 13.56 | 13.68 (13.43 to 14.20) |
| vulkan, before | 4, 5, 5 | 15.95 | 16.83 (16.01 to 28.54) |
| vulkan, after | 1, 1, 1 | 13.85 | 18.90 (15.45 to 24.42) |
| vulkan, prepass on | 0 | 14.09 | 14.51 (13.49 to 33.10) |

Sphere, headless (the prepass about a third dearer, so auto keeps it off and its probes draw it):

| Backend | Frames with the prepass | Their slowest, ms | Frame p50, ms |
|---|---|---|---|
| dx12, before | 5, 5, 5 | 6.4 to 7.3 | 4.70 |
| dx12, after | 4, 3, 2 | 6.6 to 7.1 | 4.81 |
| dx12, prepass off | 0 | | 4.82 |
| vulkan, before | 5, 5, 5 | 6.5 to 6.8 | 4.40 |
| vulkan, after | 1, 4, 1 | 6.2 to 6.6 | 4.33 |
| vulkan, prepass off | 0 | | 4.48 |

- Each re-probe of the dense grid draws one frame without the prepass, where it drew five in a row:
  on Direct3D 12 one frame of 44 to 49 ms against about 19 ms with the prepass, instead of about
  0.3 s of frames near 57 ms; on the 780M one frame of about a quarter of a second instead of 1.2 s.
  The frame's p99 halves on the RTX 5060 and falls to the prepass-on runs' on the 780M, and auto's
  GPU mean comes within 0.13 ms of the prepass on with Direct3D 12 and 0.09 ms on the 780M (1.13
  and 3.61 ms before); on Vulkan within 0.13 ms in the minimums (0.65 before), 0.70 in the
  medians, which one slow round lifts. In both builds an auto run's slowest frame drawn with the
  prepass (24 to 34 ms with Direct3D 12) is slower than the prepass-on runs' slowest (22 to 25 ms),
  which keeps auto's p99 above theirs; which frame that is was not recorded.
- On the sphere a re-probe draws the prepass one frame at a time, four frames at most (two to four
  inside the measured window, which ends during some probes; one when a reading lands more than
  50% above the minimum without it), each about 2 ms longer than the median frame.
- In the window Direct3D 12's p99 falls from 31.2 to 15.2 ms (13.6 with the prepass on) and the
  maximum from 32.0 to 21.2 ms. On Vulkan the window's maximum varies more between rounds than
  between builds (before 16.0 to 28.5 ms, after 15.5 to 24.4, the prepass on 13.5 to 33.1).

## 4. Against Unreal Engine 5.7

The UE5 comparison (branch `explore/ue5`, `docs/bench/ue5-dense.md` 5.1 and 5.2) ran the same dense
grid in a 2560x1440 window on the same RTX 5060, forward shading with 4x MSAA. Frame and GPU time,
medians of three rounds; Unreal's GPU time includes its post chain, as ours did on that branch
(`POCKET_TIME_POST=1`, 0.3 to 0.4 ms); this build's (section 3, window) does not:

| Configuration | Unreal 5.7 | Amoris on explore/ue5 | This build, prepass auto | This build, prepass on |
|---|---|---|---|---|
| No occlusion culling, Direct3D 12 | 16.53 / 16.36 | 50.89 / 50.77 | 17.90 / 18.24 | 17.27 / 17.02 |
| No occlusion culling, Vulkan | 18.22 / 18.08 | 35.59 / 36.19 | 18.19 / 18.42 | 18.62 / 18.21 |
| Occlusion culling, Direct3D 12 | 3.49 / 3.15 | 2.98 / 2.86 | 2.56 / 1.35 | 2.69 / 1.40 |
| Occlusion culling, Vulkan | 4.59 / 4.18 | 3.08 / 2.82 | 2.44 / 1.78 | 2.35 / 1.88 |

Without occlusion culling the prepass closes the gap: three times Unreal's GPU time on Direct3D 12
and twice on Vulkan before; with the prepass on, our frame is 4% (Direct3D 12) and 2% (Vulkan) above
Unreal's, and our GPU time 6% and 3% above its with our post chain counted (Unreal's prepass took
11.97 ms and its base pass 2.74 ms; ours 11.2 and 4.5 ms headless). Auto's GPU means include its
re-probe's frames without the prepass (5 to 11 of 300 in these runs, before the review: section
3.1). With occlusion culling we were already faster; the prepass makes our frame 0.73 (Direct3D 12)
and 0.53 (Vulkan) of Unreal's.

## 5. Default

`auto`. Neither fixed mode is best everywhere: `on` costs the sphere 19 to 35% (a scene with little
overdraw pays a second geometry pass), `off` costs the dense grid three times its GPU time without
occlusion culling and half again with it. Auto follows the better one within its probes' cost.
Where timestamps are missing it stays off, the renderer's behaviour before the prepass.
