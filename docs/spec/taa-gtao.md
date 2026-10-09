# TAA and GTAO

Status: Draft, implemented on branch `explore/aa` (2026-10-09)

Charter: 4.4 ("HDR, bloom, TAA, AgX"; the Pioneer note of 2026-10-09 fixes where TAA and GTAO sit
in the frame and that the defaults are measured), 4.1 (one renderer for Metal, Vulkan, Direct3D 12
and WebGPU).

This specification says how the renderer anti-aliases over time (TAA) and how it darkens indirect
light with ground-truth ambient occlusion (GTAO), where both sit in the frame, how they interact
with multisampling, splats, particles, bloom, the entity-id pass and the editor's overlays, how they
are switched, and how they are checked. The code is `crates/pocket-render/src/post.rs` (the options,
the opaque pass's format and targets), `taa.rs` and `shaders/taa.wgsl`, `gtao.rs` and
`shaders/gtao.wgsl`, the forward shader's extra outputs (`shaders/forward.wgsl`, `SceneOut`),
`skinning.rs` (last frame's skinned vertices) and `renderer.rs` (`render`). Measurements and the
defaults: [docs/bench/taa-gtao.md](../bench/taa-gtao.md); evidence: `docs/evidence/aa/`.

The schedule (3.3) listed a branch `feat/gtao` with measurements on an M5; that branch was lost
before it was merged and nothing of it survives. This is a new implementation.

## 1. The frame

1. Culling, lights, shadow cascades, skinning, splat preparation as before. With TAA the camera
   view's matrices carry this frame's jitter (3); culling and the depth pyramid use the jittered
   view-projection, so occlusion tests the depth they were drawn with.
2. **Opaque pass** (one or two phases with occlusion culling) into the opaque pass's targets (2.2):
   the HDR color and, when the options need them, the indirect share, the object motion and the
   view normals, multisampled and resolved, or drawn at one sample.
3. **GTAO** (5), when on: half-resolution passes from the depth, then a depth-aware upsample that
   multiplies each pixel's color by `1 - share * (1 - visibility)`.
4. **TAA** (4), when on: a compute pass blends the opaque pass's (jittered) image with the
   reprojected history into the new history, which is the image the rest of the frame reads.
5. **Splats** over that image, drawn with the camera's unjittered matrices (they have no motion
   vectors and are smooth already); with splats TAA also writes a copy for them to draw into, so
   they never enter the history. They test against an unjittered depth (1.1).
6. Bloom, the display transform with the optional sharpening (6), the editor's overlays (gizmo,
   selection outline) and the game UI.

The entity-id pass, the overlays, the selection mask and the ground grid read a second copy of the
view uniform without the jitter (`view_stable`); the UI is drawn from the unjittered matrices. The
grid is drawn inside the opaque pass, so with TAA it is accumulated like the scene, but from
unjittered positions.

### 1.1 The depth splats test against

Without TAA the quads copy the first sample of the opaque pass's depth to one sample and test
against it, and the tile raster reads that sample directly (docs/spec/splats.md). With TAA that
depth is jittered while the splats are not, and they never enter the history: the edge where a mesh
covers them would jump with the jitter every frame, and one jittered frame holds no unjittered edge
to recover. So with TAA and splats the renderer draws the depth again (`UnjitteredDepth` in
renderer.rs): the opaque instances from the unjittered frame group (`vs_depth`; the forward
variants' culling and their alpha test, `fs_depth_masked`, at the forward pass's mip level so
distant alpha-tested surfaces open up alike) and the sea, depth only at one sample, after each
opaque phase (the early set, then the late one with occlusion culling), into the single-sample depth
the quads would otherwise copy into (`Splats::unjittered_depth`). The tile raster reads it there
(its one-sample variant). The edges where meshes cover splats are then as stable as with
multisampling, and as aliased (one test per pixel). On a still frame at 256x144 with a cube, the
chain-link panel and a sea before a splat wall, 214 of about 2,480 splat pixels changed by more
than 0.05 from one frame to the next against the jittered depth, none against this one
(`tests/taa.rs`). It replaces the copy, which with light geometry costs more (the splats example's
garden at 1600x900: 0.005 against 0.031 ms on the RTX 5060, 0.015 against 0.14 ms on the Radeon
780M); with many_cubes' 1.6M cubes it costs 0.78 to 0.89 ms where the copy took 0.07 to 0.33
(docs/bench/taa-gtao.md 3.1).

