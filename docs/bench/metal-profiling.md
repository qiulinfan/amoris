# Metal frame profiling

Amoris follow-up to Pioneer `35a9bccf`, 2026-10-10. Device: Apple M5, macOS 27.0.1
(26A434), wgpu 30.0.1. Windows' D3D12/Vulkan measurements remain in [dx12.md](dx12.md).

## Correctness before timing

The original Metal path resolved counters in the submission that sampled them. A 100-frame
probe using the production profiler found 10 records whose ticks exactly equalled the preceding
frame's; their frame IDs nevertheless named the current frame. Resolving again after completion
identified the mismatch. An empty compute pass also returned zero calibration timestamps.
Adding a real dispatch alone did not repair early resolve: fresh-query controls still returned
zero. These are correctness experiments, not estimates of a game's error rate or performance.

Each in-flight profiler slot now owns its QuerySet, resolve buffer, readback buffer, labels and
frame ID. On Metal (including the Vulkan-to-Metal MoltenVK driver) a submission-completion
callback only marks a slot ready. The rendering
thread batches those slots' resolves into a later submission, then maps their readbacks. Busy
slots cannot accept new timestamp writes. Normal rendering never waits for GPU completion;
trace export drains both sampling and resolve submissions. Other backends retain resolve in the
sampling encoder; native Vulkan drivers and D3D12 on Windows keep that path.
The backend-specific deferral can be removed when an upstream counter-resolve fix passes the
same-frame regression on the supported Apple devices; query-slot ownership still applies.

Metal clock calibration performs a one-thread storage write. Its CPU bracket covers only the
sampling submission and its completion; resolving and reading the counters happens afterwards.
Zero, unordered and repeated calibration samples are rejected. Calibration remains outside the
measured frame window. The GPU capture follows the CPU capture window exactly, including skips,
late activation and completion with export deferred until later.

## Reproduce

```sh
POCKET_BACKEND=metal cargo test --release -p pocket-render --test frame_trace \
  --test gpu_profiler --test prepass -- --test-threads=1
POCKET_BACKEND=metal cargo test --release -p pocket-render --lib \
  gpu_clock_samples_are_current_and_bound_by_the_cpu_brackets -- --test-threads=1
cargo build --release -p pocket-render --example many_cubes
POCKET_BACKEND=metal POCKET_AA=msaa POCKET_GTAO=off \
  target/release/examples/many_cubes --dense --count 200000 --size 1280x720 \
  --headless-bench 300 --occlusion off --prepass auto --settle 2 \
  --trace frames=300,skip=60,out=out/profiler/metal-dense.json
python3 tools/frame_trace.py stats out/profiler/metal-dense.json
```

The frame-trace regression checks 100 consecutive GPU frames against their own CPU submit/wait
bounds. Another starts a trace after 16 ordinary frames with no skipped frame, captures three
frames and renders 64 more before exporting: only three GPU records may be retained. The
profiler regression submits frames without waits, checks query ownership and counts missed
capture frames without counting frames outside the capture window.

The prepass regression retains exact image and entity-coverage assertions. Its invariance
fixture separates the cylinder bottom from the ground; a separate coplanar test checks transparent
mask holes and the documented draw-order behavior for valid depth ties. Restoring the old
unmasked Equal shader makes the new transparency regression fail. Removed, hidden and pending
instances no longer enable prepass probes through a stale slot high-water mark.

The splat/TAA compositing check compares the output to that same frame's opaque TAA history.
Separate renderer histories can differ by one half-float step on Metal, including with the old
profiler; that drift must not be mistaken for a splat changing the background. Coverage, flicker
and outside-footprint assertions remain unchanged. A deliberately jittered depth buffer still
fails with 572 flickering pixels. The hidden-object motion control reuses one renderer with
matching fixed prepass/occlusion policies and resets TAA between runs. Removing the ocean
motion-target write still fails with 458 affected pixels; the original thresholds remain.

Raw traces, measurements and test logs belong under ignored `out/` or the separately authorized
results archive. This document records the method and concise results only.

## Recorded runs

Three interleaved rounds, 200,000 cubes, 1280x720, 4x MSAA, GTAO and occlusion off, 60 warm-up
frames followed by 300 recorded frames, with a two-second settle before each run. The second
round reverses the case order. All 2,700 GPU frame spans fit their own CPU submit/wait bounds,
with zero missing or dropped records. Clock-placement uncertainty was 53.5 to 79.9 us.

| Case | Recorded frames | GPU frame median, median of runs | Run medians, range |
|---|---:|---:|---:|
| Dense, prepass off | 900 | 6.368 ms | 6.278–29.601 ms |
| Dense, prepass auto | 900 | 6.478 ms | 6.191–6.637 ms |
| Sphere, prepass auto | 900 | 0.901 ms | 0.849–13.877 ms |

These captures establish frame attribution and end-to-end export. The desktop was active and
clocks were not locked; the large run-to-run variation prevents a stable speedup conclusion.
GPU frame spans come directly from trace events, while the original benchmark JSON also retains
CPU wall time and its sampled pass statistics. Auto selected off for 297–299 of each run's 300
measured frames; the remaining frames were probes.

The six `tools/prepass_compare.py --backend metal --cases ''` scenes (mixed, cubes, anim,
sailing, gi-room, pt-lab) have zero image or entity-coverage difference. The matching occlusion
comparison preserves every scene's entity coverage; pt-lab has one channel differing by one
8-bit level, all other scenes are identical.

[Raw runs and their
method](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/amoris-metal-profiling-20261010)
are kept in the results repository.
