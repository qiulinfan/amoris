# TAA and GTAO: quality, cost and defaults

The final Windows measurements keep MSAA with GTAO on discrete GPUs and TAA without GTAO on
integrated GPUs. TAA improves stationary shading and alpha-tested edges; MSAA preserves thin
geometry every frame and performs better under camera motion. Costs and quality both matter to
the defaults.

Design and API: [TAA and GTAO](../spec/taa-gtao.md). Conditions: [Windows measurement
session](quiet-2026-10-09.md). Historical source: [Pioneer measurement
document](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/quiet/aa).

## Method

`demo::aa_scene` combines a minified 6 cm checker, sub-pixel fence and wires, an alpha-tested
chain-link panel, low-roughness chrome spheres, a dark silhouette, contact geometry, a cube
moving at 2.4 m/s and a spinning torus. It uses cascaded sun shadows and sky light without bloom.
The native and browser viewport share this scene (`?demo=aa`).

Quality uses RGB PSNR after exposure and AgX, against the mean of 256 one-sample frames with
Halton jitter covering the pixel, holding the camera and scene at the evaluated moment. This
reference is a box-filtered supersampled image of the same renderer, not a separate renderer.
“Temporal” compares frame-to-frame changes with the reference's changes; “cube” measures the
moving cube, shadow and their recent positions. Quality was evaluated on the RTX 5060 with Vulkan
at 960×540; Radeon 780M D3D12 checks showed the same ordering, with different absolute values.

TAA uses camera and object motion, last-frame skinned vertices, depth-based disocclusion,
variance clipping and a 16-frame jitter cycle. GTAO runs at half resolution, darkens only indirect
light and reconstructs normals from depth. Native modes are `off`, `msaa` (4×), `taa` (one sample)
and `msaa+taa`; `POCKET_AA` and `POCKET_GTAO` override defaults. Browser overrides are `?aa=` and
`?gtao=`. Sharpening remains off.

## Quality comparisons

Still camera and scene, PSNR in dB after each frame count:

| Option | 1 | 2 | 4 | 8 | 16 | 32 | 64 |
|---|---:|---:|---:|---:|---:|---:|---:|
| off (1 sample) | 33.42 | 33.42 | 33.42 | 33.42 | 33.42 | 33.42 | 33.42 |
| msaa (4 samples) | 34.34 | 34.34 | 34.34 | 34.34 | 34.34 | 34.34 | 34.34 |
| taa | 29.69 | 35.23 | 40.21 | 38.36 | 39.13 | 39.03 | 39.13 |
| msaa+taa | 30.07 | 35.48 | 40.66 | 38.64 | 39.43 | 39.32 | 39.44 |

At frame 64, TAA gains 4.8 dB over MSAA for the full image, 6.7 dB on highlights and 7.4 dB on
alpha-tested edges. It begins below MSAA because its first frame is a single jittered sample;
variance clipping prevents it from reaching the plain jittered mean's 42.7 dB.

| Region | off | msaa | taa | msaa+taa |
|---|---:|---:|---:|---:|
| the alpha-tested chain link | 25.64 | 26.48 | 33.91 | 34.36 |
| the chrome spheres | 32.86 | 34.70 | 41.43 | 41.00 |
| the fence and wires | 32.06 | 46.36 | 39.61 | 44.05 |

MSAA sees sub-pixel wires every frame; one-sample TAA can make them faint or dashed. Combining
both improves the wires while retaining TAA's shading benefit, at a higher cost.

Camera panning at 0.6 m/s while turning, with the cube and torus moving. Cells are whole-image /
cube-region / temporal PSNR, dB:

| Panning camera | frame 30 | frame 60 | frame 90 |
|---|---|---|---|
| off | 33.47 / 33.01 / 36.00 | 33.99 / 33.90 / 36.22 | 34.16 / 32.27 / 36.72 |
| msaa | 34.51 / 35.50 / 37.09 | 35.27 / 35.97 / 37.51 | 35.41 / 33.17 / 38.08 |
| taa | 34.32 / 30.35 / 36.01 | 34.61 / 34.34 / 35.97 | 33.80 / 31.25 / 36.26 |
| msaa+taa | 34.39 / 30.38 / 36.02 | 34.71 / 34.43 / 35.98 | 33.88 / 31.18 / 36.10 |

With a stationary camera and the same moving objects:

| Still camera | frame 30 | frame 60 | frame 90 |
|---|---|---|---|
| off | 33.51 / 32.87 / 46.26 | 33.82 / 33.39 / 44.86 | 34.22 / 32.18 / 44.81 |
| msaa | 34.52 / 34.98 / 52.30 | 34.98 / 35.17 / 50.86 | 35.65 / 34.26 / 51.32 |
| taa | 37.95 / 32.34 / 41.32 | 38.12 / 35.56 / 41.66 | 36.65 / 33.33 / 41.47 |
| msaa+taa | 38.23 / 32.55 / 41.54 | 38.47 / 35.74 / 41.78 | 36.75 / 32.97 / 40.91 |

Under the pan MSAA leads by 0.2–1.6 dB on the image and 1.1–1.8 dB on temporal changes.
With a still camera TAA leads over the image by 1.0–3.4 dB, but its moving-cube region is not
consistently better. Object motion improves the cube region by 0.9–3.5 dB over camera-only
reprojection. Skinned motion improves the animated hero region by 0.7 dB. Sharpening at 0.25
recovers about 0.5 dB in motion but loses about 2 dB when still, so it stays off.

