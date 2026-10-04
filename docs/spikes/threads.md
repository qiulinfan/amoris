# Spike: the game thread and the editor thread (`threads`)

- Date: 2026-10-03. Slice 0 check "the two-thread model natively and on the web" (charter 10).
- Charter: 5.1 (two threads), 7 item 15 (the threads specification), 12 item 8 (the web form), 3.3
  (determinism and replay), 3.10 (performance). Feeds the placeholders of `docs/spec/threads.md`,
  section 12, and `docs/spec/budgets.md`, 5.2.
- Code: `spikes/threads/` (its `README.md` says how to build and run each part); results in
  `spikes/threads/evidence/`.

## Question

Can a game thread that alone owns a `bevy_ecs` world and steps it at a fixed 60 Hz publish an
immutable snapshot after each tick, while an editor reads the latest snapshot at its own rate and
sends commands (spawn, set a component, pause, step) that the game applies at a tick boundary in a
recorded order, so that replaying the command log reproduces the world hash at every tick? Does the
same model work on the web with the game in a Web Worker and the page as the editor? What do
snapshot building, publication, transfer and command latency cost for 100 to 100,000 entities? Does
a stalled editor leave the ticks alone, and does a paused or stalled game leave the editor alone?
And what should the threads specification fix, including how the editor's commands are ordered
against agents' actions arriving over MCP?

## Verdict

**Works.** Every property held in every run, natively and in the browser, at all four entity counts:

- A command log stamped with (tick, sequence) replays to the same world hash after every tick: the
  native sessions (editor and agent threads interleaved, refused commands included), the ten web
  sessions inside the worker, and the eight web sessions replayed by the native binary, whose hash
  chains are identical to the browser's tick for tick (128 to 130 ticks each, N = 100,000 included).
  Moving one command a tick later makes the replay diverge exactly at that tick.
- An editor that holds a snapshot and sleeps 200 ms (or 500 ms) per frame moves no tick: 168 ticks
  in a 2.8 s window, as expected, with the longest interval 17.2 ms natively; in the browser a page
  blocked for 200 ms sees the worker run its 12 ticks meanwhile.
- A paused game keeps the editor at its own frame rate, with edits visible in about a millisecond
  and `step` advancing exactly; a game stuck in a 2 s tick leaves the editor's frames unchanged (p95
  4.5 ms natively for a 4 ms frame).
- The costs fit a 16.7 ms tick with room: for 100,000 entities (a 2 MB snapshot) the snapshot and
  its hash take 0.35 ms back to back and 0.52 to 0.78 ms at 60 Hz natively, 0.84 to 1.0 ms in wasm;
  a native publication is a pointer swap (0.5 to 4.4 µs); a web publication (copy out of wasm memory
  and transfer) adds 0.16 ms and arrives 0.13 ms later.

The web form needs no cross-origin isolation (it passed with `crossOriginIsolated` false), and its
worker keeps ticking in a hidden tab (58.6 ticks per second over 330 s against 59.5 for a visible
control).

One finding constrains the implementation: **on Windows, a channel or park timeout wakes up to 15 ms
late** (the default timer period is 15.6 ms); `recv_timeout(5 ms)` took 10.4 ms at the median. The
game thread's "wait until the next tick or a command" therefore cannot be a plain `recv_timeout`.
The spike sleeps in slices of at most 1 ms (`thread::sleep` uses a high-resolution timer) and polls
the queue between them; `timeBeginPeriod(1)` makes the timeouts precise too (5.5 ms for 5 ms).

The limits of the evidence: the world is a toy (two `f32` pairs per entity), the editor is a thread
or a page without `egui` or a renderer, the agent is an in-process thread rather than an `rmcp`
session, and the machine ran other agents' builds meanwhile, which shows in the tails.

## What was built

`spikes/threads/` is a standalone cargo workspace of three crates plus a web page:

- **`sim`** (no threads, builds for the host and `wasm32-unknown-unknown`): a `bevy_ecs` 0.19.1
  world of N entities with a stable `Id` (assigned by the game in spawn order, never reused), `Pos`
  and `Vel`; a single-threaded schedule (`SingleThreadedExecutor`) of two chained systems, a kick of
  one entity picked by the world's PCG32 and an integration with bounces, in `+ - * /` only.
  Commands are a serde enum (`spawn`, `despawn`, `set {id, component, value}`, `pause`, `resume`,
  `step {ticks}`) that refuses unknown fields and answers `{code, message, detail}` errors
  (`bad_command`, `unknown_entity`, `not_finite`, `not_paused`). The canonical snapshot and `replay`
  live here.
- **`native`**, the `threads` binary: the game thread (`game.rs`), the editor's side of a session
  (`session.rs`), the measurements (`measure.rs`), the checks (`checks.rs`) and two probes
  (`probe.rs`: the publication slot, OS wait precision).
- **`web`**: a wasm-bindgen wrapper of `Sim`; `web/www/worker.js` is the game thread on the web,
  `editor.js` the page that measures and checks, `hidden.js` a page for the hidden-tab measurement.

### The snapshot: one canonical byte buffer

Of the three forms the task offered, the spike uses **a canonical byte buffer, published as an
`Arc`**: little-endian, a 32-byte header (magic, version, the world hash, the number of commands
applied so far, a paused flag, the entity count), then the world section that the hash covers (tick,
RNG state, next id, count) and one column per field (ids ascending, then `pos.x`, `pos.y`, `vel.x`,
`vel.y` as `f32`), 64 + 20 N bytes. The reasons:

- Charter 5.1 says the snapshot is "the same canonical data the perception layer and the world hash
  use". The determinism checks need the canonical bytes of every tick anyway to hash them, so
  publishing them costs only the swap natively (0.5 µs at the median, unpaced, for 2 MB).
- One format serves every reader: the native editor reads it in place through `SnapshotView` (rows
  found by binary search over the sorted ids), the page reads it through typed-array views without
  parsing, and the same bytes cross to the page as a transferred `ArrayBuffer` or a
  `SharedArrayBuffer` slot. An `Arc` of plain Rust data would need a second encoding for the web and
  for the hash; a triple buffer would bind the reader to a fixed slot it must copy out of or lock.
- Entities appear by stable id, never by `bevy_ecs`'s `Entity` or storage order (charter 7 item 4),
  so the native and wasm builds produce identical bytes.

The latest snapshot sits in an `arc_swap::ArcSwap<Snapshot>`. A reader takes an `Arc` and keeps it
as long as it likes; the game thread swaps the new one in and, when no reader still holds the old
one, takes its buffer back for the next build (reused on 95.6 to 100 % of ticks, 98.9 % with an
editor that holds every snapshot for 200 ms).

### The command path

- One multi-producer queue (`std::sync::mpsc`) into the game thread. An envelope carries the
  command, its source (`editor`, `agent`) and a reply channel.
- Commands are applied **between ticks, as soon as they arrive** ("eager"): while running, the game
  thread waits for the next tick's deadline in sleeps of at most 1 ms and drains the queue between
  them; while paused it blocks on the queue (a message wakes it at once). An `AtTick` mode applies
  them only at the deadline, just before the tick, for comparison. Each applied command is logged as
  `{tick, seq, source, cmd, outcome}`: `tick` is the number of ticks completed (it acts before tick
  `tick + 1`), `seq` its place in the session's order of application.
- After each batch of commands and after each tick the game publishes, so an edit made while paused
  shows at once. `step {ticks}` is logged before the ticks it runs and answered after them, so the
  answer means the stepped snapshots are out.
- `replay(n, seed, log, ticks)` applies the entries stamped `t` in `seq` order before tick `t + 1`,
  checks each outcome against the logged one and returns the hash after every tick;
  `first_divergence` names the first differing tick. Control commands are logged but skipped by the
  replay, which runs the ticks itself.

### The web form

The worker owns the wasm `Game`, ticks on its own `setTimeout` loop at 60 Hz, applies commands in
`onmessage` (between two tick tasks, so at a boundary) and publishes after each tick and command:

- **post**: the snapshot bytes are copied from wasm memory into an `ArrayBuffer` (reused when the
  page sends one back) and transferred with `postMessage`. At most two snapshots are in flight;
  beyond that the worker skips publication (never the tick) and sends the newest as soon as the page
  acknowledges. The page keeps the current buffer and returns the previous one with its
  acknowledgement.
- **sab**: three slots in a `SharedArrayBuffer`; the worker copies into its back slot and swaps it
  with the middle one by `Atomics.exchange` (a lock-free triple buffer), then `Atomics.notify`; the
  page waits with `Atomics.waitAsync` and swaps the middle slot into its front when it is fresh.

Commands go to the worker as JSON strings through `postMessage`; replies come back the same way. The
page runs the measurements and sets `document.title` to `DONE <json>` for `tools/webcheck.py`; each
run's session (log and hash chain) is in the page's report and is replayed natively afterwards.

## Measurements

Release builds, on the owner's laptop (AMD Ryzen 9 270, 8 cores and 16 threads, Windows 11 Pro
10.0.26200), with other agents building in parallel. Native times come from `Instant` (100 ns
resolution on Windows); web times from `performance.now()` (5 µs resolution with cross-origin
isolation, 100 µs without). Values are p50 / p99 unless marked; full distributions (n, mean, p50,
p95, p99, max) are in the evidence files.

### Native: tick, snapshot and publication cost

`threads bench --ticks 600` (all four N, after 30 warm-up ticks, ticks back to back while an editor
thread reads at about 120 Hz), `evidence/bench.json`. Microseconds.

| N | Snapshot bytes | Tick without publication | Snapshot + hash | Publication (swap) | Tick with publication |
|---|---|---|---|---|---|
| 100 | 2,064 | 0.4 / 0.5 | 0.5 / 0.6 | 0.2 / 0.2 | 1.1 / 1.2 |
| 1,000 | 20,064 | 2.7 / 3.6 | 2.7 / 4.2 | 0.2 / 0.3 | 4.8 / 7.3 |
| 10,000 | 200,064 | 17.3 / 19.5 | 29.7 / 45.3 | 0.2 / 0.6 | 47.6 / 71.8 |
| 100,000 | 2,000,064 | 177.2 / 263.6 | 351.1 / 633.0 | 0.5 / 2.9 | 536.3 / 830.9 |

- "Tick without publication" is a run of the systems alone (the `Off` run); the next three columns
  come from the published run, whose last column is the game thread's whole work per tick. A third
  run builds and hashes the snapshot without publishing it, as a recording would (`Hash`: 4.6, 47.9
  and 530.4 µs at the median for 1,000, 10,000 and 100,000): publishing adds only the swap.
- The editor's side at 100,000: taking the latest snapshot 0.9 µs; parsing it and recomputing its
  hash 77.2 µs.
- **At 60 Hz the same work costs more than back to back.** `threads stall` records every tick of a
  real-time run (N = 100,000): systems 338 to 354 µs at the median and 547 to 576 µs at p99,
  snapshot and hash 517 to 783 µs and 862 to 1,130 µs, the swap 3.6 to 4.4 µs. The cause was not
  isolated (a thread that sleeps 15 ms between ticks loses its caches and possibly its clock speed);
  budgets should be measured paced.

### Native: command to visible

`threads latency --samples 100`, `evidence/latency.json`: a spawn sent at a random phase against a
60 Hz game, timed to its reply and to the first snapshot that contains the new entity (the editor
spins on the slot). Milliseconds, p50 / p95 / max; the reply is within 0.1 ms of the snapshot except
at 100,000, where the snapshot build is the difference.

| N | Eager: to reply | Eager: to visible | At tick: to visible |
|---|---|---|---|
| 100 | 1.04 / 1.57 / 1.68 | 1.04 / 1.57 / 1.68 | 8.75 / 16.12 / 16.43 |
| 1,000 | 1.04 / 1.60 / 1.64 | 1.04 / 1.60 / 1.64 | 8.71 / 16.07 / 16.99 |
| 10,000 | 1.04 / 1.58 / 1.60 | 1.09 / 1.63 / 1.68 | 8.74 / 16.47 / 17.18 |
| 100,000 | 1.04 / 1.62 / 1.77 | 1.64 / 2.29 / 2.39 | 9.46 / 16.53 / 17.77 |

