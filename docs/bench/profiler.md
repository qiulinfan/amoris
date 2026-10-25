# The live profiler: what it costs and what it shows (Pioneer)

- Date: quiet re-measurement 2026-10-15 (04:52 to 05:03), replacing the provisional run 3 of
  2026-10-14 (section 4). Branch `explore/liveprof`; the build measured is commit 8a505d22's, whose
  runtime is run 3's (0b8f1da9; the commits between them change the smoke test, the specs, this
  file and one test's formatting). Charter 4.1 (the built-in profiler: per-system CPU spans and
  per-pass GPU timestamps for the editor) and 4.5 (the Pioneer note of 2026-10-14: a host that
  serves the profile turns it on, the editor adds its own viewport's GPU passes); the 10-03
  schedule's 3.4, retired on 2026-10-25 (the gap "性能面板（宿主没有 profile 推送）"). Design:
  [spec/server.md](../spec/server.md) 4 (the profile),
  [spec/host-protocol.md](../spec/host-protocol.md) 3, [spec/editor.md](../spec/editor.md) 5.
- Machine: the R1 laptop (ASUS ROG Zephyrus G14: AMD Ryzen 9 270, 16 threads, 15 GiB, RTX 5060
  Laptop and Radeon 780M, Windows 11 Pro 10.0.26200), on AC power, Balanced plan; release build
  (`opt-level = 3`, debug info), Rust 1.98.1; Chrome's default WebGPU adapter for the capture (not
  identified by the page, which reports "BrowserWebGpu, Other"; on this hybrid laptop most likely
  the 780M).
- Conditions: every other agent's work had finished; no other cargo, rustc, Unity or Chrome process
  ran. CPU load 5.3 % when the run started and 4.2 % when it ended, the RTX 5060 at 54 C both times
  (`tools/machine_conditions.py`), the shared bench lock (`out/locks/bench_lock.py`) held
  throughout, five runs of each bench. A first attempt, started 30 s after the release and editor
  builds (CPU 13.5 % at its start), is kept in `out/liveprof/quiet/after-build/`: its overheads
  agree with the tables below (back to back +2.0 and +2.8 % by the mean, paced +2.0 and +2.1 %), but
  its paced ticks fell by a fifth to a quarter over its first three runs (sailing 346 to 276 us, the
  course 461 to 344 us). The tables use the run started from idle, whose paced ticks held steady
  (270 to 284 us and 336 to 345 us).
- Raw JSON, logs, text and screenshots: `out/liveprof/quiet/` (ignored); run 3's in
  `out/liveprof/run3/`.

## 1. What the profiler costs

`cargo run --release -p amoris-runtime --features transpile --example profile_bench`: a project's
game stepped directly, 120 ticks of warm-up, then ticks without the profiler (`Game::step`) and
with it as the game thread runs it (`profile::TickTimer` on an `Instant`-based millisecond clock
behind the `Arc<dyn Fn>` the loop injects, the loop's four readings after the step and the
`try_lock` push into a shared ring), in alternating blocks (A B B A ...) of one world. Back to back:
3,000 ticks each in blocks of 250. Paced: 1,800 ticks each in blocks of 100, a tick every 16.67 ms
with the thread sleeping in slices of at most 1 ms between them, as the game thread does in real
time; each tick is timed alone, the sleeps left out. Five runs of each, taking turns. The
us-per-tick columns are the run whose overhead is the median, so they give that overhead; the
median-block overhead compares the median 250- or 100-tick block of each side.

