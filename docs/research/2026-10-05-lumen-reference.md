# UE 5.8.2 Lumen reference for Metal GI

Date: 2026-10-05. Amoris branch: `feature/metal`.
Status: source-led comparison; no implementation selected or imported.

## Evidence scope

- Reference C++ checkout: `~/Reference/UnrealEngine`, commit
  `16d75d84714512edfb744e1fd0a59e9c74d57873` (`5.8.2 release`).
- Shader inspection: the local Epic installation's `Engine/Shaders/Private/Lumen`.
  Its `Engine/Build/Build.version` reports UE 5.8.2, changelist 56702186. Matching version
  labels do not establish byte identity between these shaders and the reference commit.
- Current Epic documentation was read directly. Search snippets for macOS still report an
  obsolete unsupported HWRT status; the current page lists experimental M2+ support.
- No UE project was launched, no UE/Amoris GI benchmark was run, and no Epic source was copied.

Paths below are relative to the reference checkout unless marked `Shaders:`.

## Standard Lumen: three separate responsibilities

| Layer | Stored or evaluated information | Why it exists |
|---|---|---|
| Surface Cache | Mesh-card material properties and cached surface lighting | Cheap shading at secondary ray hits; progressively reuse lighting across bounces |
| Screen Probe Gather | Near-field incident illumination sampled from visible surfaces | Resolve local geometry and directionality, then integrate/filter into pixels |
| World Space Radiance Cache | Directional radiance in camera-relative world clipmaps | Reuse distant lighting and limit repeated long-range tracing |

This differs from conventional DDGI's main reconstruction path: visibility-weighted interpolation
of world-space irradiance probes. Standard Lumen uses screen probes for final gather; its world
cache supplies directional radiance along longer traces. The screen-probe cache configuration sets
`CalculateIrradiance=0` (`LumenScreenProbeGather.cpp:730`).

### Frame and tracing flow

1. Update Lumen scene geometry/cards and captured material data.
2. Select surface-cache pages for direct lighting and radiosity updates; combine their results
   into cached surface radiance.
3. Place uniform and adaptive screen probes, generate sampling directions, and mark needed
   world-space radiance probes.
4. Trace on-screen geometry first. Compact remaining work and trace triangles with HWRT or the
   distance-field scene with SWRT; distant lighting can use the world radiance cache.
5. Filter probe results, integrate diffuse/rough-specular illumination to pixels, and accumulate
   temporal history. Smooth reflections can use a separate tracing path.

Relevant entries under `Engine/Source/Runtime/Renderer/Private/`:

- `DeferredShadingRenderer.cpp:2894,3041`: scene update and lighting scheduling.
- `Lumen/LumenSceneRendering.cpp:2854`: mesh-card capture.
- `Lumen/LumenSceneLighting.cpp:279,583,701`: lighting update and budget/priority scheduling.
- `Lumen/LumenRadiosity.cpp:709,1109`: card-probe tracing and radiosity update.
- `Lumen/LumenScreenProbeGather.cpp:2169,2486,2590,2622,2647,2661,2711`: final gather and cache use.
- `Lumen/LumenScreenProbeTracing.cpp:789,795,877`: screen/world tracing branches.
- `Lumen/LumenRadianceCache.cpp:125,2071`: cache configuration and update scheduling.
- `Shaders:LumenSceneLighting.usf:477`: combination of cached lighting and captured materials.

### The important optimization is update scheduling

Surface lighting and world probes have independent budgets. Surface-cache updates choose pages
by priority and a texel budget derived from an update factor. Radiance-cache updates track use,
trace age, clipmap level and trace cost, and retain results across frames. These mechanisms trade
response time against GPU work; they do not calculate every cache entry every frame.

The source's default High and Epic presets provide a concrete example, not an Amoris budget:

| Setting | High | Epic |
|---|---:|---:|
| Final gather | Screen Probe Gather | Screen Probe Gather |
| Screen-probe spacing, pixels | 32 | 16 |
| Base screen-probe direction resolution | 8 x 8 | 8 x 8 |
| World-cache probe direction resolution | 16 x 16 | 32 x 32 |
| Base world-probe update budget | 100 | 150 |
| Direct-lighting update factor | 64 | 32 |
| Radiosity update factor | 128 | 64 |
| Mesh-SDF detail tracing allowed | No | Yes |

Source: `Engine/Config/BaseScalability.ini:375,422`. Actual world-cache budgets also scale with
the lighting update speed and editor state (`LumenScreenProbeGather.cpp:741`).

## Lumen Lite: the more direct probe reference

UE 5.8 introduces a Medium-quality route using Irradiance Field Gather. It reuses the world-space
radiance-cache infrastructure, calculates irradiance and occlusion for probes, then interpolates
them to visible pixels. The source labels this faster and lower quality than Screen Probe Gather.

