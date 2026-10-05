# Metal rendering and global illumination options

Date: 2026-10-05. Branch: `feature/metal`. Inspected engine commit: `a4141946`.
Status: source-led comparison, followed by the accepted implementation order in `../spec/metal-gi.md`.

## Verified starting point

- `forward.wgsl` evaluates the sun, clustered punctual lights, sky irradiance SH, prefiltered
  sky reflections and surface emission. Scene geometry does not contribute indirect illumination.
- `post.rs` uses 4x MSAA HDR color and depth. There are no normal or motion-vector targets in
  `Targets`. The opaque pass discards depth unless splats need it (`renderer.rs`).
- `GpuProfiler` has render/compute timestamps and three asynchronous readback slots. Postprocessing
  is not instrumented. Its latest results do not identify the sampled frame or missed samples.
- `showcase_bench` provides a static, warmed-up render-to-idle measurement, excluding simulation
  and presentation. Its pass sum excludes postprocessing; Metal pass boundaries can omit tile work.
  A GI benchmark also needs moving cameras, changing lights and moving occluders.
- A direct Metal capability query on this machine returned `Apple M5`,
  `supportsRaytracing = true`, and `supportsRaytracingFromRender = true`.

## Ray-query access

The installed, pinned wgpu 30.0.1 implementation has Metal acceleration-structure creation,
build/refit, shader bindings and Naga MSL ray-query translation. Its Metal adapter exposes
`EXPERIMENTAL_RAY_QUERY` when the OS and both device ray-tracing capabilities satisfy its checks.
The platform list in the feature documentation still mentions only Vulkan; it is inconsistent
with this implementation.

Amoris does not request ray queries or opt in to experimental features. Hardware/API capability
checks and source inspection do not prove an end-to-end wgpu ray-query dispatch works. Before
choosing a ray-based GI implementation, validate a triangle BLAS, instance TLAS and compute
hit/miss readback. Standalone ray-tracing pipelines are not implemented in this Metal backend;
compute ray queries provide the candidate integration path.

Local source evidence under the Cargo registry's `index.crates.io-1949cf8c6b5b557f/`:

- `wgpu-hal-30.0.1/src/metal/adapter.rs`: capability checks at 1157 and feature exposure at 1326.
- `wgpu-hal-30.0.1/src/metal/device.rs`: AS creation at 2076; RT pipeline limitation at 1885.
- `wgpu-hal-30.0.1/src/metal/command.rs`: AS build/refit at 1836.
- `naga-30.0.1/src/back/msl/ray.rs`: ray-query translation at 198.

## Comparison

Integration effort below is an engineering assessment of the current code, not a performance
measurement. GPU cost depends on the scene, resolution, sampling budget and reconstruction.

| Method | What it offers | Main limitation | Missing Amoris work |
|---|---|---|---|
| Baked lightmaps/probes | Precomputed indirect light with inexpensive runtime lookup | Fixed lighting/geometry assumptions; asset preparation | Baking/import path and lighting data |
| SSGI | Dynamic indirect detail from the visible screen | Missing off-screen/occluded information; view-dependent artifacts | Retained depth, surface data, tracing and reconstruction |
| DDGI | World-space, visibility-aware probes for dynamic diffuse indirect light | Spatial sampling and temporal response require tuning; glossy reflection is a separate problem | Ray tracing/shading, probe storage/update, visibility moments and forward-pass sampling |
| Lumen Lite / irradiance-field approach | Budgeted world probes, occlusion-aware interpolation and shared radiance caching | UE's implementation retains its scene/cache infrastructure and temporal reconstruction | A scoped probe/cache design adapted to Amoris; see the [UE source review](2026-10-05-lumen-reference.md) |
| Split Radiance Cascades | Sparse world-space probes and distance-based ray splitting for detailed diffuse GI | A newer candidate with substantial implementation and validation work | Sparse allocation, cascade representation, ray splitting and reconstruction |
| Voxel cone tracing | Dynamic diffuse and approximate glossy indirect light through a voxel hierarchy | Representation/filtering trades detail against memory and update work | Voxelization, radiance injection, hierarchy updates and cone sampling |
| Surface ray-traced GI | Trace indirect illumination at visible surfaces rather than reconstructing it from a probe grid | Sampling noise and ray work require reconstruction and a controlled budget | Acceleration structures, hit shading, surface/temporal data and denoising |
| SDF-based GI | Trace a distance-field scene and reuse its lighting representation | Geometry/update restrictions depend on implementation; Godot SDFGI excludes dynamic occluders and emissive surfaces | Distance-field generation/update, lighting integration and reconstruction |
| SHaRC | Spatially hashed, non-neural radiance caching to shorten path-traced transport | Cache scale, confidence and accumulated history trade detail/response against stability | A ray/path tracer, cache population/query/resolve and shader adaptation |
| NRC / NIRC | Learned radiance predictions reduce path-tracing work; residual sampling can correct prediction bias | Online training/inference and dynamic adaptation; available vendor/research implementations are not Metal integrations | A tracer and a Metal-compatible training/inference path, plus bias/variance validation |
| ReSTIR GI / PT | Reservoir-based spatiotemporal reuse increases the effective contribution of scarce path samples | Reconnection, visibility, history validity and correlation are difficult | Ray/path sampling, reservoirs, reuse validation and reconstruction |

