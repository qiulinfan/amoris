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

## 6. In Chrome

Status: **quiet re-measurement** (2026-10-15, branch `explore/webauto`, the second round's final
rules and committed viewport package). Nothing else ran on the machine: CPU load 3.6 to 9.1% at
each invocation's start, no other agent's builds, tests or GPU work, only the desktop compositor
and the agent's own app on the GPU, on AC power, the RTX 5060 at 51 to 69 C (below 70 C at every
start); every invocation held the shared GPU lock. One run per cell (n = 1). Two provisional rounds
came first (2026-10-14: the first, then the second after review), while other agents built and
tested on the machine (CPU load 26 to 65% at the first round's run starts, 3 to 16% at the
second's); the quiet runs reach the same decisions, and where their figures differ the text says
so. The question is not how fast, but whether the auto modes reach the right decision where
readbacks arrive hundreds of frames late and timestamps may be quantized.

**Method.** `python tools/web_auto.py run` (`table` prints these tables): Chrome 155.0.8059.41
headless (each row records the version), a 1280x720 window with the page held at 1254x564 CSS
pixels (`--canvas`, through `Emulation.setDeviceMetricsOverride`; each row records the canvas), the
viewport pages `?demo=cubes&dense` (many_cubes' dense grid, 1.6M cubes), `?demo=cubes` (the sphere)
and `?demo=occluders`, the browser's defaults (MSAA, GTAO off, occlusion culling and the prepass
auto). Runs last 30 s, 90 s for the 780M with the uncapped loop (whose lag-scaled timeout and
re-probes take longer than 30 s there); every 200 ms the page's new frames (their choices) and the
auto modes' state (the viewport's `auto_stats`: `prepass_auto`, `occlusion_auto`,
`timestamp_quantum_us`) are read. Adapters: the RTX 5060 (`--force_high_performance_gpu`) and the
Radeon 780M (Chrome's choice), both through Dawn on Direct3D 12. Frame loops: `uncapped`
(`--disable-frame-rate-limit --disable-gpu-vsync`, the benchmarks' setup) and `vsync` (the 120 Hz
display's). Chrome: `unsafe` (`--enable-unsafe-webgpu`, as every earlier browser measurement) and
`default` (without it). Which way is cheaper: the same pages with `?prepass=on` and `off` (unsafe
Chrome, 20 s each), the opaque passes' GPU time (prepass included; the latest reading every 200 ms,
median); for occlusion culling `?occlusion=on` and `off` with the prepass fixed as the auto mode
decides each page (on for the dense grid, off elsewhere), the GPU frame time. The native reference
is `many_cubes --headless-bench 600` at 1254x564 with `POCKET_AA=msaa POCKET_GTAO=off` on Direct3D
12 (`web_auto.py native`). Raw polls, summaries and machine conditions: `out/webauto/quiet/` (the
provisional rounds' beside it in `out/webauto/`; not committed).

The first round's default-Chrome runs drew a 1254x620 canvas: in the same window Chrome gives the
page 56 pixels more height without `--enable-unsafe-webgpu`, so they did not match the fixed-mode
runs (unsafe, 1254x564). The second round fixed the canvas and re-ran them; the tables below are
the quiet re-measurement's, except the unsafe `before` cells (the first round's, at 1254x564; the
quiet protocol re-ran `before` in default Chrome only).

**What the browser measures.** Readings (timestamps and occlusion culling's counters alike) arrived
357 to 373 frames late with the uncapped loop (0.3 to 2.6 s in the auto runs at 0.7 to 7 ms a frame,
up to 9.5 s in the fixed-mode run of the 780M's dense grid without occlusion culling at 25 ms a
frame; docs/bench/polish.md 3 found the same for the entity-id readback), 1 to 7 frames with vsync
(the median 1 or 2, and 7 where the dense grid without occlusion culling took longer than the
display's interval), and 1 frame natively, in the quiet runs as in the provisional ones: the lag is
the frames from a timed frame to its reading's arrival (`prepass_auto.lag`). The timestamps'
resolution (the greatest common divisor of every timestamp read) was 32 ns on the RTX 5060 and 10 ns
on the 780M with `--enable-unsafe-webgpu`, and 65.536 us (2^16 ns) on both without it, which
render.md's open question asked; other switches were not tried. `timestamp-query` was granted either
way. The way that pays (opaque passes, ms, median):

| Page, adapter | Uncapped: on, off | Vsync: on, off | Cheaper |
|---|---|---|---|
| dense (two phases), RTX 5060 | 0.159, 0.346 | 0.143, 0.298 | on (-52 to -54%) |
| dense (two phases), 780M | 0.434, 0.977 | 0.714, 1.438 | on (-50 to -56%) |
| sphere (one phase), RTX 5060 | 2.896, 2.078 | 2.754, 1.923 | off (+39 to +43%) |
| sphere (one phase), 780M | 4.931, 3.871 | 4.700, 4.280 | off (+10 to +27%) |
| occluders (one phase), RTX 5060 | 0.110, 0.106 | 0.109, 0.106 | off (+3 to +4%) |
| occluders (one phase), 780M | 0.560, 0.493 | 0.855, 0.812 | off (+5 to +14%) |

The provisional rounds' fixed-mode runs found the same ways cheaper by similar shares (the first
-50 to -55%, +19 to +45%, +4 to +11%; the second -51 to -54%, +13 to +52%, +2 to +12%); the RTX
5060's occluder page differs by less than the auto mode's 5% margin, so either choice is within it
there.

The dense grid's frames run two opaque phases (occlusion culling's auto mode turns it on, as
natively), so its two-phase choice decides; the sphere's and the occluder scene's run one.

**Decisions** of the frames' kind: the first decision and the frames from the kind's first probe to
it, probes that gave up (timeouts), and the share of the run's frames drawn with the prepass.
`Before` is the rules before this branch: master's viewport package for default Chrome (it reports
no auto state, only what each frame drew; quiet runs, 30 s on every adapter), the diagnostics-only
build `0ec2dfc0` for unsafe Chrome (the first round); `after` this branch (quiet runs):

