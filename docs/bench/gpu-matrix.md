# GPU tests on both GPUs and both backends (Pioneer, 2026-10-15)

`cargo xtask check` runs `pocket-render`'s GPU tests once, on the default adapter and backend (the
RTX 5060 with Direct3D 12 on the R1 laptop), and a test that skips for want of a GPU or a capability
passes there silently. Wave 4 found tests that failed on master on the Radeon 780M. This page
records the whole matrix before and after this branch, the cause of each failure as far as it was
established, and the tool that runs the matrix again.

Branch `explore/gpu780m`, from master `b7be59a2`. Machine: the R1 laptop (ASUS ROG Zephyrus G14,
Ryzen 9 270, 16 threads, 15 GiB, Windows 11 Pro 10.0.26200), on AC power, Balanced plan; wgpu
30.0.1, DXC 1.8.2502.11 from Windows SDK 10.0.26100.

| Configuration | Adapter, backend, driver | Timestamp period |
|---|---|---|
| `5060-dx12` | RTX 5060 Laptop, Direct3D 12, 32.0.16.1714 (NVIDIA 617.14) | 1 ns |
| `5060-vulkan` | RTX 5060 Laptop, Vulkan, NVIDIA 617.14 | 1 ns |
| `780m-dx12` | Radeon 780M, Direct3D 12, 32.0.13062.3005 | 10 ns |
| `780m-vulkan` | Radeon 780M, Vulkan, AMD proprietary driver 24.30.62.03 (LLPC) | 10 ns |
| `5060-dx12-minimal` | RTX 5060, Direct3D 12, `POCKET_GPU_MINIMAL=features,limits,timestamps` | none |
| `5060-auto`, `780m-auto` | `POCKET_BACKEND` unset as in `cargo xtask check`: Direct3D 12, Vulkan as well where tests compare backends | as above |

The results are correctness results: a test passes or fails regardless of load. The first runs
were made while up to five other agents built and ran GPU work on the machine. The durations of
section 4, the debug run, the `WGPU_DEBUG=1` check of 3 d and the probes of 3 a to 3 c are quiet
re-measurements of 2026-10-15, 16:28 to 16:51, with every other track finished: CPU 3 to 6% busy
at the start of each hold, the RTX 5060 at 54 to 62 C and idle, on AC power, Balanced plan, no
cargo, rustc or Chrome process running; the first, loaded figures are kept beside them where they
differ. The matrix runs, probes and reruns quoted here each held the shared bench lock
(`out/locks/bench_lock.py`), recorded the conditions at their start (`tools/machine_conditions.py`)
and kept their logs in ignored `out/gpu780m/` (`matrix/`, `probes/`, `reruns/`, and `quiet/` for
the re-measurements). Numbers from this track's earlier diagnostic runs, some of which did not hold
the lock and none of which kept their output, are not quoted.

## 1. The tool

`python tools/gpu_matrix.py --configs 5060-dx12,5060-vulkan,780m-dx12,780m-vulkan,5060-dx12-minimal`
builds the test binaries of `cargo test -p pocket-render` (release, as `cargo xtask check` does;
`--debug` for the debug profile) and runs every binary once per configuration with that
configuration's `POCKET_ADAPTER`, `POCKET_BACKEND` and `POCKET_GPU_MINIMAL` (inherited values
cleared; backend `auto` leaves `POCKET_BACKEND` unset), ignored tests included as in
`cargo xtask check`. Each binary runs with libtest's `--show-output`, so the output of passing tests
is kept: a passing test that printed it skipped all or part of its checks for something the device
lacks (no GPU, timestamps, ray queries, shader-f16, `indirect-first-instance`, multi-draw) is `skip`
with the line as its note, and a test still running when the binary times out (300 s; the slowest
takes about a minute) is `hang`. The table lists the tests that did not pass everywhere, with totals
per configuration and the adapter each configuration opened
(`gpu::tests::the_adapter_under_test_is_named` prints it). The exit status is 1 when a test failed,
hung or crashed, and when a configuration opened no adapter or one of its tests found no GPU (a
mistyped adapter would otherwise pass as a column of skips). `--json` keeps the results with the
commit (`-dirty` when the tree had changes), the binaries' build times and inherited `POCKET_*` and
`WGPU_*` variables; `--merge A.json B.json` tables runs made separately, for instance one
configuration per bench-lock hold, naming a configuration that occurs in several files `CONFIG#N`.
`tools/tests/test_gpu_matrix.py` checks the parser, the error rules, the merge and the table on
fixed libtest output. Each run gives every configuration's every binary an empty pipeline archive
directory for AMD's Vulkan driver, which once kept a wrongly compiled pipeline and served it to
every later run of one binary path (section 9). `--configs machine` and `--workspace`, which
`cargo xtask check` runs, are section 11's.

When `POCKET_BACKEND` names a backend, the tests that compare backends (`BackendChoice::under_test`)
run on that one and print the others as not under test; the `auto` configurations run them on every
native backend in one process, as `cargo xtask check` does.

`pocket-render` holds every GPU test of the workspace; the other crates open no device in their
tests. One configuration took 24 to 79 s (release, quiet re-measurement; section 4).

## 2. Before (master `b7be59a2`, release)

The tool and the adapter-naming test only, in one bench-lock hold from 07:47 to 08:11 (24 minutes,
past the wave's limit of about 20: the hung binary below ran until killed, the tool then having no
timeout). Tests that passed everywhere (145 of 167) are left out; `hang` is the run's `taa` binary
on `780m-dx12`, killed after 12 minutes with no CPU use (the run's JSON calls its nine tests without
a result `crash`, the tool's first version having no timeout).

| Test | 5060 D3D12 | 5060 Vulkan | 780M D3D12 | 780M Vulkan | 5060 minimal |
|---|---|---|---|---|---|
| `trace` clock calibration | pass | pass | pass | FAIL (a) | skip |
| `frame_trace` headless trace | pass | pass | pass | FAIL (a) | skip |
| `frame_trace` trace after normal frames | pass | pass | pass | pass | skip |
| `gpu_profiler` asynchronous frames | pass | pass | pass | FAIL (a) | skip |
| `neural_material` constant material | pass | pass | FAIL (c) | FAIL (c) | pass |
| `neural_material` constant on the baseline | pass | pass | FAIL (c) | FAIL (c) | pass |
| `splat::tile` antialias fades subpixel splats | pass | pass | pass | FAIL (e) | pass |
| `taa` object motion reprojects exactly | pass | pass | FAIL (f) | pass | pass |
| `taa`, the other 9 tests | pass | pass | hang (f) | pass | pass |
| `scene_changes`, 2 tests | pass | skip (b) | pass | pass | pass |
| `gpu` first-instance draws | skip (b) | pass | pass | pass | skip |
| `gi::rt` RIS against raw | pass | pass | pass | skip (b) | pass |
| `rt_shadows` ray-traced against cascades | pass | pass | pass | pass | skip |
| totals (167 tests) | 166 pass, 1 skip | 165 pass, 2 skip | 155 pass, 3 FAIL, 9 hang | 160 pass, 1 skip, 6 FAIL | 161 pass, 6 skip |

The `minimal` skips are what that device is for: no timestamps (the profiler, trace and clock
tests), no ray queries, no `indirect-first-instance`. The tool of that day did not count tests that
leave out only part of their checks (f16 without shader-f16, the draw-path variants); they show as
`skip` in section 4. The letters are the sections of 3. Wave 4's other failure, the path tracer on
the 780M (3 d), is a debug-build failure and does not show here.

## 3. Failures and their causes

**(a) The 780M's Vulkan timestamps, resolved in their sampling submission, were wrong.** On the 780M
with Vulkan the clock calibration's first round read begin 0 and end 0, and the profiler's first
frame alike. Two probes, each run in three processes on all four configurations, compare a resolve
in the command buffer that sampled the timestamps with one in a later submission after the sampling
one completed. The pass probe reads the two timestamps of an empty compute pass, an empty render
pass, a compute pass after a render pass and a pair of encoder timestamps, 40 rounds each, each with
a fresh query set, resolve buffer and readback buffer. The calibration probe repeats the trace clock
calibration's shape (an empty timestamped compute pass per round) in four variants of four rounds: a
fresh or a reused query set, with or without a wait for the GPU before the submit. A reading is
wrong when it is zero, repeats a pair read earlier in the process or (calibration) begins before the
previous round. Quiet re-measurement (16:43, CPU 3.2% busy), with the first, loaded run in
parentheses:

| Wrong readings | In the sampling command buffer | After the sampling submission |
|---|---|---|
| 780M Vulkan, pass probe | 70 of 480 (3 zero, 67 earlier pairs); first run 75 (3, 72) | 0 of 480 (0) |
| 780M Vulkan, calibration probe | 17 of 48 (17) | 0 of 48 (0) |
| RTX 5060 Vulkan, 780M and RTX 5060 Direct3D 12, either probe | 0 of 480 and 0 of 48 each (0) | 0 (0) |

The calibration counts apply that rule to the logged rounds; the probe's own tally, which compares
a round only with the rounds of its own variant, counts 10 of 48 (11 in the first run).

The zeros come with a fresh query set's first submission, the earlier pairs at random afterwards: as
if the resolve's copy had not landed when the readback copied the buffer, whose memory a fresh
buffer may share with an earlier round's. Metal and MoltenVK show the same symptom, and the profiler
already resolved there only after the sampling submission completed
([metal-profiling.md](metal-profiling.md)). `profiler::deferred_timestamps` now selects every Vulkan
adapter with AMD's vendor id as well, for the profiler and the calibration alike (charter, the
Pioneer note of 2026-10-15 under the 2026-10-10 entry). Whether AMD's driver or the order of wgpu's
commands is at fault was not established that day: wgpu-hal 30 resets the queries in the same
command buffer and copies the results with `WAIT`, the machine had no Vulkan validation layer (the
Vulkan SDK was not installed), and there was no reproduction without wgpu. Section 8 (2026-10-16)
establishes it: the driver, and the counts above are lower bounds (a wrong reading that repeats a
pair no round had read passes the rule). Deferring is correct on any adapter, at the cost of
readings arriving later, so it covers all AMD Vulkan drivers as a precaution. Before the fix,
`POCKET_TRACE` on that pair had no calibration, and the profiler's pass times there, which the
editor's profile and the prepass `auto` mode read, could be zeros or another frame's. Earlier 780M
Vulkan GPU timings were taken that way and now say so, not measured again: the GPU columns of
[dx12.md](dx12.md) and [splats.md](splats.md), [lod.md](lod.md)'s 8 ms cull pass, the
[occlusion.md](occlusion.md) row whose sums exceeded the frame's wall time and the
[taa-gtao.md](taa-gtao.md) rows. The prepass measurements on the 780M ([prepass.md](prepass.md))
used Direct3D 12.