## 2. Options and formats

### 2.1 Switches

| Option | Values | Environment | Renderer | Browser viewport |
|---|---|---|---|---|
| Anti-aliasing | `off` (1 sample), `msaa` (4 samples), `taa` (1 sample + TAA), `msaa+taa` | `POCKET_AA` | `set_antialiasing(Antialiasing)` | `set_antialiasing("…")`, `?aa=` |
| GTAO | `off`, `on`/`depth` (normals from the depth), `target` (the normal target) | `POCKET_GTAO` | `set_gtao(Gtao)`, `set_gtao_radius(m)` | `set_gtao("…")`, `?gtao=` |
| Sharpening | an amount, 0 off; applied only with TAA | — | `sharpen` | `set_sharpen(a)`, `?sharpen=` |
| TAA tuning | object motion, skinned motion, disocclusion, speed weight, `alpha`, `gamma`, jitter amplitude; `TaaMode::Jittered` (references); `reset()` | — | `taa_mut()` | — |

`FrameStats` reports `antialiasing` and `gtao`; the viewport's stats JSON carries both.

**Defaults** (`post::defaults_for`, measured: charter 4.4 and docs/bench/taa-gtao.md 5): a discrete
GPU on Vulkan or Direct3D 12 draws with `msaa` and GTAO on; an integrated one with `taa` and GTAO
off; anything else (Metal, the browser, software adapters, and Apple's GPUs and any GPU through
MoltenVK on Vulkan, known by Apple's vendor id or the driver's name) with `msaa` and GTAO off. The
environment variables and the setters override them. Tools that compare images across adapters
pin both (`tools/backend_compare.py`), and checks that compare redraws pixel for pixel pin `msaa`
without GTAO.