| Project | Pacing | Median run: without / with (us/tick) | Overhead, mean: median (runs) | Overhead, median block: median (runs) | With - without, median (us) |
|---|---|---|---|---|---|
| sailing | back to back | 65.5 / 67.3 | +2.68 % (+2.94, +2.25, +3.60, +2.68, +1.73) | +2.08 % (+1.85, +2.01, +2.08, +2.31, +2.39) | +1.76 |
| sailing-course | back to back | 82.2 / 83.6 | +1.71 % (+1.41, +1.71, +2.64, +1.92, +1.31) | +1.93 % (+1.72, +1.93, +2.22, +1.93, +2.62) | +1.40 |
| sailing | paced | 274.5 / 278.9 | +1.62 % (+4.70, +1.62, +1.37, -0.14, +1.95) | +2.33 % (+4.70, +1.84, +2.33, +0.17, +3.00) | +4.45 |
| sailing-course | paced | 336.1 / 346.8 | +3.20 % (+2.50, +4.02, +3.57, +3.20, +2.55) | +3.21 % (+1.74, +3.62, +3.21, +4.36, +2.65) | +10.76 |

Back to back, the profiler costs about 1.4 to 1.8 us a tick: +1.7 to +2.7 % by the mean and +1.9 to
+2.1 % by the median block on these deliberately cheap ticks (65 to 85 us: nine entities, three
script systems), above the 1 % hoped for. A tick reads the clock 36 times (two per system, 15
systems, and six for the loop), and one reading of this clock through the injected closure costs
31 ns: 1.12 us, most of the cost. A cheaper clock (a raw TSC) or fewer readings would lower it. Even
on the quiet machine the runs spread by about one point by the median block (+1.7 to +2.6 %) and by
two by the mean (+1.3 to +3.6 %), so a few tenths of a microsecond between such variants would need
more runs to resolve.

Paced like the real-time loop, the ticks cost about four times as much (section 2), and the quiet
runs resolve the profiler's overhead there: +1.6 % by the mean and +2.3 % by the median block on
sailing (4.5 us a tick), +3.2 % both ways on the course (10.8 us); nine of the ten runs are positive
by the mean and all ten by the median block. **This changes the provisional conclusion**: run 3
found the paced overhead inside its runs' spread and bounded it at about 3 % (about 8 us); on the
course the quiet cost passes that bound. In microseconds the paced cost is 2.5 (sailing) and 7.7
times (course) the back-to-back difference: the profiler's own work, like the rest of the tick, is
slower after a 16 ms sleep. That is the estimate for a real-time tick; the profiler's cost on
`amoris serve`'s own game thread is not measured (nothing turns the hooks off there). The same
microseconds would be 0.6 to 2.2 % of that thread's whole time per tick (section 2: 0.50 to 0.69
ms).

## 2. Where a tick goes

The batch profile (back to back, the last run's 3,000 profiled ticks, ms per tick):

| Row | sailing | sailing-course |
|---|---|---|
| `script:log` (sailing) / `script:rounding` (course) | 0.0185 | 0.0219 |
| `physics.buoyancy` | 0.0172 | 0.0177 |
| `script:take_aboard` | 0.0136 | 0.0154 |
| `physics.step` | 0.0118 | 0.0137 |
| `interface.perception` | 0.0001 | 0.0110 |
| the other 10 systems together | 0.0043 | 0.0054 |
| `tick.other` (the step outside its systems) | 0.0010 | 0.0011 |
| tick | 0.0666 | 0.0862 |

What pacing alone does, on the same world (no server, no debugger hook, no Play fork): the plain
tick (median of the runs) goes from 66.5 to 273.3 us on sailing (4.1 times) and from 85.5 to 339.3
us on sailing-course (4.0 times); per row from 1.9 times (`physics.buoyancy`) to 5 to 6 times
(`physics.step`, `script:log`, `script:take_aboard` on the course) and 10 to 21 times for the
smallest rows (`tick.other`, `script.update`, `interface.intents`; `loop.publish`, at 0.0001 ms, is
too small to compare). A tick that follows a 16 ms sleep costs this much more here; which part is
caches, core clocks or the scheduler is not separated. Run 3 found 3.5 and 3.9 times: on the quiet
machine sailing's batch tick is 10 % faster (74.3 to 66.5 us) and its paced tick 5 % slower (260.1
to 273.3 us). The paced bench reproduces the live tick without an editor (below: 0.292 ms against
0.273 ms, `script:log` 0.110 against 0.109 ms, `physics.step` 0.068 against 0.065 ms): on this
machine pacing alone accounts for most of the gap between batch ticks and live ticks without an
editor.