Metal ray tracing is the intersection infrastructure. Probe grids, cascades and surface sampling
determine where illumination is estimated and how it is reconstructed. MetalFX denoising is
another reconstruction option with its own native integration requirements; it is not assumed
available through the current wgpu renderer.

## Interpreting state of the art

The rows above describe different layers. Geometry intersection can use Metal ray queries or an
SDF representation. Probes, voxels and radiance caches approximate or reuse light transport.
ReSTIR improves sample generation/reuse, while denoising reconstructs an image from noisy samples.
Several techniques can therefore be combined in one renderer.

There is no established universal winner across all GI workloads. A useful state-of-the-art claim
must specify hardware, internal resolution, total budget, materials, scene dynamics, supported
transport and the error/temporal metrics. Equal-time error comparisons within a paper do not
predict performance on M5. Converged high-sample path tracing is a useful reference image, not a
guarantee that a real-time path tracer is the best implementation for every game.

Representative research directions verified as of 2026-10-05:

- **ReSTIR PT Enhanced (2026):** improvements to reuse cost, reconnection robustness and sample
  correlation. It is an important real-time path-tracing reference, not a measured Amoris result.
- **ReSTIR PG (2025):** guides new candidates using resampled paths from prior frames.
- **ReSTIR BDPT (2025):** improves sampling for caustics and difficult light paths. Its reported
  test workload is approximately 50 ms per frame; that is not a general 60 FPS guarantee.
- **Split Radiance Cascades (2026):** a recent sparse world-space diffuse-GI representation.
- **NRC / NIRC:** ongoing radiance-learning research. The NRC idea supports dynamic scenes,
  but its current NVIDIA library uses D3D12/Vulkan and Tensor Cores. NIRC's residual correction
  addresses bias but does not eliminate single-frame noise or remove the need for ray tracing.

SHaRC is especially worth comparing for a non-neural Metal path: its official distribution is
shader-only and not NVIDIA-hardware-specific. Portability of the technique is not a ready-made
wgpu/Metal integration. Its accumulation settings explicitly trade response speed for stability.

For Amoris, the practical validation groups are visibility-aware diffuse probes (DDGI and the
Lumen Lite approach), ray-traced transport with a non-neural radiance cache (SHaRC), and
surface/path sampling with ReSTIR reuse. Split Radiance Cascades remains a research comparison;
SSGI can complement world-space GI. These are engineering priorities, not a selected algorithm
or a measured ranking. Neural caching has an additional runtime implementation burden on Metal.

## NRC hardware feasibility: later-stage candidate

The owner initially deferred NRC on 2026-10-05, then authorized a small-network experiment after
baking, SHaRC and ReSTIR exploration. Online dynamic NRC remains distinct from that offline experiment.

There are two distinct implementation choices:

| Choice | Hardware/software conditions | Relevance to this machine |
|---|---|---|
| NVIDIA's supplied NRC library | NVIDIA Turing-or-newer GPU with Tensor Cores; D3D12/Vulkan interfaces and CUDA-backed library distribution | The supplied library is not a Metal/M5 integration |
| A Metal implementation of the NRC technique | Ray/path tracing for fresh training samples, online neural training/inference, and GPU resource synchronization | M5 supplies ray-tracing hardware and per-GPU-core Neural Accelerators; implementation and real-time cost remain unverified |

The machine's unified-memory capacity was queried directly: 34,359,738,368 bytes (32 GiB).
This is a platform inventory, not a guarantee that a chosen scene and NRC budget will fit or
meet a frame-time target.

NVIDIA's guide includes a buffer-allocation example for 1920x1080, 1 SPP, eight maximum bounces
and a 274x154 training resolution. The listed buffers total roughly 0.2 GB. This is not a minimum
GPU-memory specification or the total renderer memory requirement: scene assets, acceleration
structures, other render targets and implementation-specific working storage also need accounting.

