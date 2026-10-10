# Spike: the high-poly rendering path (`render`)

- Date: 2026-10-03. Slice 0 check "high-poly glTF import" and the rendering half of "performance
  budgets" (charter 10).
- Charter: 4.4 (rendering on `wgpu`, high-poly assets, reversed-Z, LOD, culling, instancing), 3.10
  (performance budgets), 4.1 (the web build from day one). Feeds the placeholders of
  `docs/spec/budgets.md`, sections 3 (`{NATIVE_BACKEND}`), 4 (`scene`, `import`), 5.2
  (`import.highpoly`), 5.3 and 5.4 (`web.frame`).
- Code: `spikes/render/` (its `README.md` says how to build and run each part). Every report and
  capture is regenerated into `spikes/render/out/` (ignored by git) by `spikes/render/measure.sh`.

## Question

Can one `wgpu` code base render a high-poly glTF scene both natively (Direct3D 12 on this machine,
offscreen) and in a browser (WebGPU in headless Chrome, whose WGSL compiler is Tint), with
reversed-Z depth, lit PBR-like shading under a directional light, instancing and frustum culling;
import a glTF of several million triangles off the main thread without freezing it; optimize it with
meshoptimizer and draw it with levels of detail? What do the import, the longest main-thread stall,
GPU and CPU frame time at 1920 by 1080, triangles and draw calls measure natively and on the web,
and what numbers should the budgets of charter 3.10 start from?

## Verdict

**Works, with caveats.** One `wgpu` 30.0.1 code base drew the same 4.68M-triangle glTF scene (48.2M
triangles over its instances) natively on Direct3D 12 and in headless Chrome's WebGPU, with
reversed-Z `Depth32Float`, GGX lighting under a sun, instancing, frustum culling and meshoptimizer
levels chosen by projected error; the WGSL passed Tint unchanged. The levels made the frame 13 to 44
times cheaper on the GPU and left the picture within 0.05% of differing pixels. The import ran off
the main thread natively (threads) and on the web (Web Workers sharing the file through a
`SharedArrayBuffer`), drew its first part after 0.1 s natively and 0.4 s on the web, and finished in
3.3 s (native, median of 3) and 3.8 s (web, median of 3). The caveats:

1. **The import builds every level on every load**, and the one 1.18M-triangle hull, simplified on
   one thread, bounds it at about 3 s; four threads or workers are all it can use. Cooking the
   levels once, or splitting large meshes into clusters, is the way down (not built).
2. **The native main thread stalled 8 to 52 ms once per import** with `wgpu`'s default memory hints:
   the allocator's second 128 MiB memory block was made inside a frame. Sizing the blocks for the
   scene (`MemoryHints::Manual`, 256 MiB) removed it; the longest frame was then 2.1 to 14.9 ms.
3. **The web page had gaps of 61 to 102 ms between animation frames in 5 of 17 loads** while its own
   work never exceeded 13.7 ms and no Long Task was recorded; the cause was not found.
4. **Vertex order is GPU-dependent**: ordering the shared vertex buffer for the coarsest level first
   doubled the RTX 5060's full-detail time and did nothing on the Radeon; the full level first is
   the default.
5. **Everything is offscreen** (no presentation, no vsync), and the scene has no textures, shadows
   or sea shading yet; the frame times here are the geometry pass of a later frame.
6. **The machine was shared** with other agents' work: the integrated GPU's p95s, and Chrome's
   medians across loads (13 to 20 ms at full detail), vary more than a quiet machine would show.

## What was built

One Rust crate (`spikes/render/`, standalone, 3,530 lines of Rust in 16 files, the largest
`src/gpu.rs` at 591), built natively and for `wasm32-unknown-unknown`, plus a page and a worker
script.

**The test asset.** `gen` (`src/bin/gen/`) writes `out/scene.glb`: 122,428,588 bytes, 4,681,602
unique triangles and 2,363,720 vertices in 66 meshes and 68 primitives, 100 nodes:

- an island heightfield 2,048 m square, value-noise fBm with ridges and fine detail down to about 2
  m, cut into 8 by 8 tiles of 160 by 160 quads (64 meshes of 51,200 triangles, 3,276,800 in all),
  coloured per vertex (wet and dry sand, grass, rock, scree);
- a sea quad, 40 km square;
- one 12 m sailing yacht of three primitives, 1,404,800 triangles: the hull with keel and rudder
  (1,177,600 triangles: a 1,000 by 560 grid swept along the hull with 5 mm plank seams on the
  topsides and the teak deck, a boot stripe, antifouling), the rig (mast with a sail track, boom)
  and the sail (a cambered foil 8 mm thick with panel seams). Every part is closed;
- 32 nodes placing that one mesh (glTF's own instancing), so drawing every instance at full detail
  is 48,230,402 triangles;
- three perspective cameras: `closeup` (9 m from a yacht's side), `harbour` (5.5 m above the water
  behind the fleet, looking at the island) and `overview` (650 m up).

The generator is a pure function of its arguments: two runs wrote identical bytes.

**Import** (`src/import.rs`, `src/loader.rs`). The `gltf` crate parses the `.glb` in place
(`gltf::Glb::from_slice`, then `Document::from_json`: the binary chunk is not copied) and reads each
primitive's accessors into the renderer's 20-byte vertex: position `f32x3`, normal octahedral in
`snorm16x2`, colour `unorm8x4`. Each primitive is a *part*; each node with a mesh is an instance of
that mesh's parts.

**Optimization** (`src/optimize.rs`, the `meshopt` crate on meshoptimizer 0.25). Per part:

- a chain of levels of detail, each simplified from the previous one toward half its triangles
  (`meshopt_simplifyWithAttributes`, borders locked so the terrain tiles stay stitched, normals
  weighted 0.5 and colours 1.0 in the error), until a level keeps more than 85% of the previous one
  or there are 12. A level's error is the sum of the errors of the steps that made it (an upper
  bound), in metres;