The same sailing world on the game thread of `amoris serve`, in Play at 60 ticks a second, read
with `amoris profile --ticks 600` (ms per tick and share of the game thread's time): first in the
capture's Play with the editor connected (600 ticks to tick 902, read before Stop), then in a
second Play after the editor's browser had closed (600 ticks to tick 901):

| Row | Editor connected | share | No editor | share |
|---|---|---|---|---|
| `loop.publish` (snapshot, event and log streams) | 0.222 | 32.3 % | 0.165 | 33.3 % |
| `script:log` | 0.164 | 23.8 % | 0.110 | 22.2 % |
| `physics.step` | 0.102 | 14.8 % | 0.068 | 13.8 % |
| `loop.present` (the render feed's extraction) | 0.053 | 7.7 % | 0.038 | 7.6 % |
| `script:take_aboard` | 0.047 | 6.8 % | 0.038 | 7.7 % |
| `physics.buoyancy` | 0.035 | 5.0 % | 0.032 | 6.4 % |
| the other 13 rows | 0.066 | 9.4 % | 0.045 | 9.0 % |
| tick (`Game::step`) / game thread (`busy_ms`) | 0.412 / 0.689 | | 0.292 / 0.495 | |
| largest tick | 0.815 | | 0.508 | |

Publishing and the render feed's extraction take 40 % of the game thread under `amoris serve`, with
the editor (40.0 %) or without (40.9 %): the host publishes a snapshot every tick for its reader and
extracts the feed for its capture server whoever is connected. It is the next place to look for the
game thread's cost. With the editor connected (headless Chrome drawing the viewport at 119 fps on
the same laptop) every row was slower, the whole thread 1.39 times; that cause is not separated
either. Both live threads are slower on the quiet machine than in run 3 (0.689 against 0.607 ms with
the editor, 0.495 against 0.452 ms without), as sailing's paced tick is; the cause is not separated.
Three seconds into each Play (`play-early*.json`, the first tick included), the largest tick was
1.26 ms in the capture's Play (the process's first; `physics.step` at most 0.64 ms) and 0.46 ms in
the second. An earlier run saw a 10 ms tick early in the process's first Play; it did not recur in
run 3 or in either quiet attempt (the after-build one: 1.05 and 0.51 ms), and its cause is open.

## 3. The editor against the real host

`tools/host/profiler_bench.py --capture` starts `amoris serve` on a copy of `samples/sailing` with
the worktree's `editor/dist` and `web/viewport`, and `editor/tools/profiler-capture.ts` drives
headless Chrome (WebGPU on): the editor connects, Play starts through `/api/call`, the Profiler
panel is shown and given room, and 15 s into Play the panel's text is read, then at once
`profile.frame` and `amoris profile` over the panel's own window and over 600 ticks, the overlay and
the screenshots (`out/liveprof/quiet/capture/`):

- The Profiler showed the host's 19 rows per tick from `sim.begin` to `loop.after` (`script:log`
  0.15 ms, `physics.step` 0.10 ms, `loop.publish` 0.21 ms, `loop.present` 0.05 ms), the six phases
  (Update 0.20 ms, Physics 0.10 ms, Forces 0.06 ms), the tick (0.38 ms) and the game thread's time
  (0.64 ms), and the viewport's own GPU passes from its timestamp queries (opaque+sky 0.98 ms, cull
  0.04 ms, shadows 0.01 ms, lights 0.01 ms; 1.03 ms in all) with its frame interval (8.37 ms, 119
  fps: headless Chrome without vsync).
- The same window three ways: the panel (tick-weighted over its last 12 pushes, to tick 893) tick
  0.38 ms and thread 0.64 ms; `profile.frame {ticks: 180}` (to tick 900) 0.385 and 0.641 ms; the
  CLI's `amoris profile --ticks 180 --json` (to tick 901, 25 ms after the panel was read) 0.385 and
  0.641 ms. They agree to the panel's rounding.