Apple's Metal 4 irradiance-training sample is a useful future reference. It trains a small MLP
with MPP cooperative matrix operations on the GPU and has a separate pre-trained Core ML inference
path. The sample learns HDR-environment diffuse irradiance, not a full dynamic scene's NRC.
MPSGraph also provides GPU training APIs. GPU Neural Accelerators and the separate Apple Neural
Engine must not be conflated; an inference API does not establish a working per-frame training loop.

Future feasibility should measure sampling, network training, network queries/resolve and render
interoperation together. A useful outcome is the quality/work saved at a fixed total GPU budget,
not neural-network throughput in isolation. No fixed M5 minimum specification or NRC frame time
has been established.

## Initial shortlist and decision evidence

DDGI and UE 5.8's irradiance-field approach are the world-space diffuse references. Split Radiance Cascades is a useful
research comparison for spatial detail. Surface ray-traced GI is the direct-sampling alternative.
SSGI is useful as a screen-space baseline or complement. Keep baked GI and voxel cone tracing
in the comparison when static content or a ray-query-independent implementation matters.

Before making a selection:

1. Verify the minimal Metal/wgpu ray-query path and establish complete render timing coverage.
2. Use the same enclosed color-bleeding scene, indoor/outdoor transition and real asset scene.
3. Compare thin-wall leakage, off-screen emission, small geometry, light-change response and
   camera/occluder motion. Separate diffuse GI from specular reflection quality.
4. Record added whole-frame cost, CPU encoding, AS build/refit, memory, steady-state quality
   and transient behavior. Keep workload and quality settings identical where meaningful.

The comparison above predates the implementation measurements. Current bounded results and
limitations are recorded in `../bench/metal-gi.md`; they do not establish a general method ranking.

## Primary references

- [DDGI paper and reference material](https://jcgt.org/published/0008/02/01/).
- [Split Radiance Cascades, July 2026](https://arxiv.org/abs/2607.20384).
  The abstract was inspected; the full PDF exceeded the browser's fetch limit.
- [Voxel cone tracing paper](https://research.nvidia.com/labs/rtr/publication/crassin2011givoxels/).
- [Epic's SSGI documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/screen-space-global-illumination?application_version=4.27).
- [Metal acceleration structures and ray tracing](https://developer.apple.com/documentation/metal/ray-tracing-with-acceleration-structures).
- [Apple's Metal ray-tracing performance guidance](https://developer.apple.com/videos/play/wwdc2022/10105/).
- [Metal neural rendering and denoising](https://developer.apple.com/videos/play/wwdc2026/359/).
- [wgpu 30.0.1 feature documentation](https://docs.rs/wgpu/30.0.1/wgpu/struct.Features.html#associatedconstant.EXPERIMENTAL_RAY_QUERY).
- [Production DDGI extensions](https://jcgt.org/published/0010/02/01/).
- [Godot's current SDFGI limitations](https://docs.godotengine.org/en/stable/tutorials/3d/global_illumination/using_sdfgi.html).
- [SHaRC integration guide](https://github.com/NVIDIA-RTX/SHARC/blob/main/docs/Integration.md).
- [NRC integration and platform requirements](https://github.com/NVIDIA-RTX/RTXGI/blob/main/Docs/NrcGuide.md).
- [NIRC / Two-Level Monte Carlo author implementation](https://github.com/Mishok43/IVD_NIRC).
- [RTXDI's distinction between ReSTIR DI, GI, PT and denoising](https://github.com/NVIDIA-RTX/RTXDI).
- [ReSTIR PT Enhanced (2026)](https://research.nvidia.com/labs/rtr/publication/lin2026restirptenhanced/).
- [ReSTIR PG (2025)](https://research.nvidia.com/labs/rtr/publication/zeng2025restirpg/).
- [ReSTIR BDPT (2025)](https://research.nvidia.com/labs/rtr/publication/hedstrom2025restir/).
- [NVIDIA NRC GPU prerequisites](https://github.com/NVIDIA-RTX/RTXGI/blob/main/Readme.md).
- [Apple M5 ray tracing and GPU Neural Accelerators](https://www.apple.com/newsroom/2025/10/apple-unleashes-m5-the-next-big-leap-in-ai-performance-for-apple-silicon/).
- [Metal GPU training of an irradiance MLP](https://developer.apple.com/documentation/metal/training-a-neural-network-to-render-irradiance-in-real-time).
- [MPSGraph GPU-training example](https://developer.apple.com/documentation/MetalPerformanceShadersGraph/training-a-neural-network-using-mps-graph).
- [MPP tensor operations on M5](https://developer.apple.com/download/files/Metal-Performance-Primitives-Programming-Guide.pdf).