The eager figure is the 1 ms sleep slice (a Windows `sleep(1 ms)` lasts 1.54 ms); from publication
to the editor seeing it took 0.9 to 6.1 µs. All 800 spawns were in the snapshot that answered them,
and each of the eight sessions replayed to the same hashes.

### Native: a stalled editor, a paused game, a stalled game

`threads stall --n 100000 --seconds 3` (`evidence/stall.json`): the editor takes the latest
snapshot, sends a `set`, then sleeps while still holding the snapshot. Tick start intervals are
taken from the game thread's own records over the same 2.8 s window.

| Editor frame | Editor frames | Ticks seen per frame | Ticks / expected | Tick interval p50 / p99 / max (ms) |
|---|---|---|---|---|
| 16 ms | 183 | 1 | 168 / 168 | 16.663 / 17.398 / 17.564 |
| 200 ms | 15 | 12 (max 13) | 168 / 168 | 16.669 / 17.137 / 17.187 |
| 500 ms | 6 | 30 (max 31) | 168 / 168 | 16.673 / 17.063 / 17.152 |

`threads pause --n 100000` (`evidence/pause.json`): paused at tick 18, the editor ran 1 s of 4 ms
frames (p50 4.26, p95 5.16, max 5.60 ms), every frame saw tick 18 and the paused flag, and its 24
edits were answered in 0.051 ms (max 0.118) and visible in 0.93 ms (max 1.23, the 100,000-entity
snapshot rebuilt each time). `step 1` reached tick 19 and `step 10` tick 29. After `resume`, tick
120 took 2 s: the editor's frames over those 2.3 s stayed at p50 4.20, p95 4.54, max 7.99 ms, while
the snapshot it held aged to 2,014 ms. The session replayed to the same 138 hashes.

### Native: replay and ordering

`threads replay --n 1000` (`evidence/replay.json`, `evidence/native-session.json`): an agent thread
spawns every 23 ms while the editor sends eleven commands (sets, a spawn, a despawn, pause, a set
while paused, `step 3`, resume, two refused commands). The log holds 51 commands in the order the
game applied them (agent and editor interleaved by arrival), the replay matches all 55 hashes, the
session file re-verifies, and the same log with the editor's first command moved one tick later
diverges first at tick 3, the tick it was moved to.

### Native: the slot and the OS's waits

`threads slot` (`evidence/slot.json`): a writer swapping at 60 Hz or back to back against three
reader threads that load in a loop without pause (far worse than an editor and a few MCP sessions).
Nanoseconds, p50 / p99 / max.

| Writer | Slot | Swap | Reader load (sampled) | Loads per second |
|---|---|---|---|---|
| 60 Hz | `ArcSwap` | 1,400 / 2,600 / 7,800 | 100 / 100 / 25,500 | 28.5 million |
| 60 Hz | `Mutex<Arc>` | 400 / 7,200 / 30,700 | 100 / 800 / 92,600 | 12.9 million |
| back to back | `ArcSwap` | 300 / 500 / 317,000 | 100 / 200 / 56,000 | 25.1 million |
| back to back | `Mutex<Arc>` | 100 / 2,000 / 1,432,200 | 100 / 1,500 / 289,300 | 9.3 million |

`threads timers` (`evidence/timers.json`): what a requested wait really took, 30 samples each,
milliseconds p50 / max.

| Wait | `recv_timeout` | `park_timeout` | `thread::sleep` |
|---|---|---|---|
| 1 ms | 13.85 / 14.57 | 10.64 / 15.96 | 1.54 / 1.79 |
| 5 ms | 10.43 / 11.67 | 15.68 / 17.56 | 5.36 / 5.65 |
| 10 ms | 21.27 / 24.90 | 15.79 / 23.25 | 10.22 / 10.51 |
| 1 ms, `timeBeginPeriod(1)` | 1.39 / 3.27 | 1.41 / 2.02 | 1.54 / 1.66 |
| 5 ms, `timeBeginPeriod(1)` | 5.52 / 7.61 | 5.46 / 6.03 | 5.50 / 5.57 |
| 10 ms, `timeBeginPeriod(1)` | 10.52 / 11.48 | 10.31 / 11.01 | 10.52 / 10.55 |