- The viewport's overlay read `WebGPU 119 fps · GPU 1.03 ms · 61 tps · tick 902 · 9 entities`: the
  page's frame rate and GPU time beside the host's ticks per second, which the transport now also
  labels tps.
- The chart's CPU scale is 1.5 times the 90th percentile of the samples shown: 1.15 ms, with the
  window's largest sample at 0.90 ms.

## 4. Earlier runs

Run 3 (2026-10-14 22:40 to 22:47, `out/liveprof/run3/`) was the provisional measurement these tables
replace: the same protocol with three runs, while other agents built beside it (CPU 40.8 % when it
started, 10.1 % when it ended). Its back-to-back overhead (+1.9 %, 1.3 to 1.6 us), its live shares
(publish and present 40 % with or without the editor) and the panel's agreement with `profile.frame`
and the CLI hold. Two of its conclusions changed: it found the paced overhead inside its runs'
spread (-0.7 and +1.4 % by the mean, "at most about 3 %, about 8 us"), where the quiet runs resolve
+1.6 and +3.2 % (4.5 and 10.8 us); and its pacing factor was 3.5 and 3.9 where the quiet one is 4.1
and 4.0. Its live game thread (0.452 ms without the editor, 0.607 ms with it; 1.34 times) was faster
than the quiet one (0.495 and 0.689 ms; 1.39 times).

Runs 1 and 2 (the same evening, `out/liveprof/run1/` and `run2/`) are superseded. They timed a
lighter profiled path (no loop readings, an unshared ring; 32 readings a tick) and found +1.4 to
+4.9 % back to back, in line with section 1. Their live table was read after the capture's editor
had closed, in a Play started afterwards; in run 1 the 600-tick window also spanned two Plays (the
ring was not yet cleared between worlds). Their "first build" was the runtime of 7e30b4b8 (the
clock also read around phases, names found in a map), timed with a profile_bench harness from
before its first commit.

## 5. Re-running the measurement

From the merged checkout, with nothing else running (start once `tools/machine_conditions.py` reads
a few percent of CPU: right after these builds the first paced runs are slower, as in the
after-build attempt above):

```bash
cargo build --release -p amoris-runtime --features transpile --example profile_bench
cargo build --release -p amoris-app
(cd editor && bun install && bun run build)
python tools/host/profiler_bench.py --out out/liveprof/quiet --runs 5 --capture
```

