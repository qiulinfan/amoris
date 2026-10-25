# Shadow cascades: slice culling, method and measurements

Each shadow cascade culls its casters by its box and by the extruded camera slice it shades (charter
4.4, Pioneer note of 2026-10-15; [occlusion spec](../spec/occlusion.md) 11).
`AMORIS_SHADOW_CULL=off|on` (default on), `Renderer::set_shadow_cull` and the viewport page's
`?shadow_cull=off|on` switch it. Related: [levels of detail](lod.md) (where the cascades' geometry
was first counted), [occlusion](occlusion.md).

Status: **quiet re-measurement**, 2026-10-15, 15:10 to 15:35, on the tree of `1402d77d` (its
renderer is `1846b2bd`'s, which adds `shadow_factor`'s guard for points behind the eye, section 2,
and changes no culling). No other agent's work was running: CPU load 2 to 6% at the runs' starts
(`tools/machine_conditions.py`), AC power, the RTX 5060 at 55 to 64 C at the start of each native
timing run. Every timing of sections 2 and 4 is from this run, which followed section 5 exactly.
It replaces the provisional timings taken earlier the same day on the renderer of `f9ea084e` while
five other agents' builds loaded the CPU (25 to 96%); section 4 says where the quiet run changed a
conclusion. The image comparisons and counts of sections 2 and 3 are exact: run again on the quiet
machine, they gave the same frames and the same counts in every configuration as on `1846b2bd`.

## 1. Method

- Machine: R1 (Ryzen 9 270, 16 threads, 15 GB; RTX 5060 Laptop 8 GB, driver 617.14 / 32.0.16.1714;
  Radeon 780M, 32.0.13062.3005), Windows 11, AC power. Release build of `explore/shadowcull`.
- `cargo run --release -p amoris-render --example shadow_cull -- compare` draws ten scenes at
  1280x720, each with two renderers on one device through the same frames at the same moments,
  slice culling off and on, and compares every frame to the bit; after each frame it reads back
  what every argument set drew (`Renderer::draw_counts`). Occlusion culling and the depth prepass
  are off unless `--occlusion` says otherwise (the prepass's auto mode decides by GPU timings, which
  differ between two renderers); anti-aliasing and GTAO are the adapter's defaults (4x MSAA and
  GTAO on the RTX 5060, TAA on the 780M: TAA's history must accumulate identical frames). Scenes:
  `mixed` (`demo::mixed(8)`, every pipeline variant), `mixed-orbit` (with three skinned
  characters, the camera circling low and high, 24 frames), `cubes-sphere` and `cubes-dense`
  (many_cubes' layouts, 200,000 shadow-casting cubes), `lod-0`, `lod-0.5`, `lod-off-0`,
  `lod-off-0.5` (the LOD field of [lod.md](lod.md) 1 at `t` 0 and 0.5, levels on and off),
  `lod-fly` and `lod-off-fly` (its camera walking from `t` 0 to 1, the middle of the field, over 24
  frames).
- `python tools/render/shadow_cull_bench.py compare --configs ...` runs it per backend and adapter,
  and with `+minimal` on WebGPU's baseline device (`AMORIS_GPU_MINIMAL=features,limits,timestamps`:
  no optional features, default limits, the per-batch draw path); `games` runs the amoris-app
  example `draw_paths --compare shadow_cull` on four sample projects (with what each cascade drew,
  off and on); `table` prints the tables below.
