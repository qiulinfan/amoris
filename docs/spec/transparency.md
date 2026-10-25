# Transparency

Status: Draft. Implemented (Pioneer `explore/transparency`; the charter note is dated 2026-10-16,
built and measured 2026-10-15 and, after review, 2026-10-16, machine dates); on Amoris main since
2026-10-16 (docs/imports/pioneer-2026-10-16.md).

Charter: 4.4 (the Pioneer note of 2026-10-16 on the transparent stage; the opaque pass, two-phase
Hi-Z, the depth prepass and TAA notes before it), 4.1 (one renderer for Metal, Vulkan, Direct3D 12
and WebGPU).

This specification says which instances the forward renderer draws as transparent, how they leave
the GPU-driven opaque passes, how the transparent stage sorts and draws them, how they are shaded,
what they cast into the shadow cascades, and how they meet TAA, the render scale, MSAA, the
entity-id pass, splats and particles. The code is `crates/amoris-render/src/transparent.rs` (the
stage), `materials.rs` (which rows are transparent), `scene.rs` (`FLAG_TRANSPARENT`, the slots),
`renderer.rs` (`render`, `draw_after_opaque`, `draw_transparent_layer`, `IdPass`), `post.rs` (the
passes' attachments), `gtao.rs` (GTAO into the samples), `shaders/cull.wgsl` (the camera's list) and
`shaders/forward.wgsl` (`fs_transparent`, `fs_shadow_masked`). Measurements:
[bench/transparency.md](../bench/transparency.md).

Why: glTF BLEND materials and KHR_materials_transmission glass were imported (amoris-assets
`MaterialData::alpha_mode`, `transmission`, `ior`) and the path tracer renders them
([path-tracing-nrc.md](path-tracing-nrc.md)), but the forward renderer drew them opaque: pt-lab's
glass sphere was a white ball, the Bistro's windows hid the rooms behind them
([bench/town-walk.md](../bench/town-walk.md); the Bistro is not on the machine this was built on and
has not been drawn with the stage, section 8).

## 1. Which instances are transparent

A material row (`MaterialGpu`, one per distinct look) is transparent when it is not a neural
texture's and

- its material is glTF BLEND (`FLAG_BLEND`), or
- the look's alpha (`Model.color[3]`) is below 1 and the material is not alpha-masked (MASK keeps
  testing the scaled alpha against its cutoff), or
- its transmission is above 0: KHR_materials_transmission's factor, or `Model.transmission`, which
  overrides it as `Model.ior` overrides KHR_materials_ior.

glTF says an OPAQUE material's alpha is ignored: such a row's alpha factor is 1, so a look's alpha
alone decides its coverage. Neural texture materials stay opaque whatever the look says (no
transparent pipeline decodes them). The row carries `transmission`, `ior` and the transmission
texture's layer (red channel), 80 bytes a row (64 before).

An instance slot whose material row is transparent gets `FLAG_TRANSPARENT` (bit 6) and joins
`Scene::transparent_slots`; its variant bits (3-5) are the alpha-masked variant plus the material's
double-sided bit, and it loses `FLAG_SHADOW` when its material can never cast (section 5). Scenes
without such a material are drawn exactly as before: no slot has the flag and the stage draws
nothing (its pipeline is built at start-up with the others, section 3).

## 2. Out of the opaque passes

The culling pass (`cull.wgsl` `main`) never writes a slot with `FLAG_TRANSPARENT` into the
camera's list (view 0). So a transparent instance is in no camera batch: not drawn by the opaque
pass, not in the depth prepass ([prepass.md](prepass.md)), not in the depth the Hi-Z pyramid is
built from, never `VIS_FRUSTUM` for the late culling pass, never in occlusion culling's visible
set ([occlusion.md](occlusion.md)), not in the unjittered depth splats test against. It never
occludes anything. The batch regions still count it (the cascades' lists use them).

## 3. The stage

Every frame, after the uniforms are written and before encoding (`Transparent::prepare`, trace
span `transparent sort`, `FrameStats::transparent_ms`):

1. **Cull.** For each live, visible transparent slot: the pose interpolated as the culling pass
   interpolates it (`mix` of the positions, common.wgsl's `nlerp` of the rotations at the frame's
   alpha), the mesh's bounding sphere moved and scaled with it, tested against the jittered
   camera's six frustum planes.
2. **Level of detail.** With levels on ([lod.md](lod.md)) the coarsest level whose projected error
   stays within the camera's bound, by cull.wgsl's camera rule without its hysteresis; else the
   full mesh.
3. **Sort.** By the view depth of the sphere's centre along the camera's forward axis, farthest
   first; equal depths in slot order, so a frame draws the same way every time.
4. **Lay out.** The sorted instances' `Drawn` records (48 bytes, forward.wgsl's) go in order into
   the stage's own list, written only when it changed (a still frame writes nothing). Neighbours
   with the same mesh row and the same sidedness become one instanced draw.

