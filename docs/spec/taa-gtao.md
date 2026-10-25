# TAA and GTAO

Status: Draft, implemented on branch `explore/aa` (2026-10-09); upscaling (4.1) on branch
`explore/taau` (2026-10-15)

Charter: 4.4 ("HDR, bloom, TAA, AgX"; the Pioneer note of 2026-10-09 fixes where TAA and GTAO sit
in the frame and that the defaults are measured; that of 2026-10-15 the render scale TAA upscales
from), 4.1 (one renderer for Metal, Vulkan, Direct3D 12 and WebGPU).

This specification says how the renderer anti-aliases over time (TAA) and how it darkens indirect
light with ground-truth ambient occlusion (GTAO), where both sit in the frame, how they interact
with multisampling, splats, particles, bloom, the entity-id pass and the editor's overlays, how they
are switched, and how they are checked. The code is `crates/amoris-render/src/post.rs` (the options,
the opaque pass's format and targets), `taa.rs` and `shaders/taa.wgsl`, `gtao.rs` and
`shaders/gtao.wgsl`, the forward shader's extra outputs (`shaders/forward.wgsl`, `SceneOut`),
`skinning.rs` (last frame's skinned vertices) and `renderer.rs` (`render`). Measurements and the
defaults: [docs/bench/taa-gtao.md](../bench/taa-gtao.md), upscaling's
[docs/bench/taau.md](../bench/taau.md); evidence: `out/bench-runs/aa/`, `out/taau/`.

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
   reprojected history into the new history, which is the image the rest of the frame reads. At a
   render scale below 1 steps 1 to 3 draw below the output's size and TAA upscales (4.1); from
   here on everything is the output's size.
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
| Anti-aliasing | `off` (1 sample), `msaa` (4 samples), `taa` (1 sample + TAA), `msaa+taa` | `AMORIS_AA` | `set_antialiasing(Antialiasing)` | `set_antialiasing("…")`, `?aa=` |
| GTAO | `off`, `on`/`depth` (normals from the depth), `target` (the normal target) | `AMORIS_GTAO` | `set_gtao(Gtao)`, `set_gtao_radius(m)` | `set_gtao("…")`, `?gtao=` |
| Sharpening | an amount, 0 off; applied only with TAA | — | `sharpen` | `set_sharpen(a)`, `?sharpen=` |
| Render scale | 0.25 to 1 (1: off); below 1 the anti-aliasing mode draws with TAA (4.1) | `AMORIS_RENDER_SCALE` | `set_render_scale(s)`, `render_scale()`, `scene_size()` | `set_render_scale(s)`, `?scale=` |
| TAA tuning | object motion, skinned motion, disocclusion, speed weight, `alpha`, `gamma`, jitter amplitude; upscaling's kernel sharpness, share weighting and texture mip bias; `TaaMode::Jittered` (references); `reset()` | — | `taa_mut()` | — |

`FrameStats` reports `antialiasing`, `gtao` and `render_scale`; the viewport's stats JSON carries
them.

**Defaults** (`post::defaults_for`, measured: charter 4.4 and docs/bench/taa-gtao.md 5): a discrete
GPU on Vulkan or Direct3D 12 draws with `msaa` and GTAO on; an integrated one with `taa` and GTAO
off; anything else (Metal, the browser, software adapters, and Apple's GPUs and any GPU through
MoltenVK on Vulkan, known by Apple's vendor id or the driver's name) with `msaa` and GTAO off. The
environment variables and the setters override them. Tools that compare images across adapters pin
both (`tools/render/backend_compare.py`), and checks that compare redraws pixel for pixel pin `msaa`
without GTAO.

**One-shot captures.** With TAA a single frame after a reset is one jittered sample, 4 to 5 dB below
multisampling against a supersampled reference. `Renderer::capture_still` restarts the history and
the jitter sequence and draws one jitter cycle (`STILL_FRAMES`, 16; below a render scale of 1 the
longer cycle of 4.1) at the same moment before reading back; without TAA it draws one frame. The
capture server behind the MCP and CLI `capture` (amoris-app `present.rs`) takes it once the scene's
assets have arrived, as do `showcase_bench --capture` and `sky_gi_units`. On the check scene at
256x144 a still capture reaches 40.2 dB against the mean of 64 jittered frames, against 36.5 for
multisampling and 31.3 for TAA's first frame (RTX 5060, Vulkan); a 640x360 capture of
samples/sailing took 88 ms through `amoris call capture` on the Radeon 780M (provisional). The
windowed app's capture reads the running history.

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
options or the render scale (or of the opaque pass's size alone), a camera that moved more than
25 m or turned more than 0.6 rad in a frame or changed its lens or projection, a drawn time that
went back or jumped more than a second, or `Taa::reset`.

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

### 4.1 Upscaling (a render scale below 1)

Charter 4.4, Pioneer 2026-10-15. The render scale `s` (`AMORIS_RENDER_SCALE`,
`Renderer::set_render_scale`, the viewport's `?scale=`; 0.25 to 1, default 1; every way in clamps a
value beyond that range and takes one that is not a finite positive number as 1,
`post::clamp_render_scale`) draws the opaque pass at `round(s * width) x round(s * height)` and
reconstructs the output from it with TAA, as UE's TAAU and TSR do from a screen percentage. At 1
nothing changes: the targets are the output's size and TAA runs `resolve_history`/`resolve_copy` as
above, so frames are the same pixel for pixel (checked against master, 8). Below 1:

- **What draws small**: the opaque pass and its targets (color, depth, indirect share, motion,
  normals), the depth prepass, the depth pyramid and the late occlusion test (the culling uniform's
  viewport and pyramid levels are the opaque pass's), GTAO (its half resolution is the opaque
  pass's half), particles, the sky, the sea and the editor's grid inside the opaque pass. The view
  uniform's `viewport` is the opaque pass's (the forward shader's light clusters); the unjittered
  copy keeps the output's. The depth prepass draws with the opaque pass's jittered frame group as at
  1, so the prepass's rule holds below 1 too, its ties included (docs/spec/prepass.md 4): surfaces
  that touch tie in depth along the contact line, and the longer jitter cycle puts samples there
  where scale 1 does not (8).
- **What stays at the output's size**: TAA's history and output (`Targets::hdr`), splats and the
  unjittered depth they test against (1.1), bloom, the display transform and sharpening, the
  overlays, the gizmo, the game UI and the entity-id pass (its own target and depth, the unjittered
  view: picks and coverage are the scale's own, 8). Levels of detail are picked per output pixel
  (lod.rs takes the output's height): the geometry the output shows does not depend on the scale.
- **Filtering for the output's pixel** (charter 4.4, the note after review): the forward shader
  multiplies material textures' uv gradients by the scale per axis (common.wgsl `texture_lod`; the
  alpha test of the depth-only pipelines too, and the neural textures' level), a mip bias of log2 s:
  the opaque pass samples the level the output's size would. The other filters that read screen
  derivatives scale them the same way, by the larger axis: the specular anti-aliasing's normal
  variance (`antialiased_roughness`) and the sea's footprint (its wavelets' fade and its
  roughness). Without all of it the converged image loses 4.6 to 6.1 dB; the specular term alone
  is worth 4.4 to 8.4 dB on the chrome spheres (docs/bench/taau.md 2). The editor's grid is drawn
  without the jitter (the unjittered uniform, whose factors are 1), so TAA cannot reconstruct finer
  lines than the opaque pass's pixel and they stay filtered for it.
- **Mode**: TAA must run. The anti-aliasing mode asked for is promoted to its TAA counterpart at the
  same samples (`Antialiasing::with_taa`: off and taa become taa, msaa and msaa+taa become
  msaa+taa); `Renderer::antialiasing` and `FrameStats::antialiasing` report the mode that draws,
  `FrameStats::render_scale` the scale. Back at 1 the mode asked for returns.
- **Jitter**: in the opaque pass's pixels (`FrameJitter::pixels`, the NDC offset divided by its
  size), cycling `max(16, ceil(8 / s^2))` Halton points (`taa::jitter_phases`; s^2 the ratio of the
  two pixel counts): 16 at 0.77, 18 at 0.67, 32 at 0.5. A frame puts a sample within an output
  pixel `s^2` times as often as at scale 1, so a cycle gives each output pixel about 8 samples of
  its own (FSR 2's rule: 8 at 0.5, 8.1 at 0.67, 9.5 at 0.77), where TAA at the output's size gets
  16. A still capture draws one such cycle.
- **History**: dropped when the scale changes (new targets) and when the opaque pass's size changes
  without the output's (3).

The resolve (`upscale_history`, `upscale_copy`) runs per output pixel, 8x8 a group; each group loads
the opaque pass's 10x10 pixels around the ones nearest its output pixels (tone-mapped YCoCg and
depth, as in 4). Output pixel `p`'s centre lies at `u = (p + 0.5) * scene / output` in the opaque
pass's pixels, and its pixel `q` shows the point at `q + 0.5 - jitter`. Per output pixel:

1. the 3x3 neighbourhood around the opaque pass's pixel whose sample lies nearest `u` gives the
   clip box (mean, deviation, extremes), the nearest and farthest depth and where the object motion
   is read;
2. this frame at `u`: the nine samples' mean weighted by `exp(-k d^2)`, `d` their distance from `u`
   in output pixels, `k` 4 (a standard deviation of 0.35 output pixels; 2.29, a Gaussian close to a
   Blackman-Harris window, was blurrier, docs/bench/taau.md 2), in tone-mapped YCoCg;
3. reprojection, disocclusion, the Catmull-Rom history read and the clip as in 4, at the output's
   size, with a box of 1.5 standard deviations (`TaaTuning::upscale_gamma`; 1.25 at the output's
   size): the coarser neighbourhood's box is tighter around the output's detail, and the wider one
   gains 0.9 to 1.2 dB on the still check scene without losing under motion (2 sigmas would gain
   another 0.3 to 0.6 dB and lose some still-camera stability, docs/bench/taau.md 2; measured before
   the wider box that follows). Where nothing moved (the reprojection moved under 0.01 output
   pixels) the box is 3 standard deviations and no longer intersected with the neighbourhood's
   minimum and maximum (`TaaTuning::upscale_still_gamma`, `taa::UPSCALE_STILL_GAMMA`; the owner's
   decision of 2026-10-16, charter 4.4): the coarser neighbourhood clips away detail finer than its
   pixels whenever this frame's samples miss it, and the wider box keeps what earlier frames'
   samples saw. At the output's size the same widening (`still_gamma`) stays off
   (`TaaTuning::clip_gammas` picks the pair by whether the pass upscales, and
   `taa::tests::the_still_box_widens_only_when_upscaling` pins the defaults); the browser's viewport
   takes the same default (web/viewport/pkg rebuilt with it);
4. the blend: with `alpha` the weight 4 would use, the current frame's share is
   `W / (E * (1 / alpha - 1) + W)`, `W` the sum of the nine weights and `E` its expectation over the
   jitter (`s^2 * pi / k`). It is `alpha` where the samples' weight is the expected one, 1 on the
   first frame after a reset, and small where no sample of this frame fell near the pixel: each
   output pixel accumulates the samples that landed near it, the history standing in elsewhere.
   Without this weighting the converged still image is 1.1 to 2.4 dB lower (docs/bench/taau.md 2).
   The speed term of 4 applies after it.

A disoccluded or off-screen pixel takes this frame's reconstruction alone. The pass runs as many
groups as TAA at the output's size and each loads as many pixels (10x10 of the opaque pass for 8x8
output pixels), adding the nine samples' weights. It costs the same as TAA at the output's size
with Direct3D 12 on the RTX 5060 and more elsewhere: 0.05 to 0.14 ms (34 to 38%) with Vulkan on
the RTX 5060, 0.07 to 0.52 ms (12 to 33%) on the Radeon 780M (quiet re-measurement of 2026-10-15,
docs/bench/taau.md 3); why is open (9).

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

`crates/amoris-render/tests/taa.rs`, at 256x144. Each check failed with its defect put back (in
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
- Upscaling (4.1): at 0.5 and 0.67 a still view gains 9.6 to 10.0 and 10.4 to 10.7 dB on its first
  upscaled frame by frame 64 and ends 3.4 to 3.7 and 1.2 to 2.0 dB below TAA at the output's size,
  against the mean of 64 jittered output-size frames (with the wider box where nothing moved,
  2026-10-16; before it 6.3 to 7.3 dB gained and 5.1 to 7.0 lost). The test asks for 7.5 dB gained
  and at most 6.0 (0.5) and 4.0 dB (0.67) lost, which catch these defects (the reconstruction
  reading the jitter with the wrong sign: 0.7 to 2.4 dB gained, 13 to 15 lost; the share not
  weighted by the samples: 5.0 to 5.2 gained and 8.1 to 8.4 lost at 0.5). The earlier bounds, 4.5 dB
  gained and 9.5 and 7.5 lost, let the unweighted share pass once the box widened (on all four GPU
  and API pairs; `out/decisions/mutants/`, `mutants2/`). A scale change and a resize drop the
  history; a return to 1 draws what was drawn before the scale; the id pass's coverage and picks are
  those of scale 1; a still capture settles past the first frame (from 25.9 to 27.6 dB to 35.1 to
  36.6 at 0.5; 31.9 to 33.3 before the wider box) and repeats; splats over an upscaled history do
  not flicker. At scale 1 a flicker is any change of the splats' contribution, as before; upscaled
  it must change the shown value too, since under splats that hide it the upscaled history moves a
  little (3 pixels at 0.5), which changes the contribution, not the image (splats tested against a
  depth drawn with the jittered frame group: 572 flickering pixels at 1, 787 at 0.5). The defects
  were put back on the RTX 5060 with Vulkan and the 780M with Direct3D 12; the logs stay in
  `out/taau/mutations/`.
- Below a render scale of 1 `tools/render/prepass_compare.py` (`AMORIS_AA=msaa+taa` and `taa`,
  `AMORIS_RENDER_SCALE=0.5` and `0.67`; the many_cubes, splat and static showcase cases, both GPUs
  and APIs) finds the prepass's documented ties only: many_cubes' sphere, whose cubes touch and
  which differs at scale 1 too (up to 22 levels with TAA there), and at 0.5 with MSAA and TAA 2 or 3
  pixels where a wall meets the floor (pt-lab, RTX 5060) or the ceiling (gi-room, 780M), which agree
  once the touching box is lifted by 1 cm. Measured again with the wider box where nothing moved
  (2026-10-16, the same 16 runs on both GPUs and APIs): the same 2 pixels of pt-lab by 4 levels, 1
  of gi-room's 3 by 1 level, and many_cubes' sphere (1 to 37 pixels by up to 17 levels; 1 to 52 by
  up to 15 before), nothing else.

`post::tests::defaults_by_adapter` tables the defaults (Apple's GPUs and MoltenVK given the kind
rule's TAA or GTAO).

Elsewhere:

- `examples/aa_eval.rs`: PSNR against supersampled references while converging, under motion, with
  skinning, with ablations; GTAO images; per-option costs. `tools/render/aa_bench.py` and
  `tools/render/aa_web_bench.py` run the cost matrix natively and in Chrome.
- With the defaults unchanged, captures of tests/fixtures/scenes/gi-room and
  tests/fixtures/scenes/pt-lab match a master build pixel for pixel. At a render scale of 1 every
  case of `tools/render/build_compare.py` (MSAA, MSAA with GTAO, TAA, TAA with GTAO's normal target)
  matches master `b7be59a2` pixel for pixel on both GPUs with Vulkan and Direct3D 12 (120 pairs,
  docs/bench/taau.md 1).

## 9. Open

- TAA at one sample loses geometry thinner than a pixel that only some jitter positions see (the
  clip removes it); a wider box where nothing moved keeps it but delays moving shadows on still
  surfaces (docs/bench/taa-gtao.md). Thin-feature locks (as FSR 2 does) are not implemented.
- No reactive mask for particles or other surfaces without motion vectors.
- The far edge of a ground plane against the sky does not settle under TAA at one sample (Pioneer,
  2026-10-15, [gpu-matrix.md](../bench/gpu-matrix.md) 3 c). In neural_material's walker view
  (320×180, `demo::neural_scene`, a fixed time), raising the ground's color by 0.1% moved edge
  pixels by up to 29/255 on the Radeon 780M and 10 on the RTX 5060 after three frames; changes of
  0.1% to 1% moved them by up to 39, over up to 175 pixels, after twenty. Multisampling moved none
  by more than 1. Depth and motion do not depend on the color, so some step of the resolve answers
  it discontinuously (the neighborhood clip is the first suspect). No test catches it: the neural
  material comparisons run under multisampling since that day. Reproduce by rendering that scene
  with `Antialiasing::Taa` for 3 and 20 frames at colors 1 and 1.001 times the base and comparing
  the frames.
- GTAO has no measured ground truth here (a ray-traced AO reference on the ray-query backends would
  give one); its radius and power follow XeGTAO's defaults loosely and were judged by eye. Flat
  walls at grazing angles lose about 2%; under TAA the image comes out about 0.5% darker than GTAO
  alone makes it (averaging noisy frames in tone-mapped space biases toward dark).
- The defaults for Apple GPUs and for integrated GPUs in the browser are unmeasured (the Radeon 780M
  gives Chrome no usable timestamps).
- Upscaling (4.1) loses to MSAA under a panning camera by 2.3 to 6.1 dB on the check scene, and to
  TAA at the output's size by 1.6 to 5.0 dB: the clip box comes from the opaque pass's coarser
  neighbourhood, so detail finer than its pixels is clipped away whenever this frame's samples miss
  it. Where nothing moved the wider box (on since 2026-10-16) keeps it: the still view gains 3.4 to
  4.4 dB and ends 1.2 dB above (0.77) to 2.5 dB below (0.5) TAA at the output's size, which keeps
  the tighter box there, and the region around a moving cube and its shadow loses up to 1.5 dB under
  a still camera (docs/bench/taau.md 2). No locks for thin features, no reactive mask and no
  separate history confidence (TSR's and FSR 2's tools for this) are implemented. The editor's grid,
  drawn inside the opaque pass from unjittered positions, keeps the opaque pass's pixel and is
  upsampled with the scene; particles and the sea are drawn small like the opaque geometry.
- The upscaling resolve costs more than TAA at the output's size with Vulkan and on the Radeon 780M
  for the same loads (4.1); the cause is not known (no shader profiler for the 780M here).
- Splats are not anti-aliased against the meshes in front of them: one depth test per pixel, as with
  multisampling's first sample (1.1). Drawing them jittered before TAA would let it smooth those
  edges, but TAA would reproject them by the depth behind them (they write none), which for a scene
  made of splats alone is the sky's.