| Page | Adapter | Loop | Chrome | Lag p50 (max) | Quantum, us | Kind | Before: decision at frames; prepass frames | After |
|---|---|---|---|---|---|---|---|---|
| dense | RTX 5060 | uncapped | unsafe | 372 (373) | 0.032 | two | none, 9 timeouts; 31% | on at 1052; 100% |
| sphere | RTX 5060 | uncapped | unsafe | 373 (373) | 0.032 | one | none, 5 timeouts; 39% | off at 1125; 16% |
| occluders | RTX 5060 | uncapped | unsafe | 358 (359) | 0.032 | one | none, 15 timeouts; 29% | off at 1075; 4% |
| dense | RTX 5060 | uncapped | default | 372 (373) | 65.536 | two | not reported; 30% | on at 1080; 100% |
| sphere | RTX 5060 | uncapped | default | 373 (373) | 65.536 | one | not reported; 39% | off at 1123; 16% |
| occluders | RTX 5060 | uncapped | default | 358 (358) | 65.536 | one | not reported; 28% | off at 1075; 4% |
| dense | RTX 5060 | vsync | unsafe | 1 (5) | 0.032 | two | on at 8; 100% | on at 8; 100% |
| sphere | RTX 5060 | vsync | unsafe | 1 (4) | 0.032 | one | off at 6; 1% | off at 5; 1% |
| occluders | RTX 5060 | vsync | unsafe | 1 (3) | 0.032 | one | off at 5; 1% | off at 5; 1% |
| dense | RTX 5060 | vsync | default | 1 (5) | 65.536 | two | not reported; 100% | on at 8; 100% |
| sphere | RTX 5060 | vsync | default | 1 (4) | 65.536 | one | not reported; 1% | off at 5; 1% |
| occluders | RTX 5060 | vsync | default | 1 (3) | 65.536 | one | not reported; 100% | off at 5; 1% |
| dense | 780M | uncapped | unsafe | 372 (373) | 0.01 | two | none, 4 timeouts; 41% | on at 366; 100% |
| sphere | 780M | uncapped | unsafe | 373 (373) | 0.01 | one | none, 3 timeouts; 53% | off at 1079; 9% |
| occluders | 780M | uncapped | unsafe | 372 (373) | 0.01 | one | off at 493, 5 timeouts; 8% | off at 151; 2% |
| dense | 780M | uncapped | default | 372 (373) | 65.536 | two | not reported; 100% | on at 400; 100% |
| sphere | 780M | uncapped | default | 373 (373) | 65.536 | one | not reported; 55% | off at 1099; 9% |
| occluders | 780M | uncapped | default | 372 (373) | 65.536 | one | not reported; 5% | off at 186; 2% |
| dense | 780M | vsync | unsafe | 1 (7) | 0.01 | two | on at 11; 100% | on at 13; 100% |
| sphere | 780M | vsync | unsafe | 2 (7) | 0.01 | one | off at 17; 1% | off at 21; 1% |
| occluders | 780M | vsync | unsafe | 1 (3) | 0.01 | one | off at 5; 1% | off at 5; 1% |
| dense | 780M | vsync | default | 1 (6) | 65.536 | two | not reported; 100% | on at 13; 100% |
| sphere | 780M | vsync | default | 2 (7) | 65.536 | one | not reported; 1% | off at 19; 1% |
| occluders | 780M | vsync | default | 1 (3) | 65.536 | one | not reported; 1% | off at 5; 1% |

