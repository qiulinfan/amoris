# Surface path tracing and online NRC

Status: implemented and measured on Apple M5, 2026-10-05, `feature/metal`. Since 2026-10-09
(Pioneer, charter 4.4) it also runs on Vulkan and Direct3D 12 wherever the adapter exposes ray
queries; see [Backends](#backends-pioneer-2026-10-09).

The first implementation is a complete standard surface path integrator for the engine's static scenes:
metallic-roughness GGX plus diffuse, dielectric reflection/refraction, textures and cutouts,
emissive triangles, directional/point/spot lights, environment radiance, NEE/MIS, Russian roulette
and progressive accumulation. Sampling and PDFs must agree. Geometry/animation streaming and
volumetric/BSSRDF transport are separate engine features, not claims of this integrator.
Measure practical optimizations at identical scene, resolution, transport and sampling settings.

The subsequent online NRC is deliberately a toy for one M5. It must generate new path-traced
training targets during rendering, update a small network on the GPU and query it on eligible
secondary surfaces. Demonstrate adaptation to changed lighting, bounded memory and explicit
cache/training reset. Retain uncached PT as a reference. Report approximation bias, training,
inference and total cost; do not require a production-quality performance threshold.

Keep the previous baked/probe and bounded SHaRC/ReSTIR experiments runnable. Shader sources stay
in WGSL files. GPU caches and network optimizer state remain outside simulation authority.

`pocket_render::gi::pt::PathTracer` and the native `path_trace` example own this integrator. It is
a separate ray-query research renderer (Metal, Vulkan, Direct3D 12), not a replacement of the
interactive forward/WebGPU renderer.
Static BLAS/TLAS, shading triangles and texture resources survive the progressive frames. Camera
paths use GGX visible-normal sampling, cosine diffuse, exact dielectric Fresnel, rough refraction,
delta mirror/refraction, power-weighted emissive triangle selection and uniformly sampled sky.
Punctual lights are delta samples and bypass the continuous-density MIS weight. Refraction's
eta-squared factor is compensated when choosing roulette survival. Emission remains outside NRC.

Base/emissive textures use an sRGB texture view: decoding happens before hardware interpolation.
Normal, roughness/metallic, transmission and alpha data use the linear view. Resizing color images
to the bounded common array dimensions also filters in linear light. Occlusion textures are not
multiplied into physical transport, because geometry already determines visibility. The current
texture path uses UV0, repeat/linear sampling and LOD 0, not ray footprints or glTF sampler variants.
Blend uses stochastic coverage; solid refraction uses transmission/IOR rather than alpha.

Optional `Model.transmission` and `Model.ior` override imported values only when present; glTF
KHR_materials_transmission and KHR_materials_ior are imported. Model persistence is version 2.
Look transports the optional values; the render-feed fingerprint and committed viewport match.
Frozen unparented triangle scenes are supported. Animation/skin updates, participating media,
volume absorption, nested-medium tracking and BSSRDF remain separate work. The maximum configured
path depth is a finite transport horizon, even when roulette is enabled.

`gi::nrc::OnlineNrc` owns a 14->32->32->3 ReLU MLP with 1,635 float32 parameters. Inputs are
normalized position, oriented shading normal, outgoing direction, base color, roughness and
metallicity. One uniformly rotated subset of pixels follows uncached full paths; each supplies at
most one secondary reflected-tail label per frame. Its local throughput includes direct lighting
and subsequent contributions, without dividing by potentially zero camera-throughput channels.
Those pixels never query the network. Other rough opaque secondary surfaces can query after a
configurable number of nonempty GPU updates. Glass and near-delta surfaces bypass querying.

One GPU pass computes each label's gradients, and a second reduces gradients and applies Adam;
no float atomics or CPU label/weight roundtrip is used in the render loop. The default batch is
256, bounded at 1,024. The active default cache uses 1,715,476 bytes including gradients, records,
optimizer, loss and parameter buffers. Pure PT binds a small disabled group to retain a common
layout. The network minimizes log1p-radiance MSE and clamps its predicted log radiance. This is
biased, including Jensen bias and an input representation without remaining path depth.

Each `render` call starts a new cache and accumulation sequence. `OnlineNrc::reset` resets weights
and optimizer state. Within a sequence, `--light-change-frame` scales source lighting, clears only
image accumulation and continues learning. There is no claim of support for streamed geometry or
arbitrary dynamic scene edits. Batch limits guard the timestamp query count and storage bindings
before invalid GPU resources are created. Statistics and losses are copied into GPU history and
read back only at the end; tracing and training have separate GPU timestamps.
Light-change diagnostics capture two GPU weight snapshots and one fixed real-path feature batch;
the parity-validated CPU reference evaluates them after the render loop. This readback is for
measurement and does not supply training labels or predictions to rendering.

Reproduce the full material scene and online experiment:

```sh
cargo run --release -p pocket-render --example path_trace -- samples/pt-lab \
  --width 320 --height 240 --frames 256 --samples 4 --output out/pt/lab.png
cargo run --release -p pocket-render --example path_trace -- samples/gi-room \
  --width 128 --height 128 --frames 512 --nrc true --nrc-warmup 64 \
  --light-change-frame 256 --light-change-factor 2.5 --output out/pt/nrc.png
# Windows: the same with POCKET_BACKEND=vulkan or dx12 (and POCKET_ADAPTER=5060 or 780m).
```

Controls include `--nee`, `--rr`, `--bounces`, `--rr-start`, `--workgroup 32|64|128`,
`--readback-every-frame`, `--synchronize`, `--specialize-nrc`, `--shadow-any-hit`, `--nrc-batch`,
`--nrc-depth`, `--nrc-rate`, `--nrc-train` and `--nrc-query`. Each boolean takes `true` or `false`.
Defaults retain nearest-hit shadow queries because the tested any-hit variant did not improve
M5 timing. Images have PNG previews, little-endian float32 linear RGBA sidecars and JSON reports.
See [measured results](../bench/path-tracing-nrc.md).

## Backends (Pioneer, 2026-10-09)

The tracers gate on the adapter, not on the backend: `gi::require_ray_query` accepts any adapter
that exposes wgpu's `EXPERIMENTAL_RAY_QUERY` and names the backend and adapter otherwise. wgpu 30
offers it on Metal, on Vulkan with `VK_KHR_ray_query`, and on Direct3D 12 with DXR tier 1.1 and
Shader Model 6.5 (DXC from the Windows SDK, `gpu::dx12_compiler`; FXC cannot compile it). The
browser never has it. `path_trace`, `gi_trace`, `pt_bsdf_probe` and `nrc_online_probe` take
`POCKET_BACKEND` and `POCKET_ADAPTER` like the renderer; reports name the backend. On the
Windows test machine both the RTX 5060 and the Radeon 780M expose ray queries on both backends.

`ray_query_probe` (formerly `metal_ray_query`) checks a backend before anything else: hits, misses,
masks, instance transforms, opaque traversal, shader-confirmed non-opaque candidates, and the
facing convention. A ray from the side where a triangle's vertices run counterclockwise (in the
right-handed world, as glTF winds front faces) reports a front face on all four Windows
GPU/backend pairs, which is also what the M5 images imply.

