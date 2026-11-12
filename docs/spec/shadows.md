# Shadows

Status: Draft. Spot lights' and point lights' shadow maps implemented 2026-11-12 (charter 4.4's
notes of that date).
The sun's cascades and their ray-traced alternative are older and are specified where they were
built; this file says who casts shadows in the interactive renderer and specifies the local shadow
views.

Charter: 4.4 (光照与画质: cascaded shadows, and the note of 2026-11-12 on spot lights), 3 item 7
(GPU-driven; no silent limit), 4.1 (one renderer for Metal, Vulkan, Direct3D 12 and WebGPU).

This specification says which lights cast shadows and on what, how a spot light's shadow map and a
point light's six are laid out among the views of the GPU-driven culling, how a frame chooses the
lights it shadows and reports the ones it does not, how the map is drawn and sampled, what it costs,
how it is switched off and how it is checked. The code is `crates/amoris-render/src/shadows.rs` (the
views and the choice), `batches.rs` (the argument sets), `renderer.rs` (`sync`, `render`),
`shaders/cull.wgsl` (the views' culling), `shaders/forward.wgsl` (`local_shadow`, the two light
loops, `vs_shadow`) and `shaders/common.wgsl` (`View`, `Light`).

Why: `Light.shadows` reached the renderer for every kind of light and the path tracer honoured it
([path-tracing-nrc.md](path-tracing-nrc.md)), but the interactive renderer shadowed the sun only: a
spot or point light shone through walls and floors.

## 1. Who casts and who receives

| Light | With `shadows` set | Specified in |
|---|---|---|
| Directional (the sun: the directional light of lowest id) | four cascaded shadow maps, or with `AMORIS_RT_SHADOWS=1` one ray per fragment | [occlusion.md](occlusion.md) 11 (which casters a cascade draws), [lod.md](lod.md) 4 (their levels), [transparency.md](transparency.md) 5 (transparent casters), [rt-shadows.md](rt-shadows.md) |
| Spot | a shadow map of its own, a local shadow view | this file |
| Point | six local shadow views, the faces of a cube around it | this file |

A frame has 12 local views (2): twelve shadowed spots, two point lights, or one point light and six
spots.

`Light.shadows` defaults to true, so a spot or point light casts unless its scene says otherwise.

**Casters** are the instances the cascades draw: alive, visible, with `Model.cast_shadows`
(`FLAG_SHADOW`), by their material's opacity (an alpha-masked material casts its cut-out, a
transparent one where it is at least half opaque, [transparency.md](transparency.md) 5).

**Receivers** are the surfaces that sum punctual lights: the opaque pass (`shade`) and the
transparent stage (`transparent`). The sea, water regions and their wet bed, the spray, the liquid
surface, particles and splats sum no punctual light, so no spot light lights or shadows them.

## 2. Local views

A local view is one shadow map for one frame: a perspective view from a spot light along its cone
(4), or one of the six faces of a point light's cube (4.1), handled as a cascade is. A frame has up
to `LOCAL_VIEWS = 12`, in slots 0 to 11. A spot light takes one slot, a point light six consecutive
ones (its **cost**). Slot `k` is

- view `5 + k` of the culling pass (`cull.wgsl`: 0 the camera, 1 to 4 the cascades) and region
  `5 + k` of the `visible` list;
- argument set `6 + k` of the indirect draws (`batches::local_set`; set 5 is the camera's late
  set, [occlusion.md](occlusion.md) 6);
- layer `4 + k` of the shadow array (the lighting group's binding 0, after the cascades' layers);
- `View.local_views[k]`, its view-projection.

**Capacity.** The lists, the argument sets, the baseline path's bases and the shadow array are laid
out for `capacity = min(12, the costs of the spot and point lights with shadows set)`, and for 0
with the switch off (8). Only the kind and the flag count (`shadows::casts_locally`,
`shadows::cost`): a light that changes its
intensity, pose or cone, a flashlight switched by its intensity, lays nothing out again. When the
capacity changes (such a light added or removed, its flag or the switch changed), `sync` lays the
regions out again, re-creates the shadow array with exactly `4 + capacity` layers and rebuilds the
lighting group; when it falls, `visible` goes back to the size a renderer would hold for the views
left.

**Without a shadowed spot or point light** the capacity is 0 and every buffer, list, argument set,
base and draw is what it was before local views existed: no memory, no pass, and no pixel changes
(10).

## 3. The choice each frame

`shadows::local_views` picks the frame's local views from the lights, the camera and the opaque
pass's size, and from nothing else: no state of earlier frames, and not the order the feed lists
the lights in.

