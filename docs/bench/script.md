# Script cost: the rule workload in the engine (Pioneer)

- Date: 2026-10-14; timings re-measured on a quiet machine 2026-10-15. Branch `explore/rules`.
  Charter 4.2 (performance position: "interpreted 21 to 27 times slower than the same Rust rules",
  from the script-native spike) and 8, item 1 (a JIT backend, native V8 or the browser's own engine,
  "measure the same workload first"). Budgets `tick.rules`, `tick.rules.p99` and `web.tick.rules`
  ([spec/budgets.md](../spec/budgets.md) 4 and 5).
- Machine: ASUS ROG Zephyrus G14 (budgets.md R1): AMD Ryzen 9 270 (16 threads), 15 GiB, Windows 11
  Pro 10.0.26200, on AC power; Rust 1.98.1; QuickJS-ng compiled by clang-cl 23.1.2 natively and by
  clang 23.1.2 for wasm32; Node 25.8.1; Chrome 155 (headless). CPU only.
- **The timings are a quiet re-measurement** (2026-10-15, section 6's protocol): the other tracks
  had finished, no build or other timing ran, and each run held the machine-wide bench lock
  throughout; CPU load was 3 to 7 % before each round (section 5). The run was repeated once. They
  replace the provisional run of 2026-10-14, taken while five agents shared the machine; section 5
  says what changed. The quiet runs are not calibrated runs (budgets.md 7.5). Steps, event counts
  and digests do not depend on the machine: they are exact.

## 1. The workload, its twin and the probes

`bench/rules` is the rule workload of budgets.md 4 as slice 2 ported it from the script-native spike
(57ed4ea1), written for today's SDK ([sdk.md](../sdk.md)): five project components (`Attack`,
`Spot`, `Health`, `Armor`, `Stats`); `populate` spawns 1,000 fighters at tick 1 from the system's
random stream, each aiming at another; `damage` runs every tick over one query of the five
components with eight columns: every living fighter hits its target (falloff by distance, armor, a
critical roll from `ctx.rng.fill`, the engine's `Math.sin` and `Math.cos`), the dead respawn, the
living regenerate and everyone drifts. Two changes from the port: event kinds are dotted
(`combat.damaged`, `combat.died`), and the hits go out 250 to a `combat.damaged` event, since event
data now holds at most 16 KiB as JSON (one event of 1,000 hits is 28.5 KB and fails the call). A
tick emits four `combat.damaged` events (992 hits on average) and one `combat.died` (15 kills on
average over 720 ticks).

The variants, each in a fresh world of seed 1 at 60 Hz with the default `ScriptLimits`:

- **rules**: the bundle through the real host: `install`, `swap`, then `Sim::step`.
- **twin**: the same two systems in Rust (`crates/amoris-script/examples/rules_bench/workload.rs`)
  over Rust components, with no script host: the rows in id order into columns, the same two loops
  (the same draws from the same streams, the engine's `math::sin`, `cos`, `sqrt` and `min`), the
  columns back into the components, the same events with the same data. It is the same rule written
  in Rust for this engine, boundary work included (gathering the rows, building the event data).
- **idle**: `populate`, then a script system with no query and an empty `run`: the fixed cost of a
  call.
- **scan**: `populate`, then a script system with `damage`'s query (1,000 rows, eight columns) and
  an empty `run`: the cost of handing a query over and comparing it back.

Before timing, the script world and the twin run side by side for 720 ticks and their digests (every
entity's eight fields as bits in id order, the event inbox as JSON, the event counter, the allocator
and the clock) must agree at every tick; `crates/amoris-script/tests/rules.rs` keeps that check (20
ticks) in the crate's tests. The WebAssembly module runs the same comparison in Node and in Chrome
and must end on the native final digest.

## 2. Method

- Native: `rules_bench` (release profile: `opt-level = 3`, 16 codegen units, no LTO). Web:
  `rules_web`, the same worlds behind exports, built for `wasm32-unknown-unknown` with the `web`
  profile (fat LTO, one codegen unit, `panic = "abort"`), as is (`web`) and after
  `wasm-opt -O3` with `tools/build_web.sh`'s flags (`web-opt`); timed under Node and in a module
  Web Worker of headless Chrome served cross-origin isolated (`performance.now()` at 5 us
  resolution; Node's is finer).
- Per round and variant: a fresh world (a fresh module instance on the web), 120 warm-up ticks, 600
  measured ticks. The tick is the wall time of `Sim::step` alone (natively `Instant`, on the web
  `performance.now()` around the module's `tick()` export). The step hooks time `script.update`
  and the measured system (`script:damage`, `script:idle`, `script:scan`, or `twin.damage`) with
  the same clock. A clock injected into the host (`ScriptHost::time_phases`, new in this round)
  splits each script call into **prepare** (the transaction, the `ctx` object, each query's columns
  built and wrapped), **run** (`run` and its jobs), **diff** (the checks after `run` and every
  handed-over column compared with its original) and **apply** (the changed cells and the commands
  applied, the transaction committed). Steps are the host's exact work counter.
- Rounds interleave the targets (native, Node, Chrome) in an order that rotates by round, and the
  variants within a target likewise; the two web modules alternate their order by round (`web`
  first in rounds 1, 3 and 5). The quiet runs held the machine's bench lock for their whole length
  (`bench_lock.py run -- rules_bench.py run`); `--lock` instead takes it around each timed command,
  for a shared machine. The machine conditions (`tools/machine_conditions.py`) are recorded before
  the run, before each round and after it, and each result file records its command's start and end
  (`wall`, lock waits included).
- The clock readings cost little against the workload: 13 a tick for a script world (two per
  system, five per call), 6 for the twin; a `performance.now()` call takes about 50 ns under Node,
  so under 1 us a tick on the web, below 1 % of the twin's tick.
- Statistics: per round, the median, minimum and p99 (nearest rank) of the 600 ticks; the tables
  give each round's median, the lowest minimum and the median of the rounds' p99.

## 3. Results (quiet re-measurement, 2026-10-15)

Run: `python tools/rules_bench.py run out/rules/quiet --rounds 5` inside
`bench_lock.py run --label quiet-rules --`, 2026-10-15 06:56 to 07:00, then once more into
`out/rules/quiet-repeat` (07:01 to 07:05) to see whether the first run's shape repeats. Every
target's own verification passed in both runs: the script world and the twin agreed at all 720
ticks, and the native, Node and Chrome runs (both modules) ended on the same final digest
`e13690add0b3a196`. Over those 720 ticks the rule emitted 2,880 `combat.damaged` events (714,648
hits) and 10,828 kills.

Per tick, in microseconds, from the first run. "Tick" is `Sim::step`; "System" is the measured
system alone (`script:damage`, `twin.damage`, `script:idle`, `script:scan`). Chrome's clock has 5
us steps, so its `idle` and `scan` rows are at the clock's resolution.

| Target | Variant | Tick median (rounds 1 to 5) | Tick min | Tick p99 (median of rounds) | System median | Steps |
|---|---|---|---|---|---|---|
| native | rules | 3,598 / 3,612 / 3,600 / 3,661 / 3,598 | 3,482 | 4,018 | 3,572 | 62,518 |
| native | twin | 134 / 135 / 133 / 136 / 133 | 129 | 154 | 115 | 0 |
| native | idle | 6.5 / 6.4 / 6.4 / 6.5 / 6.4 | 6.2 | 6.9 | 5.7 | 7 |
| native | scan | 262 / 257 / 268 / 262 / 257 | 254 | 306 | 260 | 14 |
| Node, web | rules | 3,171 / 3,480 / 3,176 / 3,246 / 3,299 | 2,980 | 4,405 | 3,208 | 62,518 |
| Node, web | twin | 155 / 158 / 159 / 157 / 155 | 152 | 250 | 127 | 0 |
| Node, web | idle | 9.3 / 13.9 / 7.9 / 8.0 / 8.4 | 7.6 | 27.2 | 7.1 | 7 |
| Node, web | scan | 177 / 179 / 181 / 178 / 189 | 173 | 321 | 177 | 14 |
| Node, web-opt | rules | 3,142 / 3,159 / 3,165 / 3,167 / 3,186 | 2,959 | 4,213 | 3,127 | 62,518 |
| Node, web-opt | twin | 157 / 156 / 156 / 156 / 155 | 150 | 242 | 125 | 0 |
| Node, web-opt | idle | 8.6 / 13.5 / 12.5 / 8.0 / 8.3 | 7.6 | 21.3 | 7.2 | 7 |
| Node, web-opt | scan | 179 / 181 / 178 / 189 / 180 | 172 | 318 | 178 | 14 |
| Chrome, web | rules | 3,680 / 3,140 / 3,195 / 3,130 / 3,770 | 2,980 | 4,350 | 3,155 | 62,518 |
| Chrome, web | twin | 160 / 170 / 155 / 155 / 155 | 145 | 240 | 125 | 0 |
| Chrome, web | idle | 10 / 10 / 15 / 10 / 10 | 5 | 25 | 10 | 7 |
| Chrome, web | scan | 175 / 180 / 195 / 290 / 180 | 170 | 335 | 180 | 14 |
| Chrome, web-opt | rules | 3,370 / 3,175 / 3,095 / 3,190 / 3,145 | 2,900 | 4,675 | 3,130 | 62,518 |
| Chrome, web-opt | twin | 155 / 170 / 160 / 175 / 155 | 150 | 260 | 130 | 0 |
| Chrome, web-opt | idle | 10 / 10 / 10 / 10 / 10 | 0 | 20 | 10 | 7 |
| Chrome, web-opt | scan | 180 / 180 / 180 / 180 / 175 | 170 | 310 | 175 | 14 |

The script against the twin:

| Target | rules / twin, tick (median of the round medians) | by minimum | `damage` / `twin.damage`, system median |
|---|---|---|---|
| native | 26.9 | 27.1 | 31.2 |
| Node, web | 20.7 | 19.6 | 25.3 |
| Node, web-opt | 20.3 | 19.8 | 25.0 |
| Chrome, web | 20.6 | 20.6 | 25.2 |
| Chrome, web-opt | 19.8 | 19.3 | 24.1 |

Where a script call's time goes (the median of the rounds' medians of each phase, us;
`script.update` adds under 0.1 % to the system natively and under Node):

| Target | Variant | prepare | run | diff | apply | Boundary (prepare, diff, apply) |
|---|---|---|---|---|---|---|
| native | rules | 267 | 2,807 | 135 | 334 | 21 % of the system |
| native | scan | 245 | 1.0 | 12.7 | 1.0 | |
| native | idle | 4.8 | 0.6 | 0.1 | 0.2 | |
| Node, web | rules | 171 | 2,604 | 112 | 273 | 18 % |
| Node, web | scan | 147 | 1.3 | 26.8 | 1.4 | |
| Node, web | idle | 5.7 | 1.0 | 0.1 | 0.2 | |
| Node, web-opt | rules | 174 | 2,536 | 106 | 270 | 18 % |
| Chrome, web | rules | 165 | 2,575 | 115 | 280 | 18 % |
| Chrome, web-opt | rules | 165 | 2,535 | 115 | 280 | 18 % |

The second run, `rules`, against the first (ticks in us; the medians are the medians of the round
medians):

| Target | Tick median, run 1 (rounds) | Tick median, run 2 (rounds) | rules / twin, run 2 | Tick p99 by round, run 1 and run 2 |
|---|---|---|---|---|
| native | 3,600 (3,598 to 3,661) | 3,628 (3,600 to 3,719) | 27.1 | 3,990 to 4,222; 3,917 to 4,248 |
| Node, web | 3,246 (3,171 to 3,480) | 3,330 (3,237 to 3,621) | 21.1 | 4,112 to 4,494; 5,037 to 5,232 |
| Node, web-opt | 3,165 (3,142 to 3,186) | 3,198 (3,154 to 3,222) | 20.4 | 4,180 to 5,034; 4,056 to 4,997 |
| Chrome, web | 3,195 (3,130 to 3,770) | 3,290 (3,245 to 3,890) | 20.6 | 4,230 to 4,945; 4,770 to 5,925 |
| Chrome, web-opt | 3,175 (3,095 to 3,370) | 3,140 (3,110 to 3,225) | 20.3 | 4,290 to 5,085; 4,210 to 4,905 |

The phases of the second run are within 6 % of the first's (native `prepare`, `run`, `diff`,
`apply`: 274, 2,815, 143, 335 us; the boundary 21 % natively and 17 to 18 % in WebAssembly).

`web-opt` against `web`, the `rules` tick median paired by round (`web` ran first in rounds 1, 3 and
5):

| Host | Run 1, rounds 1 to 5 | Run 2, rounds 1 to 5 |
|---|---|---|
| Node | -0.9 / -9.2 / -0.3 / -2.4 / -3.4 % | -5.6 / -4.0 / -3.4 / -0.7 / -11.0 % |
| Chrome | -8.4 / +1.1 / -3.1 / +1.9 / -16.6 % | -20.1 / -1.1 / -5.3 / -3.5 / -16.9 % |

## 4. What the figures say

1. **The ratio.** In the engine the interpreted rule costs 27 times its Rust twin natively (tick
   against tick, by the median and by the minimum alike, in both quiet runs) and 20 to 21 times in
   WebAssembly, under Node and in Chrome (19 to 21 by the minimum). Counting the systems alone,
   without the rest of the tick (19 to 45 us), the ratios are 31 and 24 to 26. The charter's 21 to
   27 (the spike's ticks, by minimum) holds as an order of magnitude: native sits at its top and
   above it for the system alone, the web at its bottom or just below it.
2. **The boundary around the call is a fifth of the script's tick.** Natively 79 % of `damage` is
   `run` and 21 % is the batch interface around it: 267 us to hand over the query (the transaction,
   the `ctx` object, the prelude's wrappers, each column built as a typed array), 135 us to compare
   the columns back, 334 us to apply the changed cells (about 3,000: `hp`, `x` and `y` of every row,
   and the kills). `run` is not interpretation alone: it also holds the natives the rule calls,
   about 3,000 of the engine's `Math.sin` and `Math.cos` (Rust's math through the sandbox), five
   `ctx.emit` calls that convert about 4,000 numbers into event data and check its size, and one
   `ctx.rng.fill`; this bench does not separate them from the interpreter. The spike split its tick
   differently, so its figures do not compare with these: its 3 to 5 % was the time inside its eight
   batch API host functions, its 3,000 `Math` host calls were costed apart at 12 to 17 % (from a
   per-call cost) and the rest, about 80 %, was interpretation (script-native.md, "Rule workload per
   tick"). Here the boundary is everything outside `run`, with the `Math` natives inside `run`. That
   boundary alone is six times the twin's whole system. The probes isolate its parts: a call costs
   about 6 us with nothing in it (5 us of it before `run`: the transaction and the `ctx` object); a
   1,000-row, eight-column query costs 260 us natively to hand over and compare back unchanged,
   about 31 ns a cell to build; applying costs about 100 ns a changed cell, since each cell is
   written by reading and writing its whole component through the accessor (`call::apply`).
3. **The shipped WebAssembly build runs the script faster than the native release build, for causes
   not isolated.** The script world's tick is 3.2 ms in Chrome and under Node (3.1 to 3.3 ms by
   module and run) against 3.6 ms natively, every phase faster (in the first run `run` by 7 to 10 %,
   `prepare` by 35 to 38 %), while the twin is slower in WebAssembly by 16 to 20 %. Interpreter
   dispatch is not the difference: both builds use QuickJS-ng's `switch`, since quickjs.c (line 51)
   sets `DIRECT_DISPATCH` to 0 under `_MSC_VER`, which clang-cl defines natively, and under
   `EMSCRIPTEN`, which the vendored `rquickjs-sys` build script defines for `wasm32-unknown-unknown`
   (since P12, section 7, the native build uses computed `goto`). Four differences between the two
   builds remain, none isolated here: the Rust profiles (the `web` profile's fat LTO and one codegen
   unit against the release profile's no LTO and 16 units, which matters most to the host's Rust
   across rquickjs, `prepare` and `apply`); QuickJS-ng's C optimization (`-O3` for wasm32 against
   `-O2` natively: cc-rs 1.6.0 caps the MSVC family, clang-cl included, at `/O2`; corrected
   2026-10-15, section 7: clang-cl's `-O2` is its compiler's `-O3`, so the level is the same, and
   turning off the five MSVC defaults that do differ gained nothing, measured quietly);
   QuickJS-ng's allocator (the host creates its
   runtime with QuickJS's default `malloc`, the Windows CRT heap natively and wasi-libc's dlmalloc
   in wasm32; `prepare`, which allocates the most, differs the most); and `panic = "unwind"` against
   `"abort"`.
4. **`wasm-opt -O3` makes the module slightly faster under Node; in Chrome the quiet runs do not
   resolve it.** This changes the provisional run's reading (no difference resolved on either host).
   Under Node the optimized module was faster in all ten paired rounds of the two quiet runs, by 0.3
   to 11 % (median 3.4 %), whichever module ran first. In Chrome its pairs in rounds 2 to 4 range
   from 5.3 % faster to 1.9 % slower; rounds 1 and 5 favour it by 8 to 20 %, but in both runs those
   are the rounds in which Chrome's `web` module ran first right after Node's runs, and in both runs
   it was slow there (3.7 to 3.9 ms against 3.1 to 3.3 in its other rounds), so they may measure
   that position, not the module. The `web` module also had the slow Node rounds (round 2 of the
   first run, round 5 of the second, 3.5 and 3.6 ms): Node's per-tick times show stretches of a few
   hundred ticks 10 to 20 % slower that start and stop within the round, at 3 to 7 % CPU load;
   `web-opt` had one round of that kind in twenty (Chrome, round 1 of the first run, 3.4 ms). Their
   cause is not isolated. `rules_bench.py table` prints the pairing.
5. **Against the budgets.** `tick.rules`: 3.6 ms at the median (round medians 3,598 to 3,719 us
   over both quiet runs), inside the spike's 4.0 ms figure and the 4.2 ms ceiling; scripts at this
   scale still take a fifth of a 60 Hz tick. `tick.rules.p99`: 3.9 to 4.2 ms in every round of
   both runs, inside the 6.5 ms figure; the provisional run's 5.9 and 7.5 ms rounds were the shared
   machine's load. Slice 2's first, uncalibrated run of its port (57ed4ea1 on origin/rebuild, its
   budgets.md 13, item 7) had 10.1 ms at the p99 and 3.5 to 4.2 ms at the median.
   `web.tick.rules`: 3.1 to 3.3 ms in Chrome's worker (both modules, both runs), far inside its
   8.3 ms ceiling; the web tick's p99 is 4.1 to 5.9 ms by round, above the native 3.9 to 4.2 ms.
   Steps: 62,518 a tick, 6 % of `steps_per_system` and 3 % of `steps_per_tick`.
6. **For charter 8, item 1 (a JIT backend).** Four fifths, the `run` share, is an upper bound on
   what a JIT could win back, not the interpreter's share: `run` also holds the natives of item 2.
   By the spike's per-call cost of the engine's `Math.sin` (120 to 134 ns with clang-cl,
   script-native.md, "Math per call"), the 3,000 `sin` and `cos` calls take about 0.4 ms, which puts
   interpretation nearer two thirds of the tick, less what `emit` and `fill` take. Section 7.2 later
   measured that closure in this hardened context at 88 to 89 ns a call over its loop (quietly; 88
   to 91 under load), which makes the calls about 0.27 ms and leaves interpretation, `emit` and
   `fill` together about 70 % of the tick (2.8 ms of `run` less 0.27, of 3.6); since the `Math`
   change the calls cost 33 to 36 ns, about 0.10 ms. Nor is the boundary's fifth independent of the
   engine: `prepare` runs the prelude's JavaScript (`__context`, `__query`) and builds the engine's
   typed arrays, and it already differs by more than a third between this engine's two builds.
   Splitting `run` (the time inside natives, or a probe making the rule's native calls without its
   arithmetic) would measure the interpreter's share before a JIT is tried; section 8 measures it
   (quiet re-measurement, 2026-10-16): interpretation is 67 to 68 % of `run` and 51 to 52 % of the
   tick, and the natives' 0.83 to 0.85 ms is nearly all the events' conversion. The browser's own
   JavaScript engine cannot be tried with this module (QuickJS-ng is compiled into it): that needs
   the same rule and the same `amoris` batch API on V8 itself, timed against the same twin, the next
   measurement this baseline makes comparable.

Candidates this baseline can measure next (none is done here; section 7 has since done the
first two):

- **P12, QuickJS-ng's computed `goto` on clang-cl (native only)**: quickjs.c turns
  `DIRECT_DISPATCH` off for any `_MSC_VER`, which clang-cl defines though it supports computed
  `goto`; a patch to the vendored QuickJS-ng (third_party, with its own P-number and tests) would
  show its worth natively in `run`. The WebAssembly build predicts nothing about it: it uses the
  `switch` as well, and WebAssembly has no indirect branch (LLVM lowers a computed `goto` to one
  `br_table`).
- **QuickJS-ng at `-O3` natively**: the build script passing clang-cl `-O3` (for example
  `/clang:-O3`) past cc-rs's `/O2`, a third_party change; it isolates one of item 3's differences.
- **QuickJS-ng's allocator**: rquickjs's `rust-alloc` feature (Rust's allocator) or another native
  heap, measured on `prepare` and `run`; another of item 3's differences.
- **Applying writes per component, not per cell**: group a call's changed cells by entity and
  component in `call::apply`, one read and one write each.
- **LTO for the native host**: the release profile with `lto = "thin"` or `"fat"` would show whether
  `prepare` and `apply` fall to the WebAssembly figures.

## 5. Conditions

- Quiet re-measurement, 2026-10-15: the other tracks had finished and no build, browser or other
  timing ran; the bench lock's log shows nothing else holding it during either run. Before the run,
  before each round and after it (`tools/machine_conditions.py`): on AC power, Balanced plan; CPU
  load 3.2 to 7.0 % (06:56 to 07:00) and 4.8 to 6.5 % (07:01 to 07:05); the RTX 5060 idle at P2,
  55 to 61 C. The binaries were rebuilt after e3eb541c (the `error_len` export and the twin's
  write-back by the entities it gathered), which leaves every digest as it was. Raw files:
  `out/rules/quiet/` and `out/rules/quiet-repeat/` (ignored).
- Against the provisional run (2026-10-14 19:52 to 20:08, five agents sharing the machine, CPU load
  51 % at its start and 27 % at its end, `out/rules/wave-2026-10-14/`): the native medians agree
  within 1 % (3,600 and 3,628 us against 3,624), and the provisional native outliers (round 1 at
  4,569 us, round 5 at 4,022 us, p99 7.5 and 5.9 ms) do not recur, so they were that run's load.
  The WebAssembly medians came out 2 to 5 % higher than the provisional ones (3,140 to 3,330 us
  against 3,035 to 3,169) and their p99 higher (4.1 to 5.9 ms by round against the provisional
  median of rounds 3.6 to 3.9 ms), with more slow rounds in the `web` module (item 4 of section 4);
  the cause is not isolated. None of the section 4 conclusions turns on that 2 to 5 % except the
  ratio's web range (19 to 20 provisionally, 20 to 21 now). `wasm-opt -O3`'s reading changed: see
  section 4, item 4.
- Chrome ran headless with a throwaway profile, the page cross-origin isolated
  (`crossOriginIsolated` true in every round), the module in a Web Worker.

## 6. Re-measuring on a quiet machine

From the worktree or checkout, with nothing else building (section 3's runs held the bench lock
around the whole `run`, `python <bench_lock.py> run --label quiet-rules -- python
tools/rules_bench.py run ...`; `--lock` is for a machine where other agents may time between its
commands):

```sh
python tools/rules_bench.py build
python tools/rules_bench.py run out/rules/quiet --rounds 5 [--lock <bench_lock.py>]
python tools/rules_bench.py table out/rules/quiet
```

`run` writes one JSON per target, build and round into the directory (raw per-tick times for the
native and Node runs, each file's start and end), the compiled sets, the verification and the
machine conditions before the run, before each round and after it, then prints the tables of section
3 and the paired `wasm-opt` comparison; on an idle machine it takes about 4.5 minutes. Its output
(2026-10-15) replaced section 3's tables, the numbers in sections 4 and 5, the in-engine figures
budgets.md shows beside `tick.rules` and `tick.rules.p99` and as `web.tick.rules`'s measurement,
and the numbers in charter 4.2's note. It sets no reference
figure: it runs no calibration workload, so it is not a calibrated run (budgets.md 7.5). A
calibrated run sets `web.tick.rules` by budgets.md 8.1 (the largest run's median × 1.25, at most
8.3 ms), and raising `tick.rules` or `tick.rules.p99` needs the owner (8.3).

## 7. QuickJS-ng's native speed: P12, the C flags, `Math` as C functions (quiet re-measurement)

- Date: 2026-10-15. Branch `explore/qjs`. Charter 4.2 (the patch list, P12). **The timings are a
  quiet re-measurement** (2026-10-15, 16:03 to 16:16, 7.6's protocol): the other tracks had
  finished, no build or other timing ran, and each run held the machine-wide bench lock throughout;
  CPU load was 3.0 to 6.3 % before each round (7.5). The main comparison was run twice. They
  replace the provisional runs of the same morning (08:16 to 10:22), taken while other agents built
  on the machine at 3 to 70 % CPU load; 7.5 says what changed, and two conclusions did (7.3, items
  2 and 4). Steps and digests are exact, and the stack measurements repeat to the byte; neither
  depends on the load.
- What was tried, each against the baseline (b7be59a2 with this section's probes, 82cbde44):
  1. **P12** (2f0d999c): QuickJS-ng's computed-`goto` dispatch under clang-cl
     (script-sandbox.md 4.2). Native only; wasm32 keeps the `switch`.
  2. **"QuickJS-ng at `-O3` natively"**: not needed. cc-rs passes clang-cl `-O2`
     (`CC_ENABLE_DEBUG_OUTPUT=1` shows the command), and clang-cl 23.1.2 maps `-O2`, like `/O2`
     and `/Ox`, to its compiler's `-O3` with the loop and SLP vectorizers (`clang-cl -O2 -### -c`),
     the level clang's `-O3` gives wasm32. Section 4, item 3's "`-O3` for wasm32 against `-O2`
     natively" was wrong. What does differ in the two builds' code generation (clang-cl's `cc1`
     arguments against clang's for wasm32, both from `-###`) is five of clang-cl's MSVC defaults:
     `-relaxed-aliasing` (no type-based alias analysis), `-fmath-errno`, `-stack-protector 2`
     (`/GS`), `-fwrapv` (signed overflow wraps, which rules out optimizations that assume it
     cannot happen) and `-fms-volatile` (volatile accesses with acquire and release semantics).
     The other differences are the platform's: `-funwind-tables=2`, function sections, the
     Microsoft language modes, the target CPU. Two builds turned the five off, neither committed:
     **flags**, P12 with the first three off (`/clang:-fstrict-aliasing`,
     `/clang:-fno-math-errno`, `/GS-`), and, after the review, **flags5**, natives with all five
     off (those three, `/clang:-fno-wrapv` and `/volatile:iso`; with them, `-###` shows only the
     platform's differences left). Both put the flags in the vendored build script's clang-cl
     branch (7.6 has the lines). Only flags5, which includes flags's three, was re-measured
     quietly.
  3. **natives** (40e6f948, on P12): the engine's `Math` functions as QuickJS-ng C functions over
     doubles (`JS_NewCFunction2` with `JS_CFUNC_f_f` and `JS_CFUNC_f_f_f`, as QuickJS-ng registers
     its own `Math.sin`), `extern "C"` wrappers around `amoris_sim::math`, in place of rquickjs
     closures with `Coerced<f64>` parameters; `hypot` (variadic) stays a closure. This applies to
     wasm32 too (sandbox.rs is shared).

### 7.1 Method

- **`qjs_micro`** (new): eleven one-statement loops of 200,000 iterations, each the `run` of a
  system in its own world through the real host (`install`, `swap`, `Sim::step`), so in the
  hardened context with the lockdown's `Math`: five interpreter shapes (`loop`: `s += i * 0.001`;
  `int`: `s = (s + i) | 0`; `call`: `s += half(i)`; `f64`: a `Float64Array` read-modify-write;
  `prop`: `o.x = o.x + o.y`) and six calls: QuickJS-ng's own `Math.sqrt` (a C function, the
  reference), the engine's `sin`, `cos`, `atan2(i * 0.001, 1.5)`, `pow(i * 0.001, 1.5)` and
  `hypot(i * 0.001, 3)`. Per round and probe a fresh world, 3 warm-up ticks, 15 timed ticks; the
  cost per iteration is the tick median over 200,000. A call's cost is its probe less `loop`, per
  round, as the script-native spike measured it (the lookup of `Math.sin` and the call). The fixed
  cost of a tick (about 10 us) is under 0.3 % of the shortest probe.
- **`rules_bench`** as sections 1 and 2 (rules, twin, idle, scan; 120 warm-up and 600 timed ticks).
- **`tools/qjs_bench.py run`**: every build verified first (its script world against the twin at
  all 720 ticks; all builds must end on one digest); then five rounds, each running every build in
  an order that rotates by round, and within a build `rules_bench` (one round) then `qjs_micro`
  (one round). The whole run held the bench lock. Comparisons are paired by round (the median of
  the rounds' ratios). On the quiet machine the rule tick's paired ratios still spread by a few
  percent between rounds (P12 against base: -7.6 to +1.0 % in quiet run 1), and two effects that
  are not the change under test show at that size or larger (7.2): a rebuild alone moves unchanged
  code (the Rust twin) by about 2 %, and a microloop can hold one of two speeds, 15 to 40 % apart,
  for a whole process.
- Runs, all quiet (7.5), in this order: **quiet run 1**, base, p12 and natives (16:03 to 16:06);
  the final state (natives) natively against both web modules under Node and in Chrome, section
  6's `rules_bench.py run --rounds 5` (16:06 to 16:10); **the flags5 run**, natives and flags5
  (16:10 to 16:13); **quiet run 2**, as quiet run 1 (16:13 to 16:16). The provisional runs they
  replace (run A, runs 1 and 2, run F and a native-against-web run, 08:16 to 10:22) are summarized
  in 7.5.

### 7.2 Results

Every build kept the workload exact: 62,518 steps a tick, the twin's digests at all 720 ticks, the
final digest `e13690add0b3a196` (natively, and for the final state in both web modules under Node
and in Chrome). `qjs_micro`'s steps are equal across builds: 400,009 a tick for `loop`, `int` and
`prop`, 600,009 for `call` and every call probe (a C function's call is polled on entry like any
call), 600,010 for `f64`.

The rule tick in us (quiet runs 1 / 2; "against" is the median of the rounds' paired ratios):

| Build | Tick median | `run` median | Tick against base | `run` against base | Twin against base |
|---|---|---|---|---|---|
| base | 3,651 / 3,613 | 2,856 / 2,808 | | | |
| p12 | 3,563 / 3,514 | 2,715 / 2,722 | -3.6 / -2.0 % | -4.9 / -2.9 % | -0.1 / +0.3 % |
| natives | 3,356 / 3,369 | 2,567 / 2,566 | -8.9 / -6.1 % | -10.6 / -8.0 % | -1.6 / -2.5 % |

P12 against base by round: the tick -6.2 / +0.5 / -3.6 / +1.0 / -7.6 % in run 1 and -1.5 / -2.9 /
-5.5 / -2.0 / -1.8 % in run 2; `run` faster in all ten rounds, by 1.6 to 6.9 %; the twin +1.9 /
+1.8 / -1.1 / -17.2 / -0.1 % and -2.6 / +2.2 / +0.3 / -0.3 / +0.4 % (in round 4 of run 1 base's
twin took 162 us, against 133 to 135 in its other rounds). Natives against p12, paired: the tick
-5.5 / -4.1 %, `run` -5.9 / -5.3 % (in nine rounds of ten by 2.8 to 7.0 %, once by 0.6 %), the twin
-2.4 / -2.2 %. The phases outside `run` (`prepare`, `diff`, `apply`) differ from base's by at most
6 us (4 %) in every build by the median. The twin is Rust that none of these changes touches, yet
natives' twin was faster than base's and p12's in all twenty paired rounds, by 0.2 to 4.3 % (19 %
once, base's slow round): a rebuild alone moved unchanged code by about 2 %, presumably through
code layout, which this bench does not isolate.

The flags5 run, flags5 against natives, paired: the tick +1.4 % (+1.7 / +1.4 / +2.8 / -0.8 /
+1.3 %), `run` +1.9 % (slower in every round, by 0.9 to 2.8 %), the twin -0.6 %; natives' own rule
tick was 3,369 us (round medians 3,339 to 3,380), as in quiet runs 1 and 2. The interpreter shapes:
`loop` -2.8 %, `int` -9.0 %, `call` +0.5 %, `f64` +10.8 %, `prop` +46.3 % (37.7 to 39.6 ns in four
rounds, 28.8 in the first, against natives' 26.1 to 26.7); the `Math` calls within 4 ns of
natives' (`sin` 33.6 against 36.0 ns, `atan2` 37.6 against 33.9).

The interpreter shapes, base's cost per iteration (quiet runs 1 / 2) and the builds against it,
paired:

| Probe | base, ns | P12 against base | natives against base |
|---|---|---|---|
| `loop` | 21.5 / 21.2 | -3.5 / +2.1 % | +3.8 / +7.5 % |
| `int` | 22.4 / 22.3 | -6.2 / -3.2 % | +3.1 / +1.0 % |
| `call` | 45.4 / 45.3 | -4.4 / -3.9 % | -1.2 / -1.5 % |
| `f64` | 74.2 / 74.0 | -2.6 / -0.6 % | -15.2 / -14.5 % |
| `prop` | 26.9 / 26.9 | -3.2 / -2.7 % | -1.8 / -2.1 % |

natives runs p12's interpreter, so what separates its column from P12's (up to 12 points, `f64`)
is the rebuild, not the interpreter. A probe can also hold one of two speeds for a whole process:
natives' `f64` took 72.8 to 73.3 ns in three rounds of fifteen across the quiet runs and 62.1 to
63.6 in the others, p12's `prop` 34.8 ns in one round against 26.0 to 26.4, flags5's `prop` as
above.

A `Math` call, ns over `loop` (quiet runs 1 / 2):

| Call | base | p12 | natives |
|---|---|---|---|
| `Math.sqrt` (QuickJS-ng's own C function) | 22.4 / 21.8 | 22.9 / 23.2 | 22.1 / 21.4 |
| `Math.sin` | 87.5 / 89.2 | 88.1 / 87.7 | 33.1 / 33.8 |
| `Math.cos` | 88.1 / 87.7 | 88.3 / 88.4 | 33.9 / 34.5 |
| `Math.atan2` | 110 / 112 | 109 / 108 | 33.0 / 33.5 |
| `Math.pow` | 149 / 148 | 144 / 144 | 74.8 / 72.6 |
| `Math.hypot` (still a closure) | 151 / 153 | 149 / 148 | 150 / 149 |

In the flags5 run natives' calls were `sqrt` 22.9, `sin` 36.0, `cos` 35.2, `atan2` 33.9, `pow`
73.7 and `hypot` 148 ns.

The final state natively and in WebAssembly (section 6's `rules_bench.py`, one run of five rounds,
16:06 to 16:10), tick medians and phase medians in us:

| Target | rules tick (rounds 1 to 5) | Median | rules / twin | prepare | run | diff | apply |
|---|---|---|---|---|---|---|---|
| native | 3,382 / 3,334 / 3,377 / 3,364 / 3,351 | 3,364 | 25.3 | 265 | 2,554 | 133 | 337 |
| Node, web | 3,033 / 3,017 / 3,003 / 2,995 / 3,068 | 3,017 | 19.6 | 161 | 2,422 | 110 | 269 |
| Node, web-opt | 2,987 / 3,002 / 3,010 / 2,954 / 2,987 | 2,987 | 19.0 | 163 | 2,399 | 104 | 265 |
| Chrome, web | 3,630 / 2,935 / 2,935 / 2,950 / 3,670 | 2,950 | 18.4 | 165 | 2,350 | 115 | 275 |
| Chrome, web-opt | 2,965 / 2,920 / 2,885 / 2,940 / 2,930 | 2,930 | 18.9 | 160 | 2,340 | 115 | 270 |

The tick's p99 (median of the rounds): native 3,782 us, Node 3,468 and 3,548, Chrome 3,575 and
3,275. `web-opt` against `web`, the `rules` tick paired by round: Node -1.5 / -0.5 / +0.2 / -1.4 /
-2.7 %, Chrome -18.3 / -0.5 / -1.7 / -0.3 / -20.2 %; Chrome's rounds 1 and 5 are again those in
which its `web` module ran first right after Node's runs (section 4, item 4).

### 7.3 What the figures say

1. **`Math` as C functions is the largest of the three gains.** An engine `sin` or `cos` call
   falls from 88 to 89 ns to 33 to 36, `atan2` from 108 to 112 to 33 to 34, `pow` from 144 to 149
   to 73 to 75 (quiet runs 1 and 2 and the flags5 run). The 54 ns or so removed is the closure's
   trampoline as a whole (rquickjs's class call, its parameter check, the `Coerced` conversions,
   `catch_unwind`); this bench does not break it down. What is left is QuickJS-ng's own call of a
   C function (its `Math.sqrt` costs 21 to 23 ns over the loop) plus the library's time, which is
   estimated here only by subtraction (11 to 13 ns for `sin`, run by run; numeric.md 6.4 timed
   `libm`'s `sin` alone at 14 to 17 ns, its loop included). The parts do not simply add under
   out-of-order execution: `atan2` costs 33 to 34 ns over the loop, less than `sqrt`'s 21 to 23
   plus numeric.md's 21 to 28 for `libm`'s `atan2`. The rule's `run` makes about 3,000 `sin` and
   `cos` calls, about 0.16 ms of trampolines at 54 ns: it fell by 5.9 and 5.3 % against p12 (148
   and 156 us by the medians), as that predicts. `hypot`, still a closure, is unchanged.
2. **P12 gains a few percent on `run` and a little on the tick.** The rule's `run` is 4.9 and
   2.9 % faster (paired medians of the two quiet runs), faster in all ten rounds; the tick 3.6 and
   2.0 % faster, in eight rounds of ten, while the twin did not move between base and p12 (-0.1
   and +0.3 %). This changes the provisional reading, which left the tick gain unresolved within
   the twin's drift (-1.9 to +1.3 % under load): on the quiet machine it is resolved. It is small,
   though, and of the size of what a rebuild alone does: natives' twin, unchanged Rust, ran about
   2 % faster than in the other two builds (7.2), so a 2 to 4 % tick difference between two
   binaries is not all the source change's. The interpreter shapes cannot attribute it either:
   P12 moved them by -6.2 to +2.1 %, natives, with the same interpreter, by -15.2 to +7.5 %
   against base (provisionally they had shown P12 1.5 to 13 % faster, the load's spread). That is
   less than computed `goto` is often credited with; the processor's prediction of the `switch`'s
   single indirect jump may explain it, which this bench does not isolate. Its cost is stack: the
   optimized interpreter frame grows (7.4), inside the 1 MiB `stack_bytes` (host.rs).
3. **`-O3` was already there, and turning off the five MSVC defaults gains nothing.** flags5 (all
   five, on natives) against natives, quietly: the tick +1.4 % (four rounds of five slower), `run`
   +1.9 % (slower in every round, by 0.9 to 2.8 %), the twin -0.6 %; if anything slower, as
   run F's two quiet rounds had suggested provisionally. The flags build (the first three, on
   P12) was measured only provisionally and was not distinguishable from p12 (paired: tick -1.5
   and +0.2 %, `run` -1.3 and -0.2 %; the call probes within 3 ns, `hypot` once 8 ns). Neither is
   adopted: no gain shown, and `/GS-` would remove stack protection from the C code that runs
   project scripts. No build flag changed.
4. **Natively `run` is still 5 to 9 % slower than in WebAssembly: the provisional parity was the
   load.** This changes the provisional reading. With all three changes (the web modules got the
   `Math` change too, not P12), the quiet run gives `run` 2,554 us natively against 2,340 to 2,422
   in the four web runs, and the tick 3,364 us against 2,930 to 3,017, 11 to 15 % slower natively.
   The loaded run had shown `run` level (2,654 us against 2,580 to 2,750) and the tick 2 to 8 %
   slower: as the review suspected, its load slowed the WebAssembly hosts more than the native one.
   Against section 3's quiet runs of the same morning, before these changes (not paired, other
   binaries), `run` fell natively from 2,807 to 2,554 us (-9 %) and in WebAssembly from 2,535 to
   2,604 to 2,340 to 2,422 (-5 to -9 % by module and host, the `Math` change's share there, not
   otherwise measured): native `run`'s excess narrowed from 8 to 11 % to 5 to 9 %, and the tick's
   (11 to 14 % then) stayed. Phase by phase, natively `prepare` is +100 to +105 us, `diff` +18 to
   +29, `apply` +62 to +72 and `run` +132 to +214. `diff` and `apply` are the host's Rust (the
   comparison, the writes). `prepare` is not Rust alone: it runs the prelude's JavaScript
   (`__context`, `__query`) and builds the typed arrays inside QuickJS-ng, so its gap may also come
   from interpretation and from QuickJS-ng's allocator, not separated here (section 4, items 3 and
   6). Section 4, item 3's other differences (the Rust profiles, fat LTO and one codegen unit
   against none and 16; the allocator; the panic strategy) were not tried here. The rule cost 25.3
   times its twin natively in that run (27 in section 3) and 18.4 to 19.6 times in WebAssembly (20
   to 21).
5. **Against the budgets.** The final state's rule tick is 3.36 to 3.37 ms natively at the median
   (natives in quiet runs 1 and 2 and the flags5 run, 3,364 us in the web run; round medians
   3,326 to 3,473), 6.1 and 8.9 % below base's paired, with its p99 (median of the rounds) 3.65 to
   4.02 ms; section 3's quiet figures were 3.6 ms and 3.9 to 4.2. In WebAssembly it is 2.93 to
   3.02 ms (Chrome's worker 2.93 and 2.95), its p99 3.3 to 3.6 ms. All are inside `tick.rules`
   (4.2 ms), `tick.rules.p99` (6.5 ms) and `web.tick.rules` (8.3 ms). budgets.md keeps section 3's
   figures, the baseline before these changes: the owner kept them on 2026-10-16 (charter 4.2).

### 7.4 What changed in the engine

- third_party: P12 (`patches/p12-direct-dispatch.diff`, pinned in `vendor.py`; `vendor.py --check`
  reproduces the tree). The vendored build script is unchanged. The debugger's instrumented
  dispatch runs under P12 too; on a debug build of the final state, amoris-app's `debug_agent` (4
  of 4) and `paused_host` (5 of 5), `cdp_e2e.mjs` (20 of 20) and amoris-debug's unit tests pass
  (`out/qjs/test-*.log`), and `cargo xtask check --only determinism,reload` passes (a release
  `amoris` against a debug one; `out/qjs/xtask-determinism-reload.log`).
- amoris-script: `sandbox.rs` registers the engine's `Math` functions through `c_math!`
  (`JS_NewCFunction2`). A script can see two changes, both toward QuickJS-ng's own builtins: a
  missing argument is NaN, as ECMAScript says (`Math.sin()` threw rquickjs's missing-argument
  error), and an error thrown inside one of them (a `BigInt` argument, a throwing `valueOf`) has an
  `at sin (native)` frame in `e.stack`, which QuickJS-ng pushes for every C function and rquickjs's
  closures did not. The host's error locations skip native frames (`caught.rs`), and so does the
  debugger's capture. Names, lengths and the prototype are as before; tests/lockdown.rs pins them
  and the frame. The results are `amoris_sim::math`'s bits: the web report's sweep keeps its pinned
  hash natively and in wasm32. A panic in one of them would abort the process at the `extern "C"`
  edge instead of becoming `sim.internal`: script-sandbox.md 4.3 records that exception and what it
  rests on.
- Stack (tests/depth.rs at `max_call_depth`, release, the deepest frame shape; the bisection's
  resolution is about 4 KiB, so two shapes can land on the same figure). Before P12, 368,468 bytes
  with `Math.sin` as a closure (docs/bench/xtask.md, 2026-10-09; again at 82cbde44,
  `out/qjs/depth-release-base.log`). With P12 (2f0d999c, still through `Math.sin` as a closure),
  454,442 (`out/qjs/depth-release-p12.log`). With both changes, `Math.sin` as a C function takes
  368,468 and `Math.hypot`, the remaining closure, 470,818 (`out/qjs/depth-release-head-*.log`): a
  level through the C function takes less stack than through the closure, so tests/depth.rs and the
  web report's depth probe now recurse through `Math.hypot` to keep measuring the deepest shape. In
  debug, 3,471,720 bytes with P12 through `Math.sin` as a closure (`out/qjs/test-p12-debug.log`; the
  figure before P12 was not kept) and 3,508,566 through `Math.hypot` (`stack_bytes` 1 MiB and 6 MiB,
  unchanged). With MSVC's `cl` (the fallback, no P12), 1,187,268 through `Math.hypot` (1,170,892
  through `Math.sin` on 2026-10-09), inside its 2 MiB with the margin (1.5 times is 1,780,902;
  `out/qjs/depth-release-head-hypot-cl.log`).

### 7.5 Conditions

- Machine as section 1; QuickJS-ng compiled by clang-cl 23.1.2 in every build (each result file
  records `AMORIS_QJS_CC`); release profile as section 2. The quiet binaries were built from 15:57
  to 16:01 (7.6): base, p12 and natives with `build --rev`, three distinct binaries, so the export
  fix held; flags5 from the checkout at ef2c92e9 with 7.6's lines, reverted after; the web run's
  `rules_bench` and both web modules by `rules_bench.py build` at ef2c92e9. Timing began two
  minutes after the last build, at 3.9 % CPU load.
- Quiet re-measurement, 2026-10-15 16:03 to 16:16: the other tracks had finished and no build,
  browser or other timing ran; the bench lock's log shows only these four runs holding it
  between 15:54 and 16:16. Machine conditions (`tools/machine_conditions.py`) before the run, each
  round and after it, CPU load over a few seconds: quiet run 1 3.4, 4.3, 3.8, 4.0, 4.1, 3.7, 4.1 %
  (start, rounds 1 to 5, end); the web run 3.8, 5.9, 4.5, 3.7, 4.6, 4.2, 4.1 %; the flags5 run
  6.2, 3.4, 6.3, 3.0, 3.8, 3.9, 3.8 %; quiet run 2 5.6, 3.3, 3.6, 4.3, 5.6, 6.3, 6.1 %. On AC
  power, Balanced plan, the RTX 5060 idle at P2, 52 to 58 C.
- Against the provisional runs (the same morning: run A, base and P12, 08:16 to 08:18; runs 1 and
  2, base, p12, flags and natives, 08:47 to 08:52 and 09:20 to 09:25; the native-against-web run,
  09:29 to 09:34; run F, natives and flags5, 10:19 to 10:22; other agents building throughout, CPU
  load 3 to 70 % before the rounds). Base's rule tick was 3.74 to 4.00 ms there and 3.61 to 3.65
  here, natives' 3.46 to 3.68 and 3.36 to 3.37. What changed:
  - P12's tick: provisionally -2.2 to -4.8 % and unresolved (the twin drifted -1.9 to +1.3 %);
    quietly -3.6 and -2.0 % with the twin level, resolved but small (7.3, item 2). Its `run`:
    -3.6 to -4.4 %, quietly -4.9 and -2.9 %.
  - Natives against p12: provisionally -5.2 and -1.9 % on the tick and -6.7 and -4.0 % on `run`;
    quietly -5.5 and -4.1 % and -5.9 and -5.3 %.
  - The interpreter shapes under P12: provisionally 1.5 to 13 % faster; quietly -6.2 to +2.1 %,
    and not attributable to P12 (7.3, item 2).
  - The `Math` calls: as C functions provisionally `sin` 35.9 and 37.2 ns, `atan2` 35, `pow` 74 to
    77; quietly 33 to 36, 33 to 34 and 73 to 75. As closures, `sin` 88 to 91 and 88 to 89 ns.
  - flags5: run F gave the tick +0.8 % and `run` +1.6 % (+3.4 and +0.8 %, +5.5 and +1.6 % in its
    two quiet rounds); quietly +1.4 and +1.9 %. The conclusion stands.
  - Native against WebAssembly: provisionally `run` level (2,654 us against 2,580 to 2,750) and
    the tick 2 to 8 % slower natively; quietly `run` 5 to 9 % and the tick 11 to 15 % slower
    natively. This changes the conclusion (7.3, item 4).
- Raw files (ignored): the quiet runs in `out/qjs/quiet`, `out/qjs/quiet-repeat` (quiet run 2),
  `out/qjs/quiet-flags5` and `out/qjs/quiet-web`, their logs `out/qjs/quiet*.log` and the builds'
  `out/qjs/quiet-build-*.log`; the quiet binaries in `out/qjs/bin/<build>/` with each build's
  revision in `build.json` (flags5's build script change in `out/qjs/quiet-flags5-build.rs.diff`);
  the provisional runs in `out/qjs/p12-wave`, `out/qjs/wave-1`, `out/qjs/wave-2`,
  `out/qjs/native-vs-web` and `out/qjs/flags5`, their binaries in `out/qjs/bin-provisional/`; the
  stack measurements in `out/qjs/depth-*.log`.

### 7.6 Re-measuring on a quiet machine

From a checkout that has this section, with nothing else building. `build --rev` exports the
revision with `git archive` into `out/qjs/src/<name>`, dates every exported file now and builds it
with the target directory the revisions share, `out/qjs/target-rev`. (Before the review it kept the
archive's dates, the commit's, which are older than the previous build into that directory, so cargo
took a second revision's sources as unchanged and kept the first one's binaries; the provisional
runs used builds of the checkout, and one export, which were not affected.) The revisions are
`explore/qjs`'s commits: 82cbde44 (b7be59a2 with the probes), 2f0d999c (P12) and 40e6f948 (the
`Math` change). A merge that keeps the branch's history keeps them; after a squash, use the commits
that carry the same three states.

```sh
python tools/qjs_bench.py build base --rev 82cbde44
python tools/qjs_bench.py build p12 --rev 2f0d999c
python tools/qjs_bench.py build natives --rev 40e6f948
python <bench_lock.py> run --label quiet-qjs -- python tools/qjs_bench.py run out/qjs/quiet \
    --builds base,p12,natives --rounds 5
python tools/rules_bench.py build
python <bench_lock.py> run --label quiet-qjs-web -- python tools/rules_bench.py run \
    out/qjs/quiet-web --rounds 5
```

flags5 is built from the checkout at the final state with the five flags added to the vendored
build script's clang-cl branch, then the edit reverted (`git checkout` the file), and run as
`--builds natives,flags5`:

```rust
    if clang_cl {
        builder.flag("/clang:-ffp-contract=off");
        builder
            .flag("/clang:-fstrict-aliasing")
            .flag("/clang:-fno-math-errno")
            .flag("/GS-")
            .flag("/clang:-fno-wrapv")
            .flag("/volatile:iso");
```

```sh
python tools/qjs_bench.py build flags5
python <bench_lock.py> run --label quiet-qjs -- python tools/qjs_bench.py run out/qjs/quiet-flags5 \
    --builds natives,flags5 --rounds 5
```

On the idle machine each `qjs_bench.py run` took 2.5 minutes (the flags5 run 2) and printed 7.2's
tables (`qjs_bench.py table out/qjs/quiet` prints them again; `--builds p12,natives` pairs against
p12); `rules_bench.py run` at the final state printed the native-against-web table in 4 minutes.
The 2026-10-15 re-measurement ran these in the order above (quiet run 1, the web run, the flags5
run), then the first `qjs_bench.py run` once more into `out/qjs/quiet-repeat` (quiet run 2), from
the `explore/qjs` worktree with `--out` (and the run directories) in the main checkout's `out/qjs`.
Its output replaced 7.2's tables and the numbers of 7.3; section 3's tables and budgets.md's figures
change only if the owner takes the web run as the new baseline.

## 8. The split of `run`: interpretation, the natives and the step profiler (quiet re-measurement)

- Date: 2026-10-15 (the provisional run); timings re-measured on a quiet machine 2026-10-16. Branch
  `explore/scriptprof`. Charter 8, item 1 (a JIT backend), 4.2 and 4.3 (the step profiler and P13,
  dated 2026-10-16). Section 4, item 6 asked for this split: `run`, four fifths of the call, held
  the rule's native calls as well as interpretation.
- **The timings are a quiet re-measurement** (2026-10-16 03:08 to 03:10, 8.5's protocol): the other
  tracks had finished, no build, browser or other timing ran, and the run held the machine-wide
  bench lock throughout (taken without a wait). Before the run, before each round and after it
  (`tools/machine_conditions.py`): on AC power, Balanced plan; CPU load 3.3 to 7.9 % (round 1's 7.9,
  the rest 3.3 to 4.1); the RTX 5060 idle (the workload is CPU only), 55 to 62 C. They replace the
  provisional run of 2026-10-15 19:19 to 19:21, taken while other agents built and timed (CPU load
  3.9 to 9.6 % before the rounds, except round 2's 34.9 %; raw files `out/scriptprof/split-wave/`).
  Against it no conclusion changed: at the medians every share moved by at most 0.1 point and the
  tick by -0.5 % (3,359 against 3,375 us), `run` not at all (2,574 us); the loaded round 2 (a 3,662
  us tick, p99 5.0 ms) does not recur; the profiler's cost a sample came out narrower
  (0.18 to 0.25 us against 0.16 to 0.63, 8.3 item 5), and timing the natives cost less in place (4.3
  ns a reading against 5.3). The quiet run is not a calibrated run (budgets.md 7.5). Steps, call
  counts and digests are exact; the profile is sampled (every 100 steps on average), so its shares
  are estimates, and deterministic: the same in every round of both runs.

### 8.1 Method

- **natives** (new, `rules_bench --variants rules,natives,profiled`): `rules` with the host's
  natives timed (`ScriptHost::time_natives`, before the swap). A program instantiated while a host
  times natives gets timed copies of them: the engine's `Math` C functions, `Math.hypot`, every
  `amoris:host` native and the console. Each outermost native call reads the clock before and after
  its body and adds the difference to a per-thread sum by kind (`math`; `events` for `emit`,
  `events` and `intent`; `rng`; `world`; `other`); `run_system` adds what `run` spent inside them to
  `PhaseTimes.natives`. Untimed programs run the untimed natives. The clock is the processor's
  time-stamp counter (`RDTSC`), scaled to milliseconds against the wall clock over 200 ms, since a
  native call takes tens of nanoseconds and the wall clock (`QueryPerformanceCounter`) has a 100 ns
  tick on Windows. A timed window holds part of a clock reading besides the body. Each round
  measures a reading back to back (a million, five times), and the tables give the natives raw and
  less one such reading a call, as bounds: the second over-subtracts when a reading costs less in
  place, as `run`'s growth with timed natives says it does (8.2). With timed natives the world is
  the same: every round's final digest is `e13690add0b3a196` in all three variants, and
  `tests/rules.rs` checks the twin's digest at every tick and that a clock advancing one unit per
  reading counts each native call of `run` once (2,992 `Math` calls, five events and one `rng.fill`
  a tick in this workload).
- **What counts as what.** "Natives" is the time inside the bodies of the host's natives.
  "Interpretation" is the rest of `run` (the untimed `rules` variant's, same round; a range, from
  the natives' two bounds): the bytecode interpreter, QuickJS-ng's own builtins
  (`Array.prototype.push`, about 4,000 calls a tick; `Math.sqrt` and `Math.min`, about 1,000 each;
  typed array reads and writes), and the calls into the natives on the script's side (QuickJS-ng's
  call of a C function, rquickjs's trampoline for the five events and the fill), which a JIT would
  replace by its own calls into the same natives.
- **profiled**: `rules` under the step profiler (docs/spec/debugger.md 14) from the round's first
  tick, a sample every 100 steps on average; its cost is the tick against `rules` in the same
  round.
- Per round (five, the order rotating): `rules`, `natives` and `profiled`, each a fresh world, 120
  warm-up ticks and 600 timed ones, as section 2; the medians are of the 600 ticks. Machine
  conditions (`tools/machine_conditions.py`) before the run, each round and after it. A share quoted
  for the run is the median of the rounds' own shares; a time, the median of the rounds' times,
  interpretation being the median `run` less the median natives, so the parts add up.

### 8.2 Results

`python tools/rules_bench.py split out/scriptprof/quiet/split --rounds 5` inside
`bench_lock.py run --label quiet-scriptprof`, 2026-10-16 03:08 to 03:10; the release `rules_bench`
rebuilt from this branch at 4a82640c. Per round, us (the medians of 600 ticks):

| Round | First | `rules` tick | `rules` `run` | `natives` `run` | Natives, raw | Native calls | Reading, ns | Natives, less readings | Interpretation | Of `run` | Of the tick |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | rules | 3,332 | 2,549 | 2,600 | 843 | 2,998 | 7.2 | 822 | 1,706 to 1,728 | 66.9 to 67.8 % | 51.2 to 51.9 % |
| 2 | natives | 3,416 | 2,588 | 2,579 | 846 | 2,998 | 7.2 | 824 | 1,742 to 1,763 | 67.3 to 68.1 % | 51.0 to 51.6 % |
| 3 | profiled | 3,370 | 2,574 | 2,593 | 854 | 2,998 | 7.2 | 832 | 1,720 to 1,742 | 66.8 to 67.7 % | 51.1 to 51.7 % |
| 4 | rules | 3,359 | 2,553 | 2,610 | 854 | 2,998 | 7.3 | 832 | 1,699 to 1,721 | 66.5 to 67.4 % | 50.6 to 51.2 % |
| 5 | natives | 3,354 | 2,576 | 2,625 | 869 | 2,998 | 7.2 | 847 | 1,708 to 1,729 | 66.3 to 67.1 % | 50.9 to 51.6 % |

Medians of the rounds: `run` 2,574 us of a 3,359 us tick; the natives 832 to 854 us; interpretation,
the rest of `run`, 1,720 to 1,742 us. The medians of the rounds' shares: interpretation 66.8 to
67.7 % of `run` and 51.0 to 51.6 % of the tick.

The natives by kind, us a tick less one reading a call to raw (calls a tick); `world` and `other`
had none:

| Round | `math` | `events` | `rng` |
|---|---|---|---|
| 1 | 25.3 to 46.8 (2,992) | 793 (5) | 3.2 (1) |
| 2 | 23.0 to 44.5 (2,992) | 798 (5) | 3.5 (1) |
| 3 | 25.1 to 46.6 (2,992) | 803 (5) | 3.3 (1) |
| 4 | 24.9 to 46.7 (2,992) | 804 (5) | 3.5 (1) |
| 5 | 24.5 to 46.0 (2,992) | 819 (5) | 3.3 (1) |

The `rules` phases (medians of the rounds, us): `prepare` 263, `run` 2,574, `diff` 140, `apply` 333;
the tick's p99 3.7 to 4.0 ms by round. Timing the natives made `run` 26 us slower at the median of
the rounds (1.0 %; by round +51, -9, +18, +56 and +49 us, a spread as wide as the effect): at that
median, two readings a call and their bookkeeping cost at most 4.3 ns a reading in place, against
7.2 ns back to back, so the lower bound of the natives over-subtracts (by at least 8 us of `math`'s
2,992 calls).

The step profiler at 625 samples a tick (interval 100):

| Round | `rules` tick | `profiled` tick | Cost a sample, us |
|---|---|---|---|
| 1 | 3,332 | 3,482 | 0.24 |
| 2 | 3,416 | 3,530 | 0.18 |
| 3 | 3,370 | 3,523 | 0.25 |
| 4 | 3,359 | 3,499 | 0.22 |
| 5 | 3,354 | 3,508 | 0.25 |

Its profile was the same in every round, and the same as the provisional run's, a deterministic
estimate from 450,061 samples of `damage` over the 720 ticks (one every 100 steps on average), by
function self (and total) samples: the prelude's `row` (`rows.row`, its binary search) 53.5 %
(53.5), `run` 41.7 % (100), `driftY`, `driftX` and `wobble` 1.6 % each; by line, `row`'s four loop
lines (amoris.ts 319, 322, 323, 324) 53.5 % together, then `rules.ts:60` (the `amount` line: four
calls and a conditional) 8.1 % and `rules.ts:78` (the regeneration line: a conditional and
`Math.min`) 4.8 %.

### 8.3 What the figures say

1. **Interpretation is about two thirds of `run` and half of the tick.** Of the untimed rule's
   `run` (2.57 ms at the median of the rounds, in a 3.36 ms tick), 0.83 to 0.85 ms is inside the
   natives and 1.72 to 1.74 ms is the rest: by the medians of the rounds' shares, 67 to 68 % of
   `run` (66.3 to 68.1 % by round) and 51 to 52 % of the tick (50.6 to 51.9 %). Section 4, item
   6's estimate (interpretation, `emit` and `fill` together about 70 % of the tick, made on an older
   build with `Math` as closures) is low: they take 75 to 76 % here; apart, interpretation is half
   of the tick.
2. **The natives' time is the events, not `Math`.** The five `ctx.emit` calls take 0.79 to 0.82 ms
   a tick: four `combat.damaged` events, which carry the tick's hits (up to 250 each, in four
   number arrays), about 0.2 ms each, and one small `combat.died` (two arrays of about 15 ids). The
   time is the conversion of each event's data: a property lookup through QuickJS-ng's API, by the
   index as a string, for every element, then a JSON serialization to check its size. The 2,992
   `Math.sin` and `Math.cos` calls take 25 to 47 us inside their bodies (8 to 16 ns a call, the
   clock reading's correction being as large as the result; their whole cost over an empty loop is
   33 to 36 ns, section 7.2, the rest being QuickJS-ng's call), and `rng.fill`'s 1,000 numbers 3.3
   us. So the events' conversion is nearly a quarter of the tick, more than the boundary around the
   call (`prepare`, `diff`, `apply`: 0.74 ms), and in this engine it can be cut without a JIT, by
   reading the elements without a lookup by name each and sizing the data without serializing it.
3. **For charter 8, item 1.** In this engine, what a faster interpreter could win back is at most
   the interpretation: 1.72 to 1.74 ms of 3.36, half of the tick, so even an interpreter that cost
   nothing would leave 1.62 to 1.64 ms (about 2.1 times faster). That bound is QuickJS-ng's own.
   The rest, the host's natives (0.83 to 0.85 ms, nearly all the events' conversion) and the
   boundary (0.74 ms), is not engine-independent: `prepare` runs the prelude's JavaScript and builds
   QuickJS-ng's typed arrays (section 4, item 6), and the conversion reads the scripts' arrays
   through QuickJS-ng's object API. Under V8 or the browser's engine, the candidates of charter 8,
   item 1, both would be other code at a cost this split does not measure, so 2.1 times is not
   their bound. The `Math` natives are negligible either way. The browser's own JavaScript engine
   still needs the same rule on V8 itself (section 4, item 6).
4. **Steps are not time.** The profile says where the budget's steps go: half of them are
   `rows.row`'s binary search (about 33 steps a lookup on 1,000 rows, from the sampled profile),
   since a step is a call or a jump and arithmetic costs none. Time is spread differently (the
   events take a quarter of the tick in five steps). A system near its step budget is cut down by
   its profile; a slow tick is found with the phases and this split.
5. **The profiler's cost.** At 625 samples a tick it added 0.11 to 0.15 ms to a 3.33 to 3.42 ms tick
   (3.3 to 4.6 %), 0.18 to 0.25 us a sample; a stack here is one to three frames deep.
   `scripts.profile` aims at 20,000 samples per run, so its sampling costs about 4 to 5 ms a run
   here. The provisional run's wider 0.16 to 0.63 us a sample (up to 11 % of the tick, about 13 ms a
   run) was the shared machine's load. Its steps, digests and profile do not depend on the load.

### 8.4 What changed in the engine

- third_party: P13 (`patches/p13-branch-poll-pc.diff`; script-sandbox.md 4.2): a jump's interrupt
  poll stores the frame's PC before the handler runs. amoris-script's tests, the web report natively
  and in WebAssembly (pinned digests and steps) included, pass unchanged; with master's `quickjs.c`
  (no P13) all of `tests/profile.rs` passes but `jumps_land_on_their_own_lines`. Stack
  (tests/depth.rs at `max_call_depth`, release, through `Math.hypot`; 2026-10-15): 466,724 bytes
  with P13 and the same without it on this branch's build (`out/scriptprof/depth-release.txt` and
  `depth-release-no-p13.txt`); 7.4's 470,818 is another build's, one step of the bisection's 4 KiB
  resolution away. The 1 MiB `stack_bytes` holds it with the 1.5 times margin (700,086). The
  debugger runs on it: amoris-debug's tests (3 of 3) and amoris-app's `debug_agent` (4 of 4) and
  `paused_host` (6 of 6) pass on a debug build of this branch.
- amoris-script: the step profiler (`crate::profile`, debugger.md 14; it arms the counter with at
  most 9,999 steps, so libregexp's direct call to the interrupt handler stops a call as without it,
  script-sandbox.md 4.1) and the timed natives (`natives::timing`, `ScriptHost::time_natives`,
  `PhaseTimes::native`, `native_calls` and `natives` by `NativeKind`). Without a recording profile
  or timed natives a call does what it did, plus two thread-local reads per system call.
- `rules_bench`: the `natives` and `profiled` variants (named in `--variants` only, so section 6's
  protocol is unchanged); `tools/rules_bench.py split` and `split-table`.

### 8.5 Re-measuring on a quiet machine

From the worktree or checkout, with nothing else building (the output directory is the main
checkout's ignored `out/`; about 2 minutes once the lock is free):

```sh
cargo build --release -p amoris-script --features transpile --example rules_bench
python <bench_lock.py> run --label quiet-scriptprof -- python tools/rules_bench.py split \
    <main checkout>/out/scriptprof/quiet/split --rounds 5
python tools/rules_bench.py split-table <main checkout>/out/scriptprof/quiet/split
```

`split` verifies the script world against the twin in its first round, records the machine
conditions before the run, each round and after it, and prints 8.2's tables (`split-table` prints
them again). The 2026-10-16 re-measurement ran it from the `explore/scriptprof` worktree at
4a82640c, the binary rebuilt first, into the main checkout's `out/scriptprof/quiet/split/` (its
console output `out/scriptprof/quiet/split-run.txt`); round 1's check against the twin passed
(`identical`, final digest `e13690add0b3a196`). Its output replaced 8.2's tables, the numbers of
8.3, section 4 item 6's and those of charter 4.2's note of 2026-10-16. The provisional run's raw
files are `out/scriptprof/split-wave/` and `out/scriptprof/split-wave.log`.