Single-sided culling in `pt_trace` no longer reads `candidate.front_face`: it compares the
triangle's winding with the ray direction, which is equal by that convention. Reading
`candidate.front_face` in the same condition as the material loads made AMD's Direct3D 12 driver
(Radeon 780M, 32.0.13062.3005) treat front faces as back faces, so every camera ray missed; the
DXIL is correct and the same shader works on Vulkan on that GPU and on both backends of the RTX
5060 ([measurements](../bench/path-tracing-nrc.md#windows-vulkan-and-direct3d-12-pioneer-2026-10-09)).
The committed hit's `front_face` (emission sidedness, entering or leaving glass) is unaffected.

Two checks keep the winding test honest, in the PT GPU test on every ray-query backend. With
`PtOptions::check_facing` (`--check-facing true`; the override constant `PT_CHECK_FACING`) `pt_trace`
compares each committed hit's `front_face` with its triangle's winding after the traversal and
counts disagreements away from grazing incidence (counter 11, `facing_mismatches`); the test turns
it on and requires zero, so a backend whose hardware convention differed, Metal included, fails it.
It changes no path but costs 2% to 3% of the trace with Vulkan and 7% to 8% with Direct3D 12 on the
RTX 5060 (gi-room and pt-lab, 320x240), so it is off by default, where it costs nothing measurable.
And the test's pt-lab render must have a known mean radiance (channel sum 0.6945 within 10%):
culling front faces instead of back faces gives 0.105, the AMD miscompile 0.262. Both were checked
by putting the defects back. The counters and sample checks alone had passed with the culling
inverted.

Two naga 30 limitations shape the shaders: its SPIR-V and HLSL writers panic (rather than report
an error) when a `ptr<function, ray_query>` is passed to another function, so queries stay local
to the function that traces; and every module gets loop bounds and a ray-query initialization
tracker by default. `PtOptions::lean_shaders` (`--lean-shaders true`), `OnlineNrc::with_shaders`
and `RayLighting::with_shaders` build the research shaders as trusted modules without those two
checks; bounds and integer-division checks stay. That is sound because every loop in the shaders
has a bound, the queries follow the tracker's rules, and the trace functions return a miss for a
nonfinite ray or an empty interval themselves, as the tracker would. It is off by default, which is
how the M5 numbers were taken; the GPU tests check that both variants trace the same paths.

The GPU tests (`ray_query_*` in `gi::pt` and `gi::rt`) run by default on every native backend of
the platform whose adapter has ray queries (Vulkan and Direct3D 12 on Windows, Metal on Apple,
Vulkan elsewhere) and skip with a note otherwise; `POCKET_ADAPTER=780m` runs them on the Radeon.
`tools/rt_bench.py` renders every configuration whose M5 report is committed and compares the
result with it, and times the documented settings per backend.

Equation references: [PBRT surface path tracing](https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer),
[PBRT dielectric BSDF](https://pbr-book.org/4ed/Reflection_Models/Dielectric_BSDF),
[Heitz GGX visible normals](https://jcgt.org/published/0007/04/01/). The code is independently authored.