- **Uncapped, before: no decision.** A probe needs its first way's four readings and the other
  way's, one per readback lag; with three readback buffers the profiler times three frames per lag,
  so every probe gave up at its 600-frame timeout with three or four readings. Over 30 s the
  first round's runs timed out 3 to 20 times and decided in two runs of twelve (both within their
  first 500 frames, while Chrome's queue was still filling: lags of 120 to 290 frames). The frames
  drew the prepass during the probes and not between them, whichever way paid: the dense grid went
  without it for 59 to 70% of its frames (the 780M's default-Chrome run decided early in every
  round, the quiet one included), the sphere drew it for 39 to 55% (the quiet default-Chrome runs
  with master's package and the first round's unsafe ones; the provisional default-Chrome runs 38
  to 57%).
- **Uncapped, after**: the timeout grows with the largest lag seen since the probe started
  (`TIMEOUT_LAGS`, twelve lags, about 4,500 frames here). Every quiet run decided right, 151 to
  1,125 frames after its kind's first probe (0.4 to 7.7 s), and its re-probes kept it (3 to 11
  decisions a run; the 780M's 90 s runs 4 to 9), with no wrong switch and no timeout of the deciding
  kind; the two-phase kind of the sphere and occluder pages still times out, 1 to 5 times a run
  (its frames come only with occlusion culling's brief probes). The first probe draws the prepass
  until it has its readings, so the sphere drew it for 16% of a 30 s run on the RTX 5060 and 9% of a
  90 s run on the 780M, all in its first probe. The provisional second round's runs decided right
  too, but later at the extremes: 113 to 2,507 frames (0.3 to 17 s; the sphere 10 to 20% of a 780M
  run with the prepass), 3 to 16 decisions a run, and one wrong switch on the occluder page, undone
  at the next probe (below).
- **Vsync** decides in 5 to 21 frames (natively 5; provisionally 5 to 23).
- **Quantized timestamps.** Before, the occluder page took the prepass in default Chrome on the RTX
  5060 and kept it (100% of the frames with vsync, in the quiet run as in the provisional one), at
  the same canvas as unsafe Chrome, which did not: its two ways differ by 3 to 4% of 0.11 ms, a
  fraction of a quantum, and the prepass way's minimum comes out low by about half a quantum more
  (it times one more pass). After (`QUANTA_PER_PHASE`, that half quantum per phase added to the
  prepass way) the occluder page never took the prepass in default Chrome on the RTX 5060 in the
  quiet runs (15 decisions: 11 uncapped, 4 with vsync; 4% and 1% of the frames with it, the probes),
  and the dense grid's two-phase frames took it at their first decision (frame 8 with vsync, 100% of
  the frames). The provisional second round's runs had taken it on the occluder page in 2 of 21
  decisions (at frame 19 with vsync, at 16,674 uncapped), each undone at the next probe (253 and
  1,516 frames later, 8% and 3% of the runs' frames, 0.004 ms more each), and its dense grid had
  gone without the prepass at the first decision with vsync (off at frame 11, on at the re-probe at
  259, 92% of the frames with it). Both are what the rule allows: differences within about a quantum
  come out either way from probe to probe and run to run, and the next probe can undo them; the
  quiet runs drew none of them. The first round tried a slack instead (the way drawn second had to
  win by two quanta per phase beyond the margin): the occluder page stayed off, but the dense grid's
  two-phase frames on the RTX 5060 in default Chrome with vsync went without the prepass (1% of the
  frames with it), the opaque passes at twice its cost and the quantized GPU frame 14% longer (1.049
  against 0.918 ms, at the first round's 1254x620). Simulated (the pages' costs and pass shares,
  eight phases of the quantum, passes 2 and 30 us apart:
  `quantized_timestamps_decide_by_differences_beyond_their_resolution`), the share of the frames
  after the first decision drawn the cheaper way:

  | Rule | Occluder page, lag 2; 360 | Dense grid, lag 2; 360 |
  |---|---|---|
  | none | 0.49; 0.51 | 0.999; 1.000 |
  | slack, two quanta per phase (first round) | 0.998; 0.999 | 0.065; 0.001 |
  | slack, one quantum per phase | 0.998; 0.999 | 0.949; 0.913 |
  | bias, half a quantum per phase (adopted) | 0.990; 0.989 | 0.999; 1.000 |
  | bias, one quantum per phase | 0.999; 1.000 | 0.946; 0.791 |

  Weighted by the ways' differences (0.004 ms on the occluder page, 0.15 ms on the dense grid) the
  adopted bias loses the least per frame; differences within about a quantum still come out either
  way, at most about that much each.
- **Native** decisions are unchanged: both adapters decide the dense grid's two-phase frames on
  and the sphere's one-phase frames off, 5 frames after the first probe (occlusion culling decides
  at frame 3), with a lag of one frame and a resolution of 32 ns (RTX 5060) or 10 ns (780M), with
  this branch's binary (the quiet re-run alike) as with the first round's diagnostics-only one; they
  drew the prepass in 599 of 600 measured dense frames and 3 to 6 sphere frames (probes; 6 in the
  quiet runs). Neither rule applies there: the lag keeps the timeout at 600 frames and the bias is 5
  to 16 ns.