- `shadow_cull -- bench SCENE` times one scene with one renderer offscreen, switching slice culling
  every 40 measured frames (the 6 frames after each switch are drawn but not kept, so a pass
  timing read back late never counts under the other mode), 4 rounds, off first in odd rounds and
  on first in even ones, after 10 warm-up frames per mode and `--settle 5`. Each frame is submitted
  and waited for. Per frame: the GPU time (the profiler's timestamped passes), the four cascades'
  summed `shadows` passes, the `cull` pass, the wall time (submit to GPU idle) and the CPU's
  encoding. `tools/render/shadow_cull_bench.py bench` runs it per configuration and scene after
  `tools/machine_conditions.py`; tables give, per mode, the median over the rounds of each block's
  p50 and the rounds' spread ((max - min) / median). Raw output: `out/shadowcull/` (not committed).

## 2. The image does not change

`shadow_cull -- compare` over the ten scenes (98 compared frames per configuration), renderer of
`1846b2bd`, run again on the quiet machine with the same result: every frame identical to the bit
with slice culling off and on, and the camera's draws equal, in each of

| Configuration | Draw path | Frames identical |
|---|---|---|
| RTX 5060, Direct3D 12 | multi-draw | 98 / 98 |
| RTX 5060, Vulkan | multi-draw | 98 / 98 |
| Radeon 780M, Direct3D 12 | multi-draw | 98 / 98 |
| Radeon 780M, Vulkan | multi-draw | 98 / 98 |
| RTX 5060, Direct3D 12, WebGPU baseline device | baseline | 98 / 98 |
| Radeon 780M, Vulkan, WebGPU baseline device | baseline | 98 / 98 |
| RTX 5060, Direct3D 12, occlusion culling forced on | multi-draw | 98 / 98 |
| RTX 5060, Vulkan, occlusion culling forced on | multi-draw | 98 / 98 |

(The first run, on `f9ea084e`, gave the same in the first six configurations and 57 of 57 frames
over five scenes with occlusion culling forced on.) `tests/shadow_cull.rs` (24 frames: the mixed
scene with three skinned characters orbiting, a 16 x 16 field walked into with levels on and off,
the cube sphere turning) passed on both backends of both GPUs.

`python tools/render/shadow_cull_bench.py games` (`draw_paths --compare shadow_cull` after 120
ticks, occlusion culling and the prepass off) found no pixel and no entity-id coverage different on
the RTX 5060 with Direct3D 12 and Vulkan and on the 780M with Direct3D 12 (twice: on `1846b2bd` and
on the quiet machine). What the cascades drew, the same in the three configurations and both runs:

| Project | Instances | Cascade instances, off -> on | Cascade triangles, off -> on |
|---|---|---|---|
| samples/anim (a skinned hero) | 22 | 22 -> 22, 22 -> 21, 22 -> 1, 22 -> 1 | 3,544 -> 1,752 |
| tests/fixtures/scenes/pt-lab | 11 | 11 -> 11, 11 -> 10, 11 -> 1, 11 -> 0 | 8,184 -> 5,902 |
| samples/sailing (the ocean among the receivers) | 5 | 2 -> 2, 4 -> 4, 5 -> 5, 5 -> 5 | 2,285 -> 2,285 |
| tests/fixtures/scenes/gi-room (no sun: a control) | 8 | none | 0 |

So anim and pt-lab test the culling (they lose 43 and 22 instance draws); sailing's casters all
stay, and its comparison only shows the switch changes nothing else.

**What the image checks catch.** Planted bugs, each built and run through `shadow_cull -- compare`
(every scene, 98 frames) and `tests/shadow_cull.rs` on the RTX 5060 with Direct3D 12 (the scripts
are kept with the raw output, `out/shadowcull/round2/`):

| Planted bug | Instance draws culled beyond the real planes (all frames) | Frames differing | GPU test |
|---|---|---|---|
| Zero margin (planes through the slice's corners) | 1,919 (0.3%) | 0 of 98 | passes |
| No normal offset in the margin | a subset of the zero margin's | 0 (implied; not run) | passes (implied; not run) |
| Planes 1 m inward | 5,760 (0.8%) | 5 of 98 (the orbit and both walks; up to 130 levels) | fails |
| The slice test keeps only spheres wholly inside its planes (cull.wgsl) | 77,486 (10.7%) | 7 of 98 (the orbit and both walks; up to 134 levels) | fails |

So the image checks catch gross errors in the planes or the shader's test but not a margin that is
too small: in these scenes the casters near the slice's edges never decide a texel a receiver
reads. (A bug that culls a subset of what an unseen bug culls is unseen too: the comparisons are
monotonic in the map's depth.) The margin rests on the derivation and on the unit test
`every_caster_a_receiver_can_read_is_kept` (spec section 11), which on this tree catches each of six
planted bugs in the planes' construction: no normal offset in the margin (so also the zero
margin, which culls more), a kernel reach of 2 texels, no widening for the jitter, the far plane at
the nearest corner, and the first slice starting at the cascades' near split or at the camera's
near plane instead of the eye.

**Points behind the eye.** With MSAA a pixel that a triangle covers only in part shades at its
centre, its world position extrapolated along the triangle's plane: on a triangle nearly edge-on to
the camera that point can land behind the eye. `shadow_factor` used to read cascade 0 there, at a
point outside the first slice (which starts at the eye), so slice culling could in principle have
changed such a pixel. Since `1846b2bd` `shadow_factor` reads no cascade at a negative view depth.
That changes the image only where a pixel's point lies behind the eye;
`tools/render/build_compare.py` against the build before (`f9ea084e`'s renderer; many_cubes' sphere
and dense layout with shadows, lod_field's 40 skinned columns; MSAA, MSAA with GTAO, TAA) found no
pixel different in any of its 27 pairs (RTX 5060 with Direct3D 12 and Vulkan, 780M with Direct3D
12): no such pixel in those frames.

**In Chrome.** The viewport page built locally from this branch (`tools/build_viewport.sh`; the
package is not committed), with `?shadow_cull=off` and `on`, through
`python tools/render/web_draw_paths.py <page> --compare shadow_cull --seconds 8 --size 1280x720`
(headless Chrome, a 1254x564 canvas, the page's 4x MSAA): `?demo=mixed` and `?demo=lod&t=0` on the
RTX 5060 (`--force_high_performance_gpu`) and on the 780M had the same id-pass coverage (59 and 266
entities) and identical screenshots (maximum difference 0). `?demo=lod&t=0&lod=off` on the RTX 5060
gave identical screenshots, but its coverage readback did not finish within the tool's 5 seconds;
on the 780M that page did not finish within the tool's 180 seconds (not tried again on the quiet
machine). All of this held on the loaded machine and again on the quiet one. The switch acts in
the browser. Quiet re-measurement (2026-10-15, CPU load 3 to 6%), the last frame's `shadows`
passes off and on, one reading each: 0.41 and 0.25 ms (`lod-0`, RTX 5060), 1.69 and 1.08 ms
(`lod-0`, 780M), 37.7 and 20.3 ms (`?demo=lod&t=0&lod=off`, RTX 5060), 0.041 and 0.036 ms
(`mixed`, RTX 5060) and 0.24 and 0.20 ms (`mixed`, 780M). For `lod-0` the GPU time's mean over the
300 frames fell from 0.81 to 0.65 ms (RTX 5060) and from 3.31 to 2.66 ms (780M). The loaded
machine's single readings (0.41 and 0.24, 1.72 and 1.12, 37.7 and 20.3 ms) were within 4% of these.

## 3. What the cascades draw

The last compared frame of each scene, per cascade: instances drawn with slice culling off / on
(read back from the indirect arguments; the same on both backends and both draw paths of the RTX
5060, and with occlusion culling forced on, which changes only the camera's counts):

| Scene | Cascade 0 | Cascade 1 | Cascade 2 | Cascade 3 | Cascade triangles off / on | Camera triangles |
|---|---|---|---|---|---|---|
| `mixed` | 2 / 1 | 64 / 41 | 65 / 65 | 65 / 58 | 20,476 / 15,042 (0.73x) | 8,188 |
| `mixed-orbit` | 12 / 7 | 73 / 47 | 77 / 77 | 77 / 53 | 20,920 / 14,870 (0.71x) | 8,189 |
| `cubes-sphere` | 0 / 0 | 0 / 0 | 0 / 0 | 222 / 0 | 2,664 / 0 | 281,412 |
| `cubes-dense` | 0 / 0 | 12 / 0 | 43,910 / 13,438 | 200,000 / 200,000 | 2,927,064 / 2,561,256 (0.88x) | 2,399,952 |
| `lod-0` | 12 / 6 | 43 / 22 | 208 / 94 | 2,134 / 1,179 | 1,430,920 / 773,256 (0.54x) | 1,441,538 |
| `lod-0.5` | 22 / 9 | 76 / 25 | 379 / 112 | 2,297 / 1,268 | 1,697,544 / 843,144 (0.50x) | 896,386 |
| `lod-off-0` | 12 / 6 | 43 / 22 | 208 / 94 | 2,134 / 1,179 | 176,734,216 / 96,190,472 (0.54x) | 169,869,314 |
| `lod-off-0.5` | 22 / 9 | 76 / 25 | 379 / 112 | 2,297 / 1,268 | 204,144,648 / 103,710,728 (0.51x) | 97,042,434 |
| `lod-fly` (t = 1) | 17 / 7 | 77 / 26 | 374 / 117 | 2,137 / 676 | 1,579,656 / 499,848 (0.32x) | 378,754 |
| `lod-off-fly` (t = 1) | 17 / 7 | 77 / 26 | 374 / 117 | 2,137 / 676 | 191,414,280 / 60,227,592 (0.31x) | 44,236,802 |

The same frames' triangles per cascade, off / on:

| Scene | Cascade 0 | Cascade 1 | Cascade 2 | Cascade 3 |
|---|---|---|---|---|
| `mixed` | 1,026 / 2 | 10,984 / 6,980 | 5,854 / 5,854 | 2,612 / 2,206 |
| `mixed-orbit` | 482 / 290 | 10,964 / 6,170 | 6,358 / 6,358 | 3,116 / 2,052 |
| `cubes-sphere` | 0 / 0 | 0 / 0 | 0 / 0 | 2,664 / 0 |
| `cubes-dense` | 0 / 0 | 144 / 0 | 526,920 / 161,256 | 2,400,000 / 2,400,000 |
| `lod-0` | 27,906 / 11,522 | 43,778 / 22,018 | 129,538 / 57,986 | 1,229,698 / 681,730 |
| `lod-0.5` | 52,482 / 17,922 | 85,378 / 27,906 | 237,186 / 68,866 | 1,322,498 / 728,450 |
| `lod-off-0` | 835,586 / 409,602 | 3,145,730 / 1,589,250 | 15,351,810 / 6,930,434 | 157,401,090 / 87,261,186 |
| `lod-off-0.5` | 1,523,714 / 655,362 | 5,488,642 / 1,736,706 | 27,852,802 / 8,077,314 | 169,279,490 / 93,241,346 |
| `lod-fly` (t = 1) | 40,450 / 14,594 | 78,082 / 28,162 | 232,578 / 70,914 | 1,228,546 / 386,178 |
| `lod-off-fly` (t = 1) | 1,179,650 / 458,754 | 5,570,562 / 1,851,394 | 27,410,434 / 8,486,914 | 157,253,634 / 49,430,530 |

- In the LOD field the cascades draw 0.31x to 0.54x the triangles: 96 million instead of 177
  million at `t = 0` and 104 instead of 204 million at `t = 0.5` with levels off (the camera draws
  170 and 97 million), 0.77 instead of 1.43 million with levels on. Over the 24 frames of the walk
  they draw 0.48x the instances and triangles. The far cascade's box (about 370 m wide around a
  slice 108 m deep, wider than the 288 m field) keeps 55% of its instances at `t` 0 and 0.5 and
  32% at `t = 1`, where the camera stands in the middle of the field.
- `cubes-dense`: the camera sees the whole grid from outside, so the far cascade keeps every cube;
  cascade 2 keeps 31%. `cubes-sphere`: the cubes stand on a sphere of 500 m around the camera, far
  beyond the 150 m shadow distance; the 222 in the far cascade's box (toward the sun, which its
  box reaches 200 m past the slice) shadow nothing on screen and are all culled.
- `mixed`: 0.73x; cascade 1 loses a third of its instances, cascade 3 a tenth, cascade 2 none.
  Over the 24 frames of the orbit (with the skinned characters) 0.53x the triangles.
- The Radeon 780M draws with TAA by default (the RTX 5060 with 4x MSAA), and TAA's jitter widens
  the slices by up to half a pixel more each frame, so with the slices on the 780M keeps a few
  more instances in some frames: one knot in cascade 3 in one of the three frames at `t` 0 and
  0.5, 4 and 13 instance-frames in cascades 2 and 3 over the walk, 34 cubes over the three
  `cubes-dense` frames. With slice culling off both GPUs draw the same counts.

## 4. Cost

`python tools/render/shadow_cull_bench.py bench` on each adapter and backend, 1280x720, occlusion
culling and the depth prepass off, the adapter's default anti-aliasing (the RTX 5060: 4x MSAA and
GTAO; the 780M: TAA); `cubes-dense` with 200,000 cubes. Quiet re-measurement, 2026-10-15 15:10 to
15:20 (CPU load 3 to 5% at the runs' starts, the RTX 5060 at 55 to 64 C). Milliseconds, the median
over the 4 rounds of each block's p50 (the rounds' spread in brackets); "Shadows" is the four
cascades' passes summed, "Frame GPU" all timestamped passes, "Wall" submit to GPU idle:

| Adapter, backend | Scene | Shadows off | Shadows on | Ratio | Cull off | Cull on | Frame GPU off | Frame GPU on | Wall off | Wall on |
|---|---|---|---|---|---|---|---|---|---|---|
| 780M, Direct3D 12 | cubes-dense | 3.73 (1%) | 2.49 (7%) | 0.67 | 0.34 (4%) | 0.34 (17%) | 16.82 (1%) | 15.43 (2%) | 18.86 (0%) | 17.45 (4%) |
| 780M, Direct3D 12 | lod-0 | 2.43 (17%) | 1.74 (11%) | 0.72 | 0.02 (20%) | 0.02 (5%) | 4.49 (18%) | 3.82 (13%) | 6.36 (16%) | 5.64 (15%) |
| 780M, Direct3D 12 | lod-0.5 | 2.99 (17%) | 1.78 (12%) | 0.59 | 0.02 (35%) | 0.02 (5%) | 4.84 (11%) | 3.56 (9%) | 6.55 (9%) | 5.19 (7%) |
| 780M, Direct3D 12 | lod-off-0 | 35.75 (8%) | 20.56 (12%) | 0.58 | 0.02 (12%) | 0.02 (19%) | 108.62 (2%) | 93.19 (4%) | 111.91 (2%) | 96.06 (4%) |
| 780M, Direct3D 12 | lod-off-0.5 | 45.45 (1%) | 24.76 (19%) | 0.54 | 0.02 (6%) | 0.02 (6%) | 88.55 (0%) | 68.15 (12%) | 92.07 (1%) | 70.96 (10%) |
| 780M, Direct3D 12 | mixed | 0.71 (9%) | 0.69 (2%) | 0.98 | 0.02 (113%) | 0.02 (35%) | 1.66 (54%) | 1.69 (21%) | 2.96 (47%) | 2.98 (17%) |
| 780M, Vulkan | cubes-dense | 3.76 (2%) | 2.59 (1%) | 0.69 | 0.33 (6%) | 0.32 (2%) | 12.21 (1%) | 10.92 (2%) | 14.05 (4%) | 12.58 (4%) |
| 780M, Vulkan | lod-0 | 2.13 (21%) | 1.53 (2%) | 0.72 | 0.02 (25%) | 0.02 (0%) | 3.64 (19%) | 3.06 (8%) | 5.16 (17%) | 4.37 (6%) |
| 780M, Vulkan | lod-0.5 | 2.52 (23%) | 1.65 (3%) | 0.66 | 0.01 (27%) | 0.01 (0%) | 3.87 (15%) | 2.93 (8%) | 5.49 (12%) | 4.28 (7%) |
| 780M, Vulkan | lod-off-0 | 45.41 (4%) | 25.35 (2%) | 0.56 | 0.01 (15%) | 0.01 (15%) | 121.13 (3%) | 101.29 (1%) | 125.31 (3%) | 105.06 (0%) |
| 780M, Vulkan | lod-off-0.5 | 58.61 (3%) | 30.71 (8%) | 0.52 | 0.01 (17%) | 0.01 (17%) | 102.49 (3%) | 74.92 (6%) | 106.39 (2%) | 78.34 (6%) |
| 780M, Vulkan | mixed | 0.67 (10%) | 0.66 (2%) | 0.98 | 0.01 (133%) | 0.01 (38%) | 1.39 (45%) | 1.37 (10%) | 2.51 (37%) | 2.52 (6%) |
| RTX 5060, Direct3D 12 | cubes-dense | 1.86 (8%) | 1.36 (19%) | 0.73 | 0.09 (6%) | 0.09 (15%) | 7.17 (9%) | 6.65 (20%) | 8.52 (9%) | 8.21 (16%) |
| RTX 5060, Direct3D 12 | lod-0 | 0.35 (0%) | 0.22 (0%) | 0.63 | 0.02 (0%) | 0.02 (0%) | 0.93 (0%) | 0.81 (0%) | 1.86 (4%) | 1.71 (7%) |
| RTX 5060, Direct3D 12 | lod-0.5 | 0.46 (0%) | 0.25 (0%) | 0.55 | 0.01 (7%) | 0.01 (7%) | 0.94 (0%) | 0.73 (0%) | 1.88 (3%) | 1.65 (1%) |
| RTX 5060, Direct3D 12 | lod-off-0 | 33.86 (3%) | 18.72 (2%) | 0.55 | 0.02 (0%) | 0.02 (6%) | 70.13 (3%) | 55.19 (2%) | 71.82 (3%) | 56.87 (2%) |
| RTX 5060, Direct3D 12 | lod-off-0.5 | 39.27 (3%) | 20.20 (1%) | 0.51 | 0.01 (7%) | 0.01 (0%) | 60.08 (2%) | 41.22 (1%) | 61.73 (3%) | 42.95 (1%) |
| RTX 5060, Direct3D 12 | mixed | 0.06 (2%) | 0.05 (2%) | 0.95 | 0.01 (7%) | 0.01 (7%) | 0.31 (1%) | 0.31 (1%) | 1.23 (3%) | 1.21 (1%) |
| RTX 5060, Vulkan | cubes-dense | 1.74 (15%) | 1.31 (15%) | 0.75 | 0.09 (14%) | 0.10 (15%) | 5.47 (12%) | 5.04 (13%) | 6.29 (11%) | 5.78 (14%) |
| RTX 5060, Vulkan | lod-0 | 0.34 (0%) | 0.21 (0%) | 0.62 | 0.01 (0%) | 0.01 (7%) | 0.84 (0%) | 0.71 (0%) | 1.40 (9%) | 1.28 (7%) |
| RTX 5060, Vulkan | lod-0.5 | 0.43 (0%) | 0.23 (0%) | 0.54 | 0.01 (0%) | 0.01 (0%) | 0.84 (0%) | 0.64 (0%) | 1.41 (3%) | 1.19 (4%) |
| RTX 5060, Vulkan | lod-off-0 | 33.42 (2%) | 18.60 (1%) | 0.56 | 0.01 (25%) | 0.01 (17%) | 69.06 (2%) | 54.50 (1%) | 70.33 (2%) | 55.73 (1%) |
| RTX 5060, Vulkan | lod-off-0.5 | 38.21 (1%) | 19.68 (1%) | 0.52 | 0.01 (10%) | 0.01 (17%) | 58.55 (1%) | 40.30 (0%) | 59.78 (1%) | 41.66 (1%) |
| RTX 5060, Vulkan | mixed | 0.04 (278%) | 0.04 (3%) | 0.93 | 0.01 (415%) | 0.01 (0%) | 0.27 (366%) | 0.27 (0%) | 0.82 (171%) | 0.81 (3%) |

- **With levels of detail off, the cascades' pass time follows their triangles.** The LOD field's
  four cascade passes cost 0.51x to 0.58x for 0.51x to 0.54x the triangles (RTX 5060: 33.9 to
  18.7 ms at `t = 0` and 39.3 to 20.2 ms at `t = 0.5` on Direct3D 12, 33.4 to 18.6 and 38.2 to
  19.7 ms on Vulkan; 780M: 35.7 to 20.6 and 45.4 to 24.8 ms on Direct3D 12, 45.4 to 25.4 and 58.6
  to 30.7 ms on Vulkan). On both GPUs the frame's GPU time falls by what the passes save, to within
  0.35 ms: 14.6 to 18.9 ms on the RTX 5060 and 15.4 to 27.6 ms on the 780M.
- **What the quiet run changed here.** The loaded machine had the 780M's frame fall about 5 ms more
  than its passes saved at `t = 0`; the provisional text called that noise from the other passes,
  and on the quiet machine the excess is gone, so the frame-time saving is now attributable to the
  slices on the 780M too. The loaded machine had also slowed the 780M's passes 1.3x to 1.8x (63 to
  34 ms instead of 36 to 21 ms at `t = 0` on Direct3D 12): an integrated GPU shares memory and
  power with the busy CPU (presumably the cause; not measured apart). The RTX 5060's passes were 3
  to 7% slower under load, and the ratios hardly moved (0.51x to 0.56x provisionally).
