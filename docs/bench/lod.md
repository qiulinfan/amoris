# Levels of detail: measurements

Status: branch `explore/lod`, 2026-10-09; native timings re-measured on a quiet machine the same
day (section 9), which supersedes those of sections 2 and 4.

Spec: [docs/spec/lod.md](../spec/lod.md). Evidence: `out/bench-runs/lod/`. Every timing in sections
2 to 7 is **provisional**: three other agents built and ran GPU work on the same machine at the same
time (Windows 11, Ryzen 9 270, RTX 5060 Laptop GPU 8 GB, Radeon 780M; NVIDIA driver 617.14, AMD
24.30.62.03 for Vulkan and 32.0.13062.3005 for Direct3D 12). The same mesh's level build took from
137 to 895 ms across runs, which says how loaded the CPU was; the GPU numbers are steadier but a
quiet-machine re-measurement is due. Triangle and instance counts are exact.

## 1. The scene and how it is measured

`demo::lod_field(48, 6)`: 2,304 instances on a 288 m square, three seeded bumpy rocks of 81,920
triangles (an icosphere subdivided 6 times, displaced by twelve seeded waves) and a (2, 3) torus
knot of 49,152, scaled 0.6 to 1.5, turned at random, under a sun with four shadow cascades, plus
the ground; 1280x720. At `t = 0` the camera stands at head height in the first cell looking along
the diagonal; at `t = 0.5` in the middle of the field. With levels off it is 170 million
triangles for the camera (97 million at `t = 0.5`) and 177 to 204 million for the cascades.

The meshes' chains (`lod_field` example output, `meshes`): every mesh gets the full 8 levels.

| Mesh | Triangles per level | Error per level (mesh units; radius 1.2 to 1.8) |
|---|---|---|
| rock_a | 81,920 · 40,960 · 20,480 · 10,240 · 5,120 · 2,560 · 1,280 · 640 | 0.0006 · 0.0012 · 0.0021 · 0.0038 · 0.0070 · 0.013 · 0.025 |
| knot | 49,152 · 24,576 · 12,288 · 6,144 · 3,072 · 1,536 · 768 · 384 | 0.0007 · 0.0016 · 0.0043 · 0.0064 · 0.013 · 0.025 · 0.049 |

`python tools/lod_bench.py --t 0 | --t 0.5 [--occlusion auto]` runs
`cargo run --release -p pocket-render --example lod_field` per backend, adapter and mode: 10 warm
frames, then 60 measured frames with levels on and 15 with levels off, each submitted and waited
for; GPU time from pass timestamps; the last frame's draws read back per level
(`Renderer::draw_counts`). Occlusion culling off unless noted, so that levels are measured alone.

## 2. Native (provisional, superseded by section 9)

GPU milliseconds, median (historical
[bench-native-t0.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/lod/bench-native-t0.json)
and
[bench-native-t05.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/lod/bench-native-t05.json)):

| Adapter, backend | `t` | Levels off | Levels on | Ratio |
|---|---|---|---|---|
| RTX 5060, Vulkan | 0 | 81.1 | 0.79 | 102x |
| RTX 5060, Direct3D 12 | 0 | 84.0 | 0.92 | 91x |
| Radeon 780M, Vulkan | 0 | 120.1 | 5.48 | 22x |
| Radeon 780M, Direct3D 12 | 0 | 121.1 | 4.29 | 28x |
| RTX 5060, Vulkan | 0.5 | 71.1 | 0.79 | 90x |
| RTX 5060, Direct3D 12 | 0.5 | 74.5 | 0.91 | 82x |
| Radeon 780M, Vulkan | 0.5 | 169.3 | 7.15 | 24x |
| Radeon 780M, Direct3D 12 | 0.5 | 188.8 | 5.36 | 35x |

What the last frame drew, the same on every adapter:

| `t` | Camera triangles off / on | Cascades' triangles off / on | Camera instances per level, on |
|---|---|---|---|
| 0 | 169,869,314 / 1,380,226 (123x) | 176,734,216 / 1,430,920 (124x) | 1, 1, 0, 0, 0, 5, 9, 2,289 |
| 0.5 | 97,042,434 / 788,866 (123x) | 204,144,648 / 1,697,544 (120x) | 1, 0, 0, 1, 2, 5, 10, 1,303 |

- Nearly every instance draws its coarsest level (640 or 384 triangles): at 720p a pixel 15 m away
  is 2.4 cm, the rocks' coarsest error. Eight levels end at 1/128 of the triangles; beyond that a
  level cannot go coarser (spec 9).
