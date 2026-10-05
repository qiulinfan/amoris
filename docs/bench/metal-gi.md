# Metal global illumination: implemented slices and measured limits

Date: 2026-10-05. Branch: `feature/metal`. Machine: Apple M5, 32 GiB unified memory.
The scene is `samples/gi-room`: static opaque room walls, two occluders, an emissive box and
a black color sky. All images are actual engine/tool output.

## 1. Baked world-space probes

The CPU triangle BVH baker handles static primitives/glTF, diffuse transport, emission and declared
lights. The asset contains nine radiance SH coefficients per probe and octahedral distance moments.
The forward shader convolves the SH bands, interpolates the moments bilinearly and uses visibility
and normal weights. Inside the volume it replaces sky-only diffuse lighting; sky specular remains.
Assets load asynchronously in native and WebGPU renderers. Interior probes are explicitly disabled.

Reproduce:

```sh
cargo run --release -p pocket-render --example bake_gi -- samples/gi-room \
  --output samples/gi-room/lighting/probes.json \
  --origin -1.65,0.35,-1.65 --spacing 0.825,0.825,0.825 --dims 5,5,5 \
  --rays 1024 --bounces 4 --seed 1 --distance-resolution 8 --max-distance 24
cargo run --release -p pocket-app -- play samples/gi-room --mute
```

Recorded bake: 125 probes, 96 triangles, 445,927 rays, 5 rejected interior probes, 0.158 s release
CPU bake. That bake ran during other CPU work; it is not an isolated performance claim.
The containment heuristic is conservative, not a general watertight-solid solver.
Sky must be color; atmosphere, animation/skinning, ocean, splats and specular/transmission transport
are outside the bake. Punctual shadows in the baker are more complete than the current realtime
punctual shader. The fixture uses emission and avoids that direct-light mismatch.

Evidence: `../evidence/gi/bake-room.json`, `baked-off.png`, `baked-on.png`, `web-editor.png`.

## 2. Metal tracing and SHaRC-style cache

`metal_ray_query` verifies actual BLAS/TLAS intersections. `gi_trace` adds finite-horizon diffuse
path tracing, emissive-triangle NEE/MIS and an independently authored spatial-hash radiance cache.
Its Update, Resolve and Query responsibilities are separate GPU passes. Secondary paths query
confident cache entries; new entries need history. Cache identity includes spatial cell, normal,
material and remaining path horizon. Resetting a render resets its history.

This is an isolated Metal tool, not yet a pass in the interactive forward renderer. It currently
accepts static opaque constant Lambertian materials, emission and a black sky. It rejects textures,
alpha, explicit lights and unsupported transport. It does not claim NVIDIA SDK parity or an
unbiased radiance-cache estimator. Sampling clipping, dropped deposits and hash collisions are reported.

```sh
cargo run --release -p pocket-render --example metal_ray_query
cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode raw --output out/gi/raw.png --width 320 --height 240 --frames 128 --samples 1
cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode sharc --output out/gi/sharc.png --width 320 --height 240 --frames 128 --samples 1
cargo test -p pocket-render metal_ -- --ignored
```

Three interleaved runs, 320x240, 128 frames, same camera/seed, six bounces, 0.25 m cells and
minimum 16 cache samples: last-64-frame GPU means 0.698 ms raw versus 0.538 ms cached (22.9% lower).
These timings include cache Update/Resolve/Render, but exclude AS creation, readback, presentation
and simulation. The full-sequence camera/ray workload and update cost are recorded in the JSON.
Warm-region ray count decreased by 48.1% in the recorded comparison.

Against a finite noisy 1024 spp reference, candidate linear HDR RMSE was 0.2964 raw and 0.2909
cached. One sample clipped among roughly a million update deposits. A lower-coverage 160x120 run
was slower with the cache. The result is workload- and history-dependent, not an engine-wide FPS claim.
See `../evidence/gi/sharc/comparison-final-320.json` and `repeated-timings-320.json`.

A later three-pair repeat of the final integrated source varied substantially: raw GPU means
0.645/0.881/0.732 ms and cached means 0.511/0.847/0.860 ms. The aggregate improvement was only
1.8%, including one slower cached run. Keep both records: the earlier 22.9% is a bounded result,
not a stable final-source speed guarantee. More controlled profiling is needed to establish
consistent latency gains. See `../evidence/gi/sharc/final-code-repeat.json`.