- Elsewhere pass time and triangles part. With levels on, the field's cascades draw about a million
  triangles, 0.54x (`t = 0`) and 0.50x (`t = 0.5`) of them with the slices, and their passes cost
  0.62x to 0.72x and 0.54x to 0.66x (RTX 5060 0.35 to 0.22 ms and 0.46 to 0.25 ms on Direct3D 12;
  780M 2.4 to 1.7 and 3.0 to 1.8 ms), presumably because costs per pass and per draw that do not
  shrink with the triangles weigh more there (not measured apart). That is 0.13 to 0.20 ms of a
  0.84 to 0.94 ms frame on the RTX 5060 and 0.6 to 1.3 ms of a 3.6 to 4.8 ms frame on the 780M: the
  frame's GPU time falls by 13.6% to 26.4% (provisionally 13.5% to 24%). The 780M's rounds spread
  up to 23% in these short frames.
- `cubes-dense` (200,000 cubes): the cascades keep 0.88x of the triangles, but their passes cost
  0.67x to 0.75x (provisionally 0.70x to 0.80x): 0.43 to 0.50 ms less on the RTX 5060 and 1.2 ms
  less on the 780M, and the frame's GPU time falls by about as much. What goes is in cascade 2 (the
  far cascade keeps every cube); a cube in the finer cascade 2 covers more texels than one in
  cascade 3, which would make its triangles dearer to raster, but this cause was not tested.