**(b) Parallel Vulkan device opens sometimes found no adapter.** Tests run on parallel threads, and
in the before run four tests that opened a Vulkan device skipped as if there were no GPU: two
`scene_changes` tests on the RTX's Vulkan, the Vulkan half of the first-instance test under Direct3D
12 and a ray-query test's Vulkan device on the 780M. A probe opened devices in 12 rounds of 12
parallel threads per GPU and backend, without a lock and through `Gpu::headless`, which now holds a
process-wide lock around device creation. Quiet re-measurement (16:45, CPU 3.5% busy), with the
first, loaded run in parentheses:

| Parallel opens failed | Without a lock | With the lock |
|---|---|---|
| RTX 5060 Vulkan | 0 of 144 (1 of 144) | 0 of 144 (0) |
| Radeon 780M Vulkan | 1 of 144 (1 of 144) | 0 of 144 (0) |
| either GPU, Direct3D 12 | 0 of 144 each (0 each) | 0 of 144 each (0 each) |

Each of the three failures was an empty adapter list ("`POCKET_ADAPTER=780m` matches no adapter;
have []", or `5060`). At about one open in 144 or fewer the probe alone cannot show the lock's
effect; the lock removes parallel enumeration by construction, and no matrix run since has had a "no
GPU" skip. The path tracer's tests had a lock of their own for this since 2026-10-09; it is now the
headless constructors'. Tests that compare backends (`BackendChoice::under_test`) also run on the
one backend `POCKET_BACKEND` names when it is set, instead of every native backend, so each column
shows its own backend; the `auto` configurations run them on both.

**(c) The neural constant-material tests ran under TAA on the 780M.** The differing pixels (25 to 29
levels in 70 to 76 pixels, f16 and f32 alike) were one row: the walker view's far edge of the ground
against the sky. An integrated GPU's default antialiasing is TAA since the TAA merge (post.rs
`defaults_for`), a discrete GPU's multisampling, and these tests took the adapter's default. Under
TAA that edge does not settle within the tests' three frames, whatever the material. A probe
compared the inline material with itself at its color times 1 + e, at the tests' fixed time (worst
difference / pixels over 2; e = 0 gave 0 / 0 everywhere). The quiet re-run of both probes (16:50,
CPU 3.4% busy) printed every value of the first run again, digit for digit, as a fixed-time render
should:

| Inline against inline x (1 + e) | e = 0.001 | 0.003 | 0.01 |
|---|---|---|---|
| 780M Vulkan, TAA, 3 frames | 29 / 85 | 28 / 99 | 27 / 85 |
| 780M Direct3D 12, TAA, 3 frames | 28 / 77 | 28 / 89 | 25 / 79 |
| RTX 5060 Vulkan, TAA, 3 frames | 8 / 16 | 18 / 56 | 29 / 115 |
| RTX 5060 Direct3D 12, TAA, 3 frames | 10 / 13 | 19 / 54 | 27 / 119 |
| all four, multisampling, 3 frames | 1 / 0 | 1 / 0 | 1 / 0 |
| 780M Vulkan, TAA, 20 frames | 37 / 122 | 38 / 159 | 39 / 157 |
| RTX 5060 Vulkan, TAA, 20 frames | 17 / 39 | 15 / 85 | 29 / 172 |

The neural material against the inline one, as the tests compare them, three frames: under TAA 25 to
29 / 70 to 76 on the 780M (both backends, f16 and f32) and 7 to 12 / 17 to 21 on the RTX 5060; with
multisampling or without antialiasing 1 / 0 on all four configurations. The walker-view comparisons
now set multisampling; `a_neural_material_draws_under_taa_with_splats` remains TAA's own check, with
its tolerance of 1% of the pixels. The edge's sensitivity grows with more frames at a fixed time, so
it is not only slow convergence; it is an open item of [taa-gtao.md](../spec/taa-gtao.md) 9 for the
TAA track.

**(d) The path tracer on the 780M with Direct3D 12, debug builds only.** pt-lab's mean radiance came
out 0.26 against 0.69: every camera ray missed and showed the sky. In debug builds the instance
carried wgpu's `DEBUG` flag, whose one effect on Direct3D 12 is compiling shaders unoptimized (DXC
`-Od -Zi`). The 780M traces the unoptimized DXIL wrongly and the optimized DXIL of release builds
rightly; `WGPU_DEBUG=1` reproduces the failure in a release build (pt-lab's channel sum 0.2599
against 0.6945 in the recorded run, and the same in the quiet re-run at `6fbedb59`), and the RTX
5060 runs the same unoptimized DXIL rightly. That points at AMD's driver, as with the `front_face`
miscompile the winding test worked around on 2026-10-09
([path-tracing-nrc.md](path-tracing-nrc.md)), but no reproduction outside the engine shows it.
`gpu::instance_flags` no longer sets `DEBUG` where the instance includes Direct3D 12, so debug-build
tests run the DXIL that ships; `WGPU_DEBUG=1` brings the unoptimized shaders back for PIX. What this
costs a debug-profile test run is not measured: one pair of runs on the RTX 5060 (with the throwaway
probes of that day in the tree) took 214 s with optimized and 186 s with unoptimized shaders, inside
the spread of repeated runs (release runs of one commit took 78 s and 120 s).

**(e) `splat::tile` antialias test on the 780M with Vulkan, once.** The tile rasterizer's frames
equalled the frame without splats (image change 0.0 where 0.155 was expected). Not reproduced in 10
recorded reruns of the splat tile tests on that configuration, each with the 4.1M-splat growth test
running beside it as in the failing run (10 more ran without the growth test; all passed), nor in
the five recorded matrix runs of that configuration since, the last on the idle machine. The failing
run had that growth test running in parallel on an integrated GPU whose memory is the system's (15
GiB, other agents building); a failed allocation or pipeline reaches only the device's error log,
which tests do not print. Open.

