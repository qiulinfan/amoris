# Rendering and model import, assessed (2026-10-02)

Where the renderer stands against three.js and the frontier, and how well high-poly OBJ files from
industrial software import. Written at `a171c85a` on an Apple M5 (32 GB) under wgpu-native 29.0.1.1
on Metal. Four agents surveyed in parallel (our renderer from its code and evidence images, three.js
r186, the frontier, an OBJ stress test against Blender), and each survey was then checked by a
separate fact-checking agent: of 31 claims about our renderer 22 held as written, of 33 about
three.js 28, of 38 about the frontier 25; the corrections are folded in below. The raw reports,
sources and checks are in `2026-10-02-rendering-assessment-sources.json` beside this file. An agent
benchmark ran on the same machine throughout, so every timing here is noisy (load averages 5 to 13
on 10 cores).

## Verdict

The technique list is broad and the images are not yet good. The renderer is a clustered forward
renderer with roughly the feature set of an AAA forward renderer of 2016 to 2019 (clustered lights
with shadows, four sun cascades with PCSS, SSAO, SSR, one-bounce SSGI, TAA, reflection probes,
DDGI-style irradiance volumes with rasterized captures, volumetric fog) plus an integrated sky,
atmosphere, day, weather, volumetric clouds, ocean, terrain and grass. The pictures read as a
stylized indie demo: mostly because of content (primitives, low-poly props, procedural and checker
textures, no authored PBR texture sets), partly because of artifacts (below) and partly because of
structural gaps (no GPU-driven drawing, no temporal upscaler, GI only screen-space and rasterized
probes). Against UE5 it is a generation or more behind; against three.js the technique lists are
close, three.js is ahead on materials, texture compression, upscaling and depth precision and far
ahead on ecosystem, and we are ahead on integration and on being a whole engine.

## The renderer as it stands

- **Frame**: CPU light gather into a 16x9x24 froxel grid (1024 lights); one 464-byte object row per
  submesh in a storage buffer (65,536 rows a frame); per-draw frustum culling and LOD choice on the
  CPU; sky environment compute when the sky changes; four 2048 sun cascades (kept between frames,
  about two of four redrawn); weather shelter map; at most one reflection-probe and two
  irradiance-probe captures a frame; a 4096 atlas of 64 local-light shadow faces; GPU particles; the
  `ids` prepass (id, velocity, normal-roughness-metallic, albedo) with half-resolution AO and
  contact shadows; quarter-resolution clouds; the scene pass (sky, grass, opaque, cut-out, skinned,
  glass, translucency or OIT, water, sprites, particles, weather); SSGI, SSR, TAA, DOF and motion
  blur, volumetric fog, bloom; the final pass (exposure, height fog, tonemap, grade and LUT,
  outlines, colour vision), project post effects, spatial upscale, UI
  (`engine/renderer/src/renderer.cpp`, `Renderer::render_scene`, 11162 to 13410).
- **Submission**: CPU-built instanced draws, one `DrawIndexed` per run of equal mesh, submesh and
  material; 32-bit indices, 40-byte vertices. No indirect or multi-draw, no GPU or occlusion
  culling, no meshlets, no bindless. LOD is discrete meshoptimizer simplification, opted into per
  entity (`MeshRenderer.lods`). A mesh is rasterized about four times a frame (prepass, scene pass
  with MSAA, about two cascades); the scene pass clears and redraws its own depth instead of reusing
  the prepass's.
- **Lighting model**: metallic-roughness GGX with Schlick-GGX Smith and an analytic split-sum fit;
  no multi-scatter energy compensation, no specular occlusion. Glass, clear coat, sheen, specular
  and anisotropy from their factors only (their textures are ignored); no iridescence or dispersion;
  the glTF `occlusionTexture` is ignored.
- **Measured** (all on the M5 at 720p or 1080p, from the docs): the showcase at 1080p 5.8 ms GPU
  (1.9 ms at half scale); the hills at 1080p 4.7 ms with 1.15M triangles submitted; the island at
  1080p 9.5 ms (scene 3.5 with MSAA 4x, water 2.9, ids 1.5, clouds 0.9); 256 skinned humanoids 0.66
  ms GPU; 200,000 GPU particles 1.3 ms. No discrete-GPU, 4K or browser numbers exist.

