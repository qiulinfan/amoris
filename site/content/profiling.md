# Profile a rendered frame

Amoris exports the render thread's CPU spans and timestamped GPU passes as Chrome Trace
Event JSON. Each pass carries a frame ID; clock calibration places GPU work on the CPU
timeline. Use this to separate encoding, submission, GPU work, and presentation costs.
The [data page](data.md) keeps recorded results and their limits.

## Record a repeatable workload

From the repository root, build the example and capture 300 frames after 60 warm-up
frames. This shell example selects native Metal on macOS:

```sh
cargo build --release -p pocket-render --example many_cubes
POCKET_BACKEND=metal POCKET_AA=msaa POCKET_GTAO=off \
  target/release/examples/many_cubes --dense --count 200000 --size 1280x720 \
  --headless-bench 300 --occlusion off --prepass auto --settle 2 \
  --trace frames=300,skip=60,out=out/profiler/dense-auto.json
python3 tools/frame_trace.py stats out/profiler/dense-auto.json
```

On Windows, use PowerShell environment assignments and the `.exe` binary; choose
`dx12` or `vulkan`. For example, after the same Cargo build:

```powershell
$env:POCKET_BACKEND = 'dx12'
$env:POCKET_AA = 'msaa'
$env:POCKET_GTAO = 'off'
.\target\release\examples\many_cubes.exe --dense --count 200000 --size 1280x720 --headless-bench 300 --occlusion off --prepass auto --settle 5 --trace frames=300,skip=60,out=out/profiler/dense-auto.json
python tools/frame_trace.py stats out/profiler/dense-auto.json
```

`--headless-bench` waits for GPU completion after each offscreen frame. This is a
controlled render workload, not game FPS: it excludes window presentation. `--bench`
instead runs a native window. Keep resolution, antialiasing, GTAO, occlusion, prepass,
scene, build and adapter fixed when comparing runs. On multi-GPU machines,
`POCKET_ADAPTER` accepts an index or a name fragment such as `nvidia` or `780m`.

To inspect a game in a native window, set `POCKET_TRACE` before launching a built host:

```sh
POCKET_TRACE=frames=300,skip=60,out=out/profiler/sailing.json \
  target/release/pocket play samples/sailing
```

The default trace specification is 300 frames after 60 skipped frames, written to
`out/profiler/trace-<backend>.json`. Tracing stops after that window. Export drains
pending work and writes the file; the recorded Windows exports cost 45–95 ms.
`many_cubes` exports outside its measured frames. A game window exporting during play
can visibly pause once. Clock calibration also waits for completion outside the
measured capture window.

## Read and compare the trace

Open the JSON in [Perfetto](https://ui.perfetto.dev/) using **Open trace file**,
or in Chrome's `chrome://tracing`. Inspect the CPU render-thread lane, GPU pass lanes,
and GPU frame spans together. Select an event to see its frame ID. The JSON's
`otherData` records the backend, adapter, capture settings, missing/dropped GPU frames,
clock uncertainty and whether placement is calibrated. An uncalibrated trace positions
GPU work at submit completion for display; it cannot establish CPU-to-GPU latency.

```sh
python3 tools/frame_trace.py stats out/profiler/dense-auto.json
python3 tools/frame_trace.py diff out/profiler/dense-off.json out/profiler/dense-auto.json
```

For the second command, first repeat the capture with `--prepass off` and save it as
`dense-off.json`. `stats` reports medians, p95, extrema and spreads; `diff` compares
B against A. Alternate run order and repeat runs before interpreting small changes.

| Metric | What it measures |
| --- | --- |
| CPU frame / headless wall time | The host's frame bracket, including submission and its explicit wait |
| Frame interval | Time between consecutive CPU frame starts; window pacing differs from headless waits |
| GPU busy | Sum of timestamped pass durations; coverage depends on enabled pass markers |
| GPU frame span | First timestamped pass start to last pass end, including intervening gaps |
| Submit to GPU start | Calibrated signed offset from CPU submit completion to the first GPU pass; it can be negative |

These metrics are not interchangeable. A pass sum does not measure all device activity,
and a browser's uncapped frame interval can reflect submission pace rather than GPU
completion. Tracing enables a timestamp scope around postprocessing; older benchmarks
without that scope exclude it.
GPU execution can begin before the CPU's `queue.submit` call returns, so a negative
submit-to-start offset is valid.

## Backend checks and deeper captures

Metal and Vulkan through MoltenVK defer query resolve until the sampling submission has
completed. Slots retain their queries, buffers, labels and frame IDs until asynchronous
readback finishes. Native Windows D3D12 and Vulkan retain same-submission resolve.
The [Metal method](https://github.com/qiulinfan/amoris/blob/main/docs/bench/metal-profiling.md)
describes the regression checks, calibration and measured uncertainty.

On Windows, RenderDoc can inspect a single D3D12 or Vulkan frame. Install RenderDoc
and set `RENDERDOC_DIR` to its installation if the tool cannot find it. Example:

```sh
python tools/renderdoc_frame.py capture --backend vulkan --adapter nvidia --frame 70 \
  --name dense-vulkan -- many_cubes --dense --bench 120
python tools/renderdoc_frame.py analyze out/profiler/rdc/dense-vulkan.rdc
```

The analysis reports per-event durations and shader statistics. D3D12 counter replay requires
Windows Developer Mode and an explicit `analyze --d3d12-counters`; without it, the tool skips those
counters. The recorded per-event replay timings cover Vulkan. Static shader instruction counts are
not counts of instructions executed by a frame. See the [Windows profiling
method](https://github.com/qiulinfan/amoris/blob/main/docs/bench/dx12.md#per-frame-profiling-pioneer-2026-10-10)
for the capture limitations.

Keep raw traces, captures and logs in ignored `out/`. Reviewed, privacy-cleaned runs can be
published in [amoris-benchmarks-results](https://github.com/qiulinfan/amoris-benchmarks-results);
the engine repository keeps methods, test fixtures and concise result tables.
