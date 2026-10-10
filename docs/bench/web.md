# Web rendering against three.js

Two measurements: the Apple M5 against three.js (2026-10-04) and the Windows laptop R1 on both of
the renderer's WebGPU draw paths (2026-10-09, provisional;
[below](#windows-chrome-155-on-d3d12-2026-10-09-provisional)).

## Apple M5 against three.js (2026-10-04)

These numbers predate later viewport changes and no longer describe the current package:
`110cffaf` (the cull pass writes the camera view's interpolated poses, the fix the open item at the
end of this section proposes; committed three minutes after these runs), `cc98f8d5` (baked GI probes
in the forward pass, 2026-10-05), the GI and shading commits of 2026-10-05 and 10-06, and the draw
paths of 2026-10-09 ([docs/spec/webgpu-baseline.md](../spec/webgpu-baseline.md)). They were not
re-measured on the M5.

- Date: 2026-10-04. Apple M5, Google Chrome 152 headless (`--headless=new --enable-unsafe-webgpu
  --disable-frame-rate-limit --disable-gpu-vsync`), window 1280x720 (canvas 1280x577, DPR 1),
  driven by `tools/web_bench.mjs` over CDP; the mean of the last 300 frames after a warm-up.
- Scene: Bevy's many_cubes sphere layout (radius 500, Fibonacci spiral, cubes facing the centre,
  camera at the centre turning 0.15/60 rad about z and x per frame), one mesh, one material, a
  directional light, 4x MSAA.
- three.js r186 (`web/bench/threejs_cubes.html`): `WebGPURenderer` with one `InstancedMesh` (its
  fastest path for copies of a mesh; frustum culling is per mesh, so every instance is drawn) and
  `MeshStandardMaterial`; GPU time from `trackTimestamp` / `resolveTimestampsAsync`.
- Amoris (`web/viewport/?demo=cubes`): `crates/pocket-viewport` (the same renderer as native,
  WebAssembly + WebGPU): per-instance GPU culling, indirect draws, clustered PBR, atmosphere sky
  with image-based light, bloom and AgX; GPU time from timestamp queries summed over passes.

| Cubes | three.js frame | three.js GPU | Amoris frame | Amoris GPU |
|---|---|---|---|---|
| 100,000 | 1.00 ms | 2.45 ms | 0.95 ms | 0.71 ms |
| 400,000 | 4.17 ms | 2.36 ms | 2.36 ms | 1.81 ms |
| 1,600,000 | 24.8 ms | 21.9 ms | 15.0 ms | 14.7 ms |

Frame time is the requestAnimationFrame interval without a frame-rate cap (it measures how fast
the page submits; GPU time is what the frames cost).

Module sizes: `pocket_viewport_bg.wasm` 1.08 MB (0.38 MB gzipped) after `wasm-opt -O3`, plus 81 KB
of JS glue; three.js r186's `three.webgpu.js` + `three.core.js` are about 1.9 MB unminified.

Open item: Amoris's GPU time in the browser at 1.6M cubes (14.7 ms at 1280x577) is far above
native Metal (2.3-3.4 ms at 2560x1440); the culling pass alone takes 3.4 ms against 1.2 ms. The
suspects are the browser's robustness bounds checks on storage-buffer reads in the per-vertex
pose interpolation and the cull pass; moving the interpolated transform into the cull pass (one
48-byte matrix per visible instance) is the planned fix.

## Windows, Chrome 155 on D3D12 (2026-10-09, provisional)

Every number here is provisional (the timing table is superseded by "Quiet re-measurement" at the
end of this section): three other agents were compiling and using both GPUs during the runs.
Identical runs differed by up to three times in frame time and by about 10% in GPU time; a
quiet-machine run will replace them.

- Machine: R1 (`docs/spec/budgets.md`): Windows 11, Ryzen 9 270, RTX 5060 Laptop (driver
  32.0.16.1714) and Radeon 780M, 15 GB.
