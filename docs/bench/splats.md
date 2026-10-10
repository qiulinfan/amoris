# Gaussian splats: rasterization, quality and defaults

Quads remain the default rasterizer on every adapter. The compute tile rasterizer is available
through `POCKET_SPLAT_RASTER=tile`, but its gains depend on pixel coverage and overlap; the final
Windows matrix does not support a faster default for an adapter kind or backend.

Design and API: [splats](../spec/splats.md). Temporal integration:
[TAA and GTAO](../spec/taa-gtao.md). Machine conditions:
[benchmark methods](quiet-2026-10-09.md).

## Rendering method

Both paths preprocess the Gaussian centres, covariances, opacity and spherical harmonics, cull
invisible splats and stably depth-sort the visible ones. Quads draw into the resolved single-sample
image after the opaque scene. The tile path bins sorted splats into 16×16 tiles and eight 8×4
sub-tiles, then composites front to back with early termination. Its per-pixel walk uses loop
conditions rather than `break` and `continue`, preserving the arithmetic and accumulation order.

The procedural garden adds two instances of a 40,000-splat orb with degree-3 spherical harmonics,
seven lit primitive meshes, a procedural sky and a directional light without shadows. At one
million garden splats, the clipped quads cover about 32 screen areas; at three million, about 64.
This stresses overlapping transparency rather than a general game scene.

Splat anti-aliasing is separately selectable. Many distant splats are smaller than a pixel and the
low-pass broadens their footprint; the optional opacity compensation reduces the resulting haze.
Splats draw after TAA and stay outside its history. Mesh occlusion uses unjittered depth. The
[specification](../spec/splats.md) defines file formats, scene components, picking and pass order.

## Final native cost — 2026-10-10

Windows 11 Pro 10.0.26200, Ryzen 9 270, 15 GB RAM; RTX 5060 Laptop with driver 32.0.16.1714
(NVIDIA 617.14), and Radeon 780M with driver 32.0.13062.3005. Release build, wgpu 30.0.1,
AC power, with desktop GUI load but no other agent running. Apple tile performance is unmeasured.

Five interleaved rounds per adapter and API, each with a fresh process, a five-second startup
settle, 20 warm-up frames and 60 measured frames. The RTX 5060 was at most 70°C before each round;
it could warm within a round. Each adapter uses its visual defaults: RTX MSAA plus GTAO, Radeon
TAA without GTAO. GPU time is the sum of timestamped splat passes, excluding CPU and presentation.
Cells are the rounds' medians in ms: quads / tiles (**tiles divided by quads**).

| Scene | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---|---|---|---|
| 1M 1600x900 | 2.80 / 3.46 (**1.24**) | 2.51 / 2.73 (**1.09**) | 14.52 / 20.18 (**1.39**) | 13.55 / 19.88 (**1.47**) |
| 3M 1600x900 | 6.97 / 8.09 (**1.16**) | 6.40 / 6.21 (**0.97**) | 44.50 / 51.81 (**1.16**) | 40.18 / 51.31 (**1.28**) |
| 1M 2560x1440 | 4.33 / 4.53 (**1.04**) | 4.10 / 3.69 (**0.90**) | 22.44 / 26.54 (**1.18**) | 20.19 / 26.48 (**1.31**) |
| 3M 2560x1440 | 10.55 / 9.98 (**0.95**) | 9.86 / 7.75 (**0.79**) | 66.07 / 62.44 (**0.95**) | 57.39 / 64.03 (**1.12**) |
| orb close-up | 2.59 / 2.49 (**0.96**) | 2.38 / 1.89 (**0.80**) | 10.81 / 14.01 (**1.30**) | 9.88 / 13.53 (**1.37**) |
| 1M anti-aliased | 2.72 / 3.86 (**1.42**) | 2.46 / 3.06 (**1.24**) | 14.34 / 21.35 (**1.49**) | 13.34 / 21.06 (**1.58**) |
| 1M 2.5x distance | 2.08 / 3.37 (**1.62**) | 1.78 / 2.63 (**1.48**) | 12.63 / 20.02 (**1.59**) | 12.69 / 19.22 (**1.51**) |
| 1M 2.5x distance AA | 1.94 / 3.99 (**2.06**) | 1.65 / 3.10 (**1.88**) | 12.43 / 21.27 (**1.71**) | 12.69 / 20.54 (**1.62**) |

At the default one-million, 1600×900 view, tiles cost 9–47% more on every configuration. They win
most clearly on RTX Vulkan with more pixels or overlap: 21% at three million, 2560×1440 and 20%
in the orb close-up. D3D12 has smaller wins there; Radeon D3D12 wins only the three-million
1440p case by 5%. Distant or anti-aliased scenes cost tiles 24–106% more because early termination
rarely fires and each touched sub-tile visits 32 pixels.

Pass breakdown, ms: quads' depth copy plus draw / tiles' binning and pair sort + raster.