**One-shot captures.** With TAA a single frame after a reset is one jittered sample, 4 to 5 dB below
multisampling against a supersampled reference. `Renderer::capture_still` restarts the history and
the jitter sequence and draws `STILL_FRAMES` (16, one jitter cycle) at the same moment before
reading back; without TAA it draws one frame. The capture server behind the MCP and CLI `capture`
(pocket-app `present.rs`) takes it once the scene's assets have arrived, as do `showcase_bench
--capture` and `sky_gi_units`. On the check scene at 256x144 a still capture reaches 40.2 dB against
the mean of 64 jittered frames, against 36.5 for multisampling and 31.3 for TAA's first frame
(RTX 5060, Vulkan); a 640x360 capture of samples/sailing took 88 ms through `pocket call capture`
on the Radeon 780M (provisional). The windowed app's capture reads the running history.

A change of anti-aliasing or GTAO rebuilds what depends on the opaque pass's format: its targets,
the forward pipelines, the sky's, the ocean's, the particles' and the grid's pipelines (each takes a
`SceneFormat`), and, when the sample count changes, the shaders that read the opaque depth. Those
shaders (hiz.wgsl's first level, the splat depth copy, the splat tile raster, gtao.wgsl, taa.wgsl)
declare it `texture_depth_multisampled_2d`; `shaders::depth_module` gives a one-sample variant
(`texture_depth_2d`, whose `textureLoad` takes a mip level where the multisampled one takes a sample
index; hiz.wgsl's sample count becomes 1). TAA's history is dropped.

### 2.2 The opaque pass's targets

| Location | Format | When | Written by | Read by |
|---|---|---|---|---|
| 0 | `Rgba16Float` | always | every pipeline of the pass | GTAO (multiplied), TAA, bloom, display |
| 1 | `Rgba8Unorm`, the indirect share | GTAO on | forward pipelines | GTAO's upsample |
| 2 | `Rg16Float`, object motion | TAA on | forward pipelines | TAA |
| 3 | `Rgb10a2Unorm`, view normals | GTAO `target` | forward pipelines | GTAO's horizon search |

The forward fragment shaders always write all four outputs; a pipeline whose format lacks a target
has no target there, and WebGPU ignores the output. The ocean writes all four too (no share, zero
motion, its own normal): it writes depth over the ground and whatever is sunk in it, which would
otherwise show through in the extra targets, GTAO darkening the water by the occlusion of the
ground under it and TAA moving it with a hidden hull (`tests/taa.rs`). The sky, particles and grid
write the color only: their pipelines have the extra targets with an empty write mask, so they keep
what is under them. The sky is drawn only where nothing was (cleared to zero: no share, no motion);
particles and the grid, blended without writing depth, keep the share and motion of the surface
they are drawn over. Multisampled targets resolve into one-sample
textures of the same format at the end of the pass and their samples are discarded; with one sample
the pass draws into those textures directly. At most 20 bytes per sample (8 + 4 + 4 + 4): within
WebGPU's default 32.

With TAA the opaque color resolves into `raw` and TAA writes its history; without TAA it resolves
into `hdr`.

## 3. Motion

**Jitter.** Frame `k` moves the projection by Halton (2, 3) point `k mod 16 + 1` minus one half, in
pixels (scaled by the tuning's amplitude), as a translation in normalized device coordinates
applied after the projection (`FrameJitter::apply`): correct for perspective and orthographic
cameras alike. `TaaMode::Jittered` cycles 1024 points instead and keeps no history: each frame is
one jittered sample, which evaluation averages into a supersampled reference.

**Camera motion** is reconstructed in the TAA pass from the depth: the 3x3 neighbourhood's nearest
depth, this frame's unjittered inverse view-projection and last frame's unjittered view-projection
give where the surface was. With the infinite reversed-Z projection depth 0 reconstructs to a point
at infinity (w = 0), and the sky reprojects by rotation alone.

**Object motion** is what the camera cannot explain: the forward shader writes, in last frame's
normalized device coordinates, the position of this point under last frame's pose minus its
position under this frame's pose, both through last frame's view-projection. A still object writes
zero, the value the other pipelines leave, so multisample resolve does not mix camera motion into
it. Last frame's pose is the instance's two tick poses (`prev_pos`, `pos`, rotations by nlerp) at
`prev_alpha = alpha - (drawn_t - last drawn_t) / dt`, where `drawn_t` is the simulated time a frame
shows: last frame's own alpha when no tick passed, an extrapolation along the tick's motion when
one did (exact for constant velocity). The vertex shader reads the instance's poses only for slots
the scene moved this tick or that belong to skinned entities (`moving`, a bit per slot the renderer
uploads when it changes; binding 6 of the frame group, vertex stage only): reading them for every
vertex of 1.6M still cubes cost the Radeon 780M 3.3 ms.

**Skinned parts.** Before the skinning pass overwrites a part's range of the vertex pool, the
renderer copies the range (last frame's skinned vertices) into `skinning::History`; the forward
vertex shader reads last frame's local position from it (`skin_prev`, a vertex-only storage buffer
in the frame group: the table's length, then per mesh the part's first vertex and where its copy
starts, then the copies). A part new this frame, or skinned into another range last frame, has no
copy, and its motion reads as rigid for one frame.

**History resets.** The history is dropped when there is none, at a resize, a change of the
options, a camera that moved more than 25 m or turned more than 0.6 rad in a frame or changed its
lens or projection, a drawn time that went back or jumped more than a second, or `Taa::reset`.

## 4. The TAA resolve

A compute pass, 8x8 invocations a group. Each group first loads its 10x10 tile of the current image,
tone-mapped (`c / (1 + luma)`) and converted to YCoCg, and its depth into workgroup memory; a
fragment version doing nine loads and conversions per pixel cost 1.5 times as much on the RTX 5060
(0.60 against 0.40 ms at 2560x1440). Per pixel:

1. the 3x3 neighbourhood's mean, standard deviation, minimum and maximum, nearest and farthest
   depth;
2. reprojection (3) at the nearest depth, plus the object motion read where that depth is;
3. off screen or behind last frame's camera: this frame alone;
4. disocclusion (optional, on): the history keeps each pixel's view depth in alpha; when none of the
   four history texels around the reprojected position (one `textureGather`) is within 5% of the
   depth this surface had last frame, plus the neighbourhood's own depth spread, the history
   belongs to something else and this frame is taken alone. The spread term keeps slopes and edges
   from counting: without it, grazing floors were rejected, which lowered PSNR under a panning
   camera below no test at all;
5. the history through a 5-tap Catmull-Rom filter;
6. variance clipping toward the box mean ± `gamma` (1.25) standard deviations, intersected with the
   minimum and maximum, in tone-mapped YCoCg;
7. blend in tone-mapped space with the current frame's weight `max(1 / (n + 1), alpha)` (n frames
   since the reset, `alpha` 0.1), raised toward 0.25 as the reprojection moves up to 8 pixels.

The ocean writes zero object motion and reprojects as a still surface under the camera (its waves
are not followed), as the sky does; particles and the grid move with the surface under them, and a
moving particle relies on the clip. The output alpha is the view depth, which
bloom and the display transform ignore.

## 5. GTAO

Four fragment passes (WebGPU core: render attachments and `textureLoad`):

1. **Depth**: half resolution (rounded up), the view distance of full-resolution pixel `2p` (sample
   0), `r32float`; the sky is a far value.
2. **Horizon search** (`r8unorm`): Jimenez et al. 2016 with XeGTAO's integration: 2 slices of 4
   steps a side over a 0.6 m radius (falloff over its last 60%), the visibility raised to 1.6. The
   view-space normal comes from the depth (on each axis the neighbour nearer in depth, so edges do
   not bend it) or from the normal target. Noise: a 4x4 pattern, or with TAA one that also changes
   every frame so TAA integrates it.
3. **Blur**: 4x4 at half resolution, weighted by depth agreement (removes the 4x4 pattern).
4. **Apply**: each full-resolution pixel takes the four nearest half-resolution texels, weighted
   bilinearly and by depth agreement (the nearest in depth when none agrees), and multiplies its
   color by `1 - share * (1 - visibility)` through a multiplicative blend.

**What is indirect.** The forward shader writes, per channel, `share = indirect * (1 - fog) /
color`, where `indirect` is the image-based diffuse light (spherical harmonics, or the baked or
neural GI field) and the prefiltered specular reflection, and `color` the final color after fog.
Direct sunlight, punctual lights and emission are never darkened; unlit materials write no share.
Multisample resolve averages shares at edges, which is close to, not equal to, the resolved
indirect over the resolved color. Particles blended over a surface keep the surface's share, so in
an occluded crease they are darkened a little too. Specular and diffuse indirect light are darkened
by the same visibility (no separate specular occlusion).

**Normals.** From the depth by default: the normal target adds a fourth target to the opaque pass
and costs more there than reconstruction costs in the horizon search (docs/bench/taa-gtao.md).

## 6. Sharpening

With TAA, the display transform can push each pixel away from the mean of its four neighbours by an
amount, in tone-mapped space (`c / (1 + luma)`, so highlights do not ring) and clamped to the five
values' range. It is applied before exposure and AgX, after bloom's input was taken.

## 7. The browser

Everything runs on WebGPU's core features: `rgba16float` write-only storage textures (the TAA
output), `textureGather` and `textureSampleLevel` in a compute shader, `r32float` and `r8unorm` and
`rgb10a2unorm` render targets, four color attachments of 20 bytes per sample. The viewport takes
the options from its URL (`?aa=`, `?gtao=`, `?sharpen=`) and draws the check scene with
`?demo=aa`.

## 8. Checks

`crates/pocket-render/tests/taa.rs`, at 256x144. Each check failed with its defect put back (in
brackets); all pass on the RTX 5060 and the Radeon 780M with Vulkan and Direct3D 12, and with
WebGPU's default limits emulated.

- An orthographic camera panning whole pixels a frame reprojects exactly: TAA's image matches a
  still camera's at the pan's end, with one sample and with four (a one-pixel reprojection offset,
  camera motion ignored). The still camera's history stays within 39 dB of the mean of 64 jittered
  one-sample frames (42.6 dB with one sample, 45.2 with four).
- A board of cubes sliding whole pixels a frame reprojects through the object motion, with one
  sample and with four (object motion reversed; with four samples the motion target left
  unresolved: 21.0 dB against 53.4). A board slid by its bone, its entity still, reprojects through
  last frame's skinned vertices: 45 to 47 dB, 23 to 24 without (the skinned history ignored).
- Multisampling with TAA ends closer to that reference than multisampling alone on the check
  scene's still view (no jitter with four samples: 38.4 dB against 40.8, multisampling 36.6).
- The id pass's coverage is the same with TAA (the jittered view in the id pass).
- GTAO leaves a scene lit only directly untouched and never brightens a pixel (GTAO on all light);
  its normal target darkens where and as much as normals from the depth do, seen from the front and
  from the side (the target in world space: 312% apart from the side; not written: 696%).
- A sea over the ground, the walled corner and a sunk cube sliding under it is left untouched by
  GTAO and by the cube's object motion (the sea writing the color only: GTAO changed 15,935 of its
  16,330 inside pixels, the hidden cube's motion 457).
- Splats behind a cube, the chain link and a sea do not flicker on a still frame under TAA or
  MSAA+TAA, with quads and with tiles, and show as much as with MSAA; away from them TAA's image
  with splats is its image without, the copy they draw into being the history (the jittered depth:
  214 flickering pixels; the alpha test without its discard: 1,291 splat pixels against 2,012; at
  mip 0: 1,767; the sea left out: 2,538; the copy 1% brighter: 34,239 pixels changed).
- A still capture under TAA lands above multisampling where TAA's first frame lands below it, and
  repeats exactly (one frame: 31.3 dB against MSAA's 36.5).
- Cuts, resizes and option changes drop the history (no cut detection).

`post::tests::defaults_by_adapter` tables the defaults (Apple's GPUs and MoltenVK given the kind
rule's TAA or GTAO).

Elsewhere:

- `examples/aa_eval.rs`: PSNR against supersampled references while converging, under motion, with
  skinning, with ablations; GTAO images; per-option costs. `tools/aa_bench.py` and
  `tools/aa_web_bench.py` run the cost matrix natively and in Chrome.
- With the defaults unchanged, captures of samples/gi-room and samples/pt-lab match a master build
  pixel for pixel.

## 9. Open

- TAA at one sample loses geometry thinner than a pixel that only some jitter positions see (the
  clip removes it); a wider box where nothing moved keeps it but delays moving shadows on still
  surfaces (docs/bench/taa-gtao.md). Thin-feature locks (as FSR 2 does) are not implemented.
- No reactive mask for particles or other surfaces without motion vectors.
- GTAO has no measured ground truth here (a ray-traced AO reference on the ray-query backends would
  give one); its radius and power follow XeGTAO's defaults loosely and were judged by eye. Flat
  walls at grazing angles lose about 2%; under TAA the image comes out about 0.5% darker than GTAO
  alone makes it (averaging noisy frames in tone-mapped space biases toward dark).
- The defaults for Apple GPUs and for integrated GPUs in the browser are unmeasured (the Radeon 780M
  gives Chrome no usable timestamps).
- Upscaling (rendering below the output resolution and reconstructing with the TAA history) is not
  done.
- Splats are not anti-aliased against the meshes in front of them: one depth test per pixel, as with
  multisampling's first sample (1.1). Drawing them jittered before TAA would let it smooth those
  edges, but TAA would reproject them by the depth behind them (they write none), which for a scene
  made of splats alone is the sky's.