GTAO with MSAA darkens the image by 0.8%; 2.5% of pixels lose over 5% luminance and none brighten.
Depth-derived and target-derived normals differ by 45.3 dB PSNR, with no visible loss in this
scene. The extra normal target costs 0.03–0.08 ms on the RTX 5060 and 0.5–2.6 ms on the 780M
in the multisampled opaque pass in the earlier quiet measurement; reconstruction remains default.

## Final native cost — 2026-10-10

Windows 11 Pro 10.0.26200, Ryzen 9 270, 15 GB RAM, RTX 5060 Laptop (driver 32.0.16.1714,
Vulkan 617.14) and Radeon 780M (32.0.13062.3005); release build with wgpu 30.0.1. On AC power,
no other agent running, with desktop GUI load. The final build includes the forward-shader changes
that avoid unused motion and normal outputs. These figures replace earlier native cost tables.

Three rounds on both adapters and APIs: medians of the frame's median timestamped GPU time with
GTAO off, ms. Postprocessing (bloom and display transform) is outside the timestamped total.
GTAO is the added pass with MSAA, using the rounds' 10th-percentile pass times for the check scene
and means for many_cubes. The check scene is shading-bound; many_cubes has 1.6M cubes at
1600×900 and is geometry-bound. Of 112 measured cells, 95 agree within 3% across rounds; the
rest within 4–18%.

| GPU, API | Workload | msaa | taa | TAA - MSAA | GTAO pass |
|---|---|---:|---:|---:|---:|
| RTX 5060, D3D12 | check scene 1600x900 | 0.33 | 0.47 | +0.15 | 0.14 |
| RTX 5060, D3D12 | check scene 2560x1440 | 0.66 | 1.07 | +0.40 | 0.39 |
| RTX 5060, D3D12 | many_cubes 1.6M | 2.52 | 2.41 | -0.11 | 0.15 |
| RTX 5060, Vulkan | check scene 1600x900 | 0.29 | 0.38 | +0.09 | 0.14 |
| RTX 5060, Vulkan | check scene 2560x1440 | 0.62 | 0.87 | +0.25 | 0.38 |
| RTX 5060, Vulkan | many_cubes 1.6M | 2.51 | 2.36 | -0.15 | 0.17 |
| 780M, D3D12 | check scene 1600x900 | 2.56 | 2.48 | -0.08 | 0.51 |
| 780M, D3D12 | check scene 2560x1440 | 4.89 | 4.73 | -0.16 | 1.32 |
| 780M, D3D12 | many_cubes 1.6M | 8.20 | 5.96 | -2.24 | 0.82 |
| 780M, Vulkan | check scene 1600x900 | 3.54 | 2.09 | -1.45 | 0.49 |
| 780M, Vulkan | check scene 2560x1440 | 7.49 | 3.72 | -3.77 | 1.23 |
| 780M, Vulkan | many_cubes 1.6M | 10.29 | 5.52 | -4.77 | 0.97 |

The RTX 5060's TAA difference is −0.15 to +0.40 ms against MSAA, so the mode better under motion
stays default. GTAO adds 0.14 ms at 1600×900 and 0.15–0.17 ms on many_cubes. On the 780M TAA
is 0.08–4.77 ms cheaper; D3D12 is almost tied on the light scene but TAA saves 2.24 ms on
many_cubes. GTAO adds 0.49–0.51 ms on the 900p check scene and 0.82–0.97 ms on many_cubes.

## Defaults and limits

| Adapter | Anti-aliasing | GTAO | Basis |
|---|---|---|---|
| Discrete, Vulkan or D3D12 | `msaa` | depth normals, on | Close AA cost; better thin geometry and motion |
| Integrated, Vulkan or D3D12 | `taa` | off | Lower measured GPU cost; GTAO is relatively expensive |
| Metal, Apple GPUs through MoltenVK, browser and other adapters | `msaa` | off | Conservative unmeasured choice |

Apple results were not measured here; browser defaults do not identify adapter kind. Earlier
Chrome 155 RTX 5060 readings at 1574×744 were off 0.23, MSAA 0.22, TAA 0.36 and MSAA+TAA
0.38 ms, with a 0.13–0.14 ms TAA pass and 0.09–0.12 ms GTAO. They were not repeated in the
final quiet session. The 780M yielded no usable browser timestamps. Both effects use WebGPU
core features.

- One-sample TAA loses some sub-pixel geometry. No thin-feature lock, reactive particle mask or
  temporal upscaling is implemented; surfaces without motion vectors rely on clipping.
- GTAO has no measured ray-traced ground truth. Grazing flat walls lose about 2% luminance;
  combining GTAO with TAA introduces an unexplained additional dark bias of about 0.5%.
- Splats draw after TAA and stay outside its history. Their unjittered depth prevents mesh-edge
  flicker but does not anti-alias those boundaries. An earlier single-round 1.6M-cube measurement
  put this depth redraw at 0.78–0.89 ms, versus 0.07–0.33 ms for a copy; it was not re-measured
  in the final session.
- Still-image capture accumulates a full 16-frame jitter cycle after reset; one fresh TAA frame
  is not representative. Option changes, resize and camera cuts reset history.

Use `aa_eval` for the supersampled-reference comparison and `tools/aa_bench.py` for the native
cost matrix; the [specification](../spec/taa-gtao.md) describes switches, history and capture rules.