- vertex cache optimization of every level's triangle order;
- one vertex order for all levels (they share one vertex buffer): `meshopt_optimizeVertexFetchRemap`
  over the levels concatenated, the full mesh first.

**Rendering** (`src/gpu.rs`, `src/shader.wgsl`, `src/scene.rs`). `wgpu` 30.0.1 draws into an
offscreen 1920 by 1080 `Rgba8UnormSrgb` target with a `Depth32Float` depth buffer: reversed-Z with
an infinite far plane (`glam`'s Direct3D-convention `perspective_infinite_reverse`, since WebGPU's
clip space is Direct3D's), compare `Greater`, cleared to 0. One pipeline, back faces culled. The
shader is GGX specular with Smith visibility and Schlick Fresnel plus Lambert diffuse under a sun, a
two-colour hemisphere for diffuse ambient and the sky or ground in the mirror direction for specular
ambient, distance fog and an ACES fit. Per frame on the CPU: every instance's every part is tested
as a bounding sphere against the frustum's five planes (no far plane), its level is the coarsest
whose error projects to at most 1 pixel (error times pixels per metre at the sphere's near
distance), and the survivors are grouped by (part, level) into one instanced `draw_indexed` each,
with an 80-byte per-instance vertex buffer (the model matrix's three rows, base colour, metallic,
roughness, level). Each frame's render pass writes timestamps at its start and end; at most two
frames are in flight (`on_submitted_work_done`).

**Native driver** (`src/bin/bench.rs`). A reader thread reads and parses the file, then 4 builder
threads take parts largest first from an atomic counter and send each finished part over a channel.
The main thread runs a 60 Hz loop: it creates the part's buffers, uploads at most 8 MiB a frame
(`queue.write_buffer`) and draws what has arrived (the harbour camera, with levels).
`--upload worker` moves buffer creation and upload onto the builder threads (`wgpu`'s device and
queue are shared between threads natively), and `--block-mb` sets the size of the memory blocks
`wgpu` sub-allocates buffers from. Then the measurements.

**Web form** (`src/web.rs`, `web/`). One wasm module of 1,130,400 bytes (277,778 gzipped at level 9,
before `wasm-opt`) serves both sides. Worker 0 fetches the `.glb`, streaming it into a
`SharedArrayBuffer` of the announced size, and the page shares that buffer with the other workers;
every worker copies it into its wasm memory, parses it (`Importer`), takes parts largest first from
an `Atomics` counter in another `SharedArrayBuffer`, and posts each part's bytes to the page as
transferred `ArrayBuffer`s. The page (`Spike`) runs the same renderer under `requestAnimationFrame`
with the same 8 MiB upload budget, then the same measurement code as natively (`src/bench.rs`). The
page reports through `document.title` for `tools/webcheck.py`, with the full report and
480-pixel-wide PNG captures as console lines that `tools/extract_web.py` turns back into files.

**meshoptimizer on `wasm32-unknown-unknown`.** The `meshopt` crate compiles meshoptimizer's C++ with
the target's C compiler and its own stub headers for wasm32; with clang 23 (`CC`, `CXX`, `AR` for
the target, `llvm-ar` on the path) it built unchanged. meshoptimizer allocates through C++
`operator new` and `operator delete`, which nothing provides on that target, so `src/wasm_alloc.rs`
defines them on Rust's allocator. The module imports nothing from `env`.

**Measurements** (`src/bench.rs`, shared). For each camera and mode: 30 warm-up frames, then 300
measured frames (the order comparisons 100), each timed on the CPU (from building the frame's camera
to the return of `queue.submit`: culling, levels, recording) and on the GPU (the timestamps), with
at most two frames in flight; the wall time per frame over the whole run is reported beside them.
The modes: `all` (every instance at full detail), `cull` (frustum culling), `lod` (culling and
levels, 1 pixel), `lod_no_instancing` (the same, one draw per instance). After the fixed cameras,
the harbour camera turns a whole circle over the 300 frames (`harbour-turn`, in `cull` and `lod`),
so culling and levels change every frame, as charter 3.10's camera path asks. The `cull` and `lod`
frames of each fixed camera are captured and compared pixel by pixel.

## Measurements

**Machine and conditions.** R1 of `docs/spec/budgets.md` (section 3): AMD Ryzen 9 270 (8 cores, 16
threads), NVIDIA GeForce RTX 5060 Laptop GPU (driver 617.14), AMD Radeon 780M (driver
32.0.13062.3005), Windows 11 Pro 10.0.26200, Chrome 154.0.8037.93, on AC power (battery status 2)
under the Balanced plan. Release builds. Other agents' builds, benchmarks and browser checks shared
the machine throughout; budgets.md asks for a quiet machine, which this was not, so the p95 columns
and the integrated GPU's numbers carry that noise. Headless Chrome drew on the Radeon 780M in every
load, `?power=high` included: Chrome 154 logs "The powerPreference option is currently ignored when
calling requestAdapter() on Windows" (crbug.com/369219127).