- Chrome 155 headless, driven by `tools/web_bench.mjs` (which now finds Chrome on Windows) with the
  switches above minus `--enable-features=Vulkan`; window 1280x720 (canvas 1254x564, DPR 1) for
  timing, 960x540 (canvas 934x384) for screenshots. Chrome's WebGPU runs on D3D12. Without flags it
  takes the Radeon 780M (`amd rdna-3`) even though the page asks for `high-performance`;
  `CHROME_FLAGS=--force_high_performance_gpu` gives the RTX 5060 (`nvidia blackwell`). Both offer
  `indirect-first-instance`, `timestamp-query`, `shader-f16` and, under `--enable-unsafe-webgpu`,
  `chromium-experimental-multi-draw-indirect`.
- Package: this branch's `web/viewport/pkg`, built by `tools/build_viewport.sh` with wasm-bindgen
  0.2.129 and wasm-opt 132 (binaryen from scoop): 2.18 MB, 0.86 MB gzipped, plus 86 KB of JS glue
  (2.69 MB and 0.95 MB without wasm-opt). The package committed on 2026-10-06 was 2.17 MB.
- Paths ([docs/spec/webgpu-baseline.md](../spec/webgpu-baseline.md)): `first-instance` is the page
  as is; `baseline` adds `?gpu_minimal=first-instance`. Natively the same switch is
  `POCKET_GPU_MINIMAL`.

### Both draw paths draw the same frame

`tools/web_draw_paths.py`, evidence in `out/bench-runs/webgpu/browser/`:

| Page | Adapter | Draw calls per frame | Entities in the id pass | Coverage difference | Pixels differing |
|---|---|---|---|---|---|
| `?demo=mixed` (8x8 cells, every mesh kind and variant, shadows) | RTX 5060 | 55 | 59 and 59 | 0 | 0 |
| `?demo=mixed` | Radeon 780M | 55 | 59 and 59 | 0 | 0 |
| `?demo=cubes&dense&count=100000` | RTX 5060 | 1 | 6,359 and 6,359 | 0 | 0 |
| samples/anim, streamed from `pocket serve` | RTX 5060 | 110 | 7 and 7 | 0 | 3.3%: the particle fountain and its bloom, simulated on the page's clock |
| samples/sailing, streamed | RTX 5060 | 10 | 3 and 3 | 0 | 0 |

