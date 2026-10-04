# Performance reference measurements

Status: Draft, slice 0

Charter: 3.10 (benchmarks of the hot paths: time per tick, fork time, observation time, frame time
at the target resolution, triangle and draw-call scale of a high-poly scene; results recorded with
each commit on reference hardware to catch regressions and guide optimization; measurements, not
pass conditions; no silent capacity limits; a profiler), 4.2.3 (performance expectations assume an
interpreter), 4.2.7 (the rule workload), 6.3 (time per tick, fork time and web build size compared
across the two lines), 12 item 2 (the reference scenes and hardware).

This specification is the framework the performance measurements live in: what is measured, on which
hardware, with which workloads, how, with which statistics, how noise is kept out, how a reference
figure is set and changed, and how the check reports a run against it. A **budget**, the name the
other specifications use, is one named measurement of section 5 with its reference figure. Since
charter 0.4 no budget is a pass condition (3.10): the `perf` step reports every measurement beside
its reference figure and the accepted baseline, warns on drift, and never fails the check or makes
it inconclusive (`checks.md`, 9 and 11). The integration step of slice 0 filled the reference
figures from the spikes' reports under `docs/spikes/` by the rule of 8.1 (section 10 says which
spike set which); a row no spike measured says "not measured" and the measurements nearest to it,
and the `perf` step reports it with the warning `perf.budget_unset` until slice 1's first calibrated
run sets it. Section 5 gives, beside each figure, a recommended ceiling derived from what the
product needs, the target optimization works toward; a spike measurement above its ceiling is a
finding for the owner (section 13).

Related: `checks.md`, 9 (the `perf` step that runs this), `threads.md` (the thread measurements),
`architecture.md`, 7.2 (the profiles timings use). Tick and `TickRate` are spec-sim's
([simulation.md](simulation.md)); snapshot, fork, restore, the world hash and replays spec-persist's
([persistence.md](persistence.md)); the script workload's host API and its step counts spec-script's
([script-host.md](script-host.md)). Those specifications name the budgets they depend on by the
names of section 5 (section 11).

## 1. Lessons that shape the framework

- **Numbers taken beside other work are worthless.** "GPU numbers are worthless while a benchmark or
  a browser page drawing a pack runs (a 2.85 ms measurement taken beside a suite was 0.49 ms alone)"
  (master `docs/development.md`, Pictures); master's rendering assessment ran beside a benchmark at
  load averages of 5 to 13 and called every timing noisy
  (`docs/research/2026-10-02-rendering-and-import-assessment.md`). Hence calibration, and noisy runs
  that are reported but never become a baseline (7.5).
- **Debug builds measure nothing useful.** Master's debug build was 25 to 31 times slower than
  release (`docs/decisions/0006-agent-runs-release.md`). Timings come from `release` and `web`
  profiles only.
- **Size and speed trade.** Master's `-Oz` web build was 16 to 19% smaller and 18 to 35% slower in
  simulation, and was not taken (`docs/web.md`, Two runtimes). Web size and web tick time are
  measured together so the same trade is made with numbers.