A light is a **candidate** when

- it is a spot or a point light with `shadows` set;
- it can be placed and lights something (`shadows::spot_view`, `shadows::point_views`): every field
  finite, a range and an intensity above 0, and for a spot a direction of some length and an outer
  angle above 0;
- the sphere around what it lights meets the camera's frustum: a point light's is the sphere of its
  range, a spot's the sphere around its lit sector (`shadows::cone_sphere`: the points within the
  range and within the outer angle of the axis). The sphere is the one through the apex
  and the cap's rim up to 45 degrees of half angle (`range / (2 cos half)` along the axis, the same
  radius), the rim's own sphere up to 90 degrees (`range cos half` along the axis, radius
  `range sin half`), and the range's sphere around the light past that.

The frustum is the unjittered camera's, widened by two pixels of the opaque pass on each side: it
holds every jittered frustum (TAA's jitter is at most half a pixel), so past the limit the choice
does not flip with the jitter's phase.

The candidates are ranked by their **size on screen**, the sphere's radius over the distance from
the eye to its centre (the radius alone under an orthographic camera), largest first, equal ones by
the lower entity id. In that order each takes its cost in slots while `capacity` has them left, and
a light whose cost no longer fits goes without (a point light with five slots left), while a
smaller spot after it may still take one. The lights taken get their slots in the order of their
ids, a point light's faces in the order of 4.1. A scene whose shadowed lights cost at most 12
therefore shadows every one in view; with more, the smallest on screen go without, and since the
choice has no memory a shadow can appear or vanish as the camera moves past the point where two
lights are equal (11).

**No silent limit** (charter 3 item 7). The candidates left without a view are counted:
`FrameStats::local_shadows` is the lights shadowed (a point light counts once),
`FrameStats::local_shadows_dropped` the shadowed lights in view that got no view, and the renderer
logs one warning when the latter becomes nonzero (and again after it has returned to 0). The
viewport's stats carry both (`local_shadows`, `local_shadows_dropped`); its page's HUD shows them
when either is nonzero.

A chosen light's row of the lights buffer carries `shadow = 1 + slot` for the frame, its first slot
for a point light (0: no map);
the buffer is written only when the lights or the choice changed.

## 4. The view

`shadows::spot_view` gives a light's view-projection: perspective, right-handed, standard depth (0
at the near plane, 1 at the far plane; the map is cleared to 1 and compared `LessEqual`, as the
cascades), square, along the light's direction with +y up (+z when the direction is within about 8
degrees of vertical).

- **Half angle**: the outer angle plus 1 degree, at least 1 and at most 80 degrees. The extra
  degree keeps what a receiver at the cone's edge reads (its offsets and the filter's taps, 6)
  inside the map, with at least two texels to spare. A cone whose outer angle is over 79 degrees is
  shadowed only inside the 160 degrees the map covers; outside it the light is unshadowed (11).
- **Near plane**: 5 cm from the light, or half the range when that is less. The plane is flat: a
  caster nearer the light than it, along the axis, is clipped out of the map, and a receiver there
  is lit. Toward the rim of a wide cone the plane cuts the sector farther from the light than on
  the axis (26 cm at 79 degrees).
- **Far plane**: the range (the range the shader lights to, at least 1 cm).
- **Size**: 2048 x 2048 (`SHADOW_SIZE`), one 32-bit float depth layer.
- **Texel**: `2 tan(half) / 2048` metres wide per metre of depth from the light (0.7 mm per metre
  for a 35 degree cone: 3.5 mm at 5 m). It travels in `Light.shadow_texel`.

### 4.1 A point light's faces

`shadows::point_views` gives a point light's six view-projections, the faces of a cube around its
position, in the order +x, -x, +y, -y, +z, -z (face `f` is slot `first + f`): perspective,
right-handed, standard depth, square, each looking along its axis with +y up (+z up for the +y face,
-z for the -y face).

- **Half angle**: `atan(1 + 8 / 2048)`, a little over 45 degrees, so that neighbouring faces
  overlap by four texels at each edge. The point a receiver reads chooses its face (6), so its
  projection lies at least four texels inside that face's map, which holds the filter's taps.
- **Near plane**: 5 cm from the light along the face's axis, or half the range when that is less;
  **far plane**: the range (at least 1 cm). A caster within 5 cm of the light along every axis is
  clipped out of every face.
- **Size**: 2048 x 2048, a layer of the shadow array each.
- **Texel**: `2 (1 + 8 / 2048) / 2048` metres wide per metre of depth along the face's axis (about
  1 mm a metre), in `Light.shadow_texel`.