**Commands.** From `spikes/render/`: `cargo build --release --bins`, `./build_web.sh`,
`cargo run --release --bin gen`, then `./measure.sh all` (native, web, import, extra; about 15
minutes here), which writes every report named below as `out/<report>.json`;
`python tools/summarize.py` prints them as tables. Each report's exact command is its line in
`measure.sh`; for example `native-nvidia-dx12` is `bench --adapter nvidia --label nvidia-dx12` and
`web-amd` is
`python tools/webcheck.py "http://127.0.0.1:8703/web/index.html?frames=300" --serve spikes/render/out --port 8703`
from the repository's root.

All frame numbers are at 1920 by 1080 (one row at 1280 by 720), 300 frames after 30 of warm-up, GPU
time from the render pass's timestamps and CPU time from building the camera to the return of
`submit`, as median / p95 in milliseconds.

### Full detail: what the levels save

`all` is 48,230,402 triangles in 68 instanced draws; `cull` at the harbour camera keeps 34,972,802;
the turning camera's `cull` frames range from open sea to the whole fleet (its p95 is the busy end).

| GPU and API | closeup `all` | harbour `all` | overview `all` | harbour `cull` | harbour-turn `cull` |
|---|---|---|---|---|---|
| RTX 5060, Direct3D 12, native | 5.90 / 7.41 | 6.66 / 8.00 | 7.42 / 8.99 | 5.42 / 5.85 | 0.34 / 5.31 |
| RTX 5060, Vulkan, native | 5.53 / 6.53 | 6.36 / 6.81 | 7.16 / 7.53 | 5.00 / 5.22 | 0.35 / 5.15 |
| Radeon 780M, Direct3D 12, native | 12.84 / 13.83 | 13.26 / 14.00 | 13.33 / 17.41 | 9.23 / 10.73 | 0.98 / 12.33 |
| Radeon 780M, Vulkan, native | 17.17 / 24.16 | 20.34 / 24.45 | 16.67 / 19.84 | 13.65 / 17.23 | 1.61 / 14.93 |
| Radeon 780M, Chrome WebGPU (`web-amd`) | 16.69 / 20.13 | 19.30 / 22.81 | 20.15 / 23.43 | 13.43 / 15.83 | 1.97 / 15.54 |
| the same, second load (`web-amd-run2`) | 13.26 / 24.32 | 15.34 / 24.87 | 14.90 / 17.72 | 10.68 / 17.42 | 1.78 / 10.05 |
| the same, third load (`web-amd-run3`) | 14.08 / 21.36 | 16.13 / 22.75 | 14.92 / 22.91 | 10.48 / 13.72 | 1.74 / 10.44 |

On the same Radeon 780M, Chrome's WebGPU took 13 to 20 ms (medians) where native Direct3D 12 took
12.8 to 13.3 ms, and three loads of the same page differed by up to 5 ms in their medians. On the
RTX 5060, Vulkan and Direct3D 12 are within 7% of each other; on the Radeon, Vulkan was 25 to 53%
slower than Direct3D 12 at full detail.

### With levels of detail: the shipped configuration

Culling and levels at 1 pixel. Triangles: closeup 394,838 in 18 draws, harbour 513,186 in 63,
overview 187,748 in 68 (46, 122 and 161 draws without instancing); the turning camera's maximum is
in the last column.

| GPU and API | closeup | harbour | overview | harbour-turn GPU | harbour-turn CPU | harbour-turn triangles, draws (max) |
|---|---|---|---|---|---|---|
| RTX 5060, Direct3D 12, native | 0.19 / 0.20 | 0.20 / 0.21 | 0.17 / 0.17 | 0.10 / 0.18 | 0.14 / 0.23 | 531,426, 63 |
| RTX 5060, Vulkan, native | 0.21 / 0.22 | 0.20 / 0.22 | 0.19 / 0.19 | 0.09 / 0.16 | 0.09 / 0.17 | 531,426, 63 |
| Radeon 780M, Direct3D 12, native | 0.69 / 1.25 | 0.68 / 0.95 | 0.60 / 0.93 | 0.34 / 0.73 | 0.17 / 0.32 | 531,426, 63 |
| Radeon 780M, Vulkan, native | 0.57 / 0.61 | 0.73 / 0.78 | 0.62 / 0.73 | 0.32 / 0.67 | 0.14 / 0.27 | 531,426, 63 |
| Radeon 780M, Chrome WebGPU (`web-amd`) | 1.30 / 1.38 | 0.84 / 1.24 | 0.98 / 1.15 | 0.57 / 1.24 | 0.10 / 0.21 | 531,426, 63 |
| the same, second load | 0.91 / 1.23 | 0.80 / 0.92 | 0.80 / 1.03 | 0.49 / 1.16 | 0.10 / 0.18 | 531,426, 63 |
| the same, third load | 0.80 / 1.10 | 0.81 / 1.26 | 0.75 / 0.94 | 0.51 / 1.13 | 0.08 / 0.17 | 531,426, 63 |
| the same at 1280 by 720 (`web-amd-720p`) | 0.75 / 0.79 | 0.72 / 0.78 | 0.66 / 0.70 | 0.35 / 0.73 | 0.07 / 0.15 | 398,150, 61 |