- Wall time per frame (submit to idle) with levels on was 2.9 to 3.2 ms on the RTX 5060 against
  0.8 to 0.9 ms of timestamped GPU work, and 5.9 to 17.8 ms on the 780M; the passes the profiler
  does not time (bloom, the display transform) and the GPU's clocks at low load are in the gap.
  The CPU's encoding stayed under 2.4 ms in every run, with levels on or off.
- The views' lists grow from 156,740 to 1,253,444 bytes (`Renderer::list_bytes`): the field's
  2,304 instances of 8-level meshes take 8 entries each per view.
- **The binding limit.** A review's probe on WebGPU's default limits (`Minimal { limits }`, 128
  MiB per binding) drew 450,000 primitive spheres (7 levels) as 377,389 instances with levels off
  and as nothing with them: `drawn` needed 151,200,000 bytes, its bind group failed validation, and
  wgpu only logged it. The renderer now lays such a scene out without levels (spec 5):
  `levels_give_way_at_the_binding_limit` (419,429 hidden spheres, 64 shown) draws the same 64
  instances and 171,840 cascade triangles with levels asked for as with them off, from 28,525,524
  list bytes; with the fallback taken out it drew 0 instances from 199,678,668.
- One anomaly: the Radeon 780M's Vulkan `cull` pass timed 8.0 ms with levels off at `t = 0` and
  0.13 ms with them on, against 0.02 ms on Direct3D 12 either way and 0.01 ms on the RTX 5060; the
  culling work is the same up to the level loop, so this is probably a timestamp taken while the
  previous frame's draws still ran. Not investigated.

**With occlusion culling's auto mode** (historical
[bench-native-t05-auto.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/lod/bench-native-t05-auto.json),
Vulkan, `t = 0.5`): levels off, the auto mode turned two-phase culling on and the camera drew
68,173,826 triangles instead of 97,042,434 (RTX 5060 66.1 ms, 780M 175.9 ms); levels on, it stayed
off (`auto-off`): the triangles hidden instances would have drawn at their levels no longer pay for
the second phase, which is what counting the level's triangles in the late pass is for (spec 4).
0.82 ms and 5.85 ms.

## 3. How far the image moves

`levels_cut_triangles_and_stay_within_a_pixel` (`tests/lod.rs`; RTX 5060, Vulkan; 10 x 10 field of
the detail-5 meshes at 640x360, the camera at `t = 0` and `t = 0.45`):

| `t` | Camera triangles off / on | Pixels showing another entity | More than 1 px from an edge | More than 2 px | Pixels apart by more than 32 |
|---|---|---|---|---|---|
| 0 | 1,843,202 / 35,746 | 942 (0.41%) | 3 | 0 | 1,808 (0.78%) |
| 0.45 | 1,159,170 / 18,402 | 1,349 (0.59%) | 14 | 0 | 1,696 (0.74%) |

The 48 x 48 field at 1280x720 (`image_diff`, `field48-*-diff.json`, strips
`field48-*-off-on-heat.png`: levels off, on, and the difference times 8):

| `t` | PSNR | RMSE (8-bit) | Pixels apart by more than 8 / 16 / 32 / 64 |
|---|---|---|---|
| 0 | 37.05 dB | 3.58 | 2.74% / 1.27% / 0.44% / 0.03% |
| 0.5 | 36.77 dB | 3.70 | 3.47% / 1.51% / 0.40% / 0.03% |

The heat maps show where: inside the rocks' silhouettes, in their shading. A coarse level
interpolates the full mesh's vertex normals over larger triangles, so the bumps' highlights
smear and shift; the silhouettes and the shadows' edges move by about a pixel. Normal maps baked
from the full mesh, or simplification weighing normals, would bring the shading back (spec 9).

**The silhouette check as first written** asked, for each pixel whose entity differs, whether the
other entity appears within two pixels in the full meshes' id image. It found 7 and 15 pixels
farther, on the ground next to rocks and on the sky between rocks: where two silhouettes overlap
with full meshes and each shrinks by a pixel, the levels show what was behind both, which the
full meshes showed nowhere near. The check now measures the distance to the reference's nearest
edge (spec 8).

## 4. Making levels