## 5. Culling and drawing

**Culling.** `Cull.planes[5 + k]` holds the six planes of the view's clip volume (`shadows::planes`,
as a cascade's box); a point light's faces are six views like any other. The culling pass keeps a
caster in a local view when its bounding sphere passes them. That test alone is exact: a caster
writes only the texels whose centres its triangles cover, all inside the frustum, so a caster
outside it writes nothing. A local view has no slice and no bound of its own: `Cull.slices` and
`Cull.lod_cascades` hold the cascades' terms and are never indexed with a local view.

`Cull.view_mask` has a bit per view culled this frame. Without a sun's shadows, and with ray-traced
sun shadows (9), the cascades are not drawn: their bits are clear, their lists and arguments stay
empty, and the local views are culled all the same (`view_count` is then `5 + n` with views 1 to 4
skipped). Without a local view the mask is all ones below `view_count` and the pass does what it
did.

**Level of detail.** A local view draws each caster at the level the camera draws it at this frame
(picked in the same dispatch, [lod.md](lod.md) 4), not at a bound of its own texels. A spot's texel
is usually finer than a pixel of the camera, so a texel bound would draw the caster finer than the
camera draws the same mesh as a receiver, and a coarse receiver lies partly inside its finer copy
and is shadowed by it ([lod.md](lod.md) 7 measured that under ray-traced shadows). With the camera's
level the mesh that casts is the mesh that receives. Two consequences: a caster far from the camera
but close to its light casts its coarse outline, magnified by the light's projection; and a coarse
level can shadow itself where its simplified shape differs from the full mesh's (the sphere
primitive's first level does at one pixel near its pole from 7 m), which a full mesh that is convex
never does. A transparent receiver's level is picked on the CPU without the culling pass's
hysteresis ([transparency.md](transparency.md) 3), so within the hysteresis band it can differ by
one level from the copy that casts.

The workgroup counters of the two-level counting are per view and level: 136 (17 views of 8
levels).

**Drawing.** After the cascades' passes, and whether they are drawn or not, one pass per local view
draws into layer `4 + k`, cleared to 1, with the cascades' two caster pipelines (opaque and
alpha-masked; depth bias constant 2 and slope 2.5, no face culling) and the batches of set `6 + k`
on whichever draw path the device has ([webgpu-baseline.md](webgpu-baseline.md) 2). The vertex
stages `vs_shadow` and `vs_shadow_masked` take the view from the list entry they read,
`(instance_index + batch.x) / stride`: 1 to 4 is a cascade, 5 on a local view. The profiler names
the passes `local shadows`; `Renderer::draw_counts` reads back what each drew
(`DrawCounts::local`).

## 6. Sampling

Both loops over a pixel's punctual lights (`shade` and `transparent` in forward.wgsl) multiply a
light's attenuation by `local_shadow` when its `shadow` word is set and the attenuation is not 0
(the point is in range and, for a spot, in the cone):

1. `t`, the map's texel at the receiver: its depth from the light times `Light.shadow_texel`; for a
   spot the depth along the axis (the clip `w` of its position in the map), for a point light the
   largest of the three coordinates of the receiver's offset from the light (its depth along the
   face whose axis that is).
2. The point read is the receiver moved by `(1.5 n + l) t`: 1.5 texels along its normal `n` and
   1 toward the light `l`.
3. A point light's face is the one whose axis is the largest coordinate of the moved point's offset
   from the light (`cube_face`; ties go to x, then y, the positive side at 0).
4. The point is projected into the map. A point behind the light's plane, outside the map's square
   or past the far plane is lit.
5. Nine comparison taps one texel apart (`textureSampleCompareLevel`, the cascades' comparison
   sampler: `LessEqual`, linear) are averaged.

There is no constant depth bias: perspective depth is `f / (f - n) * (1 - n / w)`, so a constant
in it is a fraction of a millimetre near the light and metres at the range. The two offsets are in
texels of the receiver's own depth instead. On a plane the casters' slope-scaled bias would do
alone (2.5 slopes against the two texels the taps reach), but it follows each caster triangle's
own slope, and on a curve a receiver reads its neighbours' texels: without the offsets a ball
shows acne on its lit side (10). The price is that a shadow starts about 2.5 texels off its
caster's contact.

A point every tap passes gets exactly 1, so outside every shadow the frame is the frame of
`shadows: false`.

## 7. What it adds

