# Ray-traced sun shadows (opt-in)

Status: implemented 2026-10-09 (Pioneer), branch `explore/rt`; charter 4.4, the decision of the
same date. Measurements: [bench/rt-shadows.md](../bench/rt-shadows.md). Any project can use it:
`POCKET_RT_SHADOWS=1 pocket serve samples/anim` (or `pocket play`; both read the variable when they
open the GPU; the serve path's captures were checked).

The interactive renderer's first ray-traced effect. Off by default; `POCKET_RT_SHADOWS=1` turns it
on for a native adapter that exposes wgpu's `EXPERIMENTAL_RAY_QUERY` (the RTX 5060 and the Radeon
780M on Vulkan and Direct3D 12 here; Apple GPUs on Metal). It replaces the cascaded shadow map of
the sun (the lowest-id directional light with `shadows`) with one inline ray query per shaded pixel.

## Why inline ray queries in the forward pass

The renderer is forward-shaded with no G-buffer and no depth prepass: a compute pass cannot supply
visibility before the pixels are shaded, so a screen-space shadow mask would need a new prepass.
A ray query inside the forward fragment shader's `shadow_factor` needs nothing new in the frame:
the pixel already has its world position, normal and the sun direction. It costs one ray per
shaded fragment (overdraw included), where the cascades cost four depth passes.

## What changes when it is on

- **Device** (gpu.rs): `Gpu::new` requests `EXPERIMENTAL_RAY_QUERY`, the acceleration-structure
  limits and wgpu's experimental-features opt-in only when `POCKET_RT_SHADOWS=1` and the adapter
  has the feature, never with `POCKET_GPU_MINIMAL=features` and never in the browser;
  `Capabilities::ray_query` says whether it did. `Gpu::headless_ray_query(choice, on)` asks
  explicitly, so a test or example can hold both kinds of device in one process.
- **Mesh pool** (meshes.rs): on such a device the vertex and index buffers also carry `BLAS_INPUT`
  and `STORAGE`; the pool records each mesh's vertex count and whether the GPU writes its vertices
  (skinning).
- **Acceleration structures** (rt_shadows.rs): one opaque bottom level per static mesh, built once
  when the mesh first appears; a non-opaque copy for each mesh drawn with an alpha-masked material,
  built when an instance first needs it; for each skinned mesh (vertices the skinning pass writes),
  a fast-build bottom level rebuilt in the frame's encoder after that pass, every frame it casts.
  The top level holds the instances the cascades would draw (alive, visible, casting shadows) with
  their drawn pose: the cull pass's interpolation between the last two ticks (lerp, nlerp), times
  scale. It is rebuilt only when the renderer uploaded changed slots, a bottom level was added, some
  slot is interpolating or a skinned mesh casts; a static scene keeps its build. It grows by
  doubling; growing replaces the lighting bind group.
- **Shader** (rt_shadows.wgsl, appended to forward.wgsl by `RtShadows::forward_source`, which
  renames the cascaded `shadow_factor` to `csm_shadow_factor`): the forward and ocean pipelines use
  this variant, with `enable wgpu_ray_query`. The lighting group gains bindings 11 to 14: the top
  level, a per-instance table (material row, first index, base vertex), and the mesh pool's indices
  and vertices. The ray starts at the surface pushed along its normal by 2 mm plus 1 mm per metre of
  view depth, goes toward the sun with `RAY_FLAG_TERMINATE_ON_FIRST_HIT`, `t` up to 1e5. Opaque
  geometry commits by itself; a masked caster's candidate hit is confirmed when the material's
  alpha at the hit's interpolated UV (base color factor times texture, LOD 0) reaches its cutoff,
  the test of the masked shadow pass.
- **Frame** (renderer.rs): the cascades are neither culled for nor drawn; the top level is built
  after the cull and light passes, before the opaque pass. Nothing else moves.

## Semantics that differ from the cascades

- Hard shadows (a point sun, one ray, no filtering), where the cascades filter 3x3.
- No distance limit: the cascades end at the shadow distance and fade; rays reach any caster.
- A skinned mesh with an alpha-masked material casts an opaque shadow (its non-opaque copy would
  need the per-frame rebuild too).
- The ocean surface receives but does not cast, as before; splats neither cast nor receive, as
  before.
- Masked casters pass the same alpha test at LOD 0 as in the masked shadow pass, at the ray's hit
  instead of a shadow-map texel, so cut-out edges differ by up to a texel of the map.

## Costs and limits found

- wgpu-core validates every top-level instance's bottom level at each submission that uses the top
  level (`command/ray_tracing.rs`: one dependency entry per instance, not per distinct bottom
  level), so CPU time grows with the instance count even for a static scene (about 4.5 ms per frame
  at 200,000 instances here). Rebuilding the top level every frame adds the instances' transforms
  on the CPU and the build on the GPU.
- naga's ray-query tracking costs 2% to 4% of the frame here (one query per pixel), so the forward
  module keeps every runtime check, unlike the research tracers' lean option.
- Native only. WebGPU has no ray queries; nothing of this is compiled into a pipeline there.

## Interfaces

`Renderer::rt_shadow_stats()` (instances, masked instances, bottom levels, whether this frame
rebuilt); the `rt_shadows` example (both paths on one scene: timings and captures, `--heroes N`
skinned characters, `--moving` to force a rebuild per frame); the integration test
`tests/rt_shadows.rs` (on every native backend with ray queries the pixels the two paths darken in
the mixed demo scene with three skinned characters overlap with an intersection over union above
0.85; 0.897 measured); `many_cubes --headless-bench` reports the stats.

## Next

Soft shadows from the sun's angular size (a cone of directions with temporal accumulation),
ray-traced ambient occlusion, refitting skinned meshes' bottom levels instead of rebuilding them,
compacting bottom levels, and a top level of only the casters near the view. Each is measured
against this baseline.