Native (`lod_field`, `meshes[].lod_build_ms`), on the least loaded runs: 135 to 147 ms per
81,920-triangle rock and 64 to 68 ms for the 49,152-triangle knot, so about 1.7 µs per input
triangle for the vertex cache order, seven simplifications from the full mesh, seven more cache
orders and the vertex order; under load the same meshes took up to 895 ms. Making the four meshes
themselves took 32 to 40 ms.

In Chrome (`bench-chrome.json`) the viewport page made the four meshes and their levels and the
field in 1,271 to 1,350 ms on the page's thread, against about 550 ms natively (33 ms for the
meshes, 480 to 520 ms for their levels): about 2.4 times slower in WebAssembly. A glTF of that size
would freeze the page for that long when it loads (spec 9). samples/anim's `hero.glb` (parts of 24
to 60 triangles) gets no levels.

The viewport's WebAssembly grew from 2,293,805 to 2,333,175 bytes (+1.7%), 936,992 to 955,966
gzipped (+2.0%), meshoptimizer and the demo scene included.

## 5. Checks and mutations

`python tools/lod_mutations.py --out out/bench-runs/lod/mutations-rtx5060-vulkan.json` (RTX 5060,
Vulkan, at `7c40c52a`): every mutation makes a check fail. "lod.rs" is `tests/lod.rs`: silhouettes,
draw paths, hysteresis, cascades, skinned, limit for its six tests.

| Mutation | Caught by |
|---|---|
| cull: the camera's bound eight times too loose | lod.rs: silhouettes, draw paths, hysteresis, skinned |
| cull: no hysteresis | lod.rs: hysteresis |
| cull: distance to the sphere's centre, not its nearest point | lod.rs: hysteresis, skinned |
| cull: the instance's scale ignored | lod.rs: hysteresis (the run at scale 2) |
| cull: the cascades draw the full meshes | lod.rs: silhouettes (cascades' triangles), cascades |
| cull: the cascades draw the camera's level | lod.rs: cascades |
| cull: the cascades' bound four times too loose | lod.rs: cascades |
| lod: the cascades' bound ignores `shadow_texels` | lod.rs: cascades (the run at two texels) |
| cull: a level's row one too far | lod.rs: silhouettes, hysteresis, cascades |
| cull: the late pass draws the full mesh | lod.rs: draw paths (cold, occlusion on); occlusion.rs |
| cull: the late pass forgets the level | lod.rs: hysteresis (the run with occlusion on); occlusion.rs |
| meshes: level rows' errors are the full mesh's | lod.rs: silhouettes, draw paths, hysteresis, cascades, skinned |
| meshes: a skinned copy's levels read the bind pose | lod.rs: skinned |
| renderer: level rows get no regions | lod.rs: silhouettes, skinned, limit; draw_paths.rs; occlusion.rs |
| renderer: levels kept past the binding limit | lod.rs: limit |
| generate: vertices left in their first order | pocket-assets `coarse_levels_read_a_prefix_of_the_vertices` |

- A review of the branch found three gaps the first eleven mutations did not reach: drawing the
  camera's level into the cascades left every check passing (127,528 shadow triangles became
  117,512), and so did a cascade bound four times too loose (the colour check's 2% caught only a
  factor of 16); a skinned copy's level rows reading the bind pose passed because no test drew a
  skinned mesh with levels; and a scene past the binding limit drew nothing. The cascades, skinned
  and limit tests and the five mutations after them close those gaps.
- The first run reported "distance to the sphere's centre" as surviving; run alone it was caught.
  The mutated file had been written within the clock tick of the previous restore and cargo did
  not rebuild; the tool now moves a mutated or restored file's modification time two seconds
  ahead.
- Not covered: the generator's guard that keeps errors increasing. On every chain tried (three
  rocks and the knot at five subdivisions, 20 chains) meshoptimizer's own errors already increase,
  so removing the guard changes nothing a check can see. It stays because selection depends on it.
- The hysteresis test needed a scaled instance to catch "the instance's scale ignored": the
  silhouette check's two pixels of slack hid a factor of 1.5.

`cargo xtask check --only deps,fmt,docs,clippy,build,gen,test,wasm,web` passed at `4b48eab8` (626
tests with the ignored ones, 9 more than master's 617: four in pocket-assets' `lod`, two in
pocket-render's `lod`, three in `tests/lod.rs`; 41 min 53 s with other agents building), and again
at `f12f0d73` after the review's fixes (629 tests: the cascades, skinned and limit tests; 11 min 49
s on a warm target). Its clippy run for wasm32 builds meshoptimizer's C++ there; without the pinned
LLVM on `PATH` that build stops at `failed to find tool "llvm-ar"` even with
`AR_wasm32_unknown_unknown` set, as a cold build in a scratch target showed, hence xtask's `PATH`
(spec 2).