Draw calls are those of the shadow and opaque passes (five views times the batches holding
instances; many_cubes' demo has no shadows). A build that kept the base in `first_instance` on the
baseline path (`out/bench-runs/webgpu/browser-prefix/`) showed 6 of the mixed scene's 59 entities in
Chrome, the same six cubes (the batch at offset 0 of the camera view) as the native emulation before
the fix (`native-prefix/`): Chrome drops such
draws as the WebGPU specification says, and wgpu-core's indirect validation reproduces it natively.
Natively on Vulkan (`out/bench-runs/webgpu/native/`), the mixed scene, many_cubes with shadows,
samples/anim and samples/sailing are bit-identical on all three paths with WebGPU's default limits
and no timestamps.

### Timing

One 10-second run per row (the mean of the last 300 frames), taken while the machine was quieter,
with the package before wasm-opt (`out/bench-runs/webgpu/bench/web-bench-*-noopt.json`; the shaders
and GPU work are the same as the shipped package's). Frame time is the requestAnimationFrame
interval without a cap, GPU time the sum of the timestamped passes.

| Page | Path | RTX 5060 frame | RTX 5060 GPU | Radeon 780M frame | Radeon 780M GPU |
|---|---|---|---|---|---|
| many_cubes sphere, 100,000 | first-instance | 1.01 ms | 0.28 ms | 2.15 ms | 1.09 ms |
| | baseline | 0.91 ms | 0.24 ms | 2.24 ms | 1.16 ms |
| many_cubes sphere, 400,000 | first-instance | 0.90 ms | 0.75 ms | 4.06 ms | 2.60 ms |
| | baseline | 0.91 ms | 0.72 ms | 4.26 ms | 2.78 ms |
| many_cubes sphere, 1,600,000 | first-instance | 2.82 ms | 2.67 ms | 8.63 ms | 7.48 ms |
| | baseline | 2.86 ms | 2.68 ms | 6.41 ms | 5.52 ms |
| mixed, 8x8 cells (55 draw calls) | first-instance | 0.70 ms | 0.14 ms | 2.26 ms | 0.68 ms |
| | baseline | 1.18 ms | 0.14 ms | 2.51 ms | 0.69 ms |
| mixed, 40x40 cells (55 draw calls) | first-instance | 0.72 ms | 0.43 ms | 3.01 ms | 1.15 ms |
| | baseline | 0.83 ms | 0.41 ms | 3.14 ms | 1.22 ms |

The shipped package (wasm-opt) was then run three times per row (`web-bench-nvidia.json`,
`web-bench-amd-default.json`, with `summary_filtered`): on the RTX 5060 its GPU times agree with the
table within about 10% (0.22 to 0.31, 0.66 to 0.85, 2.7 to 2.9, 0.14 to 0.16 and 0.38 to 0.41 ms)
while its frame times swung up to six times between identical runs; on the Radeon 780M the runs were
unusable (timestamps read 0 in most of them, some pages drew fewer than 200 frames in 6 s). For
reference, the package committed on 2026-10-06 (before this branch) gave 1.03, 1.13 and 2.80 ms
frames and 0.21, 0.74 and 2.63 ms GPU at 100,000, 400,000 and 1,600,000 cubes on the RTX 5060
(`old-pkg-nvidia-cubes-*.json`, one run each).

What these show, provisionally:

1. Correctness is the result: the baseline path draws what the first-instance path draws, and
   before this branch a browser without `indirect-first-instance` would have drawn only the
   batches at offset 0 (no shadows at all).
2. The baseline path costs no GPU time at these batch counts, and its CPU cost (one
   `setBindGroup` per drawn batch) is below the noise of this machine at 1 to 110 draw calls per
   frame. Thousands of batches are the open measurement.
3. At 1.6M cubes the RTX 5060 in Chrome spends 2.7 ms of GPU time (cull 0.5 ms, opaque 2.3 ms) at
   1254x564, against 14.7 ms on the M5 before `110cffaf` moved pose interpolation into the cull
   pass; machine, GPU and package all differ, so the gain is not attributed. The 780M takes 5.5 to
   7.5 ms (the gap between its two 1.6M rows is run-to-run noise, not the path).
4. The id pass's readback (agents' `render.visible`, the editor's pick) resolved 150 to 980 ms after
   the request on the viewport pages and 50 to 100 ms on the game pages in Chrome with an uncapped
   frame loop, against one or two frames natively. Explained in [polish.md](polish.md) 3: the
   uncapped page runs 350 to 370 frames ahead of Chrome's GPU process and the readback waits behind
   them; with vsync it answers in 2 to 6 frames, and an opt-in frames-in-flight limit
   (`?in_flight=N`) brings it to 3 to 7 frames uncapped at the cost of the frame rate.
5. three.js was not re-measured: `web/bench/threejs_cubes.html` imports `./vendor/three.webgpu.js`
   (r186), which is git-ignored and absent on this machine, and getting it needs a CDN or npm
   download that this run did not make.

The browser game (`tools/build_web.sh`, the `?package=` pages: the game in a Worker on QuickJS-ng in
WebAssembly, the viewport on the page) needs a clang with the wasm32 backend for `rquickjs-sys`. The
task brief expected none on this machine, but the pinned toolchain (`~/.pocket-tools/llvm-23.1.2`,
with `binaryen-version_132`) appeared at 05:12 during the run (this track did not install it), and
`rquickjs-sys`'s build script finds it by itself. `tools/build_web.sh` and `tools/build_viewport.sh`
used to force `POCKET_LLVM=/opt/homebrew/opt/llvm`, which hid the pinned toolchain and failed on
Windows; they now set it only where Homebrew's LLVM exists, and no longer stop on Windows, which has
no `~/.cargo/env`. The game module is 7.5 MB after wasm-opt. Both samples ran as browser games in
Chrome on the RTX 5060 on both draw paths (`out/bench-runs/webgpu/browser/game-*.json`):
samples/sailing at about 2.3 ms a frame and 0.08 ms of GPU time with 2 entities in view on either
path, samples/anim with the same 7 entities in view on either path; their pixels differ only where
real time moved the scene between the two runs. The viewport alone needs no C compiler.

### Quiet re-measurement (2026-10-10)

The timing table above again, on a quiet machine (no other agent;
[quiet-2026-10-09.md](quiet-2026-10-09.md), session 2) with this branch's package: master
`0871920d`'s, rebuilt with this branch's change to the splat tile raster (the cubes pages draw no
splats). `tools/web_paths_bench.py`, five rounds, every round running each page on both GPUs and
both draw paths in turn, each run a fresh headless Chrome 155.0.8059.39 with a temporary profile
(`--headless=new --enable-unsafe-webgpu --disable-frame-rate-limit --disable-gpu-vsync --window-size=1280,720 --no-first-run --no-default-browser-check`,
plus `--force_high_performance_gpu` for the RTX 5060; without it Chrome takes the Radeon 780M),
canvas 1254x564, 10 s per run, the last 300 frames. The pages pin `occlusion=off` (the provisional
runs predate occlusion culling); 4x MSAA, the browser's default
([cubes.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/web/cubes.json);
`python tools/quiet_tables.py web`). Frame time (`requestAnimationFrame` interval, p50) and GPU time
(mean of the timestamped passes over the frames whose timestamps arrived), median of five (minimum;
spread), ms; the provisional values follow the slash:

| Page | Path | RTX 5060 frame | RTX 5060 GPU | Radeon 780M frame | Radeon 780M GPU |
|---|---|---|---|---|---|
| sphere, 100,000 | first-instance | 0.46 (0.40; 16%) / 1.01 | 0.29 (0.28; 5%) / 0.28 | 2.32 (2.24; 11%) / 2.15 | 1.20 (1.07; 20%) / 1.09 |
| | baseline | 0.51 (0.49; 10%) / 0.91 | 0.26 (0.26; 5%) / 0.24 | 2.13 (2.07; 14%) / 2.24 | 1.06 (1.02; 4%) / 1.16 |
| sphere, 400,000 | first-instance | 0.91 (0.88; 5%) / 0.90 | 0.74 (0.73; 4%) / 0.75 | 3.82 (3.62; 8%) / 4.06 | 2.39 (2.31; 13%) / 2.60 |
| | baseline | 0.89 (0.88; 4%) / 0.91 | 0.74 (0.73; 4%) / 0.72 | 3.66 (3.63; 2%) / 4.26 | 2.44 (2.35; 7%) / 2.78 |
| sphere, 1,600,000 | first-instance | 2.79 (2.78; 1%) / 2.82 | 2.64 (2.62; 1%) / 2.67 | 7.48 (7.07; 14%) / 8.63 | 6.13 (5.38; 13%) / 7.48 |
| | baseline | 2.79 (2.78; 1%) / 2.86 | 2.63 (2.63; 0%) / 2.68 | 7.82 (7.05; 14%) / 6.41 | 5.62 (5.47; 22%) / 5.52 |

- **Every frame was timed on both GPUs**: all 300 frames of every run carried timestamps, also on
  the Radeon 780M, whose provisional runs read 0 in most frames.
- **RTX 5060**: the GPU times are the provisional ones within 0.05 ms; the frame times at 100,000
  cubes are half of them (0.46 and 0.51 against 1.01 and 0.91 ms), at 400,000 and 1.6M the same.
  The provisional frame times of the uncapped loop were the load, as suspected.
- **The two draw paths cost the same.** On the RTX 5060 they are within 0.03 ms of GPU time and 0.05
  ms of frame time. On the 780M, whose rounds spread 2 to 22%, the 1.6M rows overlap (5.38 to 6.20
  and 5.47 to 6.70 ms: both paths fall into the same two clusters, about 5.4 to 5.6 and 6.1 to 6.7
  ms), so the provisional 7.48 against 5.52 ms was noise; at 100,000 cubes the baseline path read
  0.14 ms less in every round (1.02 to 1.06 against 1.07 to 1.31 ms), a small difference not
  investigated.
- The 780M takes 5.6 to 6.1 ms of GPU time for 1.6M cubes in Chrome at 1254x564 (provisional 5.5
  to 7.5), 2.1 to 2.3 times the RTX 5060.