- **Silent limits hide as performance.** Master's renderer dropped draws past 65,536 object rows
  "silently, with no counter" (the assessment's Gaps table). Capacities report counters, each
  capacity's test holds it to growth or a structured error, and the `perf` step reports any drop
  (6).
- **Start-up is a cost agents pay often.** Master's Direct3D 12 first frames took 1.15 to 2.42 s
  after pipelines were made on first use (2.05 to 3.13 s before; `docs/development.md`, Current
  state), and its benchmark harness started a runtime per task.

## 2. What a budget is

```rust
// bench/budgets/*.toml hold these, one file per area of section 5 (sim, threads, render, web,
// start, contract); xtask and `pocket bench` read the directory
pub struct Budget {
    pub name: String,          // "tick.sail"
    pub workload: String,      // a workload of section 4: "sail", "rules", "bodies", "scene", ...
    pub metric: Metric,        // what is measured (section 7)
    pub target: Target,        // Native or Web
    pub stat: Stat,            // Median, P95, P99, Max or Value
    pub unit: Unit,            // Ms, KiB, Count or Percent
    pub kind: LimitKind,       // Ceiling: a cost, lower is better. Floor: a scene's minimum scale
    pub limit: f64,            // the reference figure a run is reported against, never a gate
    pub warmup: u32,           // samples discarded first
    pub samples: u32,          // samples measured
    pub hardware: String,      // "r1" (section 3)
    pub reason: String,        // why this number: the product need, or the measurement it came from
    pub set: String,           // when and from what: "2026-10-04, docs/spikes/threads.md, median x 1.25"
}

pub enum Metric {
    TickTime, ScriptShare, ForkTime, ForkMemory, RestoreTime, ReplaySize, HashTime, PublishTime,
    CommandRoundTrip,
    StallFrameTime, StallTickDrift, ImportTime, ImportFrameMax, TickTimeDuringImport,
    DebugIdleOverhead, FrameCpu, FrameGpu, PassCpu, PassGpu, SceneTriangles, SceneInstances,
    DrawCalls, TrianglesSubmitted, Dropped,
    WasmRaw, WasmBrotli, PageBrotli, SnapshotPost,
    StartHeadless, StartWindow, StartWebCold, StartWebWarm,
    CheckQuick, CheckFull, CheckPerf, BuildWebMemory, BenchSelfTest, WebSwap, BranchReady,
    MainLatenessDuringBranch,
    // the shared contract's (5.6)
    ActCall, IntentTick, PerceptionTick, Observe, StepOverhead, StepWall, ResumeLatency,
    McpObserve, McpTickOverhead,
}
```

A **ceiling** row is a cost, where lower is better: a run reports its measurement beside the
reference figure, and one past it with the warning `perf.budget_exceeded`, a possible regression to
look into. A **floor** row is a scale a benchmark scene must at least have, so that a change to the
scene cannot make a cost look better than it is (the triangles and instances of a high-poly scene,
charter 3.10); a scene below it is reported with `perf.scale_below_floor`, and its measurements are
not comparable with the reference. Each budget names one statistic: medians for throughput, p95 or
p99 for what a person feels as hitches.

## 3. Reference hardware

Reference figures are absolute numbers measured on one machine, **R1**, the owner's laptop on which
slice 0 runs. Facts measured on 2026-10-03:

| Part | Value | Measured with |
|---|---|---|
| Machine | ASUS ROG Zephyrus G14 GA403UM | `Get-CimInstance Win32_ComputerSystem` |
| CPU | AMD Ryzen 9 270 w/ Radeon 780M Graphics: 8 cores, 16 threads, 4001 MHz reported | `Get-CimInstance Win32_Processor` |
| Discrete GPU | NVIDIA GeForce RTX 5060 Laptop GPU, driver 32.0.16.1714 | `Get-CimInstance Win32_VideoController` |
| Integrated GPU | AMD Radeon 780M Graphics (RDNA 3), driver 32.0.13062.3005 | same |
| Memory | 16,388,423,680 bytes (15.3 GiB) | `Win32_ComputerSystem.TotalPhysicalMemory` |
| OS | Windows 11 Pro 10.0.26200 | `Get-CimInstance Win32_OperatingSystem` |
| Browser | Google Chrome 154.0.8037.93 | `(Get-Item chrome.exe).VersionInfo.ProductVersion` |
| Power | on AC (`Win32_Battery.BatteryStatus` 2), Windows power plan Balanced | `Get-CimInstance Win32_Battery`; `powercfg /getactivescheme` |

- **Native GPU work** runs on the discrete GPU: `wgpu` asked for `PowerPreference::HighPerformance`,
  on Direct3D 12, chosen explicitly (with both enabled `wgpu` 30 picked Vulkan). The render spike
  proposed it: on the RTX 5060 Vulkan was within 7% of it, and on the Radeon 780M Direct3D 12 was 25
  to 53% faster at full detail; with levels of detail the two were within noise on both GPUs (its
  verification), and Direct3D 12 is what Chrome's Dawn uses on Windows. The adapter's name is
  checked at the start of every frame benchmark.
- **Web GPU work** runs in headless Chrome, which draws on the integrated Radeon 780M on this
  machine (the environment the slice 0 agents were given; Chrome 154 ignores `powerPreference` on
  Windows, crbug.com/369219127). Web frame figures are therefore integrated-GPU figures, which is
  also closer to a typical player's laptop.
- **Measuring conditions**: on AC power, the power plan recorded, no other build, benchmark, browser
  page or GPU workload running (7.5 detects violations it can).

The fingerprint the check records with every run:

```rust
pub struct Machine {
    pub id: String,                 // "r1" when cpu and the discrete GPU match R1, else "other"
    pub cpu: String,
    pub logical_cores: u32,
    pub gpus: Vec<GpuInfo>,         // name, driver version, backend, as wgpu reports them
    pub os: String,
    pub on_ac_power: Option<bool>,  // Windows: GetSystemPowerStatus; None where unknown
    pub power_plan: Option<String>,
    pub chrome: Option<String>,
    pub rustc: String,
}
```

A driver, OS or Chrome version different from the recorded one does not change `id`; the report
carries a warning, and a measurement that moves after such an update is investigated before the
baseline (8.3) is accepted again.

## 4. Workloads

Each workload is a project under `bench/` (or a sample it names) with a fixed seed and a fixed input
file in the command form of `checks.md`, 8.1, so every run computes the same ticks and timings
compare like with like. A workload is deterministic by the same checks as any project.

| Workload | Content | Why |
|---|---|---|
| `sail` | Exactly the shared benchmark's `island` fixture (shared/benchmark/tasks.md 1): the sea at height 0, the Isle and two islets with colliders, and one boat, the Sloop, at (0, 400), bow north, sail furled; `Breeze` fixed at `wind_from_deg` 270 and 6 m/s (a westerly, blowing toward +x); seed 1; no marks or crates. `bench/sail/inputs.jsonl` hoists the sail at tick 1 and trims and steers at the ticks it lists (the commands of checks.md 8.1). 120 ticks warm-up, 600 measured | The showcase's simulation (charter 2.4.1, 10) |
| `rules` | 1,000 entities that each tick read components, compute damage, write back and emit events, through the script host's batch API (charter 4.2.7, "timed per tick including boundary calls") | Script cost in an interpreter (charter 4.2.3) |
| `bodies` | 200 floating rigid bodies under buoyancy and wind (chosen: the physics spike ran 7 and left hundreds unmeasured) | Physics scale |
| `churn` | persistence.md 12's fixture: 10,000 entities spawned and despawned | Snapshot and fork at crowd scale |
| `scene` | The rendering benchmark: the sea, 1 island (64 terrain tiles) and 32 high-poly boats as the render spike's generated asset holds them, and vegetation instances (not in that asset yet), with a camera that turns a full circle over 300 frames after 30 of warm-up (the spike's `harbour-turn`: static cameras miss the culling and level churn; master measured static and turning separately, the assessment's import table) | Frame time at scale (charter 3.10, 4.4) |
| `import` | The render spike's `scene.glb`: 122,428,588 bytes, 4,681,602 unique triangles (48,230,402 over its instances), written by its deterministic `gen` (SHA-256 `d45b29fd...` on R1; the generator uses the platform's `sin`, so bytes on another OS are not guaranteed), imported while `sail` runs in real time | Import never freezes the runtime (charter 4.4) |
| `stall` | The two stall tests of `threads.md`, 11 | The thread split's promise (charter 5.1) |

