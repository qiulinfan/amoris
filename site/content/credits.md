# Media and asset credits

The showcase videos are rendered with Amoris. The model authors below created the
third-party artwork; importing and filming it does not transfer that authorship.

## Models

| Asset | Authors | License | Source detail |
| --- | --- | --- | --- |
| [Dutch Ship Medium](https://polyhaven.com/a/dutch_ship_medium) | James Ray Cock: model, textures and cleanup; Rico Cilliers: sails model and textures; Nicolò Zubbini: original model | [CC0 1.0](https://polyhaven.com/license) | 69,162 triangles, 3 materials, 9 textures at 2048 × 2048 |
| [Flight Helmet](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/edc7c9e67c639d230715049ee31f9a96a6babbbe/Models/FlightHelmet) | Public source asset; Gary Hsu: conversion from Maya | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/legalcode) | 94,722 triangles, 6 materials, 15 textures at 1024 × 1024 or 2048 × 2048 |
| [Amazon Lumberyard Bistro Exterior](https://developer.nvidia.com/orca/amazon-lumberyard-bistro) | Amazon Lumberyard | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Author reports 2,799,704 unique triangles and 2,832,120 triangles including instances |

Source assets were acquired on **5 October 2026**. The machine-readable provenance
record distributed with the site preserves exact source URLs, acquisition times,
licenses and SHA-256 hashes. Flight Helmet is pinned to Khronos commit
`edc7c9e67c639d230715049ee31f9a96a6babbbe`.

## Capture and conversion scope

The ship is a presentation model for the existing deterministic sailing simulation.
Its appearance follows the boat's transform; it does not supply the game's physics
or rules. The helmet is used for material close-ups and repeatable instance loads.

The renderer currently stores imported textures in **1024-pixel array layers**.
The source texture sizes above describe the acquired files. The helmet's optional
`KHR_materials_transmission` extension is unsupported: lenses follow the opaque
material path, so these captures do not demonstrate glass refraction.

Bistro was converted from its audited FBX exterior without merging objects or reducing
geometry. Clean GLB import retained **1,296 mesh objects and 2,829,226 triangles**
(the source webpage reports a slightly different count). Its **132 materials** use
**405 embedded PNG images**. Author-provided `_Specular` maps already encode occlusion,
roughness and metalness in R/G/B. These maps preserve those channels; DirectX normal
maps had only their green channel inverted for glTF. Image payload hashes and geometry
counts were checked after export. Source images retain their full size in the GLB;
the renderer's 1K array conversion happens at runtime.

The videos verify the pictured camera route in the real native renderer. This remains
a prototype material adaptation: transparent glass follows the renderer's opaque path,
and baked AO is not sampled. These captures do not demonstrate complete glTF material
extension support or a full scene-wide asset certification.

Source geometry counts describe the assets. Frame triangle counts, LOD selection,
culling and timing belong to the recorded rendering workload.

## Bistro attribution

**Amazon Lumberyard Bistro, Open Research Content Archive (ORCA)** — Amazon
Lumberyard, July 2017. Distributed under
[Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/).
Original scene and requested citation:
[ORCA asset page](https://developer.nvidia.com/orca/amazon-lumberyard-bistro).
Conversion, texture handling and rendering changes are described above and in the
derivative capture record.