## What the images show

Evidence viewed: `tests/evidence/rendering/` showcase, pbr, materials, island, ocean, clouds, glass,
irradiance, ssr, probes, grass, atmosphere, village-day, fps.

- Good: the volumetric clouds (fair and rain decks hold up next to Unity HDRP at a glance); grass
  and the noon hills read as a pleasant stylized scene; glass refracts correctly with a clear-coat
  highlight.
- Weak, by content: primitive boxes, lollipop trees, low-poly props, checker and procedural
  textures; the showcase is fog over untextured geometry.
- Weak, by technique (each a known problem to fix):
  - the ocean toward the horizon has black horizontal dashes and pixel noise (island.png,
    ocean.png). The cause is not established: perturbed grazing normals reflecting the sky
    panorama's below-horizon ground with Fresnel near one is likely; the non-reversed Depth24Plus
    depth with the horizon ring just inside the far plane (island camera far 1500, near 0.1) may
    contribute. TAA or specular anti-aliasing alone may not remove it;
  - no specular anti-aliasing (no roughness filtering of normal maps; the water only fades its
    normals with distance), and the island sample runs MSAA without TAA, the hills no anti-aliasing
    at all;
  - reflection probes at 128 px faces are blocky and misregistered (probes.png); the lit pass takes
    the first probe or volume by entity id with only a half-unit fade, no blending between overlaps;
  - the procedural sky's below-horizon band is a flat grey with a hard edge (ssr.png,
    materials.png), and the village shows a dark band at the world's edge;
  - TAA's motion vectors ignore skinning, so animated characters would ghost if TAA were on by
    default;
  - translucent meshes in front of water are hidden by the water surface (water.md).

## Gaps, with where they are

| Gap | Where |
|---|---|
| Culling is per entity, not per submesh: every submesh of an entity shares one bounds sphere; skinned and unbounded draws are never frustum-culled | `renderer.cpp` 11735, 11876 |
| The 65,536 object rows count off-screen draws too (kept for the shadow passes) and further draws are dropped silently, with no counter | `renderer.cpp` 11750, 11790, 12066, 12526 |
| The prepass depth is not reused: the scene pass clears and re-rasterizes with LESS | `renderer.cpp` 12568, 13215 |
| Decals are not clustered: the lit shader walks the whole list (64) per pixel | `renderer.cpp` 41 to 48, 11701 |
| Frame depth is non-reversed Depth24Plus | `engine/rhi/include/pocket/rhi/device.hpp` 74 |
| No GPU texture compression or streaming (KTX2/Basis transcoded to RGBA8; terrain layers resampled to 512) | `docs/design/assets.md` |
| No temporal upscaler (render scale is bilinear plus sharpen), no HDR display output | `docs/design/rendering.md`, Render scale, Not yet |
| No area lights; fixed-resolution shadows (4x2048 cascades, 512 px local faces) | `docs/design/rendering.md` |
| Atmosphere is single-scattering into a panorama; environment reflections do not show the volumetric clouds | `docs/design/rendering.md`, Atmosphere |
| Terrain at most a 1025^2 grid, four layers, no per-layer normal or roughness maps, no streaming | `docs/design/terrain.md` |

## Against three.js (r186, 2026-09-24)

three.js r186 (npm 0.186.1; `dev` at 187dev) has two renderers: `WebGLRenderer` is still the default
import and the manual still calls `WebGPURenderer` experimental, though nearly all new work lands on
the WebGPU and TSL side, which falls back to WebGL2. Core has the full glTF PBR model
(`MeshPhysicalMaterial`: clear coat, sheen, transmission and volume, iridescence, anisotropy,
dispersion, specular, with their maps), basic, PCF and VSM shadow maps, `InstancedMesh`,
`BatchedMesh` (multi-draw with per-instance culling), discrete LOD, PMREM image lighting and light
probes. Addons on the WebGPU path: CSM and a two-cascade `SunLight`, GTAO and SSAO, SSR with a
stochastic mode, SSGI, TRAA, TAAU and FSR1, SMAA and FXAA, bloom, DOF, motion blur, god rays,
weighted blended OIT, clustered lighting (point lights only), `LightProbeGrid` (baked SH volume),
`VXGINode` (voxel cone tracing: WebGPU only, static geometry, diffuse only), `RectAreaLight`, a
Preetham sky with a 2D cloud layer, planar and flow-map water, Gaussian splats with spherical
harmonics, reversed and logarithmic depth buffers and HDR output. Path tracing is a separate project
(three-gpu-pathtracer, WebGPU path tracer in v0.0.25, 2026-09-28).