- `mixed` has 20,000 shadow triangles; its cascade passes (0.04 to 0.06 ms on the RTX 5060, 0.67
  to 0.71 ms on the 780M) are mostly per-pass overhead. Changed by the quiet run: with the slices
  they were cheaper in every round, by 0.003 ms (5 to 7%) on the RTX 5060 and 0.01 to 0.02 ms (2%)
  on the 780M; the loaded machine's change was within the rounds' spread. The frame's GPU time did
  not change measurably. (The 278% spread of the RTX 5060's Vulkan `mixed` is one block, the first
  measured after the warm-up: 0.155 ms against 0.041. The 780M's first off blocks are also its
  slowest in several scenes; the median over the rounds sets them aside.)
- **The culling pass does not get dearer** at these sizes: 0.01 to 0.02 ms for the LOD field's
  2,305 instances and 0.09 to 0.10 ms (RTX 5060) and 0.32 to 0.34 ms (780M) for 200,000 cubes,
  the same off and on within the spread. The CPU's encoding (the planes are computed there) took
  0.3 to 1.2 ms per frame either way; no configuration's median was more than 0.035 ms dearer with
  the slices, and most were cheaper. The trace below times the planes alone.
- Wall time follows the GPU time.

**many_cubes' dense layout at full size** (1,600,000 shadow-casting cubes, RTX 5060 only;
`bench --configs dx12:nvidia,vulkan:nvidia --scenes cubes-dense --count 1600000`, quiet, the GPU
at 64 C at the start): the cascades drew 2,388,681 instances and 28,664,172 triangles, 1,668,129
and 20,017,548 with the slices (0.70x; the far cascade keeps 1,401,175 of 1,600,000 cubes, the
nearer ones about a third). Their passes cost 27.1 to 15.4 ms on Direct3D 12 and 25.5 to 14.9 ms
on Vulkan (0.57x and 0.58x: they fall further than the triangles, as with 200,000 cubes), the
frame's GPU time 47.8 to 37.3 and 40.2 to 30.2 ms. That is 1.4 and 0.7 ms less than the passes
saved: the rest of the frame was that much dearer with the slices in every round (not looked into).
The culling pass: on Vulkan 0.458 to 0.469 ms, 0.011 ms (2.4%) more in every round; on Direct3D 12
0.586 and 0.581 ms, within the rounds' spread (off alone spread 10%, 0.551 to 0.610 ms). **This
changes the provisional reading**, which had Direct3D 12 0.015 ms (2.4%) dearer in every round and
Vulkan within the spread. What both runs support is that the up to nine plane tests for each cube
in a cascade's box cost at most about 0.01 ms (2%) at 1.6 million cubes; which backend shows it
consistently differed between the runs (the cause is presumed, not measured apart).