Nothing is bound that was not: the forward fragment stage still holds 8 of 8 storage buffers and
its layout 4 of 4 groups on WebGPU's defaults ([webgpu-baseline.md](webgpu-baseline.md) 3). Local
views live in

- the `View` uniform: `local_views`, twelve matrices at its end (752 to 1,520 bytes; the view
  buffers some tests build by hand are 2,048 bytes);
- the lights' padding: `Light.shadow` and `Light.shadow_texel` (a light stays 64 bytes);
- the `Cull` uniform: seventeen views' planes and `view_mask` in a padding word (1,216 to 2,368
  bytes);
- more layers of the shadow array: 16 MiB a local view in use, 96 MiB a shadowed point light, 192
  MiB at the twelve views' capacity;
- the `visible` list: 4 bytes per entry and view in use. Up to seven local views (48 bytes at
  twelve views) it does not outgrow `drawn`'s 48, so the size at which levels of detail fall back
  ([lod.md](lod.md) 5) does not move; past seven (a point light and two spots or more) `visible` is
  the larger list and levels fall back at fewer entries;
- an argument set per local view in use, and on the baseline path its bases;
- up to twelve depth passes a frame.

## 8. The switch

| Setting | Natively | In the browser | Effect |
|---|---|---|---|
| local shadows | `AMORIS_LOCAL_SHADOWS=off` / `on` (default on) | `?local_shadows=off` (viewport page) | off: capacity 0, spot and point lights unshadowed: the frame, the lists and the shadow array of a scene whose spot and point lights have `shadows` off |

`Renderer::set_local_shadows` changes it at any time (from the next frame). One light is switched
by its own `Light.shadows`.

## 9. Interactions

- **Ray-traced sun shadows** (`AMORIS_RT_SHADOWS=1`, [rt-shadows.md](rt-shadows.md)): the cascades
  are neither culled nor drawn, the local views still are; spot and point lights stay
  shadow-mapped. Levels of
  detail are off there, so both the camera and the local views draw full meshes.
- **Water**: the spray, the liquid surface and the water's medium sample the cascades' layers only
  (0 to 3) and take the array's view per call, so a re-created array reaches them.
