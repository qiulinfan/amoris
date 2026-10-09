# Occlusion culling

Status: Draft, implemented on branch `explore/hiz` (2026-10-09)

Charter: 4.4 ("compute shaders cull against the frustum and, with a Hi-Z pyramid, against
occlusion"; the Pioneer note of 2026-10-09 fixes the two-phase form, the auto mode and the switch),
3 item 7 (GPU-driven), 4.1 (one renderer for Metal, Vulkan, Direct3D 12 and WebGPU).

This specification says how the renderer culls the camera view against occlusion: the two phases,
the depth pyramid, the test, how the phases draw on the three draw paths, why the result never
drops a visible instance, when the auto mode turns it off, and how it is checked. The code is
`crates/pocket-render/src/occlusion.rs` (the pyramid, the late pass, the auto mode),
`crates/pocket-render/shaders/cull.wgsl` (`main`, the early pass, and `late`),
`crates/pocket-render/shaders/hiz.wgsl` (the pyramid), `batches.rs` (the late argument set) and
`renderer.rs` (`render`). Measurements: [docs/bench/occlusion.md](../bench/occlusion.md); evidence:
`docs/evidence/hiz/`.

The schedule (3.1) listed a branch `feat/hzb` with measurements on an M5; that branch was lost
before it was merged and nothing of it survives. This is a new implementation.

## 1. A frame

Without occlusion culling a frame culls every instance once (`cull`) and draws the camera view in
one opaque pass. With it, the camera view is drawn in two phases:

1. **Early culling** (`cull.wgsl` `main`, the same dispatch that culls the four shadow cascades):
   an instance goes to the camera's batches only if it is in the frustum *and* was visible at the
   end of the last frame. The pass also records, per instance, whether it was in the frustum and
   whether it drew it.
2. **Early opaque pass**: those batches, into the multisampled HDR target and depth (cleared;
   both stored).
3. **Depth pyramid** (`hiz.wgsl`, section 3) from the multisampled depth.
4. **Late culling** (`cull.wgsl` `late`, section 4): every instance in the frustum is tested
   against the pyramid. The visible ones the early pass did not draw go to the late argument set;
   every instance's visibility is recorded for the next frame; the pass counts what it found.
5. **Late opaque pass**: the late batches, then the ocean, the sky, particles and the editor grid
   (color and depth loaded; color resolved into the HDR image; depth stored only if the splats
   read it).

The shadow cascades keep frustum culling only. Light-space occlusion would need a pyramid per
cascade and a second shadow pass each; a cascade's casters are mostly visible from the light.

Shadows, cluster assignment and the splats' preprocessing are unchanged and run between the
early culling and the early opaque pass, as before.

## 2. Per-instance state

`state` (binding 7 of the culling passes) holds one word per instance slot:

| Bit | Name | Written by | Meaning |
|---|---|---|---|
| 0 | `VIS_VISIBLE` | `late` | visible at the end of the frame (read by the next frame's `main`) |
| 1 | `VIS_FRUSTUM` | `main` | alive, visible, of a known mesh and in the camera frustum this frame |
| 2 | `VIS_EARLY` | `main` | drawn by the early pass this frame |

`main` writes `VIS_FRUSTUM | VIS_EARLY` over the word, `late` writes `VIS_VISIBLE` over it. The late
pass uses the early pass's frustum result instead of testing the frustum again, so the two passes
can never disagree about an instance on the frustum's edge (two compiled entry points may round
differently). The buffer grows with the instance buffer and starts zeroed: after a growth nothing
counts as visible, and the next frame draws everything in the late phase. A slot reused by
another entity inherits a stale bit, which can only cost a little early work.

The state affects only which phase draws an instance, never whether it is drawn (section 5).

## 3. The depth pyramid

An `r32float` texture of `max(w / 2, 1)` by `max(h / 2, 1)` texels for a `w` x `h` depth target,
with the full mip chain (`occlusion::levels`: `floor(log2(max side)) + 1` levels; 10 for 1280x720
and 1920x1080). Mip sizes are the texture's own (halving and rounding down), so the pyramid works
at any size, not only powers of two.

Each texel holds the **minimum** depth of what it covers: the renderer uses reversed Z (1 at the
near plane, 0 at infinity), so the minimum is the farthest surface, the conservative occluder
depth. Texel `t` of level `k` covers pixels `[t << (k + 1), (t + 1) << (k + 1))` on each axis; where
the level below has an odd size, the last texel of a level also takes the last row or column below
it, so the last texel of every level reaches the edge. A pixel `p` therefore maps to texel
`min(p >> (k + 1), size_k - 1)` of level `k`.

- Level 0 (`from_depth`) reads the multisampled depth target as `texture_depth_multisampled_2d`
  with `textureLoad` and takes the minimum over every sample of its 2x2 (or, at an odd edge, up to
  3x3) pixels: a sample the early pass did not cover holds the cleared 0, so it never occludes.
- Level `k + 1` (`reduce`) reads level `k` as `texture_2d<f32>` (`textureLoad`) and writes with a
  write-only `texture_storage_2d<r32float, write>`.

One dispatch per level (8x8 threads), in one compute pass. Everything is WebGPU core: unfilterable
`r32float` sampled and storage use, `textureLoad` of a multisampled depth texture,
`textureNumSamples`. A level reads the mip below as a sampled view and writes its own mip as a
storage view of the same texture, which WebGPU allows (different subresources). The pyramid is
recreated when the target's size changes; `Renderer::resize` drops it because its level 0 binds the
old depth target.

## 4. The late test

For an instance with `VIS_FRUSTUM`, the late pass takes its interpolated pose (the same
`instance_pose` the early pass and the `Drawn` records use) and its mesh's bounding box
(`MeshInfo.box_center`, `box_half`; the mesh's `bounds.min`/`max`, grown by the same factor as the
sphere for skinned meshes' own copies, `MeshPool::add_dynamic`). It then:

1. transforms the box's centre and its three scaled, rotated half axes by the camera's
   view-projection (`Cull.view_proj`) and forms the eight corners in clip space;
2. if any corner has `w <= 1e-6` (behind the camera) or the nearest corner's depth is `>= 1`
   (reaching past the near plane), the instance is visible;
3. takes the corners' screen rectangle in pixels (y down), widens it by one pixel on each side and
   clamps it to the target (parts off screen cannot be seen);
4. picks the finest level `k` whose texels (`2^(k+1)` pixels) put the rectangle within 2x2 texels:
   `k = firstLeadingBit(extent - 1)` for an extent of more than 2 pixels, else 0, clamped to the top
   level;
5. reads the 2x2 texels covering the rectangle and takes their minimum `far`;
6. the instance is occluded when its nearest corner's depth is below it: `near < far * (1 - 1e-5)`.
   The relative margin keeps coplanar surfaces visible, such as a wall of touching boxes seen
   head-on by an orthographic camera, whose own depth and the pyramid's can differ in the last bit.

Occluded instances are not drawn; visible ones without `VIS_EARLY` are written to the late argument
set and to `drawn` (section 6). The orthographic camera needs no special case: its `w` is 1.

The pass also counts, with workgroup atomics first and one global atomic per workgroup and
counter: instances in the frustum, occluded, drawn early, drawn late, and the triangles in the
frustum and occluded (`index_count / 3` per instance, as 64-bit counts in two words with a carry).
The counters are copied to one of four mappable buffers and read a few frames later
(`OcclusionStats`, `FrameStats::occlusion_stats`).

## 5. Why a visible instance is never dropped

Only the late test removes an instance from the image, and it tests against depth the current frame
drew with the current camera. Suppose some sample `s` shows the instance in the final image. The
early phase's depth at `s` is then no nearer than the instance's surface at `s` (otherwise the
instance would be hidden there), and that surface is no nearer than the box's nearest corner:
`pyramid min over the footprint <= early depth at s <= instance depth at s <= near`. The rectangle
contains `s`'s pixel because the box contains the mesh and the rectangle bounds the box's
projection (with a pixel to spare), and the pyramid's texels for the rectangle cover every sample
of those pixels. So `near >= far`, and the test keeps the instance.

What this relies on:

- the bounding box contains the drawn mesh. For skinned meshes the box is the bind pose's grown by
  2 (the same assumption frustum culling makes with the sphere); an animation reaching further
  would be culled wrongly by both;
- the early phase's depth is real geometry: alpha-masked instances write depth only where they
  are opaque, so their holes do not occlude (the check's masked cylinder, section 8). The ocean,
  particles, splats and the sky are drawn after the pyramid and never occlude;
- the visibility state is a hint. A wrong `VIS_VISIBLE` (first frame, a resize, a camera cut, a
  slot reused, the mode just switched on, an occluder that moved) only changes which phase draws an
  instance: the first frame draws nothing early and everything visible late.

Draw order differs between the modes: early instances are drawn before late ones. With the
`Greater` depth test, two surfaces at exactly equal depth keep the first drawn, so a tie can
resolve differently with occlusion culling on. The only difference found in the comparisons is one
pixel one level apart in many_cubes' sphere (docs/bench/occlusion.md 2).

## 6. Drawing the phases

**Argument sets.** `Batches` keeps a sixth set of indexed indirect arguments after the five
views' (`batches::LATE`); the template gives it the camera's bases. The late pass counts into it
(`draws[LATE * batches + batch]`) and writes its `Drawn` records to the **same regions** of `drawn`
as the early pass, from index 0: the early draws have been consumed by then. So `drawn` stays one
region per batch (48 bytes per instance), and a late batch's base is the camera's on every path:

| Path | Late batches' base |
|---|---|
| `multi-draw` | the arguments' `first_instance` (template), one multi-draw per variant |
| `first-instance` | likewise, one draw per batch holding instances |
| `baseline` | the camera view's row of the base uniform (`view_of(LATE) = 0`) |

`draw_calls` counts both phases.

**The entity-id pass** draws the camera's batches, and the late pass overwrites the early records,
so with occlusion culling the id pass is drawn in two parts as well (`Picking::begin`, `draw`,
`finish`): after the early opaque pass, clearing its target and keeping its depth, and after the
late opaque pass, loading both; the splats' ids and the readback copy come after the second.
A pixel request's scissor applies to both.

**MSAA.** The early pass stores the multisampled color and depth instead of discarding them, and
the late pass loads them and resolves. On the RTX 5060 with Vulkan the split costs nothing
measurable; in Chrome on Direct3D 12 the sphere's early pass took 0.4 ms longer than the single
pass (docs/bench/occlusion.md).

**Interpolation.** Both passes compute the drawn pose with `instance_pose(inst, alpha)`; the late
test uses the pose the instance is drawn at.

## 7. Modes

`OcclusionMode` (`occlusion.rs`): `Off` (one culling pass, one opaque pass, the state untouched),
`On` (two phases every frame) and `Auto`, the default. `POCKET_OCCLUSION=off|on|auto` sets the
starting mode natively, `Renderer::set_occlusion` at any time, the viewport page
`?occlusion=off|on|auto`. `FrameStats::occlusion` says what the frame did (`off`, `on`, `auto-on`,
`auto-off`). A frame with no instances never runs the second phase.

**Auto.** The second phase costs a fixed part (the pyramid, the second opaque pass's load and
resolve) and a part per instance in the frustum (the late test). The auto mode counts both in the
triangles occlusion culling must remove to pay, from the late pass's counters:

    cost = 200,000 + 0.5 x instances in the frustum   (triangles)

calibrated on many_cubes on the RTX 5060, where about a million instanced triangles cost a
millisecond and the late test about a quarter of a microsecond per instance (docs/bench/
occlusion.md). The bench's dense layout at several sizes shows the crossover between 60,000 and
250,000 occluded triangles on both GPUs it was measured on; the model is conservative there by
about the probe margin below.

- A **probe** runs two phases for two frames, `p` and `p + 1`; the reading of `p + 1` decides (the
  early pass of `p` draws a stale set, so `p`'s reading does not count). It turns occlusion culling
  on if the occluded triangles exceed `1.5 x cost`. Meanwhile the frames run one phase.
- The first probe (the renderer's first frame with instances, or the first after `set_occlusion`)
  is optimistic: it keeps two phases until its reading arrives. A reading takes 2 or 3 frames
  natively, but in Chrome under an uncapped frame loop it took over 300 frames (the queue runs
  ahead of the GPU), and many_cubes dense spent them at 17 ms instead of 1.
- On, it stays on while the occluded triangles cover `cost` (readings from the frame after it went
  on), and turns off after three readings in a row that do not.
- Off, it probes again 120 frames later; after each probe that fails the wait doubles, up to 960
  frames; a probe that pays resets it. A probe whose reading has not arrived 600 frames later counts
  as failed (a lost readback).
- Frames without instances run one phase and move a pending probe, or the start of an activation,
  to the next frame that has some.

The constants were fit on one GPU. Counting in triangles makes them carry across scenes; across
GPUs they carry roughly, because a slower GPU's triangles and its fixed costs are both slower: on
the Radeon 780M, 2 to 6 times slower, the crossover fell between the same cube counts.

## 8. Limits

- Compute: the early pass binds 7 storage buffers (the state is the seventh), the late pass 7
  (no `visible`; the counters added), within the default 8. The forward pipeline's fragment stage is
  unchanged (8 of 8, docs/spec/webgpu-baseline.md 3): occlusion culling stays in compute.
- Uniform: `Cull` grows to 592 bytes (the camera's view-projection, the mode, the pyramid's levels
  and the target's size).
- Memory: 4 bytes per instance slot for the state, the pyramid (about a third more than a quarter
  of the target's pixels, 4 bytes each: 1.2 MB at 1280x720), and 32 bytes of counters with four
  32-byte readback buffers. `drawn` is not doubled (section 6).
- `MeshInfo` grows from 32 to 64 bytes (the box).

## 9. Checks

- `crates/pocket-render/tests/occlusion.rs` draws `demo::occluders` (a wall with a 0.5 m slit, an
  alpha-masked double-sided cylinder with holes, a picket fence past the wall's end, 24 small
  spheres peeking over the wall's top by a few centimetres, and 144 primitives behind) with
  occlusion culling off and forced on, through the same frames: the cold first frame, a warm frame,
  a camera cut, a six-step sideways sweep, the left wall sinking into the ground (halfway through
  the interpolation, then arrived), a resize, a camera against the slit and an orthographic camera.
  Each step's id pass is drawn in the first frame after the change. The id coverage per entity must
  match within 1e-6 and fewer than 1 in 2,000 pixels may differ by more than 2 levels; the forced-on
  renderer must have occluded at least 20 instances by the warm step. It runs on the device
  `POCKET_BACKEND` and `POCKET_GPU_MINIMAL` describe, then on the other two draw paths; it skips
  without a GPU. Reading only one of the 2x2 texels makes it fail (sweep 1: a sphere peeking over
  the wall is dropped).
- `occlusion.rs` unit tests: the pyramid's level count, the auto mode's decisions and backoff, the
  mode names.
- `cargo run --release -p pocket-app --example draw_paths -- --compare occlusion <scenes>` draws a
  scene twice in one process at one moment, off and forced on, and compares pixels and coverage.
- `python tools/occlusion_compare.py` runs that and every capture of `tools/backend_compare.py`
  (plus the many_cubes orbit) with `POCKET_OCCLUSION=off` and `on` on one backend and adapter.
- `python tools/web_draw_paths.py <page> --compare occlusion` does the same in headless Chrome
  (`?demo=occluders`, `?demo=cubes`, and with `gpu_minimal=first-instance` for the baseline path).

## 10. Open

- Phase 1 draws last frame's visible set without testing it; the common refinement tests it
  against last frame's pyramid reprojected, which helps when much of last frame's set got hidden.
  Not needed for the scenes here (section 5 holds either way).
- Shadow cascades are frustum culled only.
- Instances are the unit: a large mesh partly hidden is drawn whole. Meshlet (cluster) culling would
  need mesh splitting and a cluster pass.
- The auto mode's constants were calibrated on one GPU under load from other processes; the browser's
  readback latency makes probes long there.
- The bounding box is per mesh; a skinned part's box is the bind pose's grown by 2.
- `Cull.lod_scale` is still unused (mesh LOD, charter 4.4's meshopt).
