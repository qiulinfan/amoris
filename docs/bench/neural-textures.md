# Neural texture compression: size, quality and decode cost

Neural textures are opt-in `.ntex` materials. On the tested 1024² materials they store 8–12 channels
and the full mip chain in 5.37 bits per mip-0 texel. This trades texture memory for per-pixel
network evaluation: at 2560×1440 the final full-screen f16 decode adds about 3 ms on the RTX 5060
and 5.8–8.2 ms on the Radeon 780M. Ordinary materials keep their existing path.

Design and API: [neural textures](../spec/neural-textures.md). Conditions: [Windows measurement
session](quiet-2026-10-09.md). Historical source: [Pioneer measurement
document](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/quiet/neural-final).

## Method and current choices

`neural_encode` trains the material channels together with GPU Adam. The default has two latent
grids per pair of mips, 8 eight-bit features per grid, bilinear sampling, two positional-encoding
octaves and a 26-32-32-C network. The fine grid is one quarter of mip `2l`'s resolution. Training
uses 10,000 steps of 65,536 samples, seed 1, learning rates 0.005 (network) and 0.03 (latents),
with 75% of samples at texel centres. A seed reproduces a file on the same device and backend;
cross-device or cross-backend reproducibility is not established.

At runtime the network is generated unrolled. Mip transitions blend two decodes only within a
quarter-level band; other pixels evaluate once. `shader-f16` selects f16 where available, with
f32 otherwise. `POCKET_NEURAL_PRECISION=f32` or
`Renderer::set_neural_half_precision(false)` forces f32. The Radeon 780M is faster in f32 on
D3D12 and f16 on Vulkan, so the capability default is not a universal performance optimum.

## Size and fidelity

PSNR is in dB at texel centres against the 8-bit reference. BC high uses BC7 for color, ORM and
emissive, BC5 for normals and BC4 for height; BC low substitutes BC1 for BC7. Half res uses
BC7/BC5/BC4 at mip 1, upsampled to mip 0. Sizes include every mip and use mip-0 texel count as
the denominator; ordinary chains are about 4/3 of mip 0, neural latents about 1.07.

| Material | Size | Channels | Neural | BC high | BC low | 8-bit tight | 8-bit GPU formats |
|---|---|---|---|---|---|---|---|
| bricks | 1024 | 9 | 5.37 | 37.33 | 26.67 | 96.0 | 117.3 |
| metal | 1024 | 8 | 5.37 | 32.00 | 21.33 | 85.3 | 106.7 |
| tiles | 1024 | 12 | 5.37 | 48.00 | 32.00 | 128.0 | 160.0 |
| wood | 1024 | 9 | 5.37 | 37.33 | 26.67 | 96.0 | 117.3 |
| noise | 1024 | 12 | 5.37 | 48.00 | 32.00 | 128.0 | 160.0 |
| detail-poster | 1024 | 9 | 5.37 | 37.33 | 26.67 | 96.0 | 117.3 |
| scene-poster | 1024 | 9 | 5.37 | 37.33 | 26.67 | 96.0 | 117.3 |
| CheckerPBR (pt-lab) | 32 | 7 | 40.56 | 32.62 | 21.75 | 74.6 | 106.6 |

Bricks, metal, tiles and wood are correlated procedural PBR materials; noise stresses unrelated
high-frequency channels. The two posters are rendered images converted to materials using
luminance as height. CheckerPBR is the sample's small 32² texture: its network outweighs the
texels, so neural storage is larger than either BC baseline.

Mip-0 quality; f16 differs from f32 by at most 0.06 dB on the 1024² materials and 0.36 dB on
CheckerPBR:

| Material | Group (BC high/low) | Neural | BC high | BC low | Half res |
|---|---|---|---|---|---|
| bricks | base color (BC7/BC1) | 47.45 | 55.39 | 42.60 | 38.23 |
| bricks | normal (BC5) | 41.91 | 43.99 | 43.99 | 30.26 |
| bricks | ORM (BC7/BC1) | 45.90 | 56.69 | 47.24 | 28.50 |
| bricks | height (BC4) | 43.02 | 51.94 | 51.94 | 29.95 |
| metal | base color | 47.58 | 54.68 | 43.27 | 44.70 |
| metal | normal | 39.79 | 51.68 | 51.68 | 33.61 |
| metal | ORM | 41.54 | 48.64 | 39.31 | 37.23 |
| tiles | base color | 46.61 | 58.43 | 41.95 | 41.82 |
| tiles | normal | 40.92 | 47.83 | 47.83 | 30.93 |
| tiles | ORM | 42.07 | 53.50 | 36.63 | 32.20 |
| tiles | emissive (BC7/BC1) | 49.79 | 57.45 | 43.05 | 38.79 |
| tiles | height | 42.01 | 46.33 | 46.33 | 31.26 |
| wood | base color | 52.16 | 54.14 | 42.07 | 44.74 |
| wood | normal | 48.14 | 48.26 | 48.26 | 32.31 |
| wood | ORM | 48.05 | 50.63 | 40.27 | 33.30 |
| wood | height | 51.17 | 48.49 | 48.49 | 31.62 |
| noise | base color | 39.27 | 44.50 | 37.94 | 39.96 |
| noise | normal | 31.70 | 48.66 | 48.66 | 37.33 |
| noise | ORM | 33.84 | 42.62 | 34.07 | 31.28 |
| noise | emissive | 41.43 | 50.87 | 42.08 | 46.44 |
| noise | height | 38.62 | 49.81 | 49.81 | 43.31 |
| detail-poster | base color | 43.80 | 54.12 | 41.94 | 39.62 |
| detail-poster | normal | 38.64 | 51.56 | 51.56 | 37.91 |
| detail-poster | ORM | 41.64 | 54.95 | 45.42 | 38.98 |
| detail-poster | height | 45.04 | 51.70 | 51.70 | 39.87 |
| scene-poster | base color | 35.28 | 47.55 | 37.08 | 31.23 |
| scene-poster | normal | 30.52 | 44.15 | 44.15 | 30.45 |
| scene-poster | ORM | 33.48 | 47.40 | 38.31 | 30.73 |
| scene-poster | height | 36.65 | 43.66 | 43.66 | 31.46 |
| CheckerPBR | base color | 50.10 | 99 (exact) | 41.76 | 15.86 |
| CheckerPBR | normal | 43.72 | 48.41 | 48.41 | 40.68 |
| CheckerPBR | ORM | 46.85 | 56.19 | 32.55 | 24.34 |

The regular procedural materials reach about 40–52 dB; noise and photographic content reach
30–45 dB. Neural beats half-resolution BCn on every group of six of the seven 1024² materials;
noise loses on four of five groups. Full-resolution BC7 is usually better by 2–12 dB on base
color, and BC5 is better on every normal group. Neural base color beats BC1 on six of seven
1024² materials. These are content-dependent savings, not equivalent fidelity to full-size BCn.

Mean normal-angle error (neural / BC5 / half-resolution BC5, degrees): bricks 1.04 / 0.56 / 3.80;
metal 0.74 / 0.15 / 1.40; tiles 0.93 / 0.23 / 2.88; wood 0.55 / 0.35 / 1.49; noise
3.72 / 0.50 / 1.97; detail-poster 0.78 / 0.16 / 0.77; scene-poster 3.36 / 0.69 / 3.37.

Base-color quality by mip, neural / BC1:

| Material | mip 0 | mip 1 | mip 2 | mip 3 | mip 4 | mip 5 |
|---|---|---|---|---|---|---|
| bricks | 47.5 / 42.6 | 48.4 / 38.2 | 38.0 / 37.6 | 40.4 / 36.0 | 35.9 / 37.5 | 38.9 / 37.0 |
| metal | 47.6 / 43.3 | 48.8 / 42.4 | 40.8 / 41.0 | 44.6 / 39.2 | 36.6 / 37.3 | 40.4 / 35.8 |
| tiles | 46.6 / 42.0 | 47.8 / 42.6 | 38.5 / 39.6 | 40.9 / 40.0 | 30.5 / 38.0 | 35.0 / 39.2 |
| wood | 52.2 / 42.1 | 52.7 / 39.8 | 42.0 / 38.2 | 44.5 / 38.7 | 35.6 / 40.5 | 45.1 / 41.8 |
| noise | 39.3 / 37.9 | 41.3 / 33.8 | 27.9 / 29.3 | 30.0 / 25.1 | 21.1 / 23.9 | 25.9 / 25.9 |
| detail-poster | 43.8 / 41.9 | 46.5 / 40.8 | 38.7 / 39.5 | 43.3 / 37.9 | 35.2 / 36.2 | 40.3 / 33.7 |
| scene-poster | 35.3 / 37.1 | 37.6 / 35.2 | 30.2 / 33.5 | 34.4 / 32.1 | 27.7 / 30.9 | 32.3 / 30.2 |

Mip 2 loses 5–11 dB against mip 0. Even mips begin a latent level at one-quarter grid resolution
and receive fewer training samples; mip 4 is mostly below BC1, by 7.5 dB on tiles. A smaller
1.37-bit rate point and wider networks exist, but the default retains the measured size/quality
balance above; a 64-wide network adds 2–4 dB at about four times the decode work.

## Final native decode cost — 2026-10-10

Windows 11, Ryzen 9 270, 15 GB RAM; RTX 5060 Laptop (driver 32.0.16.1714, Vulkan 617.14) and
Radeon 780M (32.0.13062.3005); release build, wgpu 30.0.1, AC power, no other agent running.
The session still had desktop GUI load. The final measurements supersede the earlier native
costs. Three rounds agree within 0–8%; the loop waited for the RTX 5060 to reach at most 70°C
before each round, with no settling after benchmark startup.