## 6. The per-batch paths: Chrome, and skinned crowds

Headless Chrome 155 with WebGPU (Direct3D 12 under Dawn), `tools/web_bench.mjs`,
`/viewport/?demo=lod&n=48&detail=6&occlusion=off` (canvas 1254x564), first-instance path
(`bench-chrome.json`):

| Adapter | Levels off, GPU ms | Levels on, GPU ms | Levels on, frame ms |
|---|---|---|---|
| Radeon 780M (Chrome's default) | 193.7 | 3.6 to 3.9 | 6.0 to 6.2 |
| RTX 5060 (`--force_high_performance_gpu`) | 82.7 | 0.86 to 0.87 | 2.7 to 3.6 |

- With levels on, the per-batch paths issue 165 draw calls per frame (25 with levels off): every
  level that holds instances is a batch per view and variant. The CPU's encoding (`render_ms`) was
  0.8 ms either way.
- On the baseline path (`gpu_minimal=first-instance`, the RTX 5060) the GPU time doubled, 1.85 ms
  against 0.86 ms, with the same calls; the frame time did not change (2.6 ms).
- With levels off on the 780M only 121 of 200 frames carried timestamps and the frame loop ran far
  ahead of the GPU, so its frame times mean nothing; its GPU time is the median of those that did.

**Skinned crowds** (natively, `crowd-rtx5060-vulkan.json`): a skinned entity's copy has level rows
of its own (spec 3), each a live batch holding its one instance, so on the per-batch paths every
skinned part of `L` levels adds `L - 1` draw calls per view, whether the culling keeps it or not.
`lod_field --n 48 --detail 6 --occlusion off --crowd 200` adds 200 entities of `demo::bent_model`
(one skinned part of 5,120 triangles and 8 levels) to the field; RTX 5060, Vulkan, 1280x720, 60
frames, medians, provisional:

| Path | Crowd | Levels | Draw calls | CPU encoding ms | GPU ms |
|---|---|---|---|---|---|
| multi-draw | 0 | off / on | 20 / 20 | 0.80 / 0.71 | 73.0 / 0.78 |
| multi-draw | 200 | off / on | 20 / 20 | 1.98 / 2.03 | 75.0 / 0.84 |
| baseline (`POCKET_GPU_MINIMAL=first-instance`) | 0 | off / on | 25 / 165 | 1.21 / 1.17 | 70.6 / 0.72 |
| baseline | 200 | off / on | 1,025 / 8,165 | 3.28 / 6.71 | 74.4 / 0.86 |

- On the baseline path the crowd's 200 skinned copies cost 1,000 draw calls with levels off (one
  per copy and view) and 8,000 with them (eight per copy and view): 7,140 more calls, 3.4 ms more
  CPU encoding, about 0.5 µs per call natively. A browser pays more per call (wgpu's WebGPU
  backend calls into JavaScript for each); not measured there.
- The multi-draw path issues one call per view and variant whatever the batches, so the crowd costs
  it nothing in draw calls; its extra 1.2 ms of CPU is the skinning pass's per-entity work (a bind
  group per skinned part per frame), with levels on or off.
- The camera drew 18,600 more triangles with the crowd and levels (93 per column: most draw their
  coarsest level), 1,024,000 more without them. `skinned_levels_follow_the_pose` pins the count
  (35 more draw calls per entity: 7 levels by 5 views); spec 9 says what could lower it.

## 7. Mesh shaders: historical excluded experiment