**(f) The `taa` binary on the 780M with Direct3D 12, once.**
`object_motion_reprojects_a_moving_board_exactly` failed and the other nine tests then made no
progress for 12 minutes with no CPU use, until killed; the failure's message was lost with the
process. No display-driver reset was logged (System log, 07:50 to 08:12). Not reproduced in 5
recorded reruns of the binary (all 11 tests passed each time) nor in the nine recorded matrix runs
on the 780M's Direct3D 12 since (two of them `780m-auto`, two in the debug profile; three on the
idle machine). Open. A hypothesis: one test's device removal would remove every test's device, since
Direct3D 12 keeps one `ID3D12Device` per adapter and process, and could explain both. The matrix now
reports such a run as `hang` within 300 s.

## 4. After (this branch, release)

Commit `f994e00a` (clean tree), 11:39 to 11:46, 168 tests (the profiler's new unit test besides).
Every configuration passes; the skips are the minimal device's capabilities. The quiet re-run at
`6fbedb59` (clean tree, 16:28 to 16:35, one hold of 6 min 53 s) gave the same table, cell for cell;
the duration row is that run's.

| Test | 5060 D3D12 | 5060 Vulkan | 780M D3D12 | 780M Vulkan | 5060 minimal | 5060 auto | 780M auto |
|---|---|---|---|---|---|---|---|
| `trace` clock calibration | pass | pass | pass | pass | skip | pass | pass |
| `frame_trace`, 2 tests | pass | pass | pass | pass | skip (placement, GPU capture) | pass | pass |
| `gpu_profiler` asynchronous frames | pass | pass | pass | pass | skip | pass | pass |
| `gpu` first-instance draws | pass | pass | pass | pass | skip | pass | pass |
| `neural_material` constant material, level of detail | pass | pass | pass | pass | skip (f16) | pass | pass |
| `occlusion`, `prepass` draw paths | pass | pass | pass | pass | skip (first-instance paths) | pass | pass |
| `rt_shadows` ray-traced against cascades | pass | pass | pass | pass | skip | pass | pass |
| totals (168 tests) | 168 pass | 168 pass | 168 pass | 168 pass | 158 pass, 10 skip | 168 pass | 168 pass |
| all binaries, s (quiet) | 62 | 24 | 75 | 32 | 56 | 61 | 79 |
| first run under load, s | 62 | 24 | 80 | 33 | 58 | 67 | 83 |

The skips of the minimal device: no timestamps (the profiler, trace and clock tests), no ray
queries, no `indirect-first-instance` (the first-instance test, and the occlusion, prepass and
draw-path tests' first-instance paths) and no shader-f16 (the neural material tests' f16 half). The
`auto` configurations run the first-instance, ray-query and ray-traced shadow tests on Vulkan and
Direct3D 12 in one process, as `cargo xtask check` does.

The durations of the before and after runs are not comparable: the before run started with the CPU
90% busy (other agents building), the after runs with it 8% busy, and repeated runs of one commit
and configuration under that load varied by half (5060-dx12: 78 and 120 s within minutes). On the
idle machine the after run took 1 to 8% less than its first run (the 780M's Direct3D 12 and the
`auto` configurations most), which changes no conclusion. The debug profile (`63634b87`, which
changed only the text of three skip lines, 11:49 to 11:54) passes all 168 on `5060-dx12` and
`780m-dx12`, the path tracer included; the quiet debug re-run at `6fbedb59` (16:36 to 16:42) passes
all 168 on both again, in 141 s and 170 s (146 s and 180 s in the first run).

## 5. Reproduce (the quiet re-measurement)

From the repository's root (the bench lock is the R1 laptop's ignored `out/locks/bench_lock.py`),
with `~/.cargo/bin` on `PATH`:

```sh
python tools/gpu_matrix.py --build-only          # outside the bench lock
python out/locks/bench_lock.py run --label gpu-matrix -- python tools/machine_conditions.py \
  out/gpu-matrix/conditions-release.json --label gpu-matrix-release
python out/locks/bench_lock.py run --label gpu-matrix -- python tools/gpu_matrix.py \
  --configs 5060-dx12,5060-vulkan,780m-dx12,780m-vulkan,5060-dx12-minimal,5060-auto,780m-auto \
  --json out/gpu-matrix/release.json
python tools/gpu_matrix.py --debug --build-only
python out/locks/bench_lock.py run --label gpu-matrix -- python tools/gpu_matrix.py --debug \
  --configs 5060-dx12,780m-dx12 --json out/gpu-matrix/debug.json
python tools/gpu_matrix.py --merge out/gpu-matrix/release.json --markdown --all
python -m unittest tools/tests/test_gpu_matrix.py
# 3 d: the unoptimized DXIL, still mistraced on the 780M (expect FAIL and exit 1)
WGPU_DEBUG=1 python out/locks/bench_lock.py run --label gpu-matrix -- python tools/gpu_matrix.py \
  --configs 780m-dx12 --bins lib --filter ray_query_pt --json out/gpu-matrix/od-780m.json
```

The seven release configurations took 6 min 53 s in one hold on the idle machine (2026-10-15,
section 4); split them into several holds (`--configs`, then `--merge`) where a hold must stay
short. That quiet run, made with these commands (logs and JSON under `--out` and `--json` in ignored
`out/gpu780m/quiet/` instead of `out/gpu-matrix/`), set the duration row of section 4 and the
sentence on one configuration's time in section 1; the `WGPU_DEBUG=1` run failed as expected, with
exit status 1, and the tool's 6 unit tests passed. The probes of 3 a to 3 c were throwaway tests,
not committed; their source is kept with their logs in the R1 laptop's ignored
`out/gpu780m/probes/`, and the quiet re-run copied it into `crates/pocket-render/tests/` for the
run only (logs in `out/gpu780m/quiet/probes/`).

## 6. Open

- TAA's far-edge sensitivity (3 c, [taa-gtao.md](../spec/taa-gtao.md) 9): a 0.1% change of a
  material's color moves edge pixels by up to 29/255 after three frames and more after twenty.
- Whether the 780M's unoptimized-DXIL tracing (3 d) is a driver fault: no reproduction outside the
  engine. The Vulkan timestamp readings (3 a) are one (section 8).
- What made the driver compile the wrong compute pipeline binary of section 9, and which of the
  renderer's compute pipelines it is.
- Whether the profiler on AMD's Vulkan should copy its readback in a second encoder of the sampling
  submission (section 8: correct there) instead of resolving after the submission completed.
- The one-off splat (3 e) and `taa` (3 f) failures on the 780M: they did not recur in the quiet
  re-run of the matrix (release and debug); install a test logger if they recur, so the device's
  errors reach the log.
- Done 2026-10-16: `cargo xtask check` runs the matrix as its `gpus` step (section 11), the owner's
  decision.

## 7. The trace's clock calibration under CPU load (2026-10-15, `explore/robust`; quiet 2026-10-16)

After the fix of 3 a, the clock calibration test
(`trace::tests::gpu_clock_samples_are_current_and_bound_by_the_cpu_brackets`) failed once in 17 runs
on `780m-vulkan` while another process loaded the CPU, and passed 10 of 10 alone (wave 5,
`out/gpu-matrix/780m-vulkan/lib.log`: the whole lib binary, its tests in parallel, the ray-query
tests on the same adapter among them): the eight rounds' brackets had no common offset. Either a
timestamp could lie outside its CPU bracket under preemption (GPU work misplaced in `POCKET_TRACE`),
or the test's assumption was wrong. In the probe below no timestamp ever left its bracket, and every
calibration without a common offset broke the calibration's own assumption, which the test shares:
one offset for all eight rounds, which the 780M's clock rate breaks once the rounds spread out. The
wave-5 failure's own rounds were not logged, and it did not recur in the runs here: not in the
probe's single-test runs on the 780M's Vulkan under load (0 of 90 in the quiet runs, 0 of 60 in the
first), nor in its own condition, the whole lib binary in parallel under 16 busy loops, run
alternately with `3fdc5b83`'s test and with the fixed one (quiet, 2026-10-16 04:07 to 04:27: 60 runs
each in four holds, 0 failures, 6 to 26 s per run, a median 8; first runs, 2026-10-15 22:45 to
22:54: 15 each, 0 failures, 11 to 38 s per run; `lib_runs.py`). So that failure is put down to the
same cause by analogy, from the calibration-level disagreements below, which only the first runs,
with the other tracks' work on top of the busy loops, produced. The profiler is fixed, and the test
with it.