Large assets for `scene` and `import` follow the charter's open question on where they live (12,
item 3); the workload names them by content hash.

## 5. The budgets and their reference figures

All on R1. Native rows use the `release` profile, web rows the `web` profile in Chrome. The
recommended ceilings assume the projects' default `TickRate` of 60 (simulation.md 3.1, a 16.7 ms
period); a workload with another rate scales them with its period. A reference figure is
`min(ceiling, measured × 1.25)` rounded up to two significant digits (8.1), from the spike named
beside it; every slice 0 measurement was taken while other agents' builds shared the machine, so the
first calibrated `perf` run of slice 1 replaces each. "Not measured" names what slice 0 has nearest
instead. None of these figures is a pass condition (charter 3.10).

### 5.1 Simulation, native

| Budget | Workload, metric, statistic | Reference figure (from) | Recommended ceiling and reason |
|---|---|---|---|
| `tick.sail` | sail, tick time, median | Not measured: no spike ran the sail workload with scripts, intents and perception. The physics spike's sailing scene (forces and Rapier, one boat, six crates) took 26.7 to 27.4 µs a tick (mean) | 4.2 ms: a quarter of the period, so an agent can fast-forward at four times real time |
| `tick.sail.p99` | sail, tick time, p99 | Not measured; the physics spike's p99 was 31.6 to 38.1 µs, its maximum 51 to 77 µs | 8.3 ms: half the period, no visible hitch in real time |
| `tick.sail.script` | sail, the script systems' share of a tick (the time `StepHooks::system` measures around each `script:<name>` system with the injected clock, script-host.md 9), median | Not measured: no sailing scripts exist yet | a third of `tick.sail`: rules stay out of hot paths (charter 3.10, 4.2.4) |
| `tick.rules` | rules, tick time with boundary calls, median | **4.0 ms** (script-native spike, QuickJS-ng built by clang-cl as `architecture.md` 7.3 requires: medians 2,985 and 3,169 µs; its verification 2,416 and 2,467 µs; MSVC builds 2,872 to 3,317 µs) | 4.2 ms, as `tick.sail`. The measurement is within it, but scripts at this scale take a fifth of a 60 Hz tick: the charter's rule that hot logic moves into Rust primitives is load-bearing |
| `tick.rules.p99` | rules, tick time, p99 | **6.5 ms** (script-native spike, clang-cl: 4,678 and 5,172 µs; MSVC 3,922 to 4,510 µs) | 8.3 ms; the script engine's garbage collection shows here |
| `tick.bodies` | bodies, tick time, median | Not measured at 200 bodies; the physics spike's Rapier step for 7 bodies took 8.0 to 8.5 µs | 8.3 ms |
| `hash.sail` | sail, world hash per tick, median | Not measured (proxy: the physics spike's whole-state hash of its scene, bincode and FNV-1a over 23.7 KB, 20.5 to 21.0 µs; the PCE and XXH3-128 hash of persistence.md 5 was not measured, and the sail workload's other components were not in that scene) | a tenth of `tick.sail`: every recorded run hashes every tick |
| `fork.sail` | sail at tick 300, `persistence::fork` alone, median of 100 | Not measured (proxy: the physics spike's serialize 3.8 to 4.9 µs and restore 15.9 to 23.4 µs natively, through bincode; a `Clone` fork took 5.3 to 5.7 µs) | no more than `tick.sail`: the game thread's share of opening a branch costs no more than a tick |
| `branch.sail` | sail at tick 300, a fork made ready to step: `fork.sail` plus instantiating the bundle in a new host, or reusing the game thread's host when the branch steps there (threads.md 3.6), median of 20 | Not measured (nearest: the script-native spike instantiated its five modules with the freeze in 4.2 to 5.8 ms, about 100 times the proxy of `fork.sail`) | 16.7 ms: a branch or a trial is ready within a frame, so fork-based lookahead (charter 9.2) is bounded by the ticks it simulates |
| `fork.sail.memory` | sail, bytes allocated per fork | Not measured; the physics spike's snapshot was 23,764 bytes at tick 400 | small enough for 16 forks (shared/contract/mcp.md's branch limit) in the 4 GB this machine has free |
| `fork.rules` | rules, fork, median of 100 | Not measured | no more than `tick.rules` |
| `fork.churn` | churn (10,000 entities spawned and despawned, persistence.md 12's fixture), fork, median of 20 | Not measured; the threads spike encoded and hashed 10,000 entities of 20 bytes (200,064 bytes) in 29.7 µs p50 | 16.7 ms: a crowd-scale world still forks within a frame |
| `restore.sail` | sail, `restore` with `verify` from a snapshot, median of 20 | Not measured (proxy: the physics spike's restore 15.9 to 23.4 µs plus its whole-state hash 20.5 to 21.0 µs) | twice `fork.sail`: seeking in a replay restores a keyframe |
| `replay.size.sail` | sail, replay bytes per minute of play with section digests and no keyframes | Not measured | small enough to keep every benchmark match (persistence.md 14, item 6) |
| `publish.sail` | sail, snapshot publication on the game thread, paced at 60 Hz, median | Not measured (proxy: as `hash.sail`, since a publication is a `snapshot` with its digests; measured back to back, and the threads spike found paced costs 1.5 to 2.3 times higher) | a tenth of `tick.sail` (`threads.md`, 4.3) |
| `publish.rules` | rules with all 1,000 entities changed each tick, median | Not measured on the rules world; the threads spike's 1,000 entities of 20 bytes took 2.7 µs p50 to encode and hash and 0.2 µs to swap | a tenth of `tick.rules` |
| `publish.churn` | churn, `snapshot` of the 10,000-entity world, median | **0.038 ms** (threads spike, 10,000 entities of 20 bytes: snapshot and hash 29.7 µs p50, swap 0.2 µs; the churn fixture's bytes per entity are not fixed yet) | 4.2 ms: a quarter period, so a crowd-scale world can still publish every tick |
| `debug.idle` | rules, tick time of the debug-capable build not attached, over the stock build of `bench/stock/` (a standalone crate with its own `[workspace]`, its own `Cargo.lock` and no `[patch.crates-io]`, running the rule workload on unpatched QuickJS-ng, since the workspace's patch makes a stock build impossible inside it; the debugger spike needed the same, `spikes/debugger/bench-stock`) | **1%**, unconfirmed (debugger spike: 0.98 and 1.00 times stock with the handler compiled in and not installed; its verification 1.008 to 1.032, at about ±3% resolution on the loaded machine; finding 1, section 13) | 1%: the point of quickjs-ng PR #1421's runtime-optional handler is avoiding the 3.5% slowdown measured otherwise (charter 4.2.6) |

### 5.2 Threads and import, native

| Budget | Workload, metric, statistic | Reference figure (from) | Recommended ceiling and reason |
|---|---|---|---|
| `rtt.paused` | sail paused, editor stub Read and Write round trip, p95 | **0.15 ms** (threads spike, paused, 100,000 entities: replies 0.051 ms p50, 0.118 ms max; its verification 0.052 ms) | 1 ms: an edit shows in the next frame |
| `rtt.running` | sail at 60 Hz, the same, p95 | **2.1 ms** (threads spike, commands applied on arrival with 1 ms sleep slices: replies 1.57 to 1.62 ms p95) | one period: a command waits at most for the tick in progress |
| `branch.main_lateness` | sail at 60 Hz in real time while a branch steps 600 ticks (threads.md 3.6), main's tick lateness against its due instant, p99 | Not measured | 8.3 ms: lookahead never makes main hitch visibly |
| `stall.frame` | stall, presenter frame time (a 4 ms stub frame) while the game thread blocks 2 s, p95 | **5.7 ms** (threads spike: 4.54 ms; its verification 4.55 ms) | 16.7 ms: the editor stays at 60 frames per second (charter 5.1) |
| `stall.drift` | stall, the game's ticks per second while the presenter blocks 500 ms per frame, deviation | **1%** (threads spike: 168 of 168 ticks, interval p50 16.673 ms against 16.667; one tick in 168 is 0.6%) | 1% |
| `import.highpoly` | import, command to `asset ready`, median of 3 | **4.8 s** (render spike, 4 builder threads, RTX 5060 Direct3D 12 with 256 MiB memory blocks as its recipe sets them: 3,134, 3,772 and 4,051 ms; with default blocks 3,307 ms median) | from the render spike; master took 2.2 s for 5M OBJ triangles after its fixes (the assessment). A cooked asset should fall to its read and upload |
| `import.frame_max` | import, the presenter's longest frame during the import, RTX 5060 | **8.5 ms** (render spike with 256 MiB blocks: 5.2 to 6.8 ms; with default blocks 9.5 ms on the RTX 5060 and 43.5 to 51.9 ms on the Radeon 780M, which this budget would have caught) | 16.7 ms: the frame |
| `import.tick_max` | import, the largest tick time of `sail` during the import | Not measured: the render spike ran no simulation beside its import | `tick.sail.p99`: the import never stalls a tick |

### 5.3 Rendering, native

On the discrete GPU at 1920 by 1080 (the render spike's resolution), rendered offscreen so the
measurement is work and not waits for the display (7.3).

| Budget | Workload, metric, statistic | Reference figure (from) | Recommended ceiling and reason |
|---|---|---|---|
| `frame.gpu` | scene, GPU time per frame, p95 | Not measured: the render spike drew opaque geometry only (no sea, shadows, vegetation or post-processing) | 16.7 ms: 60 frames per second at 1080p; the owner may raise the target to the panel's refresh rate |
| `pass.opaque.gpu` | scene's opaque geometry pass with culling and levels at 1 pixel, GPU time, p95 | **0.23 ms** (render spike, Direct3D 12, turning camera: 0.18 ms) | the opaque pass's own share of `frame.gpu` |
| `frame.cpu` | scene, presenter CPU time per frame, p95 | Not measured for the whole frame | 8.3 ms: half a frame, the rest for the editor's interface |
| `pass.opaque.cpu` | the same pass: camera, culling, level choice, instance data and `submit`, p95 | **0.29 ms** (render spike: 0.23 ms; 1.43 ms with 2,000 more yachts drawn one by one) | the pass's share of `frame.cpu` |
| `scene.triangles` (floor) | scene, triangles before culling and LOD | **48,000,000** over instances (4,680,000 unique), the render spike's designed asset | the showcase's high-poly scale (charter 2.4.1) |
| `scene.instances` (floor) | scene, mesh instances | **97** instances (161 instance-parts) | as above |
| `frame.draws` | scene, draw calls per frame, max | **85** (render spike: 68 at the overview camera, 63 on the turn; 70 with 2,000 more yachts) | GPU-driven drawing is the goal (charter 4.4), so draws do not grow with instances |
| `frame.triangles` | scene, triangles submitted after culling and LOD, max | **670,000** (render spike: 531,426 on the turn) | what LOD and culling must bring the floor down to |
| `frame.dropped` | scene, the sum of every capacity's drop counter | 0 | fixed: no silent limits (charter 3.10, section 6) |
| `start.window` | scene, process start to the first frame presented, median of 5 | Not measured: the render spike rendered offscreen only | 2,000 ms: master's Direct3D 12 first frames after lazy pipelines (section 1) |

### 5.4 Web

In Chrome on the integrated GPU, the shipped page (no editor, no transpiler, `architecture.md`,
8.3).

| Budget | Workload, metric, statistic | Reference figure (from) | Recommended ceiling and reason |
|---|---|---|---|
| `web.tick.sail` | sail in the worker, tick time, median | Not measured; the physics spike's scene took 27.3 to 28.3 µs a tick in the worker (29.9 to 30.4 after `wasm-opt -O3`) | 8.3 ms: real time with room for posting snapshots |
| `web.tick.rules` | rules in the worker, median | Not measured: the rule workload was not built for wasm32. The script-web spike's own workload ran in the browser at 1.0 to 1.1 times its MSVC native time (0.94 to 1.32 by round) | 8.3 ms |
| `web.fork.sail` | sail, fork in the worker, median | Not measured (proxy: the physics spike's plain wasm32 build, serialize 5.6 to 5.9 µs and restore 20.7 to 25.7 µs after 20 warm-up calls; the first restores took 200 to 540 µs) | no more than `web.tick.sail` |
| `web.swap` | rules, instantiating a `Compiled` bundle at a boundary in the worker (hot-update.md 4.2), cold (first in the page) and warm, median of 5 each | Not measured (nearest: the script-web spike's context, prelude, scripts and freeze took 0.7 to 1.6 ms warm and 13.6 to 20.6 ms cold) | one worker slice, 8 ms, warm (threads.md 7.3); cold reported |
| `web.snapshot` | sail, worker to page: copy out, transfer, rebuild, median | **0.25 ms** (threads spike, 1,000 entities of 20 bytes, about the sail snapshot's size: 0.20 ms p50) | 2 ms: well inside a 60 Hz frame (`threads.md`, 7.3) |
| `web.frame` | scene at 1280 by 720, frame time, p95 | Not measured for the whole frame (opaque geometry only) | 33.3 ms: 30 frames per second; the web is a distribution and spectating channel (charter 11) |
| `web.pass.opaque` | the opaque pass at 1280 by 720, GPU time, p95 | **0.92 ms** (render spike: 0.73 ms on the turning camera) | the pass's share of `web.frame` |
| `web.size.wasm.br` | the page's `.wasm`, Brotli quality 11, KiB | Not measured: no spike built the assembled module or measured Brotli (7.4 lists the spikes' gzip sizes) | 2,400 KiB: below master's JSPI runtime at 2.4 MB with Brotli (`docs/web.md`, Costs); compared across the two lines (charter 6.3) |
| `web.size.wasm` | the same, uncompressed, KiB | Not measured (7.4) | from the build; compile time and memory follow it |
| `web.size.page.br` | everything the page fetches but the project, Brotli, KiB | Not measured | the wasm plus 100 KiB of glue, page and worker script |
| `web.start.cold` | sail, navigation to the first frame, fresh profile, median of 3 | Not measured; the script-web spike's worker took 66.8 to 87.8 ms from creation to its first tick, cold (one load at 128.9 ms under load) | 5,000 ms from a local server |
| `web.start.warm` | the same, second load in the same profile | Not measured | 2,000 ms |

### 5.5 Start-up and the check itself

| Budget | Metric | Reference figure (from) | Recommended ceiling and reason |
|---|---|---|---|
| `start.headless` | sail, process start to the ready line, median of 5 | Not measured; the script-web spike's native process ran one tick and exited in 14.1 to 25.1 ms, and the script-native spike loaded its five modules in 4.2 to 5.8 ms | 1,000 ms: agents and the benchmark harness start a runtime per task |
| `check.quick` | `cargo xtask check --quick` after a one-file change | Not measured: the command does not exist yet | 180 s: an agent's inner loop |
| `check.full` | `cargo xtask check --skip perf` after a one-file change | Not measured | 900 s: master's whole suite took 8 to 12 minutes (`docs/development.md`, Tests) |
| `check.perf` | the `perf` step alone, which must run alone anyway (`checks.md`, 9) | Not measured | 600 s |
| `build.web.memory` | the peak resident memory of the fat-LTO web-profile build of `pocket-web` (`checks.md`, 7.1), MiB | Not measured | 3,000 MiB: it must fit beside other work in the about 4 GB this machine has free |
| `bench.selftest` | the benchmark harness's self-test (`checks.md`, 8.7) | Not measured: the harness is not built yet | 60 s, as shared/benchmark/README.md, 8 proposes |

### 5.6 Budgets the shared contract defines

The shared contract names budgets of its own and asks for them to be rows of this framework; they
are measured and reported like every row above, on the `sail` workload unless the row says
otherwise, and their reasons are in the file named. No slice 0 spike built the interface layer, so
none was measured.

| Budget | Metric (owner) | Reference figure |
|---|---|---|
| `act.call` | validating and applying a call of 4 actions (shared/contract/actions.md, Performance) | Not measured |
| `intent.tick` | the `interface.intents` system per tick with the three sailing intents active (actions.md) | Not measured |
| `perception.tick` | the `interface.perception` system per tick, one skipper (shared/contract/perception.md, Performance) | Not measured |
| `perception.npc` | the same with 100 NPC observers over 1,000 perceivable entities (perception.md) | Not measured |
| `observe.sail` | `observe` at the default budget, text projection (perception.md) | Not measured |
| `step.overhead` | the time controller's per-tick overhead in `step` (shared/contract/time.md, Performance) | Not measured |
| `step.3600` | `step {ticks: 3600}` headless, wall time (time.md) | Not measured; the physics spike ran 3,000 ticks of its scene, hashing each, in 145 to 148 ms |
| `resume.latency` | from `continue` to the next tick in real time (time.md) | Not measured; the threads spike's commands took effect 1.04 ms p50, 1.6 ms p95 after sending at 60 Hz |
| `mcp.observe` | an `observe` over MCP (shared/contract/mcp.md, M11) | Not measured |
| `mcp.tick_overhead` | tick time with a client calling `observe` in a loop, over tick time without one, percent (mcp.md, M11) | Not measured |

Two consistency rules apply when they are filled: `step.3600` cannot be below 3,600 times
`tick.sail`, and `perception.tick` and `intent.tick` are parts of `tick.sail`, so their sum with the
script share stays under it.

## 6. Capacities: no silent limits

Charter 3.10: rendering "MUST NOT have silent capacity limits"; this framework applies the rule to
every fixed-size structure in the engine (GPU buffers, pools, rings, queues):

- Each either grows or refuses with a structured error, and reports a counter of what it refused or
  dropped: the renderer's `FrameStats.dropped`, the event stream's `missed` (`threads.md`, 4.4), the
  command queue's `queue.full` answers (`threads.md`, 5.2).
- Every benchmark run reports every counter; the `perf` step reports the warning
  `perf.capacity_dropped` when any is not zero (`checks.md`, 9). What fails is the capacity's own
  test below, in the `test` step, so no limit is silent whatever the benchmark numbers are.
- Each capacity has a test that drives it to twice its initial size and asserts growth or the
  structured error, written by the subsystem that owns it.

## 7. How things are measured

### 7.1 The harness

```text
pocket bench <workload> [--samples N] [--warmup N] [--breakdown] [--json]
```

`pocket-check`'s harness runs a workload's metric `warmup` times, discards those samples, runs it
`samples` times, and reports `Measurement`s (`checks.md`, 10.1) with min, median, p95, p99 and max.
The clock is injected into the harness (`measure(name, warmup, samples, clock, f)`): an `Instant`
natively and `performance.now()` from `pocket-web` in the browser, since `pocket-check` may depend
on neither `js-sys` nor a clock of its own (threads.md 3.1); it is read outside the tick. The
measured run has profiling spans off; `--breakdown` is a separate run with spans on (section 9), so
instrumentation never changes a reported number.

### 7.2 Simulation metrics

- **Pacing**: tick, hash and publication budgets are measured in real-time pacing at 60 Hz, as a
  game runs them, since the threads spike measured paced costs 1.5 to 2.3 times the back-to-back
  ones (its recommendation 9); the back-to-back figures are reported beside them for information.
- **Tick time**: wall time of `Game::step` for one tick, boundary included (the rule workload's
  "including boundary calls", charter 4.2.7), with hashing and publication off; they have their own
  budgets.
- **Hash, publication, fork**: wall time of that operation alone on the world at the workload's
  measured ticks. Fork memory: bytes allocated during the fork, counted by a counting global
  allocator in the bench binary.
- **Round trip**: from `GameClient::send` on an editor-stub thread to the reply's callback, on
  `GameThread`.
- **Web**: `performance.now()` resolution is coarsened in browsers (100 µs without cross-origin
  isolation, 5 µs with, per the W3C High Resolution Time guidance Chrome follows), so the worker
  times batches of 60 ticks and records each batch's mean as one sample. A cold browser start runs
  the first ticks 5 to 7 times slower while Chrome tiers the module up, with steady state after
  about ten ticks (`docs/spikes/script-web.md`, Verdict, 3), so web tick budgets discard at least
  120 ticks of warm-up; the start-up budgets include that cost.

### 7.3 Frame metrics

- **Offscreen**: native frame benchmarks render to a texture at the target resolution, not to a
  window, so no present mode or display refresh caps the measurement. A windowed smoke test (not
  timed) proves presentation separately.
- **GPU time**: `wgpu` timestamp queries (`Features::TIMESTAMP_QUERY`) written at the start and end
  of each frame's passes, resolved and read back a few frames later. In Chrome, WebGPU's
  `timestamp-query` feature where the adapter offers it; where it does not, `web.frame` uses the CPU
  time from one animation frame's start to `onSubmittedWorkDone`, and the report says which.
- **CPU time**: on the presenter thread, from taking the snapshot to the return of `queue.submit`.
- **Counters**: the renderer's `FrameStats` per frame (`architecture.md`, 4.10): triangles and
  instances in the scene, draw calls, triangles submitted, culled, and every drop counter.
- **Statistic**: p95 over the camera path's 300 frames after 30 frames of warm-up (pipelines made,
  caches filled), the render spike's method.

### 7.4 Sizes and start-up

- **Sizes**: the files of the shipped page after `wasm-bindgen`, with and without `wasm-opt -O3`
  (`checks.md`, 7.1; `architecture.md`, 8.1): raw bytes, Brotli at quality 11 with a 22-bit window
  (the `brotli` crate), and gzip at level 9 (`flate2`) for information. KiB are 1,024 bytes. No
  spike built the assembled module; each measured its own, raw and gzipped, after `wasm-bindgen`:
  QuickJS-ng with the script-web workload 1,243,991 and 460,733 bytes (1,382,605 and 496,761 with
  `wgpu`'s WebGPU backend; 929,116 of the code bytes are QuickJS-ng's C), the renderer with the
  importer and meshoptimizer 1,130,400 and 277,778, Rapier with its checks 2,948,924 and 974,817,
  the threads spike's game 931,227 bytes, of which 254,568 a name section (`docs/spikes/*.md`).
  Brotli was not measured.
- **Headless start**: from the harness's `Command::spawn` to the runtime's ready line (a JSON line
  `{"event": "ready"}` on stderr, written after the project is loaded, the first snapshot published
  and commands accepted).
- **Window start**: from spawn to the app's `first_present` line, written after the first frame's
  present returns.
- **Web start**: `performance.now()` at the first frame presented, whose origin is the navigation's
  start. Cold: the first load in a fresh Chrome profile. Warm: the next load in the same profile,
  with the HTTP cache and Chrome's compiled-wasm cache filled (`cargo xtask webcheck --loads 2`,
  `checks.md`, 7.4).

### 7.5 Keeping noise out

1. **Fingerprint**: a machine other than R1 cannot be compared with R1's figures; there the step
   compares each measurement with that machine's own `bench/baseline-<machine id>.json` and says so
   (`perf.not_reference`, `checks.md` 9). On battery power the run is reported with
   `perf.on_battery` and cannot become a baseline.
2. **Alone**: the `perf` step runs after every other step of the check has finished and starts
   nothing else; it does not detect other agents' processes, which is what calibration is for.
3. **Calibration**: before and after the measurements, a fixed CPU workload (a single-threaded PCG32
   and xxh3 loop sized to take about 100 ms on R1, median of 7) and a fixed GPU workload (a compute
   dispatch sized to about 10 ms on the discrete GPU, timed by timestamp queries, median of 7) run.
   Each must be within 10% (chosen) of R1's recorded values, and the after values within the same
   tolerance of the before values; otherwise the run is marked noisy with the warning `perf.noisy`,
   which names the numbers: its measurements are reported, but it cannot become a baseline. The web
   measurements repeat the CPU calibration inside the worker. R1's recorded values were not measured
   in slice 0: the calibration workloads do not exist yet. `cargo xtask perf --calibrate` runs each
   7 times on a quiet R1 and writes the medians to `bench/calibration-r1.toml` (8.1).
4. **Noise never fails the check**: none of these conditions makes the `perf` step fail or
   inconclusive (`checks.md`, 9 and 11); they decide whether a run's numbers can be trusted. A run
   on R1, on AC power, with calibration values recorded and not marked noisy is a **calibrated
   run**; only a calibrated run sets reference figures (8.1) or becomes R1's baseline (8.3).

## 8. Setting and changing reference figures

### 8.1 From the spikes to numbers

For each ceiling, the reference figure is `limit = min(ceiling, measured × 1.25)`, rounded up to two
significant digits, where `measured` is the largest of the spike's runs (and its verification's) for
the same statistic on R1 in a release build; the margin of 1.25 keeps ordinary run-to-run variation
under the figure, so a measurement past it (`perf.budget_exceeded`) points at a real change worth a
look. When the spike's number already exceeds the recommended ceiling, whoever sets the figure does
not raise the ceiling: it records the gap as a finding for the owner, with the measurement and what
would close it, to guide optimization; no slice is accepted or refused on it (charter 3.10). Floors
are set to the benchmark scene's designed scale, not measured. A budget no spike measured, or
measured only by a proxy, says so in its row, and the `perf` step reports its measurement with the
warning `perf.budget_unset` until a calibrated run on the assembled workspace sets its figure by
this rule.

Two commands set what slice 0 could not (`checks.md`, 9):

- `cargo xtask perf --calibrate` runs the CPU and GPU calibration workloads (7.5) 7 times each on a
  quiet R1 and writes their medians to `bench/calibration-r1.toml`. Without these values noise
  cannot be detected, and no run is a calibrated run.
- `cargo xtask perf --set-unset` works only after a calibrated run: it measures every budget still
  unset and, for each ceiling, writes `min(ceiling, measured × 1.25)` rounded up to two significant
  digits into its `bench/budgets/` file with `set = "<date>, <commit>, measured"`. Floors stay
  design values. A figure set this way needs no approval: the owner's approval is needed only to
  raise a figure that is already set (8.3). A measurement above its recommended ceiling is recorded
  as a finding for the owner, as above.

### 8.2 Where the numbers live

Until `xtask` exists, the tables of section 5 are the source. Slice 1 then writes `bench/budgets/`,
one file per subsection of section 5 (`sim.toml`, `threads.toml`, `render.toml`, `web.toml`,
`start.toml`, `contract.toml`), and the tables here become a block generated from it between the
markers `<!-- @generated budgets begin -->` and `<!-- @generated budgets end -->` by
`cargo xtask gen`, which `checks.md`, 5.4 keeps in step. Each entry records in `set` when it was set
and from what.

### 8.3 Changing a reference figure, and the baseline

- **Tightening** (a ceiling's figure lowered) is allowed whenever a calibrated measurement supports
  it.
- **Loosening** (a figure raised) needs the owner's approval, recorded in the entry's `set` (who,
  when, why) and given in the commit message, as master changed its golden values "only
  deliberately, with the reason in the commit message" (master AGENTS.md), so a regression is not
  hidden by moving its reference.
- **Baseline**: `bench/baseline-r1.json` keeps the last accepted measurement of every budget with
  its commit (`bench/baseline-<machine id>.json` on another machine, 7.5).
  `cargo xtask check --only perf --accept-baseline` writes it, and refuses a run marked noisy or on
  battery, and on R1 a run that is not a calibrated run (7.5). A measurement more than 10% (chosen)
  worse than the baseline is reported with the warning `perf.drift`, so slow regressions are seen as
  they happen, whatever the reference figure says.

## 9. Profiling

Charter 3.10: hot spots are found with a profiler such as Tracy, and optimization follows
measurement.

- Every crate instruments with `tracing` spans: `tick`, `tick/<system>` (spec-sim's system names),
  `boundary`, `publish`, `hash`, `fork`, `frame`, `frame/<pass>`, `import`.
- `tracing-tracy` is compiled in behind the `profile` feature of `pocket-app` and `pocket-web`
  (`architecture.md`, 7.4); release builds keep line tables (`debug = "line-tables-only"`) so Tracy
  and stack traces name lines.
- `pocket bench --breakdown` runs the workload again with a light subscriber that sums each span's
  time and reports per-span medians beside the measured numbers, so a measurement past its reference
  figure or a drift arrives with where the time went, as master's `perf` command split a frame into
  script, physics, world, state and render (master AGENTS.md).

## 10. Which spike set which budget

| Spike report | Set | Left unmeasured |
|---|---|---|
| `docs/spikes/physics.md` (Rapier, determinism, fork) | none: its bincode and FNV-1a figures are proxies for `hash.sail`, `publish.sail`, `fork.sail`, `restore.sail` and `web.fork.sail` (persistence.md 13 calls them not measured), so slice 1's first calibrated run sets those five without an approval | `tick.sail` whole, `tick.bodies` at 200 bodies, `fork.sail.memory`, `web.tick.sail` |
| `docs/spikes/script-native.md` (QuickJS-ng natively) | `tick.rules`, `tick.rules.p99` | `fork.rules`, `publish.rules`, `tick.sail.script` |
| `docs/spikes/script-web.md` (QuickJS-ng in a worker) | the web route and the script engine's share of the web size (7.4) | `web.tick.rules` (its workload was not the rule workload) |
| `docs/spikes/render.md` (wgpu, glTF, LOD) | Direct3D 12, 1920 by 1080 and 1280 by 720, the `scene` and `import` workloads, `pass.opaque.gpu`, `pass.opaque.cpu`, `web.pass.opaque`, `frame.draws`, `frame.triangles`, the two floors, `import.highpoly`, `import.frame_max` | `frame.gpu`, `frame.cpu`, `web.frame` (no sea, shadows or post-processing), `start.window` |
| `docs/spikes/threads.md` (the two threads, natively and in a worker) | `rtt.paused`, `rtt.running`, `stall.frame`, `stall.drift`, `publish.churn`, `web.snapshot`, and `threads.md`, 12 | `publish.rules` on the rules world |
| `docs/spikes/debugger.md` (PR #1421 through `rquickjs`) | `debug.idle`, unconfirmed (finding 1) | |
| None | | the web sizes and start-up times, `web.swap`, `branch.sail`, `branch.main_lateness`, `start.headless`, `check.quick`, `check.full`, `check.perf`, `build.web.memory`, `bench.selftest`, `replay.size.sail`, `import.tick_max`, the calibration values, every row of 5.6 |

A spike's toy world is smaller than the real workload; where a budget was set from a part of the
workload (the threads spike's 20-byte entities for `publish.churn`), the row says so, and the first
calibrated `perf` run on the assembled workspace replaces it, raising it only with the owner's
approval (8.3). Where a spike measured a different operation on a different format (the physics
spike's bincode and FNV-1a for the PCE and XXH3-128 operations), the row is "not measured" with the
proxy, and the first calibrated run sets it by 8.1 without an approval.

## 11. Names used elsewhere

Other specifications refer to these budgets by name (`tick.sail`, `fork.sail`, `hash.sail`,
`publish.sail`, `rtt.paused`, `web.snapshot`, `check.full` and the rest of section 5), never by a
placeholder; their own measurements (persistence.md 13, `threads.md` 12, script-host.md 11) are the
numbers section 5 cites.

## 12. Open choices

1. **The frame-rate target.** Recommendation: 60 frames per second at 1920 by 1080 natively and 30
   at 1280 by 720 in the browser on the integrated GPU. The owner may raise the native target to the
   laptop panel's refresh rate; every frame ceiling then scales.
2. **The tick-time ceiling's fast-forward factor.** Recommendation: four times real time (a quarter
   of the period). A reinforcement-learning workload would want more, set per workload when one
   exists.
3. **A second reference machine.** Recommendation: none until the owner develops elsewhere; the
   framework supports another `hardware` id with its own reference figures and calibration values.
4. **Whether the web size ceiling covers the editor page.** Recommendation: no. The shipped game
   page has a reference figure; the browser editor page is measured and reported without one until
   the editor slice (charter 10, slice 5).
5. **Counting allocations for fork memory** with a global allocator in the bench binary only.
   Recommendation: yes; the shipped binaries keep the system allocator.

## 13. Findings for the owner

One slice 0 measurement exceeded its recommended ceiling, within the noise of the machine it was
taken on: the verification of `debug.idle` (item 1). These are the gaps 8.1 asks to record:

1. **`debug.idle` exceeded its ceiling, within the machine's resolution.** The debugger spike
   measured the debug-capable engine at 0.98 to 1.00 times stock, and its verification at 1.008 to
   1.032 against the 1% ceiling, while identical bytecode measured 1.006: the loaded machine
   resolves about 3%, three times the ceiling. A calibrated run on R1, against the stock build of
   `bench/stock/`, settles it.
2. **Every number here was taken beside other agents' builds** (CPU load up to 46% at round starts
   in the script-web spike), which section 7.5 would mark noisy. They are starting points; slice 1's
   first calibrated `perf` run replaces each, and a replacement above its row needs the owner (8.3).
3. **Unexplained web frame gaps.** The render spike's page went 61 to 102 ms without an animation
   frame in 5 of 17 loads with no page work over 13.7 ms in a frame, and its verification saw 81 to
   123 ms gaps in 3 of 6 loads; no budget measures frame gaps yet, and their cause (Chrome's GPU
   process, the compositor or the shared machine) is open.
4. **`wasm-opt -O3` and the physics step.** The physics spike's optimized module ran Rapier's step
   17 to 20% slower and once per run took a 5,010 to 5,815 µs tick in Chrome; the script-web spike
   saw no change. `architecture.md` 8.1 therefore measures both builds before shipping either.
5. **Scripts are a fifth of a tick at 1,000 entities.** `tick.rules` fits its 4.2 ms ceiling at 3.2
   ms, which leaves `tick.sail.script` (a third of `tick.sail`) room for the showcase's rules only
   if hot logic moves into Rust primitives, as charter 4.2.4 says.
