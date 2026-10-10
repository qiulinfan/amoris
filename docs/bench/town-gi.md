# Sunlit Bistro full-PT showcase

2026-10-05, `feature/metal`, Apple M5. The owner requested a town scene after the Harbor
comparison. The existing ORCA Bistro exterior was restored from a verified local conversion,
with its original source and license attribution retained.
Attribution: Amazon Lumberyard Bistro, Open Research Content Archive (ORCA), Amazon Lumberyard,
July 2017, CC-BY-4.0. The original provenance and conversion records remain authoritative.

Harbor already had a directional sun (intensity 6). For this display the town uses a side/front
sun with surface-to-light direction (-0.557086, 0.742781, 0.371391), about 48 degrees elevation,
intensity 12, warm color (1,0.93,0.83), sky ambient 0.7 and photographic exposure +1.25 EV.
Sun-direction alternatives were checked in the native forward renderer before choosing the
composition. Exposure applies only to the preview; float32 HDR sidecars retain physical radiance.

## Large scene support

The source contains 2,829,226 triangles and 405 images. The tracing loader drops 33 degenerate
triangles, leaving 2,829,193 valid triangles. The previous one-million-triangle cap and default
texture-array limit prevented full-scene PT. The new native build requests actual adapter
buffer/storage/array limits and checks byte sizes plus BLAS primitive/vertex limits. It does not
silently discard geometry. `--texture-size 512` is an explicit quality/memory choice: the 405-layer
RGBA8 array is about 405 MiB, within the retained 512 MiB upload cap. Shading triangle data occupies
about 518 MiB. No claim is made that this model fits ordinary WebGPU limits.

`PtSceneOptions` and `PathTracer::with_scene_options` retain the previous constructor defaults.
Mesh iteration avoids an extra large world-triangle copy. Scene hashing serializes into an FNV
writer instead of allocating the whole JSON representation; a regression checks identical hash
bytes to the previous serializer. This is important for decoded large-image assets.

## Display

Two viewpoints use 1024x768, 128 frames x 4 spp (512 spp), seed 42 and identical source light,
camera, material and exposure per pair. One-surface direct-light controls are compared with
12-surface-horizon PT, NEE/MIS and roulette. The direct control also truncates mirror/transparent
transport; it is not a diffuse-only GI toggle. Both modes omit NRC. All four actual Metal runs
have zero GPU errors and zero nonfinite paths.

| View / mode | Trace batch ms | Mean linear RGB |
| --- | ---: | --- |
| Street / full GI | 27217.545 | (0.21022, 0.14855, 0.13197) |
| Street / direct | 5451.953 | (0.18438, 0.12841, 0.11512) |
| Facade / full GI | 27220.225 | (0.18711, 0.15097, 0.13229) |
| Facade / direct | 5434.998 | (0.16524, 0.13330, 0.11751) |

These are single-run offline rendering times, excluding scene import/setup/readback, not live
FPS or equal-quality benchmarks. Visible sampling noise remains; no denoising was applied.
The static HTML viewer in `tools/town_gi/index.html` offers two cameras, direct/full GI buttons
and a draggable/keyboard comparison. It is explicitly a pre-rendered PT display. The linked
editor is the existing interactive forward renderer, not an interactive PT pass.

Example:

```sh
target/release/examples/path_trace out/town-gi/project --texture-size 512 \
  --width 1024 --height 768 --frames 128 --samples 4 --bounces 12 \
  --exposure-ev 1.25 --output out/town-gi/view/images/street-gi.png
# Repeat with --bounces 1 and the same scene/camera/seed/exposure for the direct control.
python3 -m http.server 8797 --bind 127.0.0.1 --directory out/town-gi/view
```

Actual scene variants, source receipt, four image/HDR/JSON outputs, validation summary and the
display screenshot are under ignored `out/town-gi/`. The HTML source was checked in the in-app
browser with both viewpoints and slider keyboard updates. Renderer library checks passed:
44 tests, with three native hardware tests ignored in the ordinary suite; the complete large
scene was additionally traced on the actual M5.
