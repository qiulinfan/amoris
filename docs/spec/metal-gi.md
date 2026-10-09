# Global illumination implementation (Metal first, every ray-query backend since 2026-10-09)

Status: accepted implementation order, 2026-10-05; `feature/metal`. 2026-10-09 (Pioneer, charter
4.4): the ray-query tracers below run on Vulkan and Direct3D 12 as well as Metal, wherever the
adapter exposes `EXPERIMENTAL_RAY_QUERY`; the file keeps its name so links stay valid.

1. Bake a world-space probe volume from project geometry and lighting. Use a versioned JSON asset,
   directional radiance SH and directional distance moments. Load it asynchronously and evaluate
   occlusion-aware interpolation in the same native/WebGPU forward shader. Preserve sky specular;
   within the bake volume, baked diffuse replaces sky-only diffuse to avoid counting the sky twice.
2. Implement spatially hashed radiance caching (SHaRC) with explicit sample/update/query work,
   confidence and history control. Compare against identical uncached ray-traced lighting.
3. Explore ReSTIR GI/PT sample reuse with actual ray visibility and controlled history rejection.
   Distinguish a bounded prototype from complete multi-material path tracing.
4. Train a small network from light-transport samples and evaluate neural radiance/irradiance
   queries. Record training, query and rendering costs and quality separately, then together.

The first bake supports static triangle geometry, diffuse transport, emission and declared lights.
Unsupported material, animation, atmosphere or special-renderer content is rejected or documented
explicitly. A bake is static lighting data; moving geometry/lights requires rebaking.

Visual settings are world data. Acceleration structures, radiance caches, reservoirs and neural
optimizer state are derived rendering resources. They must not alter simulation authority or
tick determinism. Schema/feed changes rebuild the browser viewport.

Validation uses an enclosed room with colored walls, occluders and emission, then project assets.
Evidence includes GI-off/on images, bake seed and settings, ray counts, memory, whole-frame timing,
dynamic response and limitations. Timings are measurements, not passing thresholds.

The current slices and limitations are recorded in [the measured report](../bench/metal-gi.md).
Baked and learned fields are integrated into the native/WebGPU forward renderer. SHaRC and
ReSTIR currently run in the isolated ray-query tracer (`gi::rt::RayLighting`, example `gi_trace`;
Metal, Vulkan or Direct3D 12) with constant diffuse materials and emission.
ReSTIR reuse is opt-in experimentation with negative quality results; the default has no reuse.
The learned field is offline supervised approximation of the baked teacher, not online dynamic NRC.

## Backends (Pioneer, 2026-10-09)

The tracer's device, gate, probe, tests and the optional lean shaders (`RayLighting::with_shaders`,
`gi_trace --lean-shaders true`: no naga loop bounds or ray-query tracking) are shared with the
surface path tracer and described in its spec's
[Backends](path-tracing-nrc.md#backends-pioneer-2026-10-09) section. `trace` returns a miss for a
nonfinite ray or an empty interval before initializing a query. SHaRC's hash insertion races by
design (exact keys, insertion winners), so cached images differ slightly from run to run and between
backends; uncached and RIS images reproduce the M5 ones to rounding
([measurements](../bench/metal-gi.md#windows-vulkan-and-direct3d-12-pioneer-2026-10-09)).

## Surface PT and online NRC follow-up

The subsequent [surface PT and online NRC](path-tracing-nrc.md) implementation adds full static
PBR/refraction transport and actual GPU training from new traced path tails. The offline six-input
baked-field model above remains a separate experiment. The bounded ReSTIR implementation retains
its stated limitations; it is not replaced by a claim of complete ReSTIR PT.