`neural_bench` draws one 40 m ground mesh tiled eight times with the bricks material. View 0
looks straight down, decodes every pixel at about 0.9 texels/pixel (mip 0); view 1 looks across
the ground with sky and LOD 0–6. Each run takes the opaque pass's timestamp-query p50 over 150
frames after 60 warm-up frames. Cells are medians of three rounds, ms. Textured uses ordinary
RGBA8 arrays with trilinear and 16× anisotropic filtering; decode is neural minus textured.

| GPU | Backend | View | Size | Inline | Textured | Neural f16 | Neural f32 | Decode f16 | Decode f32 |
|---|---|---|---|---|---|---|---|---|---|
| RTX 5060 | Direct3D 12 | 0 | 1920x1080 | 0.34 | 0.35 | 1.88 | 2.73 | 1.52 | 2.38 |
| RTX 5060 | Direct3D 12 | 0 | 2560x1440 | 0.58 | 0.63 | 3.51 | 5.15 | 2.88 | 4.52 |
| RTX 5060 | Direct3D 12 | 1 | 1920x1080 | 0.25 | 0.32 | 1.42 | 2.07 | 1.10 | 1.75 |
| RTX 5060 | Direct3D 12 | 1 | 2560x1440 | 0.44 | 0.55 | 2.44 | 3.71 | 1.89 | 3.16 |
| RTX 5060 | Vulkan | 0 | 1920x1080 | 0.32 | 0.35 | 1.95 | 2.61 | 1.60 | 2.26 |
| RTX 5060 | Vulkan | 0 | 2560x1440 | 0.56 | 0.60 | 3.56 | 4.78 | 2.95 | 4.18 |
| RTX 5060 | Vulkan | 1 | 1920x1080 | 0.25 | 0.40 | 1.42 | 1.96 | 1.02 | 1.56 |
| RTX 5060 | Vulkan | 1 | 2560x1440 | 0.42 | 0.66 | 2.54 | 3.49 | 1.88 | 2.83 |
| Radeon 780M | Direct3D 12 | 0 | 1920x1080 | 0.72 | 1.18 | 5.59 | 4.79 | 4.41 | 3.61 |
| Radeon 780M | Direct3D 12 | 0 | 2560x1440 | 1.22 | 1.91 | 10.14 | 8.91 | 8.24 | 7.00 |
| Radeon 780M | Direct3D 12 | 1 | 1920x1080 | 0.57 | 1.46 | 3.85 | 3.30 | 2.38 | 1.84 |
| Radeon 780M | Direct3D 12 | 1 | 2560x1440 | 0.98 | 2.48 | 6.83 | 5.89 | 4.35 | 3.41 |
| Radeon 780M | Vulkan | 0 | 1920x1080 | 0.65 | 1.05 | 4.19 | 5.04 | 3.14 | 3.99 |
| Radeon 780M | Vulkan | 0 | 2560x1440 | 1.18 | 1.76 | 7.54 | 8.87 | 5.78 | 7.11 |
| Radeon 780M | Vulkan | 1 | 1920x1080 | 0.56 | 1.19 | 2.80 | 3.24 | 1.61 | 2.05 |
| Radeon 780M | Vulkan | 1 | 2560x1440 | 0.91 | 2.01 | 4.99 | 5.75 | 2.98 | 3.74 |

On the RTX 5060, full-screen f16 adds 1.52–1.60 ms at 1080p and 2.88–2.95 ms at 1440p;
f32 adds 2.26–2.38 and 4.18–4.52 ms. On the Radeon 780M, f16 adds 3.14–4.41 ms at 1080p
and 5.78–8.24 ms at 1440p; D3D12 f32 reduces those to 3.61 and 7.00 ms. The final ordinary
textured baseline is not twice as slow on Vulkan: that earlier difference was machine load.

## Scope and remaining limits

- Materials are opaque, single-sided and repeat-wrapped; there is no alpha-mask or transparent
  neural material. LOD is isotropic, with no anisotropic filtering, so grazing angles are blurrier
  than the ordinary 16×-anisotropic baseline.
- One network profile is allowed per renderer. The 64 KiB weight uniform holds about 12 default
  textures; a texture with another profile or beyond capacity is refused and uses the default
  material. Channel counts 9–12 share a padded profile; 8 does not.
- Small textures and uncorrelated high-frequency channels can lose the storage or quality benefit.
  Coarse-mip fidelity remains weaker than mip 0.
- Chrome 155 WebGPU on D3D12 supported f16 on both GPUs. Earlier full-screen opaque-pass readings
  at 1080p / 1440p were RTX f16 2.21 / 3.94 ms, f32 3.39 / 5.81 ms; Radeon f16 6.84 / 13.0 ms,
  f32 10.2 / 18.1 ms. They used only 3–11 distinct timestamp readings, were not re-measured in the
  final quiet session, and do not establish browser/native parity. Apple timing is unmeasured here.

To exercise this path, build the `neural_encode` and `neural_bench` renderer examples, encode a
material, then benchmark both precisions and views on the target adapter. Use the
[design and API](../spec/neural-textures.md) for loading and profile rules.