- **three.js is ahead on**: materials (every extension with its maps, AO maps), compressed textures
  (KTX2 kept compressed), temporal upscaling, depth precision for large scenes, area lights, splats,
  browser reach (WebGL2 fallback where WebGPU is missing: Firefox on Linux and Android, Safari
  before macOS 26), and above all ecosystem: loaders, tooling, examples, a large community, and
  artist-made sites whose baked lighting and authored assets look better than our evidence images.
- **We are ahead on**: integration (atmosphere with a day, weather, volumetric clouds, ocean,
  terrain and grass, all coupled to physics and audio, all component fields an agent sets and reads
  back), determinism, and being an engine: physics, characters, navigation, animation graphs, audio,
  UI, an editor, packaging and agent tooling, where three.js is a rendering library with physics
  wrappers and a basic web editor.
- **On high-poly OBJ** three.js is no better: `OBJLoader` parses the whole file as one string on the
  calling thread, outputs non-indexed geometry (three vertices a triangle), uses flat normals when
  `vn` is missing (smoothing groups only toggle flat shading), fans n-gons and ignores free-form
  (NURBS) records; the maintainers point to binary glTF with Draco or meshopt instead. Neither has
  STEP or IGES import (three.js has Rhino 3DM, 3MF, AMF, VRML, VTK; STEP needs OpenCascade, for
  example occt-import-js, LGPL, last released 2024-12).

## Against the frontier

- **Unreal Engine 5.8** (2026-06-17, the last UE5 feature release): Lumen consolidated on hardware
  ray tracing, with Lumen Lite (Beta) gathering probe irradiance from Lumen's world-space radiance
  cache; MegaLights (stochastic ray-traced direct lighting, production in 5.8); Nanite, with
  Foliage, Voxels and Assemblies experimental; Virtual Shadow Maps with receiver masks and
  invalidation throttling; Substrate materials (production since 5.7); TSR; a path tracer;
  heterogeneous volumes (experimental).
- **Unity 6.6** (2026-09-01): adaptive probe volumes, the GPU Resident Drawer with GPU occlusion
  culling, the STP upscaler; HDRP in maintenance mode since 6.5; WebGPU production-ready for web
  builds.
- **Games**: full path tracing with ReSTIR and neural denoising ships as a high-end NVIDIA tier
  (Resident Evil Requiem 11.7 ms at 1440p on an RTX 4070 Ti; 007 First Light's path tracing arrived
  in a 2026-09-15 update for RTX 50 cards only); upscaling is ML plus frame generation (DLSS 4.5,
  FSR 4.1 and Redstone, XeSS 3, MetalFX on Apple); Cyberpunk 2077 shipped on Mac in 2025 with path
  tracing and MetalFX. Neural texture compression and neural shading are SDKs and demos.
- **What our stack allows**: wgpu-native 29.0.1.1 (pinned in `pocket.toml`) has a ray-query feature
  flag but no BLAS or TLAS creation API, and no mesh shaders, so hardware ray tracing needs a
  wgpu-native extension or a fork (wgpu 30 itself has experimental Metal ray queries and mesh
  shaders). It does expose the native Metal device, queue and textures
  (`wgpuDeviceGetNativeMetalDevice` and friends), so MetalFX upscaling is reachable natively, and
  wgpu-hal 29.0.1 already sets extended dynamic range on a `CAMetalLayer` whose surface format is
  `Rgba16Float`, so HDR output on Apple displays needs no newer wgpu. Browser WebGPU has compute and
  (in Chrome) subgroups but no ray tracing, mesh shaders, bindless or 64-bit atomics; a Nanite-style
  software rasterizer exists for it (nanite-webgpu) only with lossy 32-bit packing.

## Industrial OBJ import

Every file loaded, vertex and triangle counts match Blender 4.5's importer exactly, negative indices
and quads are right, and smooth scans with `vn` render correctly up to 12M triangles. Typical CAD
and plant exports do not import well. Load is `world.spawn` with the mesh to the first frame that
shows it, median of three, release build, beside a running benchmark; Blender is its import call
alone; the C reader is `tools/scripts/dev/objstress/fastobj_bench.c`, a plain single-threaded reader
(the lower bound for parsing).