It records the machine's conditions before and after and writes `out/liveprof/quiet/summary.md`:
section 1's table (back to back and paced, with the clock's cost per reading and the readings per
tick), section 2's batch profile and the paced-against-back-to-back rows. The capture writes
`capture/compare.json`, `capture/amoris-profile.txt` (section 2's live table, editor connected),
`amoris-profile-no-editor.txt` (no editor), `capture/play-early.json` and
`play-early-no-editor.json` (the first ticks), and the panel's and overlay's text and screenshots
(section 3). `CHROME_FLAGS=--force_high_performance_gpu` puts Chrome's WebGPU on the RTX 5060. On
the shared laptop wrap the command in `python out/locks/bench_lock.py run --label liveprof -- ...`;
`--no-bench` runs the capture alone and `--also LABEL=path/to/profile_bench.exe` times another
build in turn.

## 6. The serve loop: publication, extraction and the editor (quiet re-measurement, 2026-10-16)

- Date: quiet re-measurement on 2026-10-16, 02:12 to 02:53 local (gizmo.md 6 followed until 03:00),
  branch `explore/serveloop`. Every other track had finished; no cargo, rustc or Chrome process ran
  apart from the benches' own, and the shared bench lock was held for every set. The CPU load read
  4.0 to 7.9 % at the sets' starts and ends and the RTX 5060 49 to 65 °C
  (`tools/machine_conditions.py` beside each set). It replaces the provisional waves of 2026-10-15
  (the first 17:50 to 20:05 under 3 to 100 % load, the second 21:50 to 22:22 at 3.4 to 8.0 %, both
  while other tracks built and timed on the laptop). Two of their findings did not hold: the tick
  that was 5 % longer on this branch (6.3) and the CPU spinner's 1.26 (6.4). 6.1's per-section
  timings are the first wave's and were not repeated.
- What was measured: section 2 left `loop.publish` (33 % of the game thread under `amoris serve` on
  sailing) and `loop.present` (8 %) as the next place to look, and every row 1.39 times slower with
  the editor connected.
- Builds (each set keeps `amoris version`, commit and source hash, beside its results): `base` is
  master `3fdc5b83`; `new` is this branch at `45f4c9fb` (the snapshot cache, the extractor's kept
  queries, the feed woken on publication, the editor's drag sending), whose source hash
  (`966bac6c...`) is that of `93232970`, the build the provisional second wave measured: the commits
  between them change only docs. The publication bench's `base` is this branch's harness compiled
  against master's runtime (`tools/runtime/publish_bench.py --build-base 3fdc5b83`). The provisional
  waves' builds are kept as `out/serveloop/bin/*-wave1.exe` and `pocket-new-wave2.exe`. Raw output:
  `out/serveloop/quiet/` (ignored): `publish/` (6.2), `live/` (6.3, 6.5), `diag/`, `speed8/` and
  `pinned/` (6.4); the provisional waves' under `wave2/`, `wave/`, `diag-*`, `try1` and `probe-*`.

### 6.1 Where the time went

`publish_bench` (an amoris-runtime example; `tools/runtime/publish_bench.py`) runs the loop's work
after each tick part by part on a project's game, paced every 16.67 ms like the real-time thread. On
sailing in Play, `loop.publish` is almost all the snapshot: the event and log streams take under 1
us and the swap into the slot 4 to 5 us (with the drop of the publication before it). The snapshot
encoded all 43 section sources each time (39 registered, 4 project components; 26 present, 26.3 KB).
Timed one by one, back to back, with an instrumented build that was not kept (`probe-sections.json`;
first wave, provisional, not repeated): `Floater` 9.8 us (7.4 KB), the physics cache 4.9 us (14.3
KB), `Collider` 4.0, `Model` 3.2 us, and the other registered component sections 1.2 to 2.7 us each
(`Name` 2.7, `Perceivable` 2.3, the empty ones 1.2 to 1.3) although most hold one row or none: each
made its query again (`world.try_query`). The four project components took 0.2 to 0.7 us. The list
of sources took 4.5 us, hashing every section 1.0 to 1.8 us, the registry's checks 0.5 to 0.9 us;
the encodes add up to 70 of the snapshot's 96 us, the rest is each section's allocation and the
world hash. In Play 7 of the 26 present sections change every tick (`physics.rapier`, `Boat`,
`Floater`, `Log`, `Transform`, `Velocity`, `SimClock`), 2 now and then (`EventCounter` once and
`EventInbox` twice in 1,800 ticks) and 17 never; the quiet runs counted the same. The render feed's
extraction likewise made its nine queries again every tick.

### 6.2 What changed, part by part

Three changes, each committed and tested on its own: the game thread takes its snapshots through
amoris-persist's `SnapshotCache` (persistence.md 4.3: a component section whose rows did not change
is taken over with its digest, and each component's query is kept; the same bytes, checked after
every tick and edit by amoris-persist's `tests/repeated.rs` and amoris-runtime's
`tests/snapshot_cache.rs`); the extractor keeps its queries (`tests/present.rs`: a viewport that
applies every delta holds the full frame's scene after every tick and edit); and `/render` sends on
publication (6.5 and gizmo.md 6). `tools/runtime/publish_bench.py --runs 5` (`quiet/publish/`),
paced, 1,800 ticks a run, the three builds taking turns: the median over the runs of each part's
median per tick in us (in brackets the median over the runs of each run's mean, and of its p95). All
three run the same harness; `full` is this branch with every section encoded (`Game::snapshot`):

| Build | Step | Streams | Extraction | Snapshot | Swap | After the step |
|---|---|---|---|---|---|---|
| base (master's runtime) | 265.7 | 0.6 | 32.4 (33.5, 44.2) | 132.2 (136.3, 181.3) | 5.1 | 171.0 (175.8, 230.2) |
| full (kept extractor queries) | 267.2 | 0.5 | 9.7 (10.0, 13.2) | 141.6 (147.5, 194.5) | 5.0 | 157.2 (163.5, 216.7) |
| cached (this branch) | 270.9 | 0.5 | 10.0 (10.4, 14.2) | 72.8 (75.7, 105.6) | 4.4 | 88.2 (91.0, 126.4) |

Per run, the work after the step was 171.2, 168.5, 169.9, 171.0 and 172.1 us on `base` and 89.7,
87.7, 88.2, 88.6 and 86.2 us on this branch: 0.52 times at the median, the snapshot 0.55 times and
the extraction 0.31 times. All fifteen runs ended on the same world hash (`f8d7b4fc...`). `full`
encodes as master does (its `Game::snapshot` now shares the section writer with the cache): back to
back on the last world both took 41 to 44 us, but paced `full` took 7 % longer (138.4 to 143.3
against 130.9 to 132.9 us over the runs); the likeliest reason is that the cheaper extraction before
it leaves less of the world in the caches the snapshot starts with (not separated). The step is
within 2 % across the builds. Against the provisional second wave the step and the work after it
came out 7 to 11 % slower on all three builds alike (the step 266 to 271 against 244 to 249 us, the
work after the step 171.0, 157.2 and 88.2 against 156.6, 143.8 and 79.7 us), so the ratios barely
moved (0.52 against 0.51; the extraction 0.31 against 0.28); section 2 found sailing's paced tick
slower on its quiet machine too (not separated).

### 6.3 Under `amoris serve`

`tools/host/serve_loop_bench.py` (`quiet/live/`): both builds serve a copy of sailing at once; five
rounds, each Play read with `profile.frame` over its 600 ticks after 3 s of settling, with no
client, with the editor's two sockets (`/ws` on every topic and `/render`, no page;
`editor/tools/serve-clients.ts`), and with the editor in headless Chrome (WebGPU on its default
adapter, the 780M, the viewport drawing at 119 to 120 fps; this branch's editor build for both
hosts, which in Play sends the same requests). Medians of the five rounds, ms per tick:

| Build | Clients | Game thread (`busy_ms`) | `tick_ms` | `loop.publish` | `loop.present` |
|---|---|---|---|---|---|
| base | none | 0.466 | 0.278 | 0.152 | 0.034 |
| base | sockets | 0.473 | 0.281 | 0.154 | 0.036 |
| base | editor | 0.643 | 0.389 | 0.205 | 0.048 |
| new | none | 0.377 | 0.279 | 0.088 | 0.010 |
| new | sockets | 0.394 | 0.283 | 0.098 | 0.011 |
| new | editor | 0.553 | 0.395 | 0.141 | 0.016 |

`loop.publish` falls to 0.58 times (no client) and 0.69 times (editor), `loop.present` to 0.30 and
0.34 times, and with them the game thread to 0.81 and 0.86 times. The two rows were 40 % of the game
thread on `base` (39 % with the editor) and are 26 % (28 %) now. The tick itself is the same on both
builds: 0.278 against 0.279 ms with no client, 0.281 against 0.283 ms with the sockets, 0.389
against 0.395 ms with the editor. This changes a provisional finding: both provisional waves had the
tick 5 % longer on this branch with no client (0.251 against 0.263 ms; first wave 0.298 against
0.312) and left it unexplained; on the quiet machine the difference is under 0.5 % (1.5 % with the
editor), so it came from the loaded machine, not from the branch. `/render` carried 54.9 frames a
second on `base` (54.9, 56.0, 55.4, 54.9 and 54.9 over the rounds: a 16 ms timer, sometimes two
ticks in one frame) and 60.0 on this branch (one per tick).

### 6.4 Why the editor slows every row

The game thread with each client set over the same build's `none` in the same set (`base` unless
noted), five rounds unless noted. `speed8` runs Play at 8 times real time (`--speed 8`: the same
tick every 2.1 ms, after about 1.8 ms of sleep instead of 16):

| Set | Sockets | Sockets and the editor's reads | Editor | Editor, Chrome on the RTX 5060 | One CPU spinner | Three spinners |
|---|---|---|---|---|---|---|
| `live` (`base`; `new`) | 1.02; 1.05 | | 1.38; 1.47 | | | |
| `diag` | | 1.02 | 1.42 | 1.23 | 1.02 | 0.97 |
| `speed8` | | | 1.33 | 1.27 | 1.01 | |
| `pinned` (host on logical CPUs 2-5, clients on the others), 3 rounds | 1.06 | | 1.41 | 1.23 | | |

- The host's work for the editor is not the cause. The editor's sockets without a page (every push,
  and 55 render frames a second on `base`) cost 1.02 to 1.06, and with the editor's reads as well
  1.02: 50 `world.get` and 10 `world.tree` a second, what the page sent in Play (700 to 715
  `world.get`, 140 to 144 `world.tree` and 14 or 15 `snapshots.list` in 14 s in every editor Play),
  all of which the game thread answers between ticks. Pinning the host and the clients to disjoint
  cores changes nothing (1.41 against 1.42), so they do not compete for a core.
- A busy CPU does not slow the game thread on the quiet machine, which changes a provisional
  finding. One process spinning on another core cost 1.02 at real time and 1.01 at speed 8, three
  spinners 0.97. The provisional second wave had one spinner at 1.26 at real time (1.04 at speed 8)
  and concluded that load slows the tick that follows a long sleep; with the machine otherwise idle
  that is not reproduced, so its 1.26 came from the spinner together with the other tracks' load
  (not separated). What stands is the long sleep itself: without clients the game thread took 0.29
  ms a tick at speed 8 against 0.45 to 0.47 ms at real time, for the same work. Whether clocks, idle
  states or caches make that difference is not separated.
- The editor page costs more, and most of it stays without the long sleep: 1.42 times with Chrome's
  WebGPU on the 780M and 1.23 on the RTX 5060 at real time, 1.33 and 1.27 at speed 8, where the
  page's traffic was the same (it refetches on `world.changed`, which the host pushed 10 to 13 times
  a second at either speed). Since a spinning process costs nothing, the cost is the page's own. The
  adapter is part of it at real time (the 780M shares the APU's power budget and memory with the CPU
  cores; the viewport took 1.6 ms of its GPU a frame against 0.2 ms on the RTX 5060), and what
  remains at speed 8 is neither the sleep, nor a shared core, nor the host's work; the page's memory
  traffic and the power it draws are candidates. These are hypotheses, not tested to the end. The
  provisional waves' ratios (the editor 1.23 to 1.53 on the 780M and 1.12 to 1.36 on the RTX 5060,
  the sockets 0.88 to 1.11) are superseded by this table.
