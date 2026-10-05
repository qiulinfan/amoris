# Web rendering against three.js

- Date: 2026-10-04. Apple M5, Google Chrome 152 headless (`--headless=new --enable-unsafe-webgpu
  --disable-frame-rate-limit --disable-gpu-vsync`), window 1280x720 (canvas 1280x577, DPR 1),
  driven by `tools/web_bench.mjs` over CDP; the mean of the last 300 frames after a warm-up.
- Scene: Bevy's many_cubes sphere layout (radius 500, Fibonacci spiral, cubes facing the centre,
  camera at the centre turning 0.15/60 rad about z and x per frame), one mesh, one material, a
  directional light, 4x MSAA.
- three.js r186 (`web/bench/threejs_cubes.html`): `WebGPURenderer` with one `InstancedMesh` (its
  fastest path for copies of a mesh; frustum culling is per mesh, so every instance is drawn) and
  `MeshStandardMaterial`; GPU time from `trackTimestamp` / `resolveTimestampsAsync`.
- Pocket3D (`web/viewport/?demo=cubes`): `crates/pocket-viewport` (the same renderer as native,
  WebAssembly + WebGPU): per-instance GPU culling, indirect draws, clustered PBR, atmosphere sky
  with image-based light, bloom and AgX; GPU time from timestamp queries summed over passes.

| Cubes | three.js frame | three.js GPU | Pocket3D frame | Pocket3D GPU |
|---|---|---|---|---|
| 100,000 | 1.00 ms | 2.45 ms | 0.95 ms | 0.71 ms |
| 400,000 | 4.17 ms | 2.36 ms | 2.36 ms | 1.81 ms |
| 1,600,000 | 24.8 ms | 21.9 ms | 15.0 ms | 14.7 ms |

Frame time is the requestAnimationFrame interval without a frame-rate cap (it measures how fast
the page submits; GPU time is what the frames cost).

Module sizes: `pocket_viewport_bg.wasm` 1.08 MB (0.38 MB gzipped) after `wasm-opt -O3`, plus 81 KB
of JS glue; three.js r186's `three.webgpu.js` + `three.core.js` are about 1.9 MB unminified.

Open item: Pocket3D's GPU time in the browser at 1.6M cubes (14.7 ms at 1280x577) is far above
native Metal (2.3-3.4 ms at 2560x1440); the culling pass alone takes 3.4 ms against 1.2 ms. The
suspects are the browser's robustness bounds checks on storage-buffer reads in the per-vertex
pose interpolation and the cull pass; moving the interpolated transform into the cull pass (one
48-byte matrix per visible instance) is the planned fix.