| File (`build/objstress/assets/`) | Size | Triangles | Load | Blender | C reader | Peak memory | GPU static / turning | Picture |
|---|---|---|---|---|---|---|---|---|
| scan_1m: scanned surface with vn | 74 MB | 1.0M | 0.96 s | 0.54 s | 0.11 s | 0.7 GB | 1.6 / 2.4 to 3.5 ms | right |
| scan_5m | 392 MB | 5.0M | 5.2 s | 3.2 s | 0.29 s | 2.1 GB | 3.9 to 4.6 / 7.5 to 10.4 ms | right |
| scan_12m (`--big`) | 959 MB | 12.0M | 17 s (13.5 to 26.6) | 8.1 s | 1.3 s | 5.3 GB | 8 to 12 / 15 to 18 ms | right, over 60 fps budget |
| machined_2m: no vn, welded, mm, Z up, 250 m out | 65 MB | 1.53M | 1.1 s | 0.38 s | 0.09 s | 0.8 GB | 1.9 ms | wrong (smoothed, on its side) |
| machined_far: the same part 4.5 km out | 26 MB | 0.62M | 0.48 s | | | 0.3 GB | | wrong (precision lost) |
| assembly_o: 4800 parts as `o`, 5 materials | 134 MB | 1.98M | 2.0 s | 1.3 s | 0.14 s | 1.25 GB | 5.9 / 7.9 ms, 4800 draws | right |
| assembly_g: the same parts as `g` only | 134 MB | 1.98M | 1.9 s | 1.1 s | | 1.2 GB | 2.4 / 5.8 ms, 5 draws | right pixels, parts merged |
| ngons: quads, concave n-gons, negative indices | 14 KB | 320 | | | | | | wrong (4 of 8 polygons) |
| cube_soff: welded cube with `s off` | | 12 | | | | | | wrong (smoothed) |

Measured again after items 1 to 5 and 8 below were fixed (2026-10-02, on Windows 11, a Ryzen 9 270
with 16 threads and an RTX 5060, release, with other agents' builds and runtimes on the machine, so
noisy; not the M5 above). Reading to a mesh is the store's load (the text read, parsed, tangents
made) timed in one process against a copy of the old reader, alternating, the median of three; spawn
to first frame is `measure.py`'s load, once each:

| File | Read to a mesh, before | After | From `.imported/` | Spawn to first frame, before / after | Peak memory (working set), before / after | Picture now |
|---|---|---|---|---|---|---|
| scan_1m | 4.34 s | 0.42 s | 0.16 s | 4.80 / 0.60 s | 513 / 443 MB | the same pixels |
| scan_5m | 23.9 s | 2.17 s | 0.81 s | 34.5 / 2.21 s | 1614 / 1163 MB | the same pixels |
| machined_2m | 5.12 s | 0.69 s | 0.21 s | 5.66 / 0.82 and 1.13 s (with `up: "z"`, `unit: "mm"`) | 553 / 503 and 535 MB | right: upright, in metres, sharp |
| machined_far | | | | | | right: the same pixels as machined_near |
| assembly_o | 9.35 s | 0.82 s | 0.32 s | | | |
| ngons | | | | | | right (8 of 8 polygons) |
| cube_soff | | | | | | right (flat) |

`tests/evidence/assets/obj-import-fixed.png` shows the fixed cases beside the engine before. On this
machine the old reader was slower than on the M5 (the standard library's string streams and
`std::map`), so the factors are larger than the M5 would show.

`tests/evidence/assets/obj-import-findings.png` shows the wrong cases beside Blender's import. The
problems, most severe first, all in `engine/assets/src/import.cpp` (`parse_obj`) unless named:

1. **Files without `vn` are smoothed per position with no crease angle, and `s` lines are ignored.**
   Welded CAD parts render as blobs: square towers round, drilled holes read as raised studs, sharp
   edges smeared. Fix: without `vn`, crease-angle normals (split a vertex between fans of faces more
   than about 30 degrees apart, Blender's auto-smooth default), `s off` / `s 0` flat, `s N` as
   smoothing groups; the angle an import option reported by `assets.describe`. **Done**
   (2026-10-02): faces joined into fans across shared edges up to `crease` (30 degrees, an import
   setting), `s off` flat, `s N` smooth within its group and apart from others (`assets_tests`
   `[obj][crease]`; `docs/design/assets.md`, Importing models).
2. **Concave n-gons are fanned from their first corner** (around line 194), so they come out right
   or wrong by where the exporter started them. Fix: fan only convex polygons; otherwise ear-clip in
   the plane of the Newell normal. **Done** (2026-10-02): convex polygons keep their fan, others are
   ear-clipped (`[obj][ngon]`); `ngons.obj` comes out right.
3. **Loading is slow and freezes the runtime.** A `state` call sent half a second into a 5M load is
   answered after 4.7 s; 12M freezes 14 to 27 s. The profile: 870 of 1168 main-thread samples in the
   `std::map` corner dedupe (lines 207 to 227), 131 in the per-line `istringstream` split (around
   line 148); the whole text is also copied into an `istringstream` (twice the file in memory). Fix:
   one pass over the bytes (no per-line strings, `std::from_chars` or fast_float, a hash or flat
   array for the dedupe, the position index directly when every corner is `v//v`), then the import
   on a worker thread with the entity drawing nothing (or its box) until the mesh is ready and
   `state` saying it is loading. **Done but the worker thread** (2026-10-02): one pass over the text
   (a line found with `memchr`, words as views, numbers by an exact fast path for exporters'
   decimals and `std::from_chars` for the rest, corners deduped through a short list per position,
   the file read at its size), about ten times faster (the table above). The thread is left: the
   store answers synchronously and its callers (the world's bounds, mesh colliders, the renderer's
   upload) take the mesh on return; loading behind them would let the wall clock decide on which
   tick a collider or a bounds appears, against the engine's determinism, unless the tick waited for
   it, which is the freeze again. With the cache below, a 5M-triangle file holds the runtime for
   about a second after its first read.
4. **Coordinates are parsed and kept as float32**, so a part far from the origin loses precision at
   load and no Transform recovers it (6 mm holes become stair-steps 4.5 km out; typical of plant,
   BIM and GIS coordinates). Fix: parse as double, subtract a double origin (the bounds' centre or a
   given pivot) before converting, and report that origin (`assets.describe`) so the part can be put
   back. **Done** (2026-10-02): `recenter` (by default when the bounds' centre lies more than 100 of
   their sizes out) and `import.origin` (`[obj][precision]`); `machined_far.obj` draws the same
   pixels as `machined_near.obj`.
5. **No unit or up-axis options.** Z-up millimetre files arrive on their side and a thousand times
   too big, while the Blender-converted formats arrive Y-up, so formats disagree. Fix: per-asset
   import settings (`assets.import {up, unit, recenter}` and a `project.toml` table), baked into the
   mesh; `assets.describe` reporting the size in metres and suggesting Z-up when the bounds look
   like it. **Done** (2026-10-02): `assets.import {path, up, unit, recenter, crease}` for the
   session and `[assets.import]` in `project.toml` (`docs/design/assets.md`, Import settings);
   `size` for every mesh, `hints` for a Z-up or millimetre file (`[obj][units]`, `runtime_tests`
   `[import][obj]`).
6. **Assemblies draw one call per part with no culling per part** (all submeshes share the entity's
   bounds sphere: inside the assembly 4800 parts and 1.98M triangles are drawn with none out of
   view), and a file with only `g` lines collapses every part into one node (lines 171 to 172),
   losing picking and instancing per part. Fix: bounds per submesh culled on their own; static parts
   sharing a material batched into one draw with a part id; `g` as parts when a file has no `o`.
7. **Huge meshes get no automatic LOD and no cluster culling**: 12M triangles are one full-cost draw
   whatever is on screen. Short term: add simplified levels when an import passes about 1M triangles
   (or have `world.lint` suggest `lods`; with `lods: [{screen: 4, ratio: 0.1}]` the 5M scan draws
   0.5M triangles at 2.5 ms turning and looks the same). Long term: meshlets at import from the
   vendored meshoptimizer (`meshopt_buildMeshlets`, `meshopt_computeMeshletBounds`) culled by
   frustum and normal cone.
8. **Parsed OBJ meshes are not cached** (`engine/assets/src/assets.cpp` 2107 to 2112, unlike the
   Blender path's `.imported/`): every start, reload and pack parses the text again. Fix: write the
   parsed mesh to `.imported/` under the same stamp scheme and read it when the stamp matches; a
   pack ships the binary. **Done** (2026-10-02): `.imported/<path>.mesh`, stamped with the text, the
   settings and the reader's version, its material libraries' hashes inside; read two and a half to
   three times faster than parsed (the table above; `[obj][cache]`). Packs do not ship it:
   `pocket pack` leaves out every dot-directory (`tools/pocket/src/pack.rs`, `copy_tree`),
   `.imported/` included, so a packed game parses on its first start and writes its own; that also
   means a pack of a Blender-read file carries no conversion, though `docs/design/assets.md` says it
   does (not changed here).
9. Peak memory is about 5.5 times the file (about Blender's), and the CPU copy stays resident after
   upload; `assets.import` of the same files took longer than the spawn path (1.8 s against 0.96 s
   at 1M; not investigated). After the fixes above the new reader peaks lower (the table above) and
   the import path is no slower than the spawn path (0.38 s import and spawn, 0.60 s spawn alone, at
   1M on the Windows machine).

## Reproducing

`tools/scripts/dev/objstress/` holds the generators and the measurement (its scripts' headers say
how):

```bash
python3 tools/scripts/dev/objstress/generate.py [--big]      # build/objstress: the project and its OBJ files (0.7 GB, 1.7 GB with --big)
./.pocket/pocket build --config release                       # measure.py runs build/release/bin/pocket_runtime
python3 tools/scripts/dev/objstress/measure.py all            # every case in cases.json -> build/objstress/runs/{logs,captures}
python3 tools/scripts/dev/objstress/measure.py freeze assets/scan_5m.obj
python3 tools/scripts/dev/objstress/measure.py load assets/scan_5m.obj 3 [--cached]   # import then spawn, median of three; the cache removed first unless --cached
python3 tools/scripts/dev/objstress/evidence.py               # tests/evidence/assets/obj-import-fixed.png from before and after captures
build/objstress/bin/fastobj_bench build/objstress/assets/scan_5m.obj
/Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup --python tools/scripts/dev/objstress/blender_import.py -- build/objstress/assets/scan_5m.obj
```

Rerun them after each import fix and compare with the table above (and on a quiet machine).

## Plan, ranked by value for effort

1. **OBJ import**: crease-angle normals and smoothing groups; ear clipping; the fast parser on a
   worker thread with a binary cache; double-precision recentring; unit and up-axis options; bounds
   per submesh, `g` as parts, automatic levels for huge meshes. All but the worker thread, bounds
   per submesh, `g` as parts and automatic levels are done (2026-10-02; items 1 to 5 and 8 above).
   Each lands with an `assets_tests` case built from the stress files (`ngons.obj`, `cube_soff.obj`
   and a small machined part are small enough to embed).
2. **Visible quality, cheap**: find and fix the ocean's dark dashes; specular anti-aliasing for
   normal maps and water; skinned motion vectors, then TAA on by default in the samples; reversed-Z
   with a 32-bit float depth; the sky's below-horizon band; multi-scatter GGX, specular occlusion
   and GTAO-style AO with bent normals; probe blending and larger probe faces.
3. **Temporal upscaling** inside the existing TAA (jitter, motion vectors and history clipping are
   there), with MetalFX as the native option on Apple; **HDR output** on Apple displays through an
   `Rgba16Float` surface.
4. **Materials**: the textures of transmission, clear coat, sheen and specular, the glTF occlusion
   texture, iridescence; KTX2 kept compressed on the GPU.
5. **Dynamic GI**: irradiance volumes that follow the camera in cascades and re-capture continuously
   (the Lumen Lite and DDGI family), with a cheaper radiance source than six raster views per probe.
6. **GPU-driven drawing**: no silent row cap, indirect draws, Hi-Z occlusion and meshlet culling
   (one indirect draw per batch on the web, which has no multi-draw-indirect).
7. **Content**: authored PBR assets for the showcase scenes (license-checked), the biggest single
   lift to how the evidence looks.
8. Native-only hardware ray tracing waits for an acceleration-structure API in wgpu-native.