The levels take the scene from 48.2M triangles to 0.19M to 0.53M, and the GPU time from 6 to 7 ms to
0.2 ms on the RTX 5060 and from 13 to 20 ms to 0.6 to 1.3 ms on the Radeon 780M. In Chrome the time
per frame with two frames in flight (`wall_ms_per_frame` in the reports) was 1.5 to 2.3 ms against
0.5 to 1.3 ms of GPU time: `onSubmittedWorkDone` resolved 0.7 to 1.5 ms after the work ended.

### Does the picture change?

Each fixed camera's `cull` and `lod` frames compared pixel by pixel: the mean absolute difference
per channel (0 to 255), then the share of pixels where a channel differs by more than 16.

| Levels simplified on | closeup | harbour | overview | LOD triangles (closeup, harbour, overview) | Simplification, summed over parts |
|---|---|---|---|---|---|
| positions only (`native-nvidia-dx12-positions-only`) | 1.14, 2.30% | 0.63, 1.26% | 0.20, 0.17% | 195,368, 238,070, 97,572 | 3.4 s |
| positions, normals (0.5) and colours (1.0), the default (`native-nvidia-dx12`) | 0.10, 0.05% | 0.10, 0.04% | 0.07, 0.01% | 394,838, 513,186, 187,748 | 9.9 s |

The default comparison on the Radeon natively and in Chrome gave the same pairs to two decimals; at
1280 by 720 in Chrome 0.11, 0.06%; 0.14, 0.07%; 0.09, 0.02%. The captures are
`out/native-<label>-<camera>-{cull,lod}.png` and, 480 pixels wide, `out/web-amd-<camera>-*.png`.

### Vertex order and full-detail time

`all` at the closeup, harbour and overview cameras (GPU medians, ms; 100 frames) for four orders of
the vertices and triangles, beside meshoptimizer's analysis of the hull's full level (ACMR: vertices
shaded per triangle with a 16-entry cache; overfetch: bytes fetched over bytes needed):

| Order | RTX 5060 D3D12 | Radeon 780M D3D12 | Radeon 780M Chrome | Hull ACMR, overfetch |
|---|---|---|---|---|
| as generated (`--no-optimize`, `?optimize=0`) | 6.58 / 7.04 / 7.79 | 20.07 / 19.51 / 21.77 | 17.16 / 17.35 / 18.23 | 1.00, 1.00 |
| triangles for the vertex cache, vertices as generated (`--fetch keep`) | 5.89 / 6.53 / 7.45 | 12.99 / 15.94 / 16.29 | not measured | 0.75, 1.00 |
| the same, vertices ordered full level first (the default) | 5.90 / 6.66 / 7.42 | 12.84 / 13.26 / 13.33 | 16.69 / 19.30 / 20.15 | 0.75, 1.00 |
| the same, vertices ordered coarsest level first (`--fetch coarsest`) | 12.43 / 13.16 / 13.94 | 12.95 / 13.39 / 13.65 | 14.65 / 14.42 / 14.26 | 0.75, 1.70 |

The vertex cache pass with the default order cut the Radeon's full-detail time by 32 to 39% and the
RTX 5060's by 5 to 10%. Ordering the vertices for the coarsest level first doubled the RTX 5060's
time and did not hurt the Radeon (in Chrome it measured faster, inside that page's load-to-load
spread). The generated grids are already in fetch order, so this asset cannot show what the fetch
pass gains on an exporter's scrambled mesh.

### Draw calls: 2,000 more yachts

`--fleet 2000` (`?fleet=2000`) adds 2,000 instances of the yacht on a ring of sea, 2,032 yachts and
6,161 instance-parts in all. At the overview camera 4,358 instance-parts survive culling:

| GPU and API | Triangles | Instanced draws | CPU | GPU | Draws without instancing | CPU | GPU | Extra CPU per draw |
|---|---|---|---|---|---|---|---|---|
| RTX 5060, Direct3D 12 | 1,362,442 | 70 | 0.51 / 0.59 | 0.39 / 0.47 | 4,358 | 1.20 / 1.43 | 0.36 / 0.37 | 0.16 us |
| Radeon 780M, Direct3D 12 | 1,362,442 | 70 | 1.21 / 1.52 | 1.11 / 1.37 | 4,358 | 1.48 / 2.38 | 1.41 / 1.52 | 0.06 us |
| Radeon 780M, Chrome WebGPU | 1,362,442 | 70 | 0.53 / 0.78 | 1.19 / 1.78 | 4,358 | 0.85 / 1.15 | 1.65 / 2.07 | 0.08 us |

The extra CPU per draw is the difference of the CPU medians over the 4,288 extra draws. At this
scale neither the CPU's culling and level choice nor one draw per instance comes near a frame: 2.4
ms or less at p95 on every path. With the fleet the turning camera peaked at 1,284,306 triangles in
67 draws.

### Import