The stage has two frame groups, `frame` and `frame_stable` with the stage's list bound as `drawn`.
A draw is a direct `draw_indexed` of the mesh row's indices whose instances are the records'
positions in the list: `first_instance` is the base, and group 3's batch base is entry 0, which is
0 on every draw path. WebGPU's core allows a direct draw's `first_instance` (only an indirect
draw's needs `indirect-first-instance`), so the three draw paths of batches.rs draw the stage the
same way ([webgpu-baseline.md](webgpu-baseline.md)); tests/draw_paths.rs includes transparent
instances.

After the ocean and the sky and before the particles and the editor's grid
(`draw_transparent_layer`), the stage draws its draws in order with one pipeline: forward.wgsl's
`vs` (`SCENE_MOTION` off) and `fs_transparent`, back faces culled, depth test `Greater`
(reversed-Z) without depth writes, the opaque pass's samples, and premultiplied blending (`One`,
`OneMinusSrcAlpha`) on the HDR color; the extra targets (indirect share, motion, normals) are left
as they are. The pipeline is built with the opaque pass's other pipelines, concurrently (par.rs),
at start-up and when the opaque pass's format changes (anti-aliasing, GTAO): built on first use, it
stalled the first frame with transparent instances by about 100 ms on Direct3D 12
([bench/transparency.md](../bench/transparency.md) 2.2).

Where they draw depends on GTAO (section 6):

- **GTAO off** (or no transparent instance this frame): in the opaque pass (the late one with
  occlusion culling), after the sky (`draw_after_opaque`), as before the stage existed.
- **GTAO on and transparent instances drawn**: the opaque pass ends after the sky
  (`Targets::attachments_before_transparent`): the extra targets resolve and drop their samples as
  usual, the color keeps its samples, unresolved. GTAO then darkens the opaque image, into those
  samples when multisampled (gtao.rs `apply_samples`: `apply`'s fragment shader with the pass's
  samples, one factor per pixel multiplied into each sample, so resolving gives the factor times the
  resolved color, as `apply` after a resolve does, to rounding). Then a pass of its own
  (`transparent`, `Targets::transparent_attachments`) loads the color and the depth, draws the
  stage, the particles and the grid, and resolves the color; its multisampled extra targets are
  cleared and dropped unread (their resolved views keep what the opaque pass resolved), its depth is
  kept when TAA or the splats read it. The same pipelines serve both cases.

