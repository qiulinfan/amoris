# Surface path tracing and online NRC

Status: implemented and measured on Apple M5, 2026-10-05, `feature/metal`.

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
a separate Metal research renderer, not a replacement of the interactive forward/WebGPU renderer.
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
```

Controls include `--nee`, `--rr`, `--bounces`, `--rr-start`, `--workgroup 32|64|128`,
`--readback-every-frame`, `--synchronize`, `--specialize-nrc`, `--shadow-any-hit`, `--nrc-batch`,
`--nrc-depth`, `--nrc-rate`, `--nrc-train` and `--nrc-query`. Each boolean takes `true` or `false`.
Defaults retain nearest-hit shadow queries because the tested any-hit variant did not improve
M5 timing. Images have PNG previews, little-endian float32 linear RGBA sidecars and JSON reports.
See [measured results](../bench/path-tracing-nrc.md).

Equation references: [PBRT surface path tracing](https://pbr-book.org/4ed/Light_Transport_I_Surface_Reflection/A_Better_Path_Tracer),
[PBRT dielectric BSDF](https://pbr-book.org/4ed/Reflection_Models/Dielectric_BSDF),
[Heitz GGX visible normals](https://jcgt.org/published/0007/04/01/). The code is independently authored.