### Web: the game in a worker

`tools/webcheck.py http://127.0.0.1:8704/index.html --serve spikes/threads/web/www --port 8704`
(Chrome 154 headless, cross-origin isolated), `evidence/web.json` and `evidence/web-summary.txt`.
Each configuration starts a fresh worker. Milliseconds, p50 / p95.

Game thread per tick in the worker (wasm32, no SIMD):

| N | Systems, back to back | Snapshot + hash, back to back | At 60 Hz, all work per tick: post | At 60 Hz: sab |
|---|---|---|---|---|
| 100 | under 0.005 | under 0.005 | 0.100 / 0.170 | 0.060 / 0.085 |
| 1,000 | 0.005 / 0.010 | 0.010 / 0.015 | 0.105 / 0.145 | 0.075 / 0.135 |
| 10,000 | 0.040 / 0.045 | 0.080 / 0.090 | 0.240 / 0.320 | 0.205 / 0.370 |
| 100,000 | 0.360 / 0.425 | 1.000 / 1.205 | 1.385 / 1.800 | 1.605 / 2.090 |

Snapshot from worker to page: the publication (copy out of wasm memory plus `postMessage`, or plus
the `Atomics` swap), the transfer (worker's publication time to the page holding it) and the page's
read (header, typed-array views, a sum over every entity's x):

| N | Publish: post | Publish: sab | Transfer: post | Transfer: sab | Page read |
|---|---|---|---|---|---|
| 100 | 0.070 / 0.115 | 0.035 / 0.045 | 0.125 / 0.180 | 0.070 / 0.090 | 0.020 |
| 1,000 | 0.065 / 0.095 | 0.030 / 0.055 | 0.115 / 0.150 | 0.065 / 0.080 | 0.015 |
| 10,000 | 0.090 / 0.125 | 0.050 / 0.100 | 0.135 / 0.195 | 0.060 / 0.080 | 0.020 |
| 100,000 | 0.160 / 0.240 | 0.185 / 0.385 | 0.130 / 0.220 | 0.085 / 0.115 | 0.100 |

Command latency (a spawn from the page, applied by the worker on arrival, until a snapshot holding
the entity reaches the page), 30 samples per run, p50 / p95 / max:

| N | post: to reply | post: to visible | sab: to reply | sab: to visible |
|---|---|---|---|---|
| 100 | 0.22 / 0.32 / 1.93 | 0.28 / 0.40 / 2.10 | 0.26 / 0.36 / 1.73 | 0.30 / 0.42 / 1.80 |
| 1,000 | 0.27 / 0.37 / 2.17 | 0.34 / 0.52 / 2.26 | 0.25 / 0.37 / 1.67 | 0.30 / 0.43 / 1.76 |
| 10,000 | 0.26 / 0.43 / 1.76 | 0.44 / 0.72 / 1.92 | 0.26 / 0.63 / 2.24 | 0.39 / 0.96 / 2.59 |
| 100,000 | 0.25 / 1.52 / 1.70 | 1.31 / 2.47 / 2.66 | 0.30 / 1.87 / 2.10 | 1.67 / 2.81 / 3.88 |

Applying commands only at the next tick instead (N = 1,000): 10.70 / 19.91 / 21.82 ms (post) and
12.19 / 20.19 / 20.50 ms (sab) to visible.

Checks, in every eager run (four N, two modes):

- **Stalled page** (the main thread busy for 200 ms): the worker ran 12 ticks meanwhile in every run
  (intervals at most 19.2 to 21.4 ms, the worker timer's jitter). In post mode the first snapshot
  the page then received was the one queued before the stall, 190 to 197 ms old, and a fresh one
  followed 0.38 to 0.62 ms later (10 publications had been deferred by the flow control); in sab
  mode the page took the newest slot at once, 6.6 to 15.4 ms old, with nothing deferred.
- **Paused game**: 1 s of the page's 4 ms frames (p50 4.76 to 5.01, p95 6.0 to 7.0 ms), no frame
  with a wrong tick, edits visible in 0.31 to 2.0 ms at the median, `step 1` and `step 10` exact,
  and a `step` with an unknown field refused with `bad_command`.
- **Stalled game** (the worker busy for 2 s): the page's frames stayed at p50 4.1 to 4.6 and p95 5.2
  to 6.0 ms (one frame of 23.3 ms in one run) while its snapshot aged to 2.0 s.
- **Replay**: all ten sessions replayed inside the worker to the same hashes, and the eight eager
  sessions replayed natively to the same hash at every tick (`evidence/web-verify.txt`).
- **Not isolated**: served by `python -m http.server` without COOP and COEP (`crossOriginIsolated`
  false), the post mode passed every check at N = 1,000 and 100,000 with the same figures at 0.1 ms
  resolution (100,000: publish 0.1, transfer 0.2, command to visible 1.5 ms), and both sessions
  replayed natively (`evidence/noniso/`).
- **Hidden page** (`scripts/hidden_check.py`: another tab brought to the front of the page's window
  for 330 s, past Chrome's 5-minute threshold for throttling a hidden page's timers, while a control
  copy of the page runs in a visible window of its own; `evidence/hidden.json`): the hidden page's
  worker ran 58.6 ticks per second (19,348 ticks, interval p50 16.73 ms), the visible control's
  59.5. The three longest gaps (0.75 to 1.06 s) hit both pages at the same moments, so the machine
  caused them; the hidden page alone had a cluster of 50 to 405 ms gaps 45 to 53 s after it was
  hidden (an earlier run without the control: 30 to 49 s). Nothing like a one-per-second throttle
  appeared.

The wasm module is 931,227 bytes as wasm-bindgen writes it (no `wasm-opt`), with 10,300 bytes of
JavaScript glue.

## Recommendation for the threads specification

`docs/spec/threads.md` (Draft) already describes this design; the spike confirms it and asks for the
following.

1. **Snapshot.** A canonical byte encoding in sections, entities by stable id in ascending order,
   columns per field, with a small header (tick, writes applied, flags) outside the hashed bytes.
   Publish after every tick and after every batch with a write; recycle the previous buffer when no
   reader holds it. The full encoding is the dominant cost of a tick (about 0.17 to 0.4 ms per MB
   natively, 0.5 ms per MB in wasm), so the budget is a function of the world's byte size, and
   incremental publication (spec open choice 2) stays the remedy if a real world misses it.
2. **The slot** (open choice 1): `arc-swap`. Under three readers that never pause, a `Mutex<Arc>`
   made the writer wait up to 30.7 µs at 60 Hz and 1.43 ms back to back, against 7.8 µs and 317 µs;
   both are small against a 16.7 ms tick, but `arc-swap`'s readers take no lock, so no reader
   descheduled inside one can hold up a publication, which is what charter 5.1 promises ("never
   stalls the tick"). A mutex is acceptable if one dependency fewer matters more.
3. **Waiting on Windows** (spec 3.2, `WaitUntil`): state that the wait must be precise to about a
   millisecond, and that `std`'s `recv_timeout` and `park_timeout` are not on Windows. Either sleep
   in slices of at most 1 ms and poll the queue (the spike: tick intervals p99 17.1 to 17.4 ms, max
   17.6 ms; about 650 wake-ups per second while idle), or call `timeBeginPeriod(1)` while real time
   runs and `timeEndPeriod(1)` when it stops (no rights needed; measured 1.4 to 5.5 ms for 1 to 5 ms
   waits). `WaitForCommand` can block on the queue without a timeout's precision (51 µs round trip).
4. **When commands apply.** Apply arrivals at the current boundary while waiting for the deadline
   (1.0 to 1.6 ms to visible natively, 0.3 to 1.7 ms on the web) rather than holding them for the
   next tick (8.7 to 9.5 ms at the median, 17.8 ms worst). Determinism does not depend on this
   choice: the recording does.
5. **Order against agents' actions.** One queue for every source (editor, MCP sessions, the host's
   imports and hot updates, network input); the game thread is the only consumer and the single
   point where order is decided, and the recording (tick, index, source, seq) is the truth a replay
   follows, whatever the arrival order was. The spec's canonical sort of a batch by `(source, seq)`
   is compatible with this and is what lockstep needs. One correction: with commands applied as they
   arrive, a boundary can see several batches (the loop goes round each time a command wakes it), so
   "the edit applies first, always" (spec 5.2) holds only within a batch. Commands scheduled with
   `at` arrive as one batch at their boundary and keep the guarantee; for the rest the spec should
   promise only "in the recorded order".
6. **Replies and reads.** A write is answered with `{seq, tick, outcome}`; a reader (editor or
   agent) knows its write is visible when a snapshot's applied count passes its `seq`, which gives
   read-your-writes without blocking the game thread. `step` is answered after its last tick.
7. **The web form** (charter 12 item 8, spec open choice 4): **a Web Worker with transferred
   `ArrayBuffer`s**, the only form. The `SharedArrayBuffer` triple buffer was no cheaper to publish
   (0.185 against 0.160 ms at 100,000), 0.05 ms faster to arrive, and needs isolation; its one
   advantage is after a page stall, where post mode delivers one stale queued snapshot first and the
   fresh one 0.4 to 0.6 ms later. Flow control is required in post mode (the spike: at most two in
   flight, the page returns the previous buffer, the worker skips publication meanwhile, never a
   tick); without it a stalled page would queue 2 MB per tick. The form works without isolation.
8. **Placeholders of spec section 12** (this spike's toy world; a real world scales with its bytes):

| Placeholder | Value from this spike |
|---|---|
| `{THREADS_PUBLISH_US}` | snapshot and hash p50: 0.5, 2.7, 29.7, 351 µs for 10^2 to 10^5 entities unpaced; 517 to 783 µs at 10^5 and 60 Hz; the swap itself 0.2 to 4.4 µs |
| `{THREADS_RTT_PAUSED_US}` | 51 µs p50, 118 µs max (reply); 0.93 ms to visible at 10^5 |
| `{THREADS_RTT_RUNNING_MS}` | 1.04 p50, 1.6 p95 with 1 ms slices; 8.7 p50, 16.5 p95 if applied at the next tick |
| `{THREADS_STALL_FRAME_P95_MS}` | 4.54 ms for a 4 ms editor frame during a 2 s tick (the stall adds nothing measurable) |
| `{WEB_SNAPSHOT_POST_MS}` | publish + transfer + read p50: 0.21 (10^2), 0.20 (10^3), 0.25 (10^4), 0.39 (10^5) |
| `{WEB_RTT_MS}` | 0.22 to 0.30 p50, 0.32 to 1.87 p95 (reply) |
| `{WEB_HIDDEN_TICK_HZ}` | 58.6 over 330 s hidden, against 59.5 for a visible control at the same time |
| `{WEB_ISOLATION_OFF}` | yes: the post form passed every check with `crossOriginIsolated` false |

9. **Budgets are measured paced.** Per-tick costs at 60 Hz were 1.5 to 2.3 times the back-to-back
   ones natively; the `perf` step should run real time, and warm up a wasm module before timing it
   (each worker that had not run a warm-up had one tick of 3.8 to 6.1 ms; the warmed ones stayed
   under 0.6 ms).

## Recipe for a later slice

1. Give entities a stable id assigned by the game and never reused; keep `Entity` on the game
   thread.
2. Build the canonical snapshot with a reusable builder (cached `QueryState`, scratch rows sorted by
   id, columns written with `as_chunks_mut::<4>()`), hash its world section (xxh3-64 here;
   spec-persist decides), and write the hash into the header.
3. Run the game on its own thread: paused, block on the queue; running, wait for the deadline in at
   most 1 ms sleeps (or with `timeBeginPeriod(1)` on Windows), applying arrivals at the current
   boundary; publish after each tick and each batch; answer `step` after its ticks; log every
   applied command with its tick and sequence number.
4. Publish through `ArcSwap<Snapshot>`; on swap, `Arc::try_unwrap` the old snapshot and keep its
   buffer for the next build.
5. Replay: apply entries stamped `t` in `seq` order before tick `t + 1`; compare outcomes and
   hashes; report the first diverging tick. Session files (`{n, seed, log, hashes}`) replay on any
   target.
6. On the web: export `snapshot_ptr` and `snapshot_len` from the wasm `Game`; the worker copies the
   bytes into a reused `ArrayBuffer` and transfers it with at most two in flight; the page returns
   the previous buffer with each acknowledgement; commands travel as JSON strings and are applied in
   `onmessage`. Check with `tools/webcheck.py` and replay the page's sessions natively.

## Versions and URLs

| What | Version | Where |
|---|---|---|
| Rust | 1.98.1 (`rust-toolchain.toml`), targets host and `wasm32-unknown-unknown` | rustup |
| `bevy_ecs` | 0.19.1, `default-features = false`, `std` (0.20.0-rc.2 was the newest, not stable) | https://crates.io/crates/bevy_ecs/0.19.1 |
| `arc-swap` | 1.9.2 | https://crates.io/crates/arc-swap/1.9.2 |
| `serde` / `serde_json` | 1.0.229 / 1.0.151 | https://crates.io/crates/serde_json |
| `xxhash-rust` | 0.8.19 (`xxh3`) | https://crates.io/crates/xxhash-rust/0.8.19 |
| `wasm-bindgen` and `wasm-bindgen-cli` | 0.2.129 (the CLI was already installed) | https://crates.io/crates/wasm-bindgen/0.2.129 |
| Chrome | 154.0.8037.93, `--headless=new`, through `tools/webcheck.py` | installed |
| Python, `websocket-client` | 3.14.3, 1.9.2 | installed |

Nothing was downloaded or installed beyond the crates above from crates.io.

## Problems met and how they were solved

1. **`bevy_ecs` 0.19 has no `ExecutorKind`.** The executor is set with
   `Schedule::set_executor(SingleThreadedExecutor::default())`; without the `multi_threaded` feature
   the default is single-threaded anyway.
2. **Late ticks from timed waits on Windows.** A probe before the game loop showed `recv_timeout`
   and `park_timeout` waking 5 to 15 ms late; the loop sleeps in 1 ms slices instead (see the timers
   table).
3. **A session that never started.** In the "systems only" bench the first snapshot was never
   published, and the editor waited for it; the first snapshot is now always published.
4. **`step 10` reported tick 20 instead of 29.** The editor waited for a snapshot whose applied
   count covered the step, which the first stepped tick already satisfied. `step` is now answered
   after its last tick (and still logged before it, so the log keeps the order of effects).
5. **A stalled page and post mode.** Without flow control every tick would queue a 2 MB transfer for
   the blocked page; at most two are in flight now, and the worker publishes the newest when the
   page acknowledges.
6. **A test expectation off by one.** A command moved from boundary 3 to boundary 4 first changes
   the hash after tick 4, so `first_divergence` returns the tick it was moved to.
7. **Not solved: one slow tick in wasm** (3.8 to 6.1 ms) in each worker that ran no warm-up, likely
   V8 tiering up the module; which tick it was and why were not investigated.

## What remains open

- **Hidden pages**: why a hidden page's worker loses a cluster of ticks 30 to 50 s after hiding was
  not investigated (a lower priority for a background renderer is a guess); a headed browser, a
  laptop on battery and other browsers were not tried. The worker drops the ticks it falls behind by
  more than four periods (game time slows rather than bursts), which the time model must own.
- **Real world size**: many component types, strings, the registry and the event stream of spec 4.4
  were not built; neither was incremental publication.
- **The presenter**: no `egui` editor, no `winit` wake-up on publication, no renderer reading
  snapshots; the native editor is a thread with sleeps.
- **MCP**: the agent is an in-process thread; no `rmcp` session sent commands.
- **Spec details not exercised**: the canonical `(source, seq)` sort, a bounded queue with
  `queue.full`, `at`-scheduled commands, a panic on the game thread, one snapshot in flight on the
  web instead of two.
- **Other platforms**: wait precision on Linux and macOS was not measured; slices combined with
  `timeBeginPeriod(1)` were not measured; WebAssembly threads with shared memory were not tried.