- The editor adds the same time a tick on both builds (0.177 ms on `base`, 0.176 ms on this branch,
  `live`), so its factor is larger on the cheaper build (1.38 and 1.47): the host can do less work
  per tick, which 6.2 and 6.3 do, and the page's share stays.

### 6.5 The render feed at rest

Paused edit world, the host process's cycles over 30 s (`QueryProcessCycleTime`, as a share of one
core at the cycle counter's 3.99 GHz), three tries per build, the builds alternating (`quiet/live`):

| Build | Nothing connected | A `/render` socket |
|---|---|---|
| base | 0.454, 0.465, 0.471 % | 0.868, 0.876, 0.868 % |
| new | 0.251, 0.246, 0.243 % | 0.310, 0.302, 0.304 % |

A `/render` socket added 0.40 % of a core on `base` (the medians 0.465 and 0.868 %), whose 16 ms
timer looked at an empty mailbox 62 times a second, and 0.06 % on this branch (0.246 and 0.304 %),
where it wakes only on a publication; the provisional second wave's two tries gave the same. With
nothing connected this branch used less as well (0.25 against 0.47 %), which none of the changes
plainly explains (not traced). The readings of CPU time beside them (`GetProcessTimes`, in 15.6 ms
steps: 0.05 to 0.94 % over all the quiet tries, 0.00 to 1.09 % in the first wave) cannot resolve
these differences. Edits reach the feed at once: gizmo.md 6.