**Cross-check with a per-frame trace** (quiet, `AMORIS_TRACE=frames=60,skip=20`, the `lod_field`
example `--lod off --t 0 --frames 90 --warm 10 --settle 5`, occlusion culling in its default auto
mode, RTX 5060 with Direct3D 12, one process with `AMORIS_SHADOW_CULL=off` and one with `on`;
`python tools/render/frame_trace.py diff`): the `shadows` passes' median fell from 33.36 to 18.47 ms
(0.554x), the frame interval from 65.96 to 51.20 ms; the culling pass stayed at 0.015 ms; the CPU's
`uniforms` span, which now computes the planes, went from 11 to 15 microseconds. (Provisionally:
34.58 to 19.53 ms, 0.565x, the frame interval 68.56 to 54.24 ms, `uniforms` 14 to 19
microseconds.)

## 5. Re-measuring on a quiet machine

From the worktree, release build:

```sh
cargo build --release -p amoris-render --example shadow_cull --example lod_field
cargo build --release -p amoris-app --example draw_paths
```

Then each line below under the bench lock while other agents still share the machine. `bench`'s
default scenes are section 4's (`lod-off-0,lod-off-0.5,lod-0,lod-0.5,cubes-dense,mixed`, 4 rounds,
blocks of 40, `--settle 5`):

