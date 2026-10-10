# Global illumination: implemented slices and measured limits (Metal, then Vulkan and Direct3D 12)

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

Evidence: `out/bench-runs/gi/bake-room.json`, `baked-off.png`, `baked-on.png`, `web-editor.png`.

## 2. Metal tracing and SHaRC-style cache

`metal_ray_query` (since 2026-10-09 `ray_query_probe`, any backend) verifies actual BLAS/TLAS
intersections. `gi_trace` adds finite-horizon diffuse
path tracing, emissive-triangle NEE/MIS and an independently authored spatial-hash radiance cache.
Its Update, Resolve and Query responsibilities are separate GPU passes. Secondary paths query
confident cache entries; new entries need history. Cache identity includes spatial cell, normal,
material and remaining path horizon. Resetting a render resets its history.

This is an isolated Metal tool, not yet a pass in the interactive forward renderer. It currently
accepts static opaque constant Lambertian materials, emission and a black sky. It rejects textures,
alpha, explicit lights and unsupported transport. It does not claim NVIDIA SDK parity or an unbiased
radiance-cache estimator. Sampling clipping, dropped deposits and hash collisions are reported.

```sh
cargo run --release -p pocket-render --example ray_query_probe   # formerly metal_ray_query
cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode raw --output out/gi/raw.png --width 320 --height 240 --frames 128 --samples 1
cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode sharc --output out/gi/sharc.png --width 320 --height 240 --frames 128 --samples 1
cargo test -p pocket-render --lib ray_query   # formerly `metal_ -- --ignored`
```

Three interleaved runs, 320x240, 128 frames, same camera/seed, six bounces, 0.25 m cells and
minimum 16 cache samples: last-64-frame GPU means 0.698 ms raw versus 0.538 ms cached (22.9% lower).
These timings include cache Update/Resolve/Render, but exclude AS creation, readback, presentation
and simulation. The full-sequence camera/ray workload and update cost are recorded in the JSON.
Warm-region ray count decreased by 48.1% in the recorded comparison.

Against a finite noisy 1024 spp reference, candidate linear HDR RMSE was 0.2964 raw and 0.2909
cached. One sample clipped among roughly a million update deposits. A lower-coverage 160x120 run was
slower with the cache. The result is workload- and history-dependent, not an engine-wide FPS claim.
See `out/bench-runs/gi/sharc/comparison-final-320.json` and `repeated-timings-320.json`.

A later three-pair repeat of the final integrated source varied substantially: raw GPU means
0.645/0.881/0.732 ms and cached means 0.511/0.847/0.860 ms. The aggregate improvement was only
1.8%, including one slower cached run. Keep both records: the earlier 22.9% is a bounded result,
not a stable final-source speed guarantee. More controlled profiling is needed to establish
consistent latency gains. See `out/bench-runs/gi/sharc/final-code-repeat.json`.

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
See `out/bench-runs/gi/restir/comparison-160-32.json`. Full-path PT requires additional shift
mappings, reconnection/support and correlation handling; the investigation does not implement those
yet.

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
teacher peak (6.4274); these are random volume/normal samples, not a rendered-image or
path-reference quality score. The real Metal shader matched an independent f64 reference on 1,190
queries with maximum absolute error 1.16e-6 and no GPU errors. The rendered field is smoother and
loses detail.

640x640 headless renderer, warm 30 completed frames then 120 measured frames: render-call-to-idle
mean 0.913 ms with baked probes versus 9.455 ms with scalar float32 MLP inference. This includes
the complete static rendering workload but excludes simulation/presentation. Metal pass timestamps
are not whole-frame cost. The current network path is slower; it does not use Metal 4 MPP tensor
kernels or perform online updates. A future NRC prototype must measure training and inference
together with fresh path samples, not assume this result predicts an optimized NRC implementation.

Evidence: `out/bench-runs/gi/neural-training.json`, `neural-parity.json`, `neural-on.png`,
`baked-render-bench.json`, `neural-render-bench.json`, and `web-neural.png`.

## Validation boundary

The native workspace tests pass, including the CPU baker/reservoir/asset checks. The two ignored
Metal hardware tests were also executed explicitly and passed. Neural teacher tests, trained-model
GPU/CPU parity, native captures, native release build, rebuilt wasm viewport, real WebGPU asset
loading/rendering and the scoped dependency/generation checks were verified. The browser helper's
editor page does not publish `pocketSamples`; its zero sample count is not a measured FPS.

