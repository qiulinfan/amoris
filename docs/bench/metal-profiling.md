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

Each in-flight profiler slot now owns its range of the profiler's one QuerySet and resolve buffer,
its readback buffer, labels and frame ID. One QuerySet, not one a slot: native Metal gives a process
32 counter sample buffers, one a timestamp QuerySet, across all its devices, and the 33rd fails
("Cannot allocate sample buffer"), which wgpu 30 reports as an unexpected device error and so loses
that device (MoltenVK created 64 without loss). With a QuerySet a slot, three a renderer, the
eleventh renderer alive in a process lost its device, as one of a test binary's parallel cases did
(2026-10-25). On Metal (including the Vulkan-to-Metal MoltenVK driver) a
submission-completion callback only marks a slot ready. The rendering thread batches those slots'
resolves into a later submission, then maps their readbacks. Busy slots cannot accept new timestamp
writes. Normal rendering never waits for GPU completion; trace export drains both sampling and
resolve submissions. Other backends retain resolve in the sampling encoder; native Vulkan drivers
and D3D12 on Windows keep that path, except Vulkan on AMD adapters, where the Radeon 780M read zeros
and earlier values there too and which defer like Metal (Pioneer, 2026-10-15,
[gpu-matrix.md](gpu-matrix.md) 3 a). The backend-specific deferral can be removed when an upstream
counter-resolve fix passes the same-frame regression on the supported Apple devices; query-slot
ownership still applies.

Metal clock calibration performs a one-thread storage write. Its CPU bracket covers only the
sampling submission and its completion; resolving and reading the counters happens afterwards.
Zero, unordered and repeated calibration samples are rejected. Calibration remains outside the
measured frame window. The GPU capture follows the CPU capture window exactly, including skips,
late activation and completion with export deferred until later.

MoltenVK 1.4.2 can also read a query pool after wgpu has destroyed it. It marks a command buffer's
queries available in a Metal completed handler (`MVKCommandEncoder::finishQueries`) that holds the
pool without retaining it, and its timeline semaphores are MTLEvents that the GPU signals before
Metal runs that handler. wgpu's wait for the work can therefore return first; a query set dropped
then is destroyed, and the handler reads freed memory. On the M5 (2026-10-25), with their case locks
removed, 18 of 30 TAA test runs and 5 of 30 water test runs died with SIGSEGV in that handler, none
without timestamps (`AMORIS_GPU_MINIMAL=features,limits,timestamps`), and a test logger saw each of
a water run's 65 query sets destroyed at its owner's drop. Every timestamp query set is now a
`profiler::TimestampSet` (the profiler's, the path tracers' and the trace clock's), whose drop on
MoltenVK first waits for its queue to go idle through wgpu-hal's `wait_for_idle`: MoltenVK commits
an empty command buffer and waits for its completion, which Metal reports only after the handlers of
the command buffers before it. With the wait, the TAA tests ran 210 times and the water tests 50
times on MoltenVK with parallel cases, and none crashed. One TAA run failed
`picking_and_captures_work_below_scale_one`'s repeated still capture; the 197 runs after it did not
repeat that, and it is not the prepass's or occlusion culling's auto mode switching mid-capture:
each of their four fixed settings draws that still image the same, to the bit. KhronosGroup/MoltenVK
retains the pools since c0af3a5658ba (PR #2839), after 1.4.2; the wait can go once the MoltenVK in
use has it.

## Reproduce

```sh
AMORIS_BACKEND=metal cargo test --release -p amoris-render --test frame_trace \
  --test gpu_profiler --test prepass -- --test-threads=1
AMORIS_BACKEND=metal cargo test --release -p amoris-render --lib \
  gpu_clock_samples_are_current_and_bound_by_the_cpu_brackets -- --test-threads=1
cargo build --release -p amoris-render --example many_cubes
AMORIS_BACKEND=metal AMORIS_AA=msaa AMORIS_GTAO=off \
  target/release/examples/many_cubes --dense --count 200000 --size 1280x720 \
  --headless-bench 300 --occlusion off --prepass auto --settle 2 \
  --trace frames=300,skip=60,out=out/profiler/metal-dense.json
python3 tools/render/frame_trace.py stats out/profiler/metal-dense.json
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

Pass-boundary timestamps may overlap, including across consecutive render passes. The trace
exports the sum of sampled pass intervals and their union coverage separately from the frame
span. None of these measures hardware utilization or removes stalls inside an interval. The
analysis tool derives sum and coverage from raw events, including older traces whose `busy_ms`
field named the interval sum. The signed gap between frame spans can be negative when they
overlap. Individual pass rows remain sampled interval durations, not isolated shader costs.

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

The six `tools/render/prepass_compare.py --backend metal --cases ''` scenes (mixed, cubes, anim,
sailing, gi-room, pt-lab) have zero image or entity-coverage difference. The matching occlusion
comparison preserves every scene's entity coverage; pt-lab has one channel differing by one
8-bit level, all other scenes are identical.

[Raw runs and their
method](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/amoris-metal-profiling-20261010)
are kept in the results repository.