This historical experiment is excluded from the import. Its [standalone
source](https://github.com/qiulinfan/amoris-pioneer/blob/9a3e4b0814959629202629845b85205c42086505/crates/pocket-render/examples/meshlet_spike.rs)
and [recorded
trials](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/lod)
remain in Pioneer at the fixed source commit. It made its own device; the production renderer and
browser do not use it. The existing indexed LOD path and its defaults are unchanged.

**Who exposes them** (historical
[survey](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/lod/mesh-shader-survey.json)):
wgpu 30's `EXPERIMENTAL_MESH_SHADER` on all four (adapter, backend) pairs, and on Microsoft's
software adapter. Limits: 256 output vertices and primitives everywhere; mesh workgroups of up to
128 invocations (256 on the 780M's Vulkan driver); task payload 16 KiB.

**What runs.** The spike's WGSL (`enable wgpu_mesh_shader`, `@task`, `@mesh`,
`var<task_payload>`) goes through naga on both backends, although wgpu's documentation says naga
supports mesh shaders on Vulkan only:

| Adapter, backend | Smallest mesh pipeline (`--triangle`) | The spike |
|---|---|---|
| RTX 5060, Vulkan | draws | draws; images identical to the indexed path |
| RTX 5060, Direct3D 12 | draws | draws once worked around (below); identical images |
| Radeon 780M, Direct3D 12 | draws | draws; identical images |
| Radeon 780M, Vulkan | **device lost** | **device lost** |

- On the 780M's Vulkan driver (24.30.62.03) creating any mesh pipeline from naga's SPIR-V loses the
  device ("Unexpected error variant (driver implementation is at fault)"), even one workgroup of
  one invocation writing one triangle, and with runtime checks off
  (`create_shader_module_trusted`, no workgroup zeroing). Direct3D 12 on the same GPU works.
- naga 30's HLSL for a task shader whose entry point takes `local_invocation_index` declares
  `SV_GroupIndex` twice ("redefinition of parameter 'lid'"), which DXC rejects; taking
  `local_invocation_id` instead works.

**How fast** (`meshlets-*.json`; 576 rocks of 81,920 triangles = 843 meshlets of at most 64
vertices and 124 triangles each, meshopt cone weight 0.25; 1280x720; GPU ms, median of 20 to 30
frames). "Indexed" is one instanced `draw_indexed` of the instances whose bounding sphere is in the
frustum, as the renderer's culling does per instance; "culled" is a task shader testing 32
meshlets of an instance per workgroup against the frustum and their normal cones; "no task" draws
every meshlet straight from the mesh stage:

| Camera, adapter, backend | Indexed | Meshlets, culled | Meshlets, no culling | No task shader | Triangles indexed / culled |
|---|---|---|---|---|---|
| corner, RTX 5060, Vulkan | 5.62 | 6.24 | 9.09 | 7.82 | 47.2 M / 30.1 M |
| corner, RTX 5060, Vulkan, unchecked | 5.62 | 6.17 | 9.94 | 7.30 | 47.2 M / 30.1 M |
| corner, RTX 5060, Direct3D 12 | 5.62 | 9.81 | 15.48 | 13.09 | 47.2 M / 30.1 M |
| corner, Radeon 780M, Direct3D 12 | 15.12 | 23.15 | 36.10 | 40.75 | 47.2 M / 30.1 M |
| inside, RTX 5060, Vulkan | 1.65 | 1.52 | 9.29 | 7.21 | 14.5 M / 7.5 M |
| inside, RTX 5060, Direct3D 12 | 1.74 | 2.94 | 15.91 | 14.12 | 14.5 M / 7.5 M |
| inside, Radeon 780M, Direct3D 12 | 6.68 | 7.37 | 49.57 | 45.34 | 14.5 M / 7.5 M |

("corner": every rock in view; "inside" (`--inside --big 2.5`): the camera among rocks 2.5 times
larger, 177 of 576 instances in the frustum, many partly.)

- Meshlet culling drops a third of the triangles from the corner (the cone test: little is outside
  the frustum) and half of what per-instance culling keeps from inside, and the images match the
  indexed path pixel for pixel.
- It is faster than the instanced indexed draw only in one case, inside on the RTX 5060 with
  Vulkan (8%). Elsewhere the mesh stage's cost outweighs the triangles saved: drawing every
  meshlet costs 1.4 to 9 times the indexed draw, and Direct3D 12 pays more than Vulkan on the same
  GPU (DXC from naga's HLSL against naga's SPIR-V).
- The renderer's indirect path draws coarse levels of detail at a distance (section 2), where
  this meshlet path without its own LOD drew full meshes. These results did not establish a
  production replacement, and the 780M's Vulkan driver did not accept the mesh pipeline. The
  experiment remains a historical limitation rather than a runnable part of this import.

## 8. What did not work

- **Too smooth rocks, too high a camera.** The first field put the camera 4 m up at the corner of
  rocks whose surfaces the simplifier could halve within half a millimetre: every instance but two
  drew its coarsest level, which measured the coarsest level and nothing else. The rocks got
  rougher waves (up to the finest subdivision's resolution) and the camera came down to head
  height inside the first cell; still most instances draw the coarsest level (section 2).
- **Levels with ray-traced shadows.** Rays leaving a coarse receiver meet its full mesh: the
  intersection over union with the cascades fell from 0.935 to 0.764. Raising the ray origin by the
  level bound restored the counts but not the places (0.879: contact shadows shrank by the offset).
  Ray-traced shadows now draw full meshes (spec 7).
- **meshopt's `optimize_vertex_fetch_remap`** in the `meshopt` crate truncates its table to the
  number of used vertices, so a mesh with unused vertices would lose remap entries; the vertex order
  is computed in Rust instead.

## 9. Quiet re-measurement (2026-10-09)

Same machine and drivers, no other agent running, on AC power (conditions:
[quiet-2026-10-09.md](quiet-2026-10-09.md)), master `ef35e1af`.
`POCKET_AA=msaa POCKET_GTAO=off python tools/lod_bench.py --adapters nvidia,780m --backends vulkan,dx12 --t 0 --rounds 3`
and the same at `--t 0.5` (the tool now takes `--rounds`: each round runs every adapter, backend and
level mode in turn); raw runs in
[bench-t0.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/lod/bench-t0.json)
and
[bench-t0.5.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/lod/bench-t0.5.json),
tables by `python tools/quiet_tables.py lod`. GPU ms (frame p50), median of three rounds (minimum;
spread):

| Adapter, backend | `t` | Levels off | Levels on | Ratio | Provisional off / on (ratio) |
|---|---|---|---|---|---|
| RTX 5060, Vulkan | 0 | 67.79 (67.65; 0%) | 0.78 (0.71; 8%) | 87x | 81.1 / 0.79 (102x) |
| RTX 5060, Direct3D 12 | 0 | 69.17 (69.12; 0%) | 0.86 (0.86; 1%) | 80x | 84.0 / 0.92 (91x) |
| Radeon 780M, Vulkan | 0 | 125.75 (125.53; 3%) | 4.94 (4.88; 2%) | 25x | 120.1 / 5.48 (22x) |
| Radeon 780M, Direct3D 12 | 0 | 112.73 (104.61; 11%) | 4.56 (4.42; 10%) | 25x | 121.1 / 4.29 (28x) |
| RTX 5060, Vulkan | 0.5 | 58.13 (57.93; 0%) | 0.78 (0.77; 0%) | 75x | 71.1 / 0.79 (90x) |
| RTX 5060, Direct3D 12 | 0.5 | 59.06 (58.85; 2%) | 0.84 (0.84; 5%) | 70x | 74.5 / 0.91 (82x) |
| Radeon 780M, Vulkan | 0.5 | 105.23 (98.79; 7%) | 5.14 (5.10; 6%) | 20x | 169.3 / 7.15 (24x) |
| Radeon 780M, Direct3D 12 | 0.5 | 86.92 (84.60; 14%) | 4.76 (4.46; 10%) | 18x | 188.8 / 5.36 (35x) |

- **Levels on** barely moved: 0.78 to 0.86 ms on the RTX 5060 (provisional 0.79 to 0.92) and 4.6
  to 5.1 ms on the 780M (4.3 to 7.2). The load had inflated mostly the **levels-off** frames, by
  20 to 26% on the RTX 5060, and on the 780M by 1.6 to 2.2x at `t = 0.5` (at `t = 0` not at all),
  so the ratios are smaller: 70 to
  87x on the RTX 5060 (provisional 82 to 102x) and 18 to 25x on the 780M (22 to 35x). What levels
  buy stays two orders of magnitude on the discrete GPU and more than one on the integrated one.
- Wall time with levels on: 1.3 to 1.5 ms on the RTX 5060 with Vulkan and 2.0 to 2.1 ms with
  Direct3D 12 (provisional 2.9 to 3.2 ms), 6.1 to 7.3 ms on the 780M (5.9 to 17.8).
- Triangle counts, instances per level and list bytes are unchanged (they are exact).
- **Making levels** (section 4): 141 to 223 ms per 81,920-triangle rock (median 153 to 155 for each
  of the three rocks; 142 of the 144 rock builds at most 188 ms, the other two 220 and 223 ms) and
  68 to 78 ms for the knot (median 71) over the 48 runs; the provisional "least loaded" 135 to 147
  and 64 to 68 ms lie 4 to 13% under these medians, near this run's minimums (141 and 68 ms), and
  the 895 ms seen then was the load.

The Chrome rows of section 6 and the skinned-crowd table were not re-measured.