- `Lumen.cpp:55`: selects the two final-gather methods.
- `Engine/Config/BaseScalability.ini:326`: Medium selects `r.Lumen.FinalGatherMethod=0` and reduces
  Surface Cache size/update work; it also disables skeletal-mesh tracing in that preset.
- `LumenIrradianceFieldGather.cpp:168`: uses clipmaps, irradiance/occlusion probe layouts and a
  per-frame trace budget; explicitly sets `CalculateIrradiance=1`.
- `LumenIrradianceFieldGather.cpp:352,557,604`: marks probes needed by visible geometry, updates
  the shared cache, then interpolates to diffuse and rough-specular outputs.
- `Shaders:LumenIrradianceFieldInterpolation.ush:88`: uses distance moments and visibility
  weighting; probe validity participates in interpolation.

This route has an irradiance-field reconstruction structure related to DDGI, while retaining
Lumen's scene tracing and cache infrastructure. It is not a standalone drop-in DDGI library.
Its downsampled interpolation relies on temporal image reconstruction. Medium uses SSR for smooth
reflections and probe-derived rough specular. Moving/invalid probes and view-dependent relocation
remain important quality cases to evaluate.

## Metal support: use current source and page content

The current Epic macOS table lists Lumen software tracing on M1+ and hardware tracing/MegaLights
as experimental on M2+. The local 5.8.2 source enables Lumen and RT capabilities for Metal SM6;
Metal SM5 has Lumen/distance-field support but its RT flags are disabled.

- `Engine/Config/Mac/DataDrivenPlatformInfo.ini:54,95`: platform capability differences.
- `Engine/Source/Runtime/Apple/MetalRHI/Private/MetalDevice.cpp:373`: device ray-tracing capability.
- `Engine/Source/Runtime/Apple/MetalRHI/Private/MetalRHI.cpp:1120`: runtime RT feature setup.
- `Engine/Source/Runtime/Apple/MetalRHI/Private/MetalRHIPrivate.h:48`: Shader Converter compile gate.
- `Lumen/LumenHardwareRayTracingCommon.cpp:174`: project, CVar and view eligibility.

The previously verified M5 Metal capability is consistent with this route. It does not demonstrate
that a particular UE project has HWRT enabled or establish performance on this machine.

The UE support category also differs from dedicated hardware acceleration: M2 supports Metal
ray-tracing APIs, while dedicated hardware-accelerated ray tracing arrived on Mac with M3.
An engine's HWRT-path support threshold does not by itself identify the chip's fixed-function
ray-tracing hardware. See [Apple's M3 announcement](https://www.apple.com/newsroom/2023/10/apple-unveils-m3-m3-pro-and-m3-max-the-most-advanced-chips-for-a-personal-computer/).

## Implications for Amoris

These are engineering inferences from the inspected implementations:

1. Add the **Lumen Lite / irradiance-field approach** to the world-space probe shortlist beside
   DDGI. Compare probe visibility, relocation, update priority, memory and light-change response.
2. Separate scene tracing, ray-hit lighting, probe/cache update and final reconstruction. Each
   needs its own budget and measured cost.
3. Evaluate simple hit shading first for Amoris's bounded material model. Add a surface-lighting
   cache when measurements establish that hit shading or repeated bounce work warrants it.
4. Standard UE Lumen excludes Forward Shading (`RenderUtils.cpp:2622`) and relies on GBuffer and
   temporal data. Amoris's forward renderer currently has HDR plus 4x MSAA depth, and discards
   depth when splats do not need it. Screen-gather integration requires retained/readable depth,
   surface information, motion/history handling and explicit sample/edge reconstruction.
   Probe-based forward integration is a separate design choice, not an unchanged UE pipeline.
5. Evaluate GI separately from smooth reflections. A successful diffuse-probe implementation
   does not by itself supply accurate mirror reflections.
6. Keep performance comparisons at a fixed internal resolution and reconstruction quality.
   Epic's console budgets are not predictions for M5 or Amoris.

No GI method has been selected; this adds a source-backed UE reference to the comparison.

## Official references

- [Lumen technical details](https://dev.epicgames.com/documentation/unreal-engine/lumen-technical-details-in-unreal-engine).
- [Lumen performance guide, including Lite](https://dev.epicgames.com/documentation/unreal-engine/lumen-performance-guide-for-unreal-engine?lang=en-US).
- [UE 5.8 release notes](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-8-release-notes?lang=en-US).
- [Current macOS requirements](https://dev.epicgames.com/documentation/unreal-engine/macos-development-requirements-for-unreal-engine?lang=en-US).