```sh
python tools/render/shadow_cull_bench.py bench --configs dx12:nvidia,vulkan:nvidia --out out/shadowcull/quiet/bench
python tools/render/shadow_cull_bench.py bench --configs dx12:780m,vulkan:780m --out out/shadowcull/quiet/bench
python tools/render/shadow_cull_bench.py bench --configs dx12:nvidia,vulkan:nvidia --scenes cubes-dense --count 1600000 --out out/shadowcull/quiet/bench-1.6m
python tools/render/shadow_cull_bench.py table out/shadowcull/quiet/bench out/shadowcull/quiet/bench-1.6m
```

The `table` output replaces section 4's table, its bullets' figures and the 1.6M paragraph (it
labels the 1.6M rows with their count), and the charter's measured paragraph. The trace
cross-check, once with `M=off` and once with `M=on`, then the diff (through the bench lock's Python
on Windows, name the example with its `.exe`):

```sh
AMORIS_BACKEND=dx12 AMORIS_ADAPTER=nvidia AMORIS_SHADOW_CULL=$M AMORIS_TRACE=frames=60,skip=20,out=out/shadowcull/quiet/trace-$M.json target/release/examples/lod_field --lod off --t 0 --frames 90 --warm 10 --settle 5
python tools/render/frame_trace.py diff out/shadowcull/quiet/trace-off.json out/shadowcull/quiet/trace-on.json
```