The `.glb` was read in 28 to 31 ms natively and parsed in under 1 ms; Chrome's worker streamed it in
179 to 263 ms from the local server. Building the 68 parts is the import: simplification took 5.6 to
15.2 s summed over parts and threads (the spread is the shared machine's) and the cache and fetch
passes 1.0 to 2.0 s; the hull alone (1,177,600 triangles, one part, so one thread) took 2.6 to 4.7
s. Three runs each, separated by slashes:

| Run (natively: 4 builder threads unless said) | Import ms (median) | First part drawn ms | Longest main-thread frame ms | Of it, buffer creation ms |
|---|---|---|---|---|
| RTX 5060 D3D12, 1 thread (`native-import-t1`) | 7787 / 6995 / 8077 (7787) | 3055 / 2752 / 2830 | 8.9 / 8.8 / 23.8 | 8.1 / 7.9 / 22.4 |
| RTX 5060 D3D12 (`native-import-t4`) | 3552 / 2871 / 3307 (3307) | 103 / 102 / 103 | 9.2 / 9.3 / 9.5 | 8.1 / 8.0 / 7.8 |
| the same, threads create and fill buffers (`--upload worker`) | 3121 / 3535 / 2767 (3121) | 119 / 136 / 102 | 6.9 / 4.9 / 4.7 | not on this thread |
| the same, 256 MiB memory blocks (`--block-mb 256`) | 3772 / 4051 / 3134 (3772) | 136 / 102 / 103 | 5.2 / 6.8 / 6.8 | 0.2 / 1.0 / 0.2 |
| RTX 5060 Vulkan | 3693 / 3727 / 3609 (3693) | 102 / 102 / 103 | 2.3 / 2.7 / 2.5 | 0.9 / 1.6 / 1.1 |
| Radeon 780M D3D12 | 3448 / 3186 / 2956 (3186) | 102 / 102 / 102 | 51.9 / 43.5 / 46.1 | 50.7 / 41.9 / 44.6 |
| the same, threads create and fill buffers | 3713 / 2977 / 2978 (2978) | 119 / 102 / 102 | 40.2 / 22.4 / 36.6 | not on this thread; `submit` waited |
| the same, 256 MiB memory blocks | 4863 / 3872 / 4103 (4103) | 153 / 119 / 120 | 4.5 / 2.1 / 14.9 | 0.4 / 0.1 / 0.3 |
| Radeon 780M Vulkan | 3072 / 3075 / 3039 (3072) | 102 / 103 / 103 | 30.2 / 30.4 / 31.8 | 29.2 / 29.3 / 30.8 |

The long native frames are one buffer creation per import, for the same small part (a 1.7 MB terrain
tile) every time: the moment the allocated bytes outgrow `wgpu`'s first 128 MiB memory block
(`MemoryHints::Performance`) and the allocator makes a second. With 256 MiB blocks
(`MemoryHints::Manual`, `--block-mb 256`) the whole asset fits the block made at start-up and the
stall is gone. Creating the buffers on the builder threads does not help on the Radeon: the main
thread's `submit` waited for the allocation instead. The RTX 5060 under Vulkan showed no such stall.

On the web (Radeon 780M, Chrome) the page measured its own work inside each animation frame, the gap
between animation frames, and Long Tasks:

| Run | Import ms (median) | First part drawn ms | Longest page work in a frame ms | Longest gap between frames ms | Long Tasks |
|---|---|---|---|---|---|
| 1 worker (`web-import-w1`) | 8726 / 7827 / 9551 (8726) | 4123 / 3734 / 4624 | 6.5 / 5.5 / 7.5 | 11.3 / 10.4 / 27.9 | 0 / 0 / 0 |
| 4 workers (`web-import-w4`) | 4190 / 3780 / 3798 (3798) | 424 / 400 / 409 | 6.4 / 4.1 / 6.6 | 70.1 / 15.4 / 10.7 | 0 / 0 / 0 |
| 4 workers made, wasm instantiated, before timing (`web-import-w4-prewarm`) | 4303 / 3999 / 4301 (4301) | 446 / 386 / 410 | 4.5 / 5.9 / 8.0 | 61.2 / 97.8 / 101.5 | 0 / 0 / 0 |

Over all 17 page loads of the campaign the page's own work in a frame never exceeded 13.7 ms and no
Long Task was recorded, but 5 loads had a gap between animation frames over 33 ms (80, 70.1, 61.2,
97.8 and 101.5 ms): two at the file's arrival, three in the middle of the parts, each after a frame
of at most 2.8 ms of page work. Whether the gaps are the browser's (its GPU process or compositor)
or the shared machine's was not established.

## Proposed starting budgets

For the placeholders of `docs/spec/budgets.md`. "Rule" is that document's 8.1:
`min(ceiling, measured x 1.25)`, rounded up to two significant digits. Native rows are the RTX 5060
on Direct3D 12, web rows the Radeon 780M in Chrome, as budgets.md section 3 assigns them.

| Placeholder | Proposed | Measured on R1 (report) | Note |
|---|---|---|---|
| `{NATIVE_BACKEND}` | Direct3D 12 | Vulkan within 7% on the RTX 5060's GPU time and 0.05 to 0.2 ms lower on CPU; Direct3D 12 25 to 53% faster on the Radeon | Direct3D 12 is the path Chrome's Dawn takes on Windows, needs no runtime beyond the OS (FXC), and was faster on the integrated GPU. Size the memory blocks (`MemoryHints::Manual`) whichever backend |
| `{NATIVE_RES}`, `{WEB_RES}` | 1920 by 1080; 1280 by 720 | both measured | |
| `{SCENE_ISLANDS}`, `{SCENE_BOATS}` | 1 island (64 tiles), 32 boats | `out/scene.glb` | vegetation is not in this asset |
| `{FRAME_SAMPLES}`, `{FRAME_WARMUP}` | 300 frames of a camera turning a whole circle, after 30 | `harbour-turn` | static cameras miss the culling and level churn |
| `scene.triangles` (floor) | 48,000,000 over instances (4,680,000 unique) | 48,230,402 (4,681,602) | floors follow the designed scene, not the measurement |
| `scene.instances` (floor) | 97 mesh instances, 161 instance-parts | 97, 161 | 2,000 more yachts are the draw stress |
| `frame.triangles` (max) | 670,000 | 531,426 (`native-nvidia-dx12`, harbour-turn `lod`) | rule; 1,284,306 with 2,000 more yachts |
| `frame.draws` (max) | 85 | 68 (overview `lod`); 63 on the turn | rule; instanced draws do not grow with instances (70 with 2,000 more yachts) |
| `frame.gpu` (p95) | 16.7 ms for the frame; 0.23 ms for this geometry pass | 0.18 ms (harbour-turn `lod`) | the rule would set 0.23 ms for a whole frame that will hold the sea, shadows and post-processing: keep the ceiling until slice 3 measures a full frame, and use 0.23 ms as the opaque pass's own budget for this asset |
| `frame.cpu` (p95) | 0.29 ms | 0.23 ms (harbour-turn `lod`) | rule; 1.43 ms with 2,000 more yachts drawn one by one |
| `frame.dropped` | 0 | 0: nothing here has a fixed capacity (the instance buffer doubles) | |
| `import.highpoly` (median of 3) | 4.2 s for this asset | 3,307 ms (`native-import-t4`); 3,772 ms with 256 MiB blocks | rule; the import ends when every part is on the GPU. A cooked asset should fall to its read and upload |
| the import's longest presenter frame (no placeholder yet) | 16.7 ms, the frame | 6.8 ms with 256 MiB blocks on the RTX 5060; 14.9 ms on the Radeon | with default memory hints 9.5 ms and 52 ms: the budget would have caught it |
| `web.frame` (p95, 1280 by 720) | 33.3 ms for the frame; 0.92 ms for this geometry pass | 0.73 ms GPU (`web-amd-720p`, harbour-turn `lod`) | as `frame.gpu`; full detail without levels was 18 to 23 ms at p95 |
| web import (no placeholder yet) | 4.8 s, first part 0.52 s | 3,798 ms and 409 ms (`web-import-w4`, medians) | rule |
| web page work in a frame during import (no placeholder yet) | 16.7 ms | 13.7 ms at most over 17 loads | the gaps between frames (up to 101.5 ms) need explaining before a gap budget is set |
| `web.size.wasm` (for information) | | 1,130,400 bytes, 277,778 gzipped (`wasm-bindgen`, no `wasm-opt`) | renderer, importer and meshoptimizer only |

## WGSL and Tint

The shader (`src/shader.wgsl`, 112 lines) compiled in Chrome 154's Tint on the first load and in
every run after, with no error or warning in the console; natively `naga` translated it to HLSL for
FXC (`d3dcompiler_47.dll`: `wgpu`'s default `Dx12Compiler::Auto` falls back to FXC when no
`dxcompiler.dll` is on the path, as here) and to SPIR-V for Vulkan. Nothing had to change for Tint.
What it avoids, by construction, is what master found Tint rejects where `naga` accepts
(`docs/web.md` and `docs/design/rendering.md` on master): it samples no texture, so the uniform
control flow rule around `textureSample` does not arise; no expression mixes `*` with `^` or `&&`
with `||` without parentheses; branches are `select` rather than divergent `if`; `pow` with a
possibly negative base is written as products (`pow5`); and it uses only core vertex formats
(`float32x3`, `snorm16x2`, `unorm8x4`, `float32x4`) and no optional extension (`f16`, subgroups). A
renderer with textures must recheck the uniformity rule in the browser for every shader that samples
one (`AGENTS.md`, rule 10 on master; charter 4.4).

## Recipe for slice 3

1. **One renderer, every backend.** `wgpu` 30 with its default features; native Direct3D 12 chosen
   explicitly on Windows (`Backends::DX12`), the browser's WebGPU on `wasm32`
   (`Backends::BROWSER_WEBGPU`). The same WGSL goes to `naga` natively and to Tint in Chrome; load
   the page with `tools/webcheck.py` after every shader change (charter 4.4).
2. **Depth.** `Depth32Float`, compare `Greater`, clear to 0, and the projection
   `glam::camera::rh::proj::directx::perspective_infinite_reverse` (near maps to 1, infinity to 0).
   Frustum culling then has five planes, from the view-projection's rows: left `r3 + r0`, right
   `r3 - r0`, bottom `r3 + r1`, top `r3 - r1`, near `r3 - r2`.
3. **Import off the main thread.** Parse the `.glb` in place (`gltf::Glb::from_slice`,
   `gltf::json::Root::from_slice`, `gltf::Document::from_json`) and read accessors with
   `Primitive::reader`. Natively: a reader thread, then a pool of builder threads taking parts
   largest first from an atomic counter. On the web: one worker streams the file into a
   `SharedArrayBuffer` (the page is cross-origin isolated), all workers copy it into their wasm
   memory, take parts from an `Atomics` counter, and transfer each part's bytes to the page.
4. **Per part: levels, then order.** `meshopt_simplifyWithAttributes` from each level to the next,
   half the triangles, borders locked (tiles stay stitched), normals at 0.5 and colours at 1.0 in
   the error, the error summed along the chain; stop past 85% or at 12 levels. Then
   `meshopt_optimizeVertexCache` per level and one `meshopt_optimizeVertexFetchRemap` over all
   levels, the full mesh first. Call the C functions where the `meshopt` crate's wrappers are wrong
   (problem 4).
5. **Upload progressively.** The thread that owns the device creates a part's buffers when the part
   arrives and writes at most 8 MiB a frame; a part draws once all its bytes are written. Natively
   the builder threads may create and fill buffers themselves, which shortened the RTX 5060's
   longest frame but not the Radeon's.
6. **Draw.** Per frame on the CPU: sphere against frustum per instance and part, the coarsest level
   whose error is within 1 pixel, one instanced `draw_indexed` per (part, level) from an
   80-byte-per-instance vertex buffer that grows by doubling (no fixed capacity, so nothing can be
   dropped). At most two frames in flight.
7. **Measure every frame.** Timestamps at the start and end of each pass (`timestamp-query` is
   granted in Chrome 154 on the Radeon 780M, natively on both GPUs); CPU time from building the
   camera to `submit`'s return; triangles and draws per frame.
8. **Size the GPU memory blocks.** `MemoryHints::Manual` with blocks that hold the scene's buffers
   (256 MiB here), or grow them before they are needed: making a block inside a frame was the native
   main thread's only long frame.
9. **Cook it.** Not done here: store each part's levels and vertex order next to the source (or in a
   pack) so that a later load is read, copy and upload. The import measurements say what that would
   leave (read and parse against simplify and optimize).