No full `cargo xtask check` pass, complete ReSTIR PT, online dynamic NRC, arbitrary production
material/animation coverage or engine-wide performance gain is claimed by these experiments.

## Windows: Vulkan and Direct3D 12 (Pioneer, 2026-10-09)

Decision: charter 4.4 (Pioneer, 2026-10-09). Branch `explore/rt`; machine, drivers and the
provisional status of every timing as in [path-tracing-nrc.md,
Windows](path-tracing-nrc.md#windows-vulkan-and-direct3d-12-pioneer-2026-10-09). The isolated tracer
runs unchanged on Vulkan and Direct3D 12 (RTX 5060 and Radeon 780M); its two GPU tests now run by
default on every backend with ray queries (`ray_query_cache_requires_history_and_resets`,
`ray_query_ris_matches_raw_and_bounds_history`, formerly macOS-only and ignored) and pass on all
four GPU/backend pairs. `metal_ray_query` became `ray_query_probe` (section 2).

### Quality against the M5 comparisons

`tools/rt_bench.py correctness` renders the M5's SHaRC and ReSTIR configurations and a finite
reference per backend at the M5's settings (`out/bench-runs/rt/correctness-5060.json`,
`correctness-780m.json`). HDR RMSE against that reference, linear and after Reinhard x/(1+x),
next to the M5's numbers against its reference:

| Configuration | Linear RMSE, Vulkan / D3D12 (RTX 5060) | M5 | Reinhard RMSE, Vulkan / D3D12 | M5 |
|---|---:|---:|---:|---:|
| SHaRC comparison: raw, 320x240, 128 frames | 0.29659 / 0.29659 | 0.29645 | 0.01692 / 0.01692 | 0.01692 |
| SHaRC comparison: cached | 0.08769 / 0.12393 | 0.29091 | 0.01336 / 0.01319 | 0.01350 |
| ReSTIR, 160x120, 32 frames: raw | 0.09734 / 0.09734 | 0.10541 | 0.03050 / 0.03050 | 0.03091 |
| RIS K1, no reuse | 0.09734 / 0.09734 | 0.10540 | 0.03050 / 0.03050 | 0.03091 |
| Temporal reuse | 0.14072 / 0.14072 | 0.14713 | 0.04116 / 0.04116 | 0.04149 |
| Spatial reuse | 0.56169 / 0.56169 | 0.56433 | 0.02368 / 0.02368 | 0.02423 |
| Both | 1.52920 / 1.52909 | 1.52907 | 0.03407 / 0.03407 | 0.03447 |

- Uncached and RIS paths are the M5's: 109 (Vulkan) and 92 (Direct3D 12) of the 128 raw frames
  have exactly the M5's counters, the rest differ by at most 4e-5, and the previews match the M5's
  in 99.9% of pixels. The mean of the 1,024 spp reference equals the M5's (0.256928 against
  0.256929, mean of RGB).
- The cached image's linear RMSE is a firefly lottery, not a quality difference: one pixel off by
  37 already contributes 0.077 to it at this size, and the raw image's 0.297 is mostly one pixel
  off by 125. Which fireflies the cache absorbs depends on the hash insertion race, so it differs
  by backend (0.088 on Vulkan, 0.124 on Direct3D 12, 0.124 on both backends of the 780M) while it
  is stable from run to run on one backend (four repeats each, identical to five digits). The
  Reinhard RMSE, which bounds fireflies, agrees with the M5 within 2.3%: the cache helps quality as
  little here as there.
- Cache statistics agree with the M5 to about 1% (4.88M hits over the run against 4.86M, 1.08M
  deposits, 5.4k dropped deposits, 85k hash collisions, one clipped sample), and cached previews
  match the M5's in 44% of pixels exactly, 0.3% off by more than 8 (`strip-sharc-m5-vulkan.png`).
- ReSTIR numbers differ from the M5's by the reference: this reference is 64 spp x 32 frames (the
  M5's "2,048 spp" did not record its split); every reuse mode keeps its rank and its Reinhard RMSE
  within 3%, so the negative reuse results stand.

### Timing (provisional, superseded below), RTX 5060