The comparisons and counts (sections 2 and 3) are exact and need no quiet machine; to repeat them:

```sh
python tools/render/shadow_cull_bench.py compare --configs dx12:nvidia,vulkan:nvidia,dx12:780m,vulkan:780m,dx12:nvidia+minimal,vulkan:780m+minimal --out out/shadowcull/quiet/compare
python tools/render/shadow_cull_bench.py compare --configs dx12:nvidia,vulkan:nvidia --occlusion on --out out/shadowcull/quiet/compare-occlusion
python tools/render/shadow_cull_bench.py games --configs dx12:nvidia,vulkan:nvidia,dx12:780m --out out/shadowcull/quiet/games
python tools/render/shadow_cull_bench.py table out/shadowcull/quiet/compare out/shadowcull/quiet/compare-occlusion out/shadowcull/quiet/games
```

The Chrome check needs the viewport built locally (`sh tools/build_viewport.sh`; restore the
committed `web/viewport/pkg` afterwards) and `web/` served:

```sh
python -m http.server -d web 8090
python tools/render/web_draw_paths.py "http://127.0.0.1:8090/viewport/?demo=lod&t=0" --name lod-0 --out out/shadowcull/quiet/web/rtx --compare shadow_cull --seconds 8 --size 1280x720 --chrome-flags=--force_high_performance_gpu
```

and the same without the flag for the 780M, and with `?demo=mixed` and `?demo=lod&t=0&lod=off`.

## 6. Open

- A cascade is drawn again every frame even when its box, the camera and its casters did not
  change. Skipping it needs a generation for everything a cascade's content depends on (instance
  poses and looks, skinned vertices, mesh and material changes, and with slice culling the camera),
  which the scene does not keep today.
- The planes bound casters by their spheres, so a long thin caster whose sphere reaches the slice
  is drawn whole; and the far cascade still keeps everything between its slice and the sun up to
  the box's near plane, 200 m back, whether or not it is hidden from the light.
- The far plane uses the slice's deepest corner. A receiver's light depth is bounded more tightly by
  the camera's depth buffer (which the Hi-Z pyramid already reduces): sample distribution
  shadow maps fit both the cascades and these planes to it. Not tried.
- The browser has single readings of the passes and 300-frame means of the GPU time (section 2),
  and the LOD field with levels off is too heavy for `web_draw_paths.py`'s readback in Chrome: its
  coverage there is unchecked (its screenshots matched on the RTX 5060, on the loaded and the quiet
  machine).