**Method.** A throwaway probe test (not committed; its source is
`out/robust/probes/probe_test.rs.txt`, pasted into `trace.rs`'s tests for the runs) calls
`calibration_samples` 20 times per process and logs every round's CPU bracket and timestamps, in 30
processes per configuration (10 for the idle runs off the 780M's Vulkan); `calib_runs.py` then runs
the calibration test itself, alone (`--exact`, one thread), 30 times (10). "Load" is one busy-loop
process per logical CPU (16, `burn.py`) for the whole hold. `analyze.py` judges every calibration
again from its raw rounds, with and without the drift allowance below, so every row is judged by the
same rules. The probe calls the fixed `intersect`, so it is built against this track's `trace.rs`;
the fix leaves `calibration_samples` alone, so the rounds do not depend on the build. The old test
is `3fdc5b83`'s, without the allowance, in a release lib binary built from that commit's `trace.rs`
and run with `--procs 0`; the fixed test is this track's. Every hold took the bench lock and
recorded `tools/machine_conditions.py` at its start and end (AC power, Balanced plan).

The table is the quiet re-measurement (2026-10-16, 03:48 to 04:07, every other track finished,
binaries built from `7ea61b0a`): CPU 1 to 6% busy at each hold's start, the RTX 5060 at 59 to 66 C,
no cargo, rustc or Chrome process running. An idle 780M Vulkan hold that began while a game on the
RTX 5060 was closing (53% CPU) was made again at 03:53; its 600 calibrations also all intersected.
The repeat row is a second loaded hold of the same sampling.

| Configuration | Load | Calibrations | Median bracket | Offset drift, ppm (range over processes) | No common offset | With the allowance | Old test failed | Fixed test failed |
|---|---|---|---|---|---|---|---|---|
| 780M Vulkan | none | 600 | 110 us | 1833 (1624 to 1981) | 0 | 0 | 0 of 30 | 0 of 30 |
| 780M Vulkan | 16 busy loops | 600 | 610 us | (too noisy) | 0 | 0 | 0 of 30 | 0 of 30 |
| 780M Vulkan, repeat | 16 busy loops | 600 | 141 us | 1875 (850 to 3062) | 0 | 0 | not run | 0 of 30 |
| 780M Direct3D 12 | none | 200 | 125 us | 1853 (1529 to 1964) | 0 | 0 | 0 of 10 | 0 of 10 |
| 780M Direct3D 12 | 16 busy loops | 600 | 770 us | (too noisy) | 0 | 0 | 0 of 30 | 0 of 30 |
| RTX 5060 Vulkan | none | 200 | 83 us | -17 (-221 to 193) | 0 | 0 | 0 of 10 | 0 of 10 |
| RTX 5060 Direct3D 12 | none | 200 | 102 us | -82 (-181 to -22) | 0 | 0 | 0 of 10 | 0 of 10 |

The offset drift is the slope of a least-squares line through one process's 20 offsets against time
(each at its narrowest round; residuals 2 to 8 us idle): the CPU clock gains that many microseconds
per second on the GPU's timestamps read at the period the device reports. In the loaded Vulkan hold
(not its repeat) and the loaded Direct3D 12 hold the offsets scatter by hundreds of microseconds (a
median largest residual of 283 and 360 us), too much for a slope; the repeat's, 7 us, gives one,
though its range is wide.

The first runs (2026-10-15, 18:21 to 19:21, provisional: the other tracks' work kept 5 to 55% of the
CPU busy at the holds' start, the busy loops ran on top of it) gave the same idle rows within their
spread (780M 1874 and 1851 ppm, brackets 134 and 126 us; RTX 5060 24 and 16 ppm) and differed under
load: the 780M's Vulkan brackets were a median 7.5 ms and 4.4 ms (its repeat), with 4 and 1 of 600
calibrations without a common offset, while its Direct3D 12 kept 126 us brackets.

**Findings.** No round in the 6,000 calibrations was stale, zero or out of order, and no GPU
timestamp lay outside its own bracket. But the Radeon 780M's timestamps run 0.18 to 0.19% slower
than their reported 10 ns period, on Vulkan and Direct3D 12 alike (the counter advances as if at
about 99.82 MHz against the CPU's clock; the RTX 5060's agrees to within the measurement's 0.03%).
An idle calibration's eight rounds span a median 1.9 ms on the 780M's Vulkan (at most 5.9 ms), which
the rate error turns into 3.5 us (at most 11), hidden in brackets 90 to 200 us wide (p5 to p95). On
the quiet machine the busy loops alone spread the rounds on both of the 780M's backends, not only
Vulkan: its Vulkan calibrations span a median 11.8 ms (p95 30 ms, at most 49 ms) in one hold and 2.9
ms (at most 32 ms) in the repeat, Direct3D 12's 7.5 ms (at most 27 ms), and at those distances the
rate error, at most 90 us, stays inside the brackets: no loaded calibration lacked a common offset
(0 of 1,800). Only the first runs spread the Vulkan rounds over hundreds of milliseconds (spans a
median 141 ms, p95 475 ms, at most 679 ms; 126 ms and at most 335 ms in the repeat), so that rounds
disagreed by the rate error times their distance, more than the narrowest brackets leave: 5 of 1,200
loaded calibrations (0.4%). In those 5 the conflicting brackets lay 128 to 305 ms apart, every
conflict was a later round's GPU time too early, by 10 to 150 us, and they need 0.005 to 0.051% of
drift to agree, within the measured rate. The quiet runs change two statements of the first: CPU
load alone did not break a calibration here, and Direct3D 12 is not spared the spreading. What
spread the first runs' Vulkan rounds over hundreds of milliseconds, the other tracks' CPU work or
their GPU work, was not measured; inferred from the bracket widths, the thread waiting for each
round is likely woken late. Wave 5's failure, 1 in 17, also came with other work on the machine.

**What it did to `POCKET_TRACE`.** A refused calibration left the trace with its other calibration
alone, a single offset for every frame, which on the 780M drifts 1.8 ms from the truth per second of
distance from it (the start and end calibrations' interpolation corrects the drift only when both
exist), or with none; and a calibration that still intersected stated a half-width that left out up
to 0.19% of its span (in the quiet loaded Vulkan holds a median half-width of 108 us where the
allowance gives 120, and 59 where it gives 61; in the first, 751 where it gives 825).

**The fix** (`trace.rs`): the calibration's offset holds at the middle of its narrowest bracket, and
every other bracket is widened on both sides by `MAX_CLOCK_DRIFT` (0.5%, 2.7 times the 780M's rate
error) of its farthest distance in time from that moment before the intersection; `at_s` is that
moment, which the start and end calibrations' interpolation uses. The test allows each sample the
same drift. Applied to the same rounds, the rule finds a common offset for all 6,000 calibrations,
and costs quiet idle calibrations 1.6 to 2.4% of their half-width (780M Vulkan 45.8 to 46.9 us, 780M
Direct3D 12 55.2 to 56.1, RTX 5060 Vulkan 38.1 to 38.7, Direct3D 12 44.6 to 45.3). A unit test
builds the 780M's case (0.19% slow, rounds 40 ms apart): no common offset without the allowance, the
true one within the half-width with it. A refused calibration now also logs a warning. With the fix,
`tools/gpu_matrix.py --bins lib,frame_trace,gpu_profiler` passes on `5060-dx12`, `5060-vulkan`,
`780m-dx12` and `780m-vulkan` (134 tests each; 2026-10-15 20:04, in the bench lock).

Reproduce (the R1 laptop; probe source and scripts in its ignored `out/robust/probes/`, the quiet
runs' output in `out/robust/quiet/`): build the old test's binary from
`git show 3fdc5b83:crates/pocket-render/src/trace.rs` and this track's, copying each out of
`target/` (they share its file name), then paste `probe_test.rs.txt` into the tests module of
`crates/pocket-render/src/trace.rs` and build the probe's; from the worktree's root:

```sh
cargo test -p pocket-render --release --lib --no-run      # outside the lock; copy out the binary
P=C:/Users/rynne/Desktop/amoris-pioneer/out
python $P/locks/bench_lock.py run --label calib -- python $P/robust/probes/calib_runs.py $PROBE \
  $P/robust/calib --label idle-780m-vulkan --procs 30 --tests 30
python $P/locks/bench_lock.py run --label calib -- python $P/robust/probes/calib_runs.py $OLD \
  $P/robust/calib --label idle-780m-vulkan-oldtest --procs 0 --tests 30
# both again with --load; with --backend dx12 (--procs 10 --tests 10 idle); and
# --adapter 5060 --backend vulkan|dx12 --procs 10 --tests 10
python $P/robust/probes/analyze.py $P/robust/calib/*-probe.jsonl
python $P/locks/bench_lock.py run --label calib -- python $P/robust/probes/lib_runs.py \
  $P/robust/lib-runs --label load-780m-vulkan --load --runs 15 old=$OLD fixed=$NEW
```

Each calibration hold takes 1 to 5 minutes and each whole-binary hold of 15 runs per build about 5
(9 while the other tracks ran); the loaded ones keep 16 busy loops running, so make them while no
other work is timed. Then remove the probe again (`git checkout crates/pocket-render/src/trace.rs`).

## 8. The 780M's Vulkan timestamps: a driver fault (2026-10-16, `explore/vkvalid`)

Branch `explore/vkvalid` from master `6af291db`; the machine and drivers of the table above (AMD's
Vulkan driver 24.30.62.03 reports API 1.3.302, NVIDIA 617.14 API 1.4.351), with the Vulkan SDK
1.4.363 that the owner approved (scoop, `~/scoop/apps/vulkan/current`). The layers are not
registered with the system, so no other run loads them: `tools/vk_validate.py` gives one command
`VK_LAYER_PATH` (the SDK's `Bin`), `VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation` and
`VK_LAYER_SETTINGS_PATH`, a settings file that enables the core checks with spirv-val,
synchronization validation (`validate_sync`, `syncval_full_validation`), every error, warning,
performance and information message into a log file without a message limit, and no shader
validation cache (which would be written under the user's profile), then counts the log's messages.
Release builds have no wgpu debug messenger, so the layer's own log receives everything. Two
deliberately invalid modes of the reproduction below are the positive controls: timestamps written
without a reset give `VUID-vkCmdWriteTimestamp-None-00830` (6 in 3 rounds), and the copies without
the barrier between them `SYNC-HAZARD-READ-AFTER-WRITE` (3).

**wgpu's commands are valid.** The two probes of 3 a, one process per GPU under validation, logged
no error and no warning on either GPU, while the 780M still read at least 29 of 160 pairs wrong when
they were resolved in the sampling submission (0 of 160 after it completed) and at least 4 of 16
calibration rounds (0 of 16): a zero pair, a pair older than the round before it and two repeats,
one of them of the previous variant's last pair (the probe's own tally, which compares a round only
with its own variant's rounds, said 2). At least, because these probes compare a reading only with
the readings before it, and a wrong reading that repeats a pair no round had read passes them
(below). `VK_LAYER_LUNARG_api_dump` shows what wgpu-hal 30 records for one such round of an empty
timestamped compute pass, which read (0, 0): in one `vkQueueSubmit`, a command buffer with the reset
and the two `BOTTOM_OF_PIPE` timestamps and a later one with `vkCmdCopyQueryPoolResults`
(`TYPE_64 | WAIT`) into the resolve buffer, a buffer barrier (TRANSFER to TRANSFER, `TRANSFER_WRITE`
to `TRANSFER_READ`) and `vkCmdCopyBuffer` into the readback buffer. The specification makes
`vkCmdCopyQueryPoolResults` a transfer operation whose writes "must be synchronized using
`VK_PIPELINE_STAGE_TRANSFER_BIT` and `VK_ACCESS_TRANSFER_WRITE_BIT`", ordered after the commands
that reference its queries earlier in submission order when `WAIT` is set: the barrier wgpu records.
For queries an earlier submission wrote, wgpu-core records the query copy in a command buffer of its
own ("Deferred query set resolve") and the barrier and the buffer copy in the next one; that round
read correct values.

**A reproduction without wgpu.** `crates/pocket-render/examples/vk_timestamps.rs` records the same
sequence through ash (0.38, already in the lock file through wgpu-hal; a dev-dependency for non-wasm
targets) with a fresh query pool per round, a device-local resolve buffer kept from round to round
(a copy that misses the query results reads the previous round's pair) and a host-visible, coherent
readback buffer the host sets to a sentinel before each submission, then variants of it; by default
it runs every variant but the two controls. Each round's reference is the host's own read of the two
queries after the fence (`vkGetQueryPoolResults` with `TYPE_64 | WAIT`, which the `host` mode shows
to be right), so a reading is wrong exactly when it differs from it. Three processes of 200 rounds
per mode and GPU (2026-10-16 11:48, CPU 29% busy and a game running on the RTX 5060; logs in the R1
laptop's ignored `out/vkvalid/ts-truth/`):

| Mode: what lies between the timestamps and the host's read | Wrong on the 780M | RTX 5060 |
|---|---|---|
| `same`: query copy, TRANSFER buffer barrier, buffer copy, in one command buffer (wgpu's resolve) | 519 of 600 | 0 of 600 |
| `deferred`: `same`'s copies in a second submission, after the timestamps' one completed | 530 | 0 |
| `barrier`: `same` with a global ALL_COMMANDS barrier before the query copy | 593 | 0 |
| `global-transfer`: the barrier between the copies a global TRANSFER memory barrier | 100 | 0 |
| `split`: the barrier and the buffer copy in a second command buffer of the same submission | 0 | 0 |
| `buffer-src-all`: only the barrier's source stage changed, to ALL_COMMANDS | 0 | 0 |
| `buffer-src-compute`: only COMPUTE_SHADER added to the barrier's source stage | 0 | 0 |
| `buffer-all`: the buffer barrier with ALL_COMMANDS as both of its stages | 0 | 0 |
| `buffer-compute`: COMPUTE_SHADER and SHADER_WRITE added to the barrier's source | 0 | 0 |
| `global`: an ALL_COMMANDS to ALL_COMMANDS global memory barrier | 0 | 0 |
| `direct`: the query copy straight into the readback buffer | 0 | 0 |
| `host`: `vkGetQueryPoolResults` with `TYPE_64` and `WAIT` after the fence | 0 | 0 |
| `fill`: `vkCmdFillBuffer` in place of the query copy, then `same`'s barrier and copy | 0 | 0 |

All but three of the wrong readings were exactly the previous round's pair, what the resolve buffer
held before the submission; the three were zeros, each process's first round (the resolve buffer's
fresh memory). No sentinel, so the buffer copy always reached the host, and the host's own reads
were nonzero and increasing throughout. Under validation (one process of 100 rounds per GPU) no
error or warning, and the 780M as before: `same` 93 of 100, `deferred` 93, `barrier` 94,
`global-transfer` 15, every other mode 0.

An earlier run of the table (06:49, CPU 4% busy at the start; `out/vkvalid/ts-final/`) had no
reference: it counted a reading wrong when it began before the latest end it had seen. That catches
only the first wrong reading of a series, because the next one repeats a pair no round had read, and
it gave 49, 34, 18 and 25 of 600 for the four failing modes and 0 for the others, as above. Its
counts, those of 3 a and those of the probes above are lower bounds. The rates depend on the
machine's state; which modes fail did not differ between the two runs.

**Through wgpu.** A throwaway probe with the same kind of reference (a later copy of the resolve
buffer in a submission of its own; `out/vkvalid/zz_split_probe.rs`, not committed), three processes
of 200 rounds of an empty timestamped compute pass (11:51, the same conditions): resolved with the
readback copy in the sampling encoder, 419 of 600 pairs wrong on the 780M (416 the previous
round's); with the readback copy in a second encoder of the same `queue.submit`, 0 of 600; resolved
after the sampling submission completed, 0 of 600; the RTX 5060 0 of 600 in each. Under validation,
one process each: 95, 0 and 0 of 200, no error or warning. wgpu-core records the barriers that bring
a command buffer's resources into use in a command buffer of their own ahead of it, so the second
encoder's copy takes the `split` layout.

**The cause.** The 780M's Vulkan driver does not order a buffer copy after a
`vkCmdCopyQueryPoolResults` earlier in the same command buffer when the barrier between them has the
TRANSFER source stage the specification names: in most rounds here the copy read its source before
the query results landed. A barrier whose source stage is ALL_COMMANDS or includes COMPUTE_SHADER
(accesses unchanged) orders them, and so does a command buffer boundary; the same barrier after a
buffer fill works. That fits a driver that writes query results with a compute dispatch its TRANSFER
stage does not wait for (an inference, not checked in the driver). The timestamps themselves are
right: a query copy straight to the host's memory or a host read never failed. The fault is AMD's
driver; wgpu's sequence is valid.

**What it means for the engine.** The deferred path of `profiler::deferred_timestamps` is right for
a reason the 2026-10-15 note did not know: wgpu-core puts the resolve of queries that an earlier
submission wrote into a command buffer of its own. Waiting for the GPU is not what fixes it: the raw
`deferred` mode, both copies in one command buffer after the timestamps completed, fails too (530 of
600). Nothing changes: AMD's Vulkan adapters keep the deferred path (only driver 24.30.62.03 on the
780M was measured). A narrower workaround exists in wgpu's API, the readback copy in a second
encoder of the sampling submission (0 of 600 above); the engine does not use it. The path tracer's
readbacks (`gi/pt.rs`, `gi/rt.rs`) resolve queries that earlier submissions wrote, in an encoder of
their own, so wgpu splits them the same way. The report draft for AMD is 10 A.

## 9. The 780M's Vulkan pipeline archive kept a wrong compile (2026-10-16, `explore/vkvalid`)

After the wave-6 merges, `tests/rt_shadows.rs` failed on the 780M with Vulkan for the main
checkout's `target/release/deps/rt_shadows-f3765e88bb0aa974.exe` ("characters: shadows fall in the
same places (intersection over union 0.191)") every time, while the same file copied elsewhere
passed. It still did on 2026-10-16: 2.7 s, every pipeline found in the driver's cache.

**Where the cache is.** AMD's Vulkan driver keeps the pipelines it compiles in archives
`%LOCALAPPDATA%\AMD\VkCache\<16 hex digits>9113209E63BE76D3[_N].parc`: PAL archive files, chained
entries of a 16-byte key and an LZ4-compressed payload, mostly AMDGPU ELF pipeline binaries with
their PAL metadata (`tools/amd_vkcache.py` lists, compares and copies them). The file name's first
part follows the executable's path: the same binary at another path starts a new archive, another
binary at the same path adds to the old one. Each run of this test wrote to two files, `.parc` and
`_1.parc`, holding mostly the same pipelines (the test opens two devices, one with ray queries;
which file is which device's was not checked). What the keys are made of is not established. They
follow the executable's file name: the main checkout's binary and a replay under the same file name
at another path have the same 638 keys, while an executable named `rt_shadows.exe` has only 164 of
its 337 keys in common with them. They are not the pipeline alone: one pipeline sits under different
keys in the two files, and across populations under load one key held binaries of three different
vertex shaders (by the driver's API shader hash). `amdvlk64.dll` reads `AMD_VK_PIPELINE_CACHE_PATH`
(next to `AMD_VK_PIPELINE_CACHE_FILENAME`, `AMD_VK_USE_PIPELINE_CACHE` and others); with it the
archive goes into that directory (created when missing; a relative path counts from the working
directory) and `VkCache` is left alone. The main checkout's archive was created on 2026-10-15 at
17:12:14, during the wave-5 matrix of `3fdc5b83`, and last written on 2026-10-16 at 05:05:34, during
the wave-6 matrix. It was copied out, read only; nothing in `VkCache` was edited or deleted (the
runs at new paths below added their own archives there).

**Reproduction.** One test, the 780M with Vulkan, each run in the bench lock (logs in the R1
laptop's ignored `out/vkvalid/cache/`):

| Binary and path, with the driver's archive | Result |
|---|---|
| the main checkout's binary at its path, its archive in `VkCache` | FAIL (0.191), 2.7 s |
| the same, `AMD_VK_PIPELINE_CACHE_PATH` at an empty directory | pass, 13.7 s |
| the same, at a directory holding a byte copy of that archive | FAIL (0.191), 4.2 s |
| the copy with one unrelated entry made to miss (its key changed), the control | FAIL (0.191) |
| the copy with the other suspect entry (a depth-only pipeline) made to miss | FAIL (0.191) |
| the copy with the compute pipeline entry below made to miss | pass, 2.7 s |
| the main binary under its file name at a new path, the copy renamed to that path's prefix | FAIL (0.191), 2.0 s |
| this branch's build of `6af291db` there, under the same file name, with the same copy | FAIL (0.191), 1.9 s |
| this branch's build of `9e0bf9f3`, then of `6af291db`, at one new path; `6af291db` at another; each `6af291db` again | pass (5 runs) |
| `9e0bf9f3` on `780m-vulkan`, then on `780m-auto`, then `6af291db` on `780m-vulkan`, in private archives, under the main binary's file name and under `rt_shadows.exe` | pass (6 runs) |

`9e0bf9f3`'s renderer differs from that of `3fdc5b83`, the wave-5 build, only in `trace.rs`. One
entry breaks the test, and it travels with the archive: any `6af291db` binary of that file name
fails from a copy of it.

**When the wrong binary was written.** The archive holds the entries of two runs. The first, the
wave-5 matrix's run of `3fdc5b83` on `780m-vulkan` (2026-10-15 17:12, passed), appended entries 0 to
212 of `_1.parc` (0 to 219 of `.parc`); 131 of the 134 pipeline binaries among them are of pipelines
that only that renderer has (the transparency merge changed `common.wgsl`, which `shaders::source`
puts at the head of every module). The second appended the rest, after a block of non-ELF entries
like the one that opens each file, and holds only pipelines that the `6af291db` renderer builds: the
wave-6 matrix's `780m-vulkan` run of `rt_shadows`, the first run of that build on the 780M's Vulkan
(built at 05:01:02), from 05:05:32 to 05:05:39 on 2026-10-16, when the archive was last written.
That run failed with intersection over union 0.191, and the matrix's `780m-auto` run four minutes
later failed the same way in 1.8 s (`out/wave6/gpu-matrix.json`). The wrong binary, entry 266 of
`_1.parc`, lies among the second run's entries, and no entry of the first holds its pipeline: the
driver compiled it wrongly in the run in which the test first failed, and the archive then served it
to every later run from that path and file name. It is not an earlier build's entry found again
under a key the new build reused, the stale entry this track set out to reproduce.

**What differs.** The wrong entry holds a compute pipeline (64-thread workgroups, 2 user SGPRs, 40
VGPRs). Which of the renderer's it is was not identified: the test's characters are skinned
(`samples/anim/models/hero.glb`), and the skinning pass is a compute pass with 64-thread workgroups,
as is the transparent stage's index reversal, new in that merge. Its binary and the one the driver
compiles for the same key into a fresh archive have the same API shader hash, the same 128-bit
pipeline cache hash in the PAL metadata, identical metadata otherwise and 1664 bytes of code each,
of which 59 of 416 dwords differ (by their encodings, in operand fields such as register numbers;
not disassembled). The same pipeline's entry in the other file (entry 282 of `.parc`), written in
the same run, holds the clean binary, and so does every other archive on the laptop that holds this
pipeline: 171 entries in 97 archive sets in `VkCache` and this track's private directories, from
runs of every track's test binaries whose renderer has the pipeline. By the driver's own hashes both
binaries came from the same SPIR-V and pipeline state; one compile of it came out different, and
nothing in the archive tells the two apart.

**Not reproduced.** What made that compile differ is not established. It was not reproduced by the
replays above (11 runs), by one population after a renderer built first on a device with ray queries
and one after a renderer built first on a device without (the order of the test's two devices), nor
by any other population from scratch: 1 of the 173 compiles of that pipeline found in the laptop's
archives came out wrong. Load makes the archive depend on timing: two idle populations from scratch
were identical (337 keys, equal payloads once an 8-byte field of the non-ELF entries is masked),
while each of 8 populations under 16 busy loops differed from the idle one in 20 to 37 of the 337
entries. Whether those are other compiles of the same pipelines or the same binaries under other
keys was not checked (the vertex shaders above); all 8 passed.

Validation: the test under the Khronos layer with synchronization validation logged, on either GPU,
only `VUID-StandaloneSpirv-None-10684` (343 times on the 780M, 371 on the RTX 5060: spirv-val's rule
against explicit layout decorations on Function-storage variables, which naga's SPIR-V breaks and
wgpu filters from its own messenger, gfx-rs/wgpu#7696) and 144
`Undefined-Value-ShaderOutputNotConsumed` warnings (fragment outputs without an attachment, legal);
the failing run from the archive copy logged the same. The pipelines are otherwise created validly.

**Mitigations.**

- Tests, implemented: `tools/gpu_matrix.py` removes `target/<profile>/gpu-matrix-vkcache/` at the
  start of each run and gives every configuration's every binary an empty archive directory there,
  `<config>/<binary>/`, through `AMD_VK_PIPELINE_CACHE_PATH` (the adapter-naming run, the listing
  and the tests alike; a directory that cannot be emptied stops the run, and `--json` records the
  directory as `driver_cache`). Wave 6 shows what that changes: its `780m-auto` run failed in 1.8 s
  from the archive its `780m-vulkan` run had just written. With the guard every configuration
  compiles afresh, so a wrong compile fails one configuration of one run instead of every later run
  of that binary path. A configuration whose tests also run Vulkan on the 780M compiles those
  pipelines again: in a check of the two at 12:21 (a game running), `rt_shadows` took 16 s on
  `780m-vulkan` and 27 s on `780m-auto`, whose backend comparisons run Direct3D 12 and Vulkan,
  against 4.7 s there in the wave-5 matrix, which shared the archive. The cost, on `780m-vulkan`
  (190 tests, all pass; the 18 binaries' run times summed): 77.7 s in each of two guarded runs,
  against 39.8 s with warm `VkCache` archives and 91.0 s with none (one run each; the cold one was
  the first launch of freshly built binaries), measured 2026-10-16 11:17 to 11:22 on `baef354b` with
  the guard's first version uncommitted (the same work for one configuration), minutes after the
  laptop had probably resumed from sleep; conditions not recorded. Why the cold run took longer than
  the guarded ones is not explained. 53 MB under `target/`. Other drivers ignore the variable.
- Not chosen: running each binary from a copy named by its contents separates builds but not runs (a
  wrong compile would serve every later run of that build, as in wave 6, whose wrong binary came
  from the build's first run) and leaves a new archive in `VkCache` for every build of every binary
  (2 to 8 MB each, never removed).
- Engine, measured, not implemented: an application pipeline cache does not keep the driver from
  serving its archive. A probe build of `6af291db` whose 40 pipeline creations all take a
  `wgpu::PipelineCache` (one per device, created empty, `Features::PIPELINE_CACHE`;
  `out/vkvalid/pc_patch.py`, not committed) failed from the renamed archive copy as the same build
  without it did (0.191, 2.0 s: nothing compiled). A cache the engine kept per build would start
  empty for a new build, which is when wave 6's wrong binary was compiled. wgpu-hal 30 fixes
  `VkApplicationInfo` (application `wgpu` version 1, engine `wgpu-hal` version 2) with no way to
  change it short of creating the instance through wgpu-hal, and whether the archive's keys depend
  on it was not measured. The engine's behavior is unchanged.
- By hand: when a 780M Vulkan test fails for one binary path only, run it again with
  `AMD_VK_PIPELINE_CACHE_PATH` at an empty directory before bisecting (AGENTS.md). `VkCache` is the
  system's: whether to delete the main checkout's archive is the owner's decision.

**Reproduce** (the R1 laptop):

```sh
cargo build --release -p pocket-render --example vk_timestamps
target/release/examples/vk_timestamps --adapter 780m --rounds 200     # section 8's table
python tools/vk_validate.py --log out/vkvalid/same.vvl.txt -- \
  target/release/examples/vk_timestamps --adapter 780m --rounds 100   # validated
python tools/vk_validate.py --log out/vkvalid/ctl.vvl.txt -- \
  target/release/examples/vk_timestamps --adapter 780m --rounds 3 --modes unreset,nobarrier
# Section 9 from a copy of the main checkout's archive (VkCache's D88C37DF34840526*.parc, copied
# to out/vkvalid/drvcache/stale-master) and any 6af291db rt_shadows binary renamed
# rt_shadows-f3765e88bb0aa974.exe in a directory D: a first run with AMD_VK_PIPELINE_CACHE_PATH at
# an empty directory shows D's prefix (the new archive's first 16 hex digits). Then
python tools/amd_vkcache.py seed out/vkvalid/drvcache/stale-master out/vkvalid/drvcache/s PREFIX
AMD_VK_PIPELINE_CACHE_PATH=out/vkvalid/drvcache/s POCKET_BACKEND=vulkan POCKET_ADAPTER=780m \
  D/rt_shadows-f3765e88bb0aa974.exe ray_traced_shadows_fall_where_cascades_do --exact  # FAILED
python tools/amd_vkcache.py seed out/vkvalid/drvcache/stale-master out/vkvalid/drvcache/m \
  PREFIX f297407b                       # the wrong entry made to miss: the same run passes
python tools/amd_vkcache.py diff DIR_A DIR_B   # two archives' entries compared by key
```

The runs above went through the track's scripts in the R1 laptop's ignored `out/vkvalid/`, as
section 7's probes are kept: `run.py` for the repeated and validated runs (the settings of
`tools/vk_validate.py`), `cache_exp.py` and `sandbox.py` for the archive runs, `race.py` for the
populations, `elfmeta.py` for the PAL metadata, and the two throwaway probes `zz_split_probe.rs` and
`pc_patch.py`.

## 10. Draft reports for AMD

Drafts only: the owner decides whether and where to send them. Both are on the R1 laptop: Windows 11
Pro 10.0.26200, AMD Ryzen 9 270 with Radeon 780M Graphics, AMD Software driver 24.30.62.03 (Vulkan,
API 1.3.302; Direct3D 32.0.13062.3005), Vulkan SDK 1.4.363 for validation.

**A. A TRANSFER barrier does not order a buffer copy after vkCmdCopyQueryPoolResults.**

In one command buffer: `vkCmdResetQueryPool` (2 timestamp queries), two `vkCmdWriteTimestamp`
(`BOTTOM_OF_PIPE`), `vkCmdCopyQueryPoolResults`
(`VK_QUERY_RESULT_64_BIT | VK_QUERY_RESULT_WAIT_BIT`, stride 8) into a device-local buffer,
`vkCmdPipelineBarrier` (src `TRANSFER`, dst `TRANSFER`, a buffer memory barrier `TRANSFER_WRITE` to
`TRANSFER_READ` on that buffer), `vkCmdCopyBuffer` into a host-visible coherent buffer and a barrier
to `HOST`/`HOST_READ`; submit with a fence, wait, read. Expected: the two timestamps of this
submission, which `vkGetQueryPoolResults` (`WAIT`) returns after the fence. Actual: in 519 of 600
submissions (3 processes of 200, a new query pool each time), the values the device-local buffer
held before the submission (the previous submission's timestamps, or zeros in fresh memory). The
Khronos validation layer with synchronization validation reports nothing. Correct in 600 of 600
with: the barrier's source stage `ALL_COMMANDS`, or `COMPUTE_SHADER` added to it, accesses
unchanged; the barrier and copy in a second command buffer of the same submission; the query results
copied straight to the host-visible buffer; `vkGetQueryPoolResults`; or `vkCmdFillBuffer` in place
of the query copy with the same barrier. Still wrong with the timestamps in an earlier, completed
submission (530 of 600), with a global `ALL_COMMANDS` barrier before the query copy (593 of 600) and
with a global `TRANSFER` memory barrier in place of the buffer barrier (100 of 600). The RTX 5060 in
the same machine (NVIDIA 617.14) is correct in every variant. Reproduction:
`crates/pocket-render/examples/vk_timestamps.rs` (one file, ash), run as
`vk_timestamps --adapter 780m --rounds 200`, which runs every variant and counts each one's wrong
readings against `vkGetQueryPoolResults`; section 8 has the table.

**B. A compute pipeline compiled once into a wrong binary, which the pipeline archive then served to
every later run.**

A test binary of our engine (wgpu 30 on Vulkan) rendered wrongly in its first run after a rebuild
and in every later run from that path; from any other path, or with `AMD_VK_PIPELINE_CACHE_PATH` set
to an empty directory, it renders correctly. Copying the archive
(`%LOCALAPPDATA%\AMD\VkCache\D88C37DF348405269113209E63BE76D3*.parc`) into such a directory
reproduces the failure, also at another path with the archive renamed to that path's prefix and a
binary of the same file name; changing the key of one entry, a compute pipeline, fixes it. That
entry's ELF has the same API shader hash, the same `.xgl_cache_info.128_bit_cache_hash` and the same
PAL metadata as the ELF the driver writes for the same key from scratch, but 59 of its 416 code
dwords differ. The same pipeline's entry in the archive's other file, written in the same run, holds
the clean binary, as do 171 other entries for that pipeline in 97 other archives on the machine. The
archive's order shows the wrong entry was written in the run that first failed, the first run of
that build from that path; an earlier build's run had created the archive and passed. Runs from
scratch on an idle machine produce identical archives; under CPU load 20 to 37 of 337 entries differ
from run to run. We could not produce that binary again. Validation (Khronos layer, synchronization
validation) reports only `VUID-StandaloneSpirv-None-10684` for every run, failing or not. We can
provide the archive, both ELFs and the test binary. Section 9 has the measurements.

## 11. In `cargo xtask check` (2026-10-16, the owner's decision)

The owner put the matrix into `cargo xtask check` (charter 4.6): its `gpus` step, after `test`,
runs `python tools/gpu_matrix.py --configs machine --workspace` and judges its JSON
([checks-slice1.md](../spec/checks-slice1.md) 24 has the step's rules and codes). Branch
`explore/decisions`, from master `6af291db`.

**`--configs machine`.** The lib binary's `gpu::tests::the_machine_adapters_are_listed` prints every
adapter of every native backend with the index `POCKET_ADAPTER` takes for it there, and the adapter
each backend opens without it. On the R1 laptop:

```text
machine adapter: vulkan 0 IntegratedGpu AMD Radeon 780M Graphics
machine adapter: vulkan 1 DiscreteGpu NVIDIA GeForce RTX 5060 Laptop GPU
machine default adapter: vulkan NVIDIA GeForce RTX 5060 Laptop GPU
machine adapter: dx12 0 IntegratedGpu AMD Radeon 780M Graphics
machine adapter: dx12 1 DiscreteGpu NVIDIA GeForce RTX 5060 Laptop GPU
machine adapter: dx12 2 Cpu Microsoft Basic Render Driver
machine default adapter: dx12 NVIDIA GeForce RTX 5060 Laptop GPU
```

`machine_configs` tells adapters apart by name and names each by a word of its name that no other
adapter's name contains, a model number first (`5060`, `780m`), so one value picks it on every
backend; an adapter without such a word (two of one model) is named by its index on each backend and
gets no `auto` configuration. It runs every adapter on every native backend that lists it; `auto` on
every adapter but the default one, whose `auto` configuration is what `test` runs (its failures fail
`test`, and its skips are its backends' skips, which that adapter's columns show); and the minimal
device on the default adapter with the platform's default backend. Software adapters (device type
`Cpu`) are left out; with none left the tool prints so and exits with status 3, which the step
reports as skipped. On R1 that is the seven configurations of section 4 but `5060-auto`. When `test`
did not run that configuration in the same check (`--only gpus`, `--skip test`, or `POCKET_ADAPTER`,
`POCKET_BACKEND` or `POCKET_GPU_MINIMAL` set in the environment `test` inherits), the step passes
`--default-auto` and the tool runs it as `default-auto`, with no `POCKET_ADAPTER` and no
`POCKET_BACKEND`, as `test` would (after review, 2026-10-16).

**`--workspace`.** `cargo test -p pocket-render` and `cargo test --workspace` build different
binaries: with the workspace's other members, 29 of the 175 crates below pocket-render get features
`-p` leaves out (serde's `rc`, glam's `serde`, `libm` and `scalar-math`, among others; `cargo tree
-e features` both ways), so `-p` builds the renderer's tree again. `--workspace` builds what `test`
built and, after it, builds nothing. It runs the binaries `test` ran, too, which matters: the TAA
test's PSNR at scale 1 printed 40.7 dB with the workspace's binary where the `-p` one printed 40.8
(RTX 5060, Vulkan), as glam's `scalar-math` and `libm` would make it, changing the CPU side's
floating point (not traced further).

**The run.** `cargo xtask check --only gpus` at `d7329c29` with this branch's changes (the wider
upscaling box where nothing moved among them, [taau.md](taau.md) 2), the workspace's tests built
beforehand, in one bench-lock hold, 2026-10-16 06:18 to 06:27, while other tracks built on the
machine (the durations are not quiet measurements): PASS in 8 min 58 s, of which the first 11 s went
to starting, the tool's build (nothing to build) and the listing. 191 tests per configuration, 1,146
cells: 1,135 pass and 11 skip, every skip on the minimal device, each a `gpus.skipped` warning in
the report.

| Configuration | Adapter, backend | Tests | All binaries, s |
|---|---|---|---:|
| `5060-dx12` | RTX 5060, Direct3D 12 | 191 pass | 82 |
| `5060-vulkan` | RTX 5060, Vulkan | 191 pass | 53 |
| `780m-dx12` | Radeon 780M, Direct3D 12 | 191 pass | 121 |
| `780m-vulkan` | Radeon 780M, Vulkan | 191 pass | 79 |
| `780m-auto` | Radeon 780M, Direct3D 12, and Vulkan where tests compare backends | 191 pass | 96 |
| `5060-dx12-minimal` | RTX 5060, Direct3D 12, `POCKET_GPU_MINIMAL=features,limits,timestamps` | 180 pass, 11 skip | 80 |

The minimal device's skips: no timestamps (`frame_trace`'s two tests, `gpu_profiler`, the clock
calibration), no `indirect-first-instance` (the first-instance test and the occlusion and prepass
first-instance paths), no shader-f16 (the neural material's two tests' f16 half) and no ray queries
(`rt_shadows` and the transparency test's ray-traced shadows), the ten of section 4 and the
transparent stage's test added since.

**After review (2026-10-16).** The step now also fails on a test binary that exits with an error
none of its tests explains or that lists no tests (`gpus.binary_failed`), counts skips past the
report's 20 warnings in a last `gpus.more_skipped`, and runs `default-auto` when `test` did not run
it (above). `cargo xtask check --only gpus` at `107cf4a7` (the lib binary has 139 tests now: the
TAA clip box's unit test), the workspace's tests built beforehand, in one bench-lock hold, 07:15 to
07:25: PASS in 10 min 4 s. The first 88 s went to the tool's build and the listing: pocket-app's
test binaries were relinked, since its engine version follows the commit, made after the tests were
built (pocket-render's binaries were up to date); after `test` in a full check nothing is relinked.
The binaries ran 498 s in all. 192 tests per configuration, 1,344 cells: 1,333 pass, 11 skip (the
same eleven on the minimal device), no binary failed; the summary ends "default-auto because the
test step did not run in this check". Not a quiet measurement either (other tracks' state unknown).

| Configuration | Tests | All binaries, s |
|---|---|---:|
| `5060-dx12` | 192 pass | 81 |
| `5060-vulkan` | 192 pass | 32 |
| `780m-dx12` | 192 pass | 95 |
| `780m-vulkan` | 192 pass | 40 |
| `default-auto` (RTX 5060, Direct3D 12, and Vulkan where tests compare backends) | 192 pass | 80 |
| `780m-auto` | 192 pass | 95 |
| `5060-dx12-minimal` | 181 pass, 11 skip | 75 |

Reproduce: `cargo xtask check --only gpus` (the bench lock on the shared machine; build first with
`cargo test --workspace --release --locked --no-run` with the C compilers `cargo xtask` sets, so the
hold times the tests only). The report is `out/check/report.json`, the tool's output and table
`out/check/gpus.log`, its JSON `out/check/gpus.json`, each binary's log
`out/check/gpus/<config>/<binary>.log`; `python tools/gpu_matrix.py --merge out/check/gpus.json
--markdown --all` tables the run again.