`tools/rt_bench.py timing --rounds 5`, gi-room 320x240, 128 frames, 6 bounces, 0.25 m cells,
minimum 16 samples; GPU ms per frame (update + resolve + render) over the last 64 frames, median of
five interleaved rounds with min to max:

| | Vulkan | Direct3D 12 | M5 |
|---|---:|---:|---:|
| Raw | 0.132 [0.132, 0.135] | 0.139 [0.131, 0.144] | 0.698 (later 0.645 to 0.881) |
| SHaRC | 0.139 [0.139, 0.140] | 0.163 [0.159, 0.173] | 0.538 (later 0.511 to 0.860) |
| Raw, lean shaders | 0.089 [0.089, 0.090] | 0.095 [0.095, 0.101] | |
| SHaRC, lean shaders | 0.106 [0.106, 0.107] | 0.131 [0.131, 0.141] | |
| Rays per frame, raw vs SHaRC | 360,043 vs 186,899 (-48%) | the same | -48.1% |

- On the RTX 5060 the cache does not pay: it removes 48% of the rays, as on the M5, but its update
  and resolve passes and the hash probing cost more than the rays saved, so the cached frame is 5%
  (Vulkan) to 17% (Direct3D 12) slower, 18% to 38% with lean shaders. The 5060 traces rays about
  5x faster than the M5 while the cache's memory-bound work does not shrink as much. This repeats
  the later M5 finding (aggregate gain 1.8%, one cached run slower) with a clear sign.
- Lean shaders (no naga loop bounds or ray-query tracking, `gi_trace --lean-shaders true`) speed
  the raw trace up by 33% on Vulkan and 31% on Direct3D 12.

### Timing on a quiet machine (2026-10-09)

`tools/rt_bench.py timing` with no other agent running, five rounds on the RTX 5060 and three on
the Radeon 780M (conditions and files as in [path-tracing-nrc.md, Timing on a quiet
machine](path-tracing-nrc.md#timing-on-a-quiet-machine-2026-10-09)); GPU ms per frame (update +
resolve + render), median (minimum; spread); the provisional RTX 5060 medians follow the slash:

| | RTX 5060 Vulkan | RTX 5060 D3D12 | 780M Vulkan | 780M D3D12 |
|---|---:|---:|---:|---:|
| Raw | 0.132 (0.132; 0%) / 0.132 | 0.144 (0.135; 7%) / 0.139 | 0.830 (0.798; 7%) | 0.828 (0.827; 3%) |
| SHaRC | 0.139 (0.139; 0%) / 0.139 | 0.159 (0.159; 9%) / 0.163 | 0.710 (0.700; 2%) | 0.745 (0.734; 2%) |
| Raw, lean shaders | 0.090 (0.090; 0%) / 0.089 | 0.100 (0.095; 9%) / 0.095 | 0.598 (0.597; 6%) | 0.678 (0.670; 4%) |
| SHaRC, lean shaders | 0.106 (0.105; 0%) / 0.106 | 0.143 (0.142; 0%) / 0.131 | 0.566 (0.561; 1%) | 0.626 (0.614; 3%) |

- On the RTX 5060 the provisional finding holds: the cache costs 5% (Vulkan) and 10% (Direct3D 12)
  more than it saves, 18% and 43% with lean shaders.
- **On the Radeon 780M the cache pays** (new): 14% faster than raw with Vulkan and 10% with
  Direct3D 12 (5% and 8% with lean shaders). It removes the same 48% of the rays; on a GPU that
  traces about six times slower than the RTX 5060 the rays saved outweigh the cache's update,
  resolve and hash probing, which they do not on the faster tracer. Whether to cache is a property
  of the GPU's trace rate, as the M5's small gain already hinted.

### Reproduce (Windows)

```sh
POCKET_BACKEND=vulkan cargo run --release -p pocket-render --example gi_trace -- samples/gi-room \
  --mode sharc --output out/gi/sharc.png --width 320 --height 240 --frames 128 --samples 1
cargo test --release -p pocket-render --lib ray_query
python tools/rt_bench.py correctness --adapter 5060 --only sharc-reference,sharc-raw,sharc
python tools/rt_bench.py timing --adapter 5060 --only sharc-raw,sharc-cached,sharc-raw-lean,sharc-cached-lean
```