- **Occlusion culling's auto mode** reads its counters, not timestamps, and is unchanged. Against
  its fixed modes in Chrome (unsafe, the prepass fixed as the auto mode decides; GPU frame, ms,
  median; quiet runs):

  | Page, adapter | Uncapped: occlusion on, off | Vsync: on, off | Cheaper | Auto |
  |---|---|---|---|---|
  | dense, RTX 5060 | 0.871, 8.471 | 0.935, 8.621 | on (-89 to -90%) | on |
  | dense, 780M | 3.312, 24.499 | 4.105, 23.981 | on (-83 to -86%) | on |
  | sphere, RTX 5060 | 2.927, 2.618 | 2.619, 2.338 | off (+12%) | off |
  | sphere, 780M | 6.370, 5.465 | 6.577, 5.722 | off (+15 to +17%) | off |
  | occluders, RTX 5060 | 0.265, 0.208 | 0.268, 0.207 | off (+27 to +29%) | off |
  | occluders, 780M | 1.323, 1.066 | 2.174, 1.478 | off (+24 to +47%) | off |

  The provisional second round found the same ways cheaper (dense -83 to -90%; the others +6 to
  +33%, the sphere's smallest at +6%). The auto mode decided that way in every run of all three
  rounds, both Chrome configurations and both loops (in the last half of each quiet run 100% of the
  dense grid's frames had two phases, none of the others'), without a timeout: its first probe
  holds two phases until its reading, 22 to 362 frames uncapped in the auto runs (the 780M's dense
  grid and sphere decided at 22 to 38, while Chrome's queue was still filling) and 5 to 9 with
  vsync, and brief probes 120 to 960 frames apart failed.
  Natively (many_cubes, the dense grid and the sphere; it has no occluder scene) it decides the same
  at frame 3.

**Checks.** The change is in the auto mode only; the fixed modes draw as before:
`tools/prepass_compare.py` over the many_cubes captures (Direct3D 12, RTX 5060; first round) finds
the dense grid and the shadowed cubes identical and the sphere's tie pixels one level apart, as in
section 2; `cargo test -p pocket-render --test prepass --test occlusion --test hiz` passes on
Direct3D 12 and Vulkan (RTX 5060) and Direct3D 12 (780M) (first round; the second round changes
the decision rules only), and the crate's unit tests pass (spec/prepass.md 7).

**Quiet re-measurement** (2026-10-15, conditions above), from the repository root; each command
held the GPU lock for 1 to 10 minutes, and `table` prints the tables above:

    python tools/web_auto.py run --out out/webauto/quiet/quiet.jsonl --gpus nvidia --label quiet
    python tools/web_auto.py run --out out/webauto/quiet/quiet.jsonl --gpus 780m --loops vsync \
        --label quiet
    python tools/web_auto.py run --out out/webauto/quiet/quiet.jsonl --gpus 780m \
        --loops uncapped --seconds 90 --label quiet
    python tools/web_auto.py run --out out/webauto/quiet/quiet-truth.jsonl --chrome unsafe \
        --modes on,off --seconds 20 --label truth
    python tools/web_auto.py run --out out/webauto/quiet/quiet-truth.jsonl --pages dense \
        --chrome unsafe --modes "occlusion=on&prepass=on,occlusion=off&prepass=on" \
        --seconds 20 --label truth
    python tools/web_auto.py run --out out/webauto/quiet/quiet-truth.jsonl \
        --pages sphere,occluders --chrome unsafe \
        --modes "occlusion=on&prepass=off,occlusion=off&prepass=off" --seconds 20 --label truth
    python tools/web_auto.py native --out out/webauto/quiet/quiet-native.jsonl
    git archive master web | tar -x -C out/webauto/quiet/web-before
    python tools/web_auto.py run --web out/webauto/quiet/web-before/web --chrome default \
        --out out/webauto/quiet/quiet-before.jsonl --label before
    python tools/web_auto.py table --before out/webauto/quiet/quiet-before.jsonl \
        --after out/webauto/quiet/quiet.jsonl --truth out/webauto/quiet/quiet-truth.jsonl \
        --native out/webauto/quiet/quiet-native.jsonl

`before` serves master's viewport package (`bfdef38e`) beside this branch's tools.