## 3. ReSTIR GI exploration and negative reuse results

The GPU prototype generates real secondary-surface candidates with area-measure PDFs, then runs
temporal/spatial reservoir merging and a fresh selected-triangle visibility query. Direct NEE/MIS
and secondary emission are accounted separately from reflected-tail radiance. Miss and zero
proposals count toward M; history is bounded/reset. It never substitutes color-history averaging.

```sh
cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode restir --reuse none --candidates 1 --output out/gi/ris.png \
  --width 160 --height 120 --frames 32 --samples 1
```

K1/no-reuse versus same-seed raw had HDR RMSE 4.04e-8 in the calibration. The final GPU test
confirmed the M cap and reset; a reuse run performed real visibility tests and found blocked rays.

The current reuse implementation lacks complete support/M corrections and is a biased static
prototype. It is not complete ReSTIR PT. Equal 32-frame K1 HDR RMSE against a finite reference:

| Mode | Linear HDR RMSE |
|---|---:|
| Raw | 0.1054 |
| RIS, no reuse | 0.1054 |
| Temporal | 0.1471 |
| Spatial | 0.5643 |
| Both | 1.5291 |

Default reuse is **none**. Other modes remain explicit experimental controls; they are not quality
improvements. K4/no-reuse reduced RMSE to 0.0928 but used about three times as many rays, so it is
not an equal-work win. Reuse timings are provisional and must not be used as a final speed claim.
See `../evidence/gi/restir/comparison-160-32.json`. Full-path PT requires additional shift mappings,
reconnection/support and correlation handling; the investigation does not implement those yet.

## 4. Small learned diffuse field

The trainer fits the baked field using position and normal inputs, not unbiased path-traced labels.
The 6->32->32->3 ReLU MLP has 1,379 parameters (5,516 raw parameter bytes). Its JSON defines
normalization explicitly. Native and WebGPU forward shaders evaluate it per pixel; `neural_gi`
takes precedence over `baked_gi`. This is offline neural GI, not online dynamic NRC.

```sh
uv sync --project tools/neural/gi --python 3.14
tools/neural/gi/.venv/bin/python tools/neural/gi/train.py \
  --input samples/gi-room/lighting/probes.json --output samples/gi-room/lighting/network.json \
  --steps 4000 --samples 32768 --seed 7 --device mps
cargo run --release -p pocket-render --example neural_gi_probe -- samples/gi-room/lighting/network.json
```

MPS training took 8.19 s. Held-out MSE 0.01463 and PSNR 34.51 dB use the report's stated linear
teacher peak (6.4274); these are random volume/normal samples, not a rendered-image or path-reference
quality score. The real Metal shader matched an independent f64 reference on 1,190 queries with
maximum absolute error 1.16e-6 and no GPU errors. The rendered field is smoother and loses detail.

640x640 headless renderer, warm 30 completed frames then 120 measured frames: render-call-to-idle
mean 0.913 ms with baked probes versus 9.455 ms with scalar float32 MLP inference. This includes
the complete static rendering workload but excludes simulation/presentation. Metal pass timestamps
are not whole-frame cost. The current network path is slower; it does not use Metal 4 MPP tensor
kernels or perform online updates. A future NRC prototype must measure training and inference
together with fresh path samples, not assume this result predicts an optimized NRC implementation.

Evidence: `../evidence/gi/neural-training.json`, `neural-parity.json`, `neural-on.png`,
`baked-render-bench.json`, `neural-render-bench.json`, and `web-neural.png`.

## Validation boundary

The native workspace tests pass, including the CPU baker/reservoir/asset checks. The two ignored
Metal hardware tests were also executed explicitly and passed. Neural teacher tests, trained-model
GPU/CPU parity, native captures, native release build, rebuilt wasm viewport, real WebGPU asset
loading/rendering and the scoped dependency/generation checks were verified. The browser helper's
editor page does not publish `pocketSamples`; its zero sample count is not a measured FPS.

No full `cargo xtask check` pass, complete ReSTIR PT, online dynamic NRC, arbitrary production
material/animation coverage or engine-wide performance gain is claimed by these experiments.
