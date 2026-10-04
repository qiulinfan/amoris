# Gaussian splats on the reference machine

- Date: 2026-10-04. Machine: Apple M5 (10 cores), 32 GB, macOS 27, Metal 4; Vulkan through
  MoltenVK. Branch `feat/splat`. Design and passes: [docs/spec/splats.md](../spec/splats.md).
- **Noise.** Every run below shared the machine with other agents' work (physics benchmarks on all
  cores, a PyTorch training run on the GPU through MPS, Blender, another renderer's benchmarks; load
  average 13 to 23). The figures are the best of three runs and are not a calibrated baseline
  (budgets.md 1: numbers taken beside other work are unreliable). Re-run them on a quiet machine
  before comparing with anything.

## Scene and method

`cargo run --release -p pocket-render --example splats -- --count N --headless-bench 100
[--key-bits B]`: the procedural garden (docs/spec/splats.md 7) with N garden splats plus two
instances of a 40k-splat orb with degree-3 spherical harmonics, seven lit primitive meshes, the
procedural sky, a directional light without shadows, 4x MSAA meshes, offscreen 1600x900, the
camera orbiting the scene (the sort's input changes every frame). 20 frames of warm-up, then 100
frames with the splat draw on and 100 with it off. Per-pass times are GPU timestamps around each
pass, averaged; "frame" is the wall time from the start of the frame's CPU work to the GPU going
idle (`device.poll(wait)`), which includes the submission and synchronization overhead the
timestamps do not.

The garden's quads are large in total: at 1M splats their area (each quad's pixels, clipped per
splat to the screen's size) is 32x the screen, at 3M 64x. Many distant splats are below a pixel and
are widened by the 0.3 px^2 low-pass to about 11 pixels each.

## Results (Metal, headless 1600x900)

Two sessions an hour apart (the second after rebasing on main 640cc8f), each the best of three
runs; a cell gives the range over the two sessions.

| Garden | Submitted / visible | Key bits | Preprocess | Sort | Depth copy | Draw | GPU (sum of passes) | Frame (wall) |
|---|---|---|---|---|---|---|---|---|
| 1M | 1,069,361 / 874,525 | 16 | 0.88-0.99 ms | 0.65-0.68 ms | 1.13-1.20 ms | 4.0-5.1 ms | 7.6-8.9 ms | 14.9-16.0 ms |
| 1M | | 24 | 0.99-1.03 ms | 0.95-0.98 ms | 0.96-1.03 ms | 5.2-6.2 ms | 8.8-10.1 ms | 16.3-17.9 ms |
| 1M | | 32 | 0.96-0.99 ms | 1.33-1.37 ms | 0.89-1.00 ms | 5.2-5.8 ms | 9.1-10.1 ms | 16.8-18.1 ms |
| 3M | 3,046,792 / 2,463,475 | 16 | 2.39-2.48 ms | 1.87-1.90 ms | 0.65-1.51 ms | 21.8-26.0 ms | 28.8-31.4 ms | 35.8-42.1 ms |
| 3M | | 24 | 2.34-2.41 ms | 2.85-2.99 ms | 0.71-1.20 ms | 22.4-29.2 ms | 29.6-36.0 ms | 39.5-43.0 ms |
| 3M | | 32 | 2.28-2.63 ms | 3.77-3.98 ms | 0.77-0.82 ms | 20.9-30.4 ms | 28.3-38.4 ms | 42.9-43.5 ms |

With the draw off the same frames take 4.3-5.4 ms (1M) and 5.9-9.6 ms (3M) of wall time. The
opaque pass costs 0.2-0.7 ms more while splats draw (it keeps its multisampled depth for the copy).
The depth copy should be about 0.1 ms (it was 0.06 ms in one Vulkan frame); the higher Metal
figures are probably the opaque pass's depth store landing in the copy's timestamps. The wall
frame exceeds the sum of the pass timestamps by 5-10 ms when the draw is on, more than the
submission overhead seen with it off (1.5-2.5 ms); part of the tile-based GPU's vertex and binning
work for the splat pass probably falls outside that pass's timestamps.

GPU memory: 94 MB of splat buffers at 1M, 261 MB at 3M (section 3 of the spec).

**Windowed** (`--bench 300`, 1280x720 logical, 2560x1440 physical, Immediate present, 24-bit keys,
60 frames skipped, 300 measured): 1M frame p50 21.0 ms (mean 21.2, p95 28.5), GPU 16.9 ms; 3M
frame p50 57.8 ms (mean 55.7, p95 64.8), GPU 41.2 ms. A second run under heavier load measured 1M
at p50 25.4 ms.

**Vulkan** (MoltenVK, same headless benchmark): the image matches Metal's (mean absolute difference
0.0003 of 255 per channel). Timings were too disturbed by the concurrent load to compare (1M, 24
bits: preprocess 0.92 ms, sort 0.94 ms, draw 12-19 ms across runs).

## What the draw costs and why it is drawn this way

All at 1M, 1600x900, single runs:

| Variant | Splat draw |
|---|---|
| In the 4x MSAA opaque pass, after the sky (the simplest hook) | 16.7 ms |
| Same, one instanced 4-vertex strip per splat instead of 16k-quad batches | 16.8 ms (no difference) |
| Same, `discard` below 1/255 instead of blending a zero | 25.4 ms |
| Separate pass into the resolved single-sample image (the design) | 7.7-8.2 ms (6.2 ms best of three with `exp2`) |
| Single-sample, alpha without the Gaussian (`exp` removed) | 6.6 ms |
| Single-sample, blending off | 4.3 ms |
| Single-sample, quads shrunk to 2% (geometry only) | 1.7 ms (3M: 4.3 ms) |

The draw is bound by blending: on the Apple GPU blending into four samples costs about twice
blending into one, and the fragment count (32x to 64x the screen) dominates the geometry (1.7 ms per
million splats). In the 4x MSAA pass the cost also did not fall in proportion to resolution
(800x450: 11.5 ms, 1600x900: 17.0 ms, 3200x1800: 34.0 ms), because sub-pixel splats keep a minimum
footprint.

## The sort

3M splats (2.46M visible), 24-bit keys, three passes, each kernel timed in its own compute pass:
histogram 0.61 ms, scan 0.17 ms, scatter 2.23 ms (sums over the three passes). At 1M, 16-bit keys
(two passes) sort in 0.68 ms, 32-bit (four) in 1.37 ms. The GPU unit test
(`cargo test -p pocket-render --lib splat`) checks keys and values against a stable CPU sort.

## Reproduce

```
cargo run --release -p pocket-render --example splats -- --count 1000000 --headless-bench 100 --key-bits 24
cargo run --release -p pocket-render --example splats -- --count 3000000 --bench 300
POCKET_BACKEND=vulkan DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib cargo run --release -p pocket-render --example splats -- --headless-bench 100
POCKET_GPU_MINIMAL=limits,features cargo run --release -p pocket-render --example splats -- --count 3000000 --capture out.png
```