- **Transparency**: a transparent surface is shadowed as an opaque one (the same function in the
  stage's loop), and casts into a local view by the opacity test of the cascades.
- **TAA and MSAA**: the maps are not jittered; with MSAA a partly covered pixel shades at its
  centre, extrapolated along its triangle, and a point so moved outside the map is lit.
- **Time**: lights are drawn at their latest tick's pose, casters between the last two ticks: a
  spot carried by a moving body can lead its mesh's shadow by up to a tick.
- **A lamp around its light**: a shadow-casting mesh that encloses the light (beyond the near
  plane's 5 cm) shadows the whole cone, or for a point light everything; such a mesh needs
  `cast_shadows` off.
- **Orthographic cameras**: the light clusters assume a perspective camera (cluster.wgsl,
  `cluster_of`), so punctual lights, and with them their shadows, are not reliable under an
  orthographic one. Not changed here.

## 10. Checks

- `cargo test -p amoris-render --lib shadows`: `a_spot_view_holds_its_cone` (random spots, vertical
  and nearly vertical axes: every lit point from the near plane to the range, for outer angles up
  to 79 degrees, is in the map with two texels to spare, and a sphere holding it passes the view's
  planes), `a_point_lights_faces_hold_its_sphere` (random point lights: every lit point from the
  near plane to the range, on the faces' seams and corners too, lies in the face `cube_face` picks
  at the depth the shader takes, four texels inside its edges, and a sphere holding it passes that
  face's planes), `the_cone_sphere_holds_the_sector`, `local_views_pick_by_size_on_screen_then_id`
  (the largest win, equal ones by the lower id, any order of the lights gives the same views and
  slots, what is out of view or cannot cast is neither drawn nor counted),
  `local_views_count_a_point_light_six_times` (a point light that no longer fits is counted while a
  smaller spot after it takes a view; a point light's faces are consecutive slots in face order),
  `the_shaders_hold_the_same_constants` (the WGSL's literal sizes follow the Rust constants; both
  of the culling pass's guards over its 136 counters, the first of which zeroes them: compute
  pipelines run without automatic workgroup zeroing). `batches::` checks that each argument set
  draws its own view's list, and renderer.rs checks the three uniform sizes at compile time.
- `cargo test -p amoris-render --test local_shadows`, on Metal, on MoltenVK and with
  `AMORIS_GPU_MINIMAL=features,limits,timestamps` (the baseline path on WebGPU's default limits):
  - a spot aimed obliquely at a floor past a cube, with one sample and with 4x MSAA: in the cube's
    shadow the floor is the frame with the spot's intensity 0, everywhere else on the floor the
    frame of `shadows: false`, the cube's lit faces are not shadowed by the cube; without the cube
    the shadowed spot changes no pixel (the floor a caster of its own map, light meeting it from
    straight down to 68 degrees); one local view drawn, no cascade drawn without a sun;
  - a point light in the spot's place, the same (the floor lit from straight down to 73 degrees,
    across the cube map's seams), and a point light 2 m up with a cube on each side and a slab below
    it: each face that looks at the floor holds its cube's shadow, the floor outside the shadows is
    the frame of `shadows: false`, and the switch off draws that frame to the bit (with the point
    light always reading its -y face, the second fails);
  - `shadows: false`, and `shadows: true` with the switch off, draw the same frame to the bit with
    the lists of the scene without the light; each view in use adds 4 bytes per list entry; the
    switch turned back and forth returns each frame to the bit; a point light's six views add 24
    bytes per entry;
  - casters by opacity (an opaque cube and a blend of alpha 0.6 cast, clear glass and a blend of
    alpha 0.35 do not, an alpha-masked quad casts its checker), and a blended pane is shadowed;
  - thirteen shadowed spots in view: twelve are drawn and one counted, the smallest on screen from
    two cameras, the higher id of two equal ones, the same frame to the bit with the lights listed
    in reverse; a spot behind the camera takes no view and is not counted;
  - a ball drawn as its full mesh does not shadow its lit side (without `local_shadow`'s offsets it
    does);
  - the three draw paths draw the same local view, also with occlusion culling and the depth
    prepass forced on;
  - a rock 50 m from the camera is drawn into the local view at the camera's level, and at level 0
    with levels off;
  - beside a shadow-casting sun the spot's shadow keeps the sun's light and the sun's shadow the
    spot's; on a device with ray queries the same with ray-traced sun shadows, no cascade drawn.
- `tools/render/build_compare.py` against the build before: `cubes-shadows`, `cubes-dense` and
  `gi-room` identical on Metal and MoltenVK, `crowd` too with MSAA (under TAA its skinned crowd
  differs by up to 9 of 255 between two runs of either build, and by no more between the builds);
  `pt-lab`, whose spot has `shadows` set, differs, and is identical with
  `AMORIS_LOCAL_SHADOWS=off`.
- The `shadow_cull` example's `compare` (every scene identical on Metal, MoltenVK and the baseline
  device), `tools/render/occlusion_compare.py` and `tools/render/prepass_compare.py` on Metal (no
  coverage changes; pt-lab's one-level difference under occlusion culling is the same with
  `AMORIS_LOCAL_SHADOWS=off`).
- A host drawn by the viewport page in headless Chrome (`render=ws://HOST/render`), also with
  `gpu_minimal=features,limits,timestamps` and with `local_shadows=off`: a spot over a cube, and a
  point light among four cubes (the shadows in the -y, +x and -x faces; the baseline path draws the
  same frame). There a point light's six passes took 1.5 to 3 ms of GPU time a frame on the M5, an
  interval sum dominated by clearing and drawing six 2048 x 2048 layers.
- `tools/render/build_compare.py` against the spot lights' build: every case identical but
  `pt-lab`, whose point light has `shadows` set; with that light's `shadows` off, and with
  `AMORIS_LOCAL_SHADOWS=off`, pt-lab is identical too on Metal and MoltenVK (the spot's sampling
  did not move a bit).

Not run: Direct3D 12 and Windows (deferred by the owner, 2026-10-25).

## 11. Open

- **Cached maps**: a still light with still casters redraws its map every frame.
- **Soft penumbrae**: the nine taps are a fixed filter; a light has no size.
- **Resolution**: every local view is 2048 x 2048 whatever its size on screen, a point light's
  faces too (96 MiB a light); an atlas or per-light sizes would cost less and let more lights
  cast.
- **Point lights' faces** are culled and drawn whether the camera sees what they shadow or not; a
  face whose frustum misses the camera's could be left out.
- **Hysteresis in the choice** past the limit (3).
- **Culling by the receivers**: a local view draws every caster in its frustum, not only those
  that can shadow what the camera sees (as the cascades' slices do).
- **Levels by the map's texel**, with the caster's and the receiver's levels reconciled (5).
- **Cones wider than 160 degrees** (4).
- Punctual lights and their shadows on the water, the sea, particles and splats (1).