| Scene | RTX 5060, D3D12 | RTX 5060, Vulkan | 780M, D3D12 | 780M, Vulkan |
|---|---|---|---|---|
| 1M 1600x900 | 1.93 / 1.10 + 1.45 | 1.93 / 0.84 + 1.25 | 7.87 / 7.67 + 5.10 | 6.36 / 7.79 + 4.70 |
| 3M 1600x900 | 4.78 / 3.11 + 2.68 | 4.87 / 2.35 + 2.31 | 25.34 / 22.67 + 8.71 | 19.36 / 22.68 + 8.31 |
| 1M 2560x1440 | 3.45 / 1.31 + 2.24 | 3.50 / 1.05 + 1.93 | 15.81 / 10.44 + 7.99 | 13.13 / 10.96 + 7.15 |
| 3M 2560x1440 | 8.28 / 3.65 + 3.91 | 8.29 / 2.82 + 3.31 | 46.52 / 29.35 + 12.47 | 37.27 / 31.24 + 11.84 |
| orb close-up | 1.96 / 0.84 + 0.97 | 1.92 / 0.60 + 0.82 | 6.79 / 5.97 + 3.58 | 5.96 / 6.13 + 3.09 |
| 1M anti-aliased | 1.86 / 1.09 + 1.86 | 1.85 / 0.84 + 1.58 | 7.69 / 7.62 + 6.17 | 6.28 / 7.81 + 5.64 |
| 1M 2.5x distance | 1.18 / 1.06 + 1.38 | 1.19 / 0.80 + 1.22 | 3.65 / 6.28 + 4.17 | 3.36 / 6.07 + 3.67 |
| 1M 2.5x distance AA | 1.04 / 1.05 + 2.02 | 1.04 / 0.78 + 1.69 | 3.53 / 6.07 + 5.52 | 3.32 / 5.91 + 4.95 |

On the integrated GPU, binning and sorting alone cost 5.9–31 ms, before rasterization. The raster
can be cheaper than drawing quads while the whole tile path still loses. D3D12's RTX tile sort and
raster also cost more than Vulkan's. These costs prevent a broad tile default.

## Fidelity and implementation checks

Quads against tiles reach 57.2–64.4 dB PSNR on RTX and 55.8–60.1 dB on Radeon under TAA.
Mean differences are 0.02–0.12 and 0.06–0.17 of 255 respectively; maximum channel differences
are 4–17 and 10–24. The Radeon differences affect about 0.004% of pixels. Cross-API comparisons
reach 64.2–72.3 dB on RTX and 70.4–73.4 dB on Radeon. The rasterizers are close, not bit-identical.

The shipped walk rewrite was tested against the earlier walk over five interleaved rounds and
16 image pairs. All pairs were byte-identical. Raster-pass medians at one million splats, ms:

| Walk | RTX D3D12 | RTX Vulkan | Radeon D3D12 | Radeon Vulkan |
|---|---:|---:|---:|---:|
| Earlier loop | 11.43 | 1.34 | 7.46 | 4.45 |
| Shipped loop | 1.45 | 1.25 | 5.09 | 4.76 |

The D3D12 improvement is about 8× on RTX and 1.5× on Radeon; Radeon Vulkan is 7% slower.
This fixes tile execution cost without changing the default. Stable-sort tests compare GPU keys
and values with a CPU oracle; picking tests cover overlap, tie order, opacity and mesh occlusion.
Chrome validated the changed shader and the core-limit kernels, rather than repeating this cost
matrix. Browser validation establishes compatibility, not native/browser performance parity.

## Reproduction and limits

```sh
cargo build --release -p pocket-render --example splats --example image_diff
python tools/splats/matrix.py --adapters nvidia,780m --backends dx12,vulkan \
    --rounds 5 --frames 60 --cool 70 --settle 5 \
    --scene "1M=--count 1000000" \
    --scene "3M1440p=--count 3000000 --size 2560x1440" \
    --out out/bench-runs/splats.json
python tools/quiet_tables.py splatm out/bench-runs/splats.json
cargo test --release -p pocket-render --lib splat
```

Add close-up, distant and anti-aliased scenes before changing a default; compare both complete
rasterizers, including their sorts and depth work. Keep generated measurements and captures under
ignored `out/`. Use `compare_matrix.py` for image comparisons across rasterizers and APIs.

- This is a high-overdraw procedural workload; its ratios do not predict an arbitrary captured
  scene.
- Finite tile-pair capacity and transparency ordering remain part of the rasterizer's contract;
  inspect overflow diagnostics when increasing splat count or footprint.
- Splats outside TAA do not receive temporal edge anti-aliasing. Picking uses an opacity threshold
  and does not mean every faint rendered contribution is selectable.
- The older Apple M5 runs shared heavy CPU/GPU work and selected best runs. They are not a quiet
  calibration and do not justify a Metal tile default or a Windows-to-Apple speed comparison.

Historical methods and measurements: [Pioneer source
document](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/quiet/splats).