A double-sided instance must draw its back faces, then its front faces, before the next instance.
Its draw uses the mesh row's **two-sided index list**: the row's indices twice, the first copy with
every triangle's winding reversed. Through the pipeline that culls back faces, the reversed copy
draws the mesh's back faces (they face the camera reversed; `surface` turns their normal toward the
viewer, as it does for any face) and the second copy its front faces, and the primitives of an
instanced draw rasterize and blend in order, instance by instance: a run of double-sided instances
is one draw. The lists live in an index buffer of the stage's own, made on the GPU the first time a
row is drawn double-sided (two copies from the mesh pool's index buffer and
`transparent_index.wgsl`, which swaps two indices of each triangle of the first copy), and kept for
the rows drawn since. A row the mesh pool replaces (a block volume's chunk mesh, voxels.md 7.2) has
its list made again when next drawn (amended 2026-10-18: a double-sided water chunk that grew drew
its old list, and parts of lakes vanished). The buffer is bound whole as storage by the
reversal, so it is never larger than the device's storage binding limit (`Placement::reserve`):
when the frame's lists do not fit, a larger buffer is made with the frame's lists only while one may
be larger; at the limit the buffer is refilled with the frame's lists if they fit together, and
otherwise the lists that do not fit are not made and their draws draw front faces only, the same
every frame until the rows drawn change (nothing is made again each frame). A row whose list alone
passes the limit is never made. The entity-id pass draws front faces from the mesh pool, as for an
opaque double-sided instance.

Two earlier forms were measured ([bench/transparency.md](../bench/transparency.md) 2.1): two draws
per instance (back faces culled, then front faces culled, a pipeline switch each), which cost 2,000
draws for 1,000 instances and about 1 ms of CPU; and one draw with two instances per record, nothing
culled, each discarding the other side, which saved that CPU time but rasterized and shaded both
sides twice (0.3 to 1.9 ms more GPU time than the lists, 1.9 ms on the Radeon 780M's Direct3D 12).

## 4. Shading

`fs_transparent` computes the surface as the opaque pass does (`surface`: textures, normal map,
specular anti-aliasing), then:

- **Coverage** `a`: the base color's alpha (factor times texture). An alpha-masked material that
  is transparent through transmission discards below its cutoff and has coverage 1.
- **Light the surface adds** `C`: as `shade` lights an opaque surface (the sun with its cascades or
  ray-traced shadow, the cluster's punctual lights, the sky's image-based light, baked GI, the
  emission), with F0 from the IOR (`((ior - 1) / (ior + 1))²`, 0.04 at 1.5) and the diffuse color
  multiplied by `1 - t`: KHR_materials_transmission's transmission `t` (factor times texture) is
  the share of the dielectric's light that passes through instead of being diffused. Specular
  reflection and emission are unchanged by `t`. Unlit materials add their albedo and emission.
- **Light behind that passes** `T = (1 - a) + a (1 - m) t (1 - E) L`, with `m` the metalness, `E`
  the specular lobe's directional albedo for the dielectric F0 (`env_brdf`, the share the surface
  reflects rather than lets through, larger at grazing angles) and `L` the luminance of the base
  color. No refraction: what is behind is seen undisplaced.
- **Fog**: the surface's own light is fogged as an opaque surface's where it hides what is behind:
  `mix(a C, fog (1 - T), f)`, `f` the fog amount at the surface; what shows through carries the fog
  of its own depth.

The output is `(mix(a C, fog (1 - T), f), 1 - T)`: the blend leaves `out + T * behind`. With one
blend factor for every channel (dual-source blending is an optional WebGPU feature, so the baseline
cannot use it) the base color tints what shows through by its luminance only; its hue shows in the
light the surface reflects and emits. Glass that is tinted by hue would need dual-source blending
or a copy of the opaque image (section 8).

## 5. Shadows

A transparent instance casts into the cascades as an alpha-masked caster: its variant has the
masked bit (`MaterialPool::variant`), and `fs_shadow_masked` tests `alpha * (1 - transmission)`
(transmission factor only) against the material's cutoff (0.5 unless a MASK material sets it): a
BLEND material casts where it is at least half opaque, so foliage that an exporter marked BLEND
still casts its cut-out shadow. That opacity is at most the alpha factor (material times look)
times `1 - transmission`, a texture's alpha being at most 1; a transparent row whose bound is below
its cutoff can never cast (`MaterialPool::casts`: clear glass, a look's alpha of 0.35), and its
slots lose `FLAG_SHADOW`, so no cascade draws them only to discard every fragment (a quarter of
1,000 such spheres' GPU cost, [bench/transparency.md](../bench/transparency.md) 2.3) and the
ray-traced top level holds none. Ray-traced shadows ([rt-shadows.md](rt-shadows.md)) make the same
test in `rt_blocks` (the masked variant gives the caster non-opaque geometry), except for skinned
casters: rt_shadows.rs gives every skinned caster its opaque bottom level (a masked copy would be
rebuilt every frame too), so a skinned BLEND material that can cast casts its whole silhouette
there, as a skinned MASK material does; a skinned one that cannot cast casts nothing on either path.
For every other material transmission is 0 and the tests are unchanged. Coloured or partial shadows
would need another shadow representation (section 8).

## 6. TAA, render scale, MSAA, GTAO

- **TAA.** Transparent surfaces are drawn jittered with the opaque pass, before the TAA resolve,
  so their edges are anti-aliased like everything else, and write no motion vectors: TAA
  reprojects their pixels with the depth and the object motion of what is behind them. A moving
  transparent object, or a still one in front of a background at another depth while the camera
  moves, therefore reprojects from the wrong place; the variance clip limits the trail. Accepted
  and measured ([bench/transparency.md](../bench/transparency.md) 3), as an engine without
  translucency velocity does; drawing them after TAA without jitter, as splats are, would trade the
  trail for aliased edges and a second, unjittered depth of the whole opaque scene.
- **Render scale.** Below 1 they are drawn at the internal size with the opaque pass and upscaled
  with it (textures' gradients scaled alike, [taa-gtao.md](taa-gtao.md) 4.1); tests/transparency.rs
  checks that the panes converge to their colours at 0.67.
- **MSAA.** The pipelines have the pass's samples; blending is per sample; no alpha to coverage.
- **GTAO.** GTAO darkens the opaque image before the stage draws (section 3): what shows through a
  transparent surface is darkened by GTAO and then attenuated by `T`, and the surface's own light,
  the particles and the grid are not darkened. The pixel is `C + T (B (1 - s (1 - v)))` for the
  surface's light `C`, the opaque color `B`, its indirect share `s` and the visibility `v`, the
  usual order of ambient occlusion before translucency. Before the review the stage drew first and
  GTAO multiplied the whole blended pixel, `(C + T B)(1 - T s (1 - v))`, the share only attenuated
  by `T`: a pane of coverage 0.5 passed about 0.69 of the occlusion behind it and darkened its own
  light (3.8% in the review's probe). tests/transparency.rs checks the order to within 0.2%. GTAO's
  depth is the opaque scene's. In frames without transparent instances the particles and the grid
  are still drawn before GTAO, which darkens them by the share of what is behind them (as before).

## 7. Picking, splats and particles

- **Entity ids.** The id pass's last call draws the stage's draws (unjittered frame group over its
  list, the id pipeline: front faces, depth written) after the opaque batches and before the
  splats: a transparent object is picked and covers, in `render.visible`'s coverage, what shows
  through it, even nearly clear glass. Agents perceiving the view through that coverage therefore
  no longer see what is behind glass; the alternatives (no id depth for transmissive surfaces, so
  what is behind stays visible and the glass is not picked; or a second id layer for what shows
  through) are an owner's decision, not taken here. `Renderer::pick_ray` tests every slot's box and
  is unchanged.
- **Splats** are drawn after TAA over the resolved image, tested against the opaque depth only:
  a splat behind a transparent surface is drawn over it.
- **Particles** are drawn after the stage, tested against the opaque depth: a particle behind a
  transparent surface is drawn over it. Both are known limits of compositing separately sorted
  layers.

## 8. Not done

- Refraction, rough-glass blur and hue-tinted transmission: a copy of the opaque image sampled by
  the transparent pass (as Filament and three.js render KHR_materials_transmission), or
  dual-source blending where the device has it.
- Order-independent transparency (per-pixel order of intersecting or interpenetrating
  surfaces): sorting is per instance by the bounding sphere's centre, so two instances that
  intersect, or one inside another's sphere, may composite in the wrong order where they overlap.
- Translucent shadows (coloured or partial); the shadow test reads the transmission factor, not its
  texture.
- Transparent neural-texture materials.
- The Bistro (town-walk) drawn with the stage: its glass (13 transparent primitives,
  [bench/town-walk.md](../bench/town-walk.md)) will change town-walk's images and timings; the
  asset is not on the machine the stage was built on.
- Images reproducible to the bit: from run to run of one build, pt-lab's glass sphere and the 4x
  MSAA edge of the card beside it differ by one half-float step in up to 32 pixels
  ([bench/transparency.md](../bench/transparency.md) 4), where master's renderer draws the scene the
  same every time. The cause is NVIDIA's Direct3D 12 driver, which stores wrong colours in 4x MSAA
  samples under a blend that reads the destination: the noise appears only on the RTX 5060 with
  Direct3D 12 and 4x MSAA ([bench/gpu-matrix.md](../bench/gpu-matrix.md) 15.1). `prepass_compare.py`
  and `occlusion_compare.py` compare 8-bit images, which do not see the step, and it does not move
  `transparency_pt.py`'s region errors at their four decimals.

## 9. Checks

- `cargo test -p amoris-render --test transparency`: overlapping half-transparent quads compose
  back to front whatever the feed's order (to the bit), double-sided and single-sided BLEND quads
  seen from behind, a look's alpha on the inline material; a transparent pane over a field of
  opaque cubes leaves the image identical with the prepass and occlusion culling forced on, every
  cube shows through, none is counted occluded, and the pane is picked; one renderer switched
  through anti-aliasing and GTAO draws the images of renderers made so, and the stage after GTAO
  changes no pixel where GTAO darkens nothing; GTAO takes from a pane's pixels its transmittance
  times what it takes without the pane; the panes converge under TAA at render scale 0.67;
  transparent casters shadow by their opacity under the cascades and, with ray queries, the
  ray-traced shadows (clear glass and alpha 0.35 are no casters). With 4x MSAA and one sample.
- `cargo test -p amoris-render --lib`: `materials::` (which rows are transparent and which cast),
  `scene::` (the transparent slot set through rebuild, hiding, removal and reset), `transparent::`
  (the order, the draws, the two-sided lists' placement under a small limit, the reversal shader).
- `cargo test -p amoris-render --test draw_paths`: the stage on the three draw paths.
- `tools/render/build_compare.py` (no transparent material: identical to the build before), and
  `tools/render/prepass_compare.py` and `tools/render/occlusion_compare.py` (pt-lab included:
  identical with either feature forced on).
- `python tools/render/web_draw_paths.py "http://127.0.0.1:8090/viewport/?demo=transparent"` (a
  viewport built with `tools/build_viewport.sh`, `web/` served): the stage in a browser on WebGPU's
  two draw paths (`--compare occlusion`: occlusion culling off and on).
- `tools/render/transparency_pt.py`: pt-lab's forward image against the path tracer, by region;
  `tools/render/transparency_bench.py`: the stage's cost and (`--ghost`) TAA on transparent
  surfaces.