### 6.6 Re-running

The recorded runs used builds made before the 2026-10-16 rename, which read the old names:
`out/serveloop/bin/pocket-base.exe` (master `3fdc5b83`) and `out/serveloop/bin/pocket-new.exe` (this
branch at `45f4c9fb`). A re-run builds its own, each commit from its exported tree after
`python tools/amoris_rename.py --tree <dir>`, so that it reads the `AMORIS_*` variables the tools
set and the samples' renamed scene format (`tools/runtime/publish_bench.py --build-base` does this
itself). Since 2026-10-25 Pioneer is deleted and neither commit resolves: on Amoris main the nearest
are `c4d33d6f` for `base` (the parent of `0b444a23`, before the snapshot cache) and `bf79ed95` for
`new` (the end of the series that integrates the whole merged `explore/serveloop`, later than
`45f4c9fb`), both from before the rename; use them in place of `3fdc5b83` and `45f4c9fb` below.
They are not the recorded trees, so a re-run measures them afresh. From the branch's root, build
`cargo build --release -p amoris-runtime --features transpile --example publish_bench`, the base
harness
`python tools/runtime/publish_bench.py --build-base 3fdc5b83 --into out/serveloop/base-build`
(master's runtime, its own target directory, about 4 minutes),
`cd editor && bun install --frozen-lockfile && VITE_AMORIS_EXPOSE=1 bun run build`, and both
commits' `amoris` and editor, each in its own directory and target (gizmo.md 6 uses them too):

```bash
R=out/serveloop/rerun
build() {  # NAME COMMIT: the commit's amoris and editor, renamed, in $R/NAME
  mkdir -p $R/$1 && git archive $2 | tar -x -C $R/$1 && python tools/amoris_rename.py --tree $R/$1
  (cd $R/$1 && cargo build --release -p amoris-app --bin amoris)
  (cd $R/$1/editor && bun install --frozen-lockfile && VITE_AMORIS_EXPOSE=1 bun run build)
}
build base 3fdc5b83; build new 45f4c9fb
```

Each command under
`python out/locks/bench_lock.py run --label quiet-serveloop --max-hold-min 25 -- ...` on an idle
machine (about 8, 15, 8, 6 and 4 minutes; `out/serveloop/run-quiet.sh` ran them with gizmo.md 6's
three sets):

```bash
R=out/serveloop/rerun; B=$R/base/target/release/amoris.exe; N=$R/new/target/release/amoris.exe
python tools/runtime/publish_bench.py --out out/serveloop/quiet/publish --runs 5 \
    --also base=out/serveloop/base-build/publish_bench-3fdc5b83.exe
python tools/host/serve_loop_bench.py --out out/serveloop/quiet/live --rounds 5 --idle 30 \
    --idle-rounds 3 --clients none sockets editor --amoris base=$B --amoris new=$N
python tools/host/serve_loop_bench.py --out out/serveloop/quiet/diag --rounds 5 --idle 0 \
    --clients none sockets-reads editor editor-dgpu busy1 busy3 --amoris base=$B
python tools/host/serve_loop_bench.py --out out/serveloop/quiet/speed8 --rounds 5 --idle 0 --speed 8 \
    --clients none editor editor-dgpu busy1 --amoris base=$B
python tools/host/serve_loop_bench.py --out out/serveloop/quiet/pinned --rounds 3 --idle 0 \
    --host-cpus 2-5 --client-cpus 0-1,6-15 --clients none sockets editor editor-dgpu \
    --amoris base=$B
```

Each writes `summary.md` beside its raw JSON and the machine's conditions: the first 6.2's table
(the per-run lines from the JSON's `loop.us`), the second 6.3's table, `/render`'s frames a second
and 6.5's idle tries, and the second to fifth the `busy_ms` that 6.4's ratios divide (each client
set over `none` of the same build and set).