10. **Build the web form** with `build_web.sh`: clang for meshoptimizer's C++, `operator new` and
    `delete` from Rust (`src/wasm_alloc.rs`), `wasm-bindgen --target web`, module workers.

## Versions and URLs

| What | Version | Where |
|---|---|---|
| Rust | 1.98.1 (`rust-toolchain.toml`), targets `x86_64-pc-windows-msvc` and `wasm32-unknown-unknown` | https://www.rust-lang.org |
| `wgpu`, `wgpu-core`, `wgpu-hal`, `naga` | 30.0.1 | https://crates.io/crates/wgpu |
| `meshopt` (meshoptimizer 0.25 vendored) | 0.6.2 | https://crates.io/crates/meshopt, https://github.com/zeux/meshoptimizer |
| `gltf`, `gltf-json` (features `utils`, `names`) | 1.4.1 | https://crates.io/crates/gltf |
| `glam` | 0.33.12 | https://crates.io/crates/glam |
| `bytemuck` | 1.25.2 | https://crates.io/crates/bytemuck |
| `serde_json` | 1.0.151 | https://crates.io/crates/serde_json |
| `futures-channel`, `futures-core` | 0.3.34 | https://crates.io/crates/futures-channel |
| `pollster` (native) | 0.4.0 | https://crates.io/crates/pollster |
| `png` (native) | 0.18.1 | https://crates.io/crates/png |
| `wasm-bindgen`, `wasm-bindgen-cli` | 0.2.129 | https://crates.io/crates/wasm-bindgen |
| `wasm-bindgen-futures`, `js-sys`, `web-sys` | 0.4.79, 0.3.106, 0.3.106 | https://crates.io/crates/web-sys |
| `cc` (builds meshoptimizer) | 1.6.0 | https://crates.io/crates/cc |
| clang, `llvm-ar` (wasm32 C++) | 23.1.2, at `~/.pocket-tools/llvm-23.1.2` (already on the machine) | https://github.com/llvm/llvm-project |
| MSVC (native C++ and linking) | Visual Studio 18 Build Tools, found by `cc` | |
| Google Chrome (headless, `tools/webcheck.py`) | 154.0.8037.93 | |
| NVIDIA driver | 617.14 (`nvidia-smi`; Windows' 32.0.16.1714) | |
| AMD driver | 32.0.13062.3005 (`wgpu` adapter info) | |

Nothing was downloaded or installed beyond the crates above.

## Problems met and how they were solved

1. **`wgpu` chose Vulkan.** With Direct3D 12 and Vulkan both enabled, `request_adapter` returned the
   RTX 5060 on Vulkan. The backend is now explicit (`--backend`, Direct3D 12 by default), and Vulkan
   is measured separately.
2. **`wgpu` 30's API.** `RequestAdapterOptions` gained `apply_limit_buckets` (filled by `Default`),
   `DepthStencilState`'s `depth_write_enabled` and `depth_compare` are `Option`s,
   `PipelineLayoutDescriptor` takes `bind_group_layouts: &[Option<&BindGroupLayout>]` and
   `immediate_size`, and render passes and pipelines take `multiview_mask`. glam 0.33 deprecates
   `Mat4::perspective_infinite_reverse_rh` for
   `glam::camera::rh::proj::directx::perspective_infinite_reverse` (Direct3D's clip space, which is
   WebGPU's). The `gltf` crate needs its `names` feature for `Node::name`.
3. **meshoptimizer's C++ on `wasm32-unknown-unknown`.** The `meshopt` crate's build script compiles
   it with the target's compiler and calls `llvm-ar` by name on Windows hosts, so the build needs
   `CC_wasm32_unknown_unknown=clang`, `CXX_...=clang++`, `AR_...=llvm-ar` and clang's directory on
   the path (`build_web.sh`). meshoptimizer's default allocator is `operator new` and
   `operator delete`, undefined on that target; `src/wasm_alloc.rs` defines `_Znwm`, `_Znam`,
   `_ZdlPv` and `_ZdaPv` on Rust's allocator with a 16-byte size header. The module then imports
   nothing from `env`.
4. **Two wrapper bugs in `meshopt` 0.6.2.** `optimize_vertex_fetch_remap` truncates the remap table
   to the number of unique vertices, which loses entries whenever a vertex is unused, and
   `simplify_with_attributes_and_locks` passes its lock slice's pointer whatever the slice's length
   while the C side reads one byte per vertex (an empty slice reads out of bounds). Both are called
   through `meshopt::ffi` instead, the lock array as a null pointer.
5. **Levels by position alone changed the picture.** With the simplifier's error on positions only,
   the far levels erased the plank seams' shading and made the boot stripe jagged while staying
   within a pixel geometrically: 2.30% of the closeup's pixels differed by more than 16 levels.
   Normals (weight 0.5) and colours (weight 1.0) in the error brought it to 0.05% or less, for more
   triangles (the LOD frames' counts above) and longer simplification.
6. **The vertex order for all levels.** Ordering the shared vertex buffer by the coarsest level
   first (so a far level reads a compact prefix) made the full-detail frame about twice as slow on
   the RTX 5060 (12.43 against 5.90 ms at the closeup): the full mesh's fetch locality is what the
   full-detail frame pays for. The order is now the full mesh first; the comparison is in the
   measurements.
7. **One buffer creation per import held the main thread 8 to 52 ms.** Traced per part
   (`RENDER_SPIKE_TRACE=1`), it was always the same 1.7 MB terrain tile, on both GPUs: the moment
   the allocated bytes outgrew `wgpu`'s first 128 MiB memory block (`MemoryHints::Performance`
   starts at 128 MiB, `wgpu-hal`'s `AllocationSizes`), so the allocator made a second block inside a
   frame. Creating buffers on the builder threads moved the wait into the main thread's `submit` on
   the Radeon; 256 MiB blocks (`MemoryHints::Manual`, `--block-mb 256`) made the block at start-up
   and removed the stall on both GPUs.
8. **Gaps between the page's animation frames.** In the first web runs a single
   `response.arrayBuffer()` of 122 MB in the worker, or four workers each fetching the file,
   coincided with gaps of 45 to 94 ms with no Long Task on the page. One worker now streams the body
   into a `SharedArrayBuffer` and shares it, which ended the four parallel fetches (1.4 s each) but
   not the gaps: they came back in 5 of the campaign's 17 loads, also mid-import with a pre-warmed
   pool (open).
9. **A module worker that fails to parse** reports to the page only an `error` event without a
   message (`worker error: undefined`); a duplicated `const` in `worker.js` cost a run to find.
10. **A shared machine.** Other agents' builds, benchmarks and browser checks ran during every
    measurement here (load on 16 threads and on both GPUs), which shows as the gap between median
    and p95 on the integrated GPU. Every number is from one run unless said otherwise.

## What remains open

- **Presentation.** Everything here is offscreen: no swapchain or canvas, no vsync, no compositor;
  `start.window` and `web.start.*` were not measured.
- **The rest of charter 4.4's image basics**: cascaded shadows, specular anti-aliasing, MSAA or TAA,
  textures (and with them Tint's uniformity rule around `textureSample`) and compressed textures.
  The full-detail frames alias visibly on the deck seams (the captures).
- **GPU-driven drawing**: indirect draws, culling on the GPU, Hi-Z occlusion culling, clusters with
  their own levels (meshoptimizer's `buildMeshlets` and its cluster LOD). WebGPU has no
  multi-draw-indirect. Here the draw count stayed at 70 or fewer with instancing, and the CPU's
  culling, level choice and recording cost at most 1.5 ms (2.4 ms at p95) even for 4,358 draws, so
  this scene does not yet need it.
- **Cooked levels.** The import builds every level on every load; simplification is most of it, and
  the largest part (the hull, 1.18M triangles, simplified on one thread) bounds it. Splitting large
  meshes into clusters to simplify in parallel, or cooking once, are the two ways down; neither was
  built.
- **The page's frame gaps** of 61 to 102 ms in 5 of 17 loads (problem 8, Import) were not explained:
  the browser's GPU process or compositor, or the shared machine.
- **Memory.** Peak process memory was not measured, natively or per worker; on the web each worker
  holds its own copy of the 122 MB file in wasm memory.
- **Rendering from a worker.** The renderer runs on the page's thread. The charter's web form puts
  the game in a worker (5.1); drawing from a worker through `OffscreenCanvas` was not tried.
- **Other hardware**: the owner's Apple M5 (Metal), other browsers, a quiet machine.
- **The level threshold.** One pixel of an error that mixes metres with weighted normal and colour
  deviation is a tuned rule, not a geometric bound; it held the captures within 0.05% of differing
  pixels here, with no textures.
- **Chrome's flags.** `tools/webcheck.py` starts Chrome with `--enable-unsafe-webgpu`; whether a
  default Chrome grants `timestamp-query` with the same resolution (the medians here, such as 0.57
  ms, are not multiples of 0.1 ms) was not checked.
