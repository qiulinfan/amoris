# The write trace: what it costs a tick (Pioneer)

- Date: 2026-10-15, branch `explore/writes`. Design: [spec/debugger.md](../spec/debugger.md) 13
  (`debug.writes`, `time.step {trace}`); the fifth finding of [agent-debug.md](agent-debug.md);
  charter 4.3's dated note.
- Machine: the R1 laptop (AMD Ryzen 9 270, 16 threads, 15 GiB, Windows 11 Pro 10.0.26200), on AC
  power, Windows' Balanced power plan; release build (`opt-level = 3`, debug info), Rust 1.98.1.
- Conditions: **quiet re-measurement**, 2026-10-15 15:41 to 15:46, the branch at `10b9cf27`. The
  other agents' work had finished: no cargo, rustc or Chrome process ran, and one hold of the shared
  bench lock (`out/locks/bench_lock.py`, label `quiet-writes`) covered all ten processes. Each
  process's conditions (`tools/machine_conditions.py`) were sampled right before and after it: CPU
  load 3.3 % to 7.6 %; the RTX 5060 at 51 to 60 C with no GPU user but the desktop app (the bench
  runs on the CPU). These numbers replace provisional ones (three processes a set, CPU load 3.6 %
  to 24.8 % while other agents built and measured). On the sailing sample each mode's median ratio
  to `off` moved by under 1 point, its spread over five runs a little wider. On the rule workload
  `off` ran 3 to 9 % faster and the probes and traces 3 to 4 % faster, so their ratios rose from
  +193 to +210 % to +202 to +210 %. No conclusion changed.
- Raw JSON and conditions: `out/writes/quiet/` (ignored): `<set>-<run>.json` with their
  `-conditions-{start,end}.json`, and `driver.log`. The provisional runs are in
  `out/writes/bench4/`; `out/writes/bench3/` holds the same protocol before the last change to the
  probe (a component scripts cannot name failed with the refusal's suggestions once a call) and
  `bench3/off-vs-base/`.

## Method

`python tools/write_trace_bench.py run OUT --runs 5 --lock <bench_lock.py>` runs the example
`write_trace_bench` (pocket-runtime, release) five times on each of two projects, one process
each, stepping the game directly. Each process warms up, counts the script statements a tick runs
(a debugger hook that only counts, untimed), then times modes in blocks, each tick alone, the
blocks of a round in rotating order on one world:

- **off**: `Game::step`.
- **probe F**: the script host's recorder alone on field F (`Game::record_script_writes`): the
  program instrumented and F's staged value probed at every statement of every script system call.
- **trace F**: the whole trace of F (`WriteTrace::step`): the recorder when scripts can read F, the
  boundary's writes, and F compared after every system (its slots first when scripts can read it,
  else its component's JSON).

The first tick of each block, where the program is instantiated again when the mode before it
differed (instrumented or not), is timed apart; "first tick" below is the median over rounds and
runs. The sets:

- **sailing** (`samples/sailing`: three script systems, **15 statements a tick**): 600-tick
  blocks, 20 rounds, 120 ticks of warm-up (11,980 timed ticks per mode and run). Fields:
  `Sloop.Boat.rudder` (nothing writes it here), `Sloop.Log.distance` (the `log` script writes it
  every tick: a row and a statement each tick) and `Sloop.Model` (a component scripts cannot read:
  the trace leaves the recorder off and reads its JSON after every system).
- **rules** (`bench/rules`, the rule workload of 1,000 fighters, [script.md](script.md): **22,947
  statements a tick**): 60-tick blocks, 6 rounds, 30 ticks of warm-up (354 timed ticks per mode and
  run). Fields of entity 2, a fighter: `Health.hp` (no hit landed on it in these ticks: no row) and
  `Spot.x` (written every tick through a helper's result, `x[i] += driftX(...)`: each row is `at`
  `scripts/rules.ts:79:13` `via` `scripts/combat.ts:29:5`, the helper's `return`).

## Results (quiet re-measurement, 2026-10-15)

Medians of the per-tick times of five processes; the ratio to `off` is each run's.

| sailing | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 | Against off | First tick |
|---|---|---|---|---|---|---|---|
| off | 72.1 us | 72.9 us | 71.2 us | 72.3 us | 71.3 us | 1 | 78 us |
| probe `Sloop.Boat.rudder` | 76.5 us | 76.8 us | 76.4 us | 76.7 us | 76.4 us | +6.1 %, +5.3 %, +7.3 %, +6.1 %, +7.2 % | 2,153 us |
| probe `Sloop.Log.distance` | 77.2 us | 77.5 us | 76.9 us | 77.2 us | 76.9 us | +7.1 %, +6.3 %, +8.0 %, +6.8 %, +7.9 % | 84 us |
| probe `Sloop.Model` | 73.5 us | 73.6 us | 72.6 us | 73.5 us | 72.7 us | +1.9 %, +1.0 %, +2.0 %, +1.7 %, +2.0 % | 79 us |
| trace `Sloop.Boat.rudder` | 82.3 us | 80.8 us | 80.6 us | 80.7 us | 80.6 us | +14.1 %, +10.8 %, +13.2 %, +11.6 %, +13.0 % | 93 us |
| trace `Sloop.Log.distance` (a row each tick) | 84.0 us | 83.0 us | 82.3 us | 82.5 us | 82.4 us | +16.5 %, +13.9 %, +15.6 %, +14.1 %, +15.6 % | 96 us |
| trace `Sloop.Model` | 101.7 us | 101.8 us | 99.2 us | 100.0 us | 99.0 us | +41.1 %, +39.6 %, +39.3 %, +38.3 %, +38.8 % | 2,186 us |

| rules | Run 1 | Run 2 | Run 3 | Run 4 | Run 5 | Against off | First tick |
|---|---|---|---|---|---|---|---|
| off | 3,559 us | 3,582 us | 3,574 us | 3,569 us | 3,555 us | 1 | 5,631 us |
| probe `2.Health.hp` | 10,749 us | 10,975 us | 10,933 us | 11,030 us | 10,822 us | +202.0 %, +206.4 %, +205.9 %, +209.1 %, +204.4 % | 13,107 us |
| probe `2.Spot.x` | 10,877 us | 11,041 us | 11,011 us | 11,067 us | 10,893 us | +205.6 %, +208.2 %, +208.1 %, +210.1 %, +206.4 % | 11,050 us |
| trace `2.Health.hp` | 10,894 us | 10,936 us | 10,992 us | 10,905 us | 10,869 us | +206.1 %, +205.3 %, +207.6 %, +205.6 %, +205.7 % | 10,933 us |
| trace `2.Spot.x` (a row each tick) | 10,968 us | 11,014 us | 11,029 us | 10,978 us | 10,956 us | +208.2 %, +207.5 %, +208.6 %, +207.6 %, +208.2 % | 10,993 us |

What the tables say, as far as these modes separate it:

- **The script side's cost grows with the statements a tick runs.** On the rule workload the probe
  adds 7.4 ms to a 3.6 ms tick, about 320 ns for each of its 22,947 statements (313 to 327 ns in
  the ten probe processes: the instrumented program's trace call and the probe's read): the tick
  takes three times as long. The data breakpoint's probe costs about the same per statement
  (debugger.md 10: 352 ns, 4.4 times the web workload's tick). On the sailing sample's 15
  statements it is about 4 to 5 us a tick (+5 to +8 %). A trace of a field of a script-heavy game
  costs that game's statements, whatever the field.
- **The engine side** (the boundary's writes and the field compared after each of the tick's
  systems) is the difference between trace and probe of the same field: about 4 us a sailing tick
  for a field scripts can read (the rudder; its slots compared, the JSON built only when they
  changed; 5 us for the log's distance, which also makes a row each tick), within the runs' spread
  on the rule workload.
- **A component scripts cannot read** (`Model`) runs no recorder: the program stays uninstrumented.
  Its JSON, built after every system, costs about 28 us a sailing tick (+38 to +41 %). Before the
  review's fix the recorder was armed for it and failed at every statement with a refusal and its
  suggestions: 6.6 times the tick for the probe and 7.1 to 7.3 times for the trace on the same bench
  (the review's `out/review-writes/bench-*.json`, on a busier machine). The probe now fails once a
  call, on a cheap check (+1.0 to +2.0 %: the instrumented program alone).
- **A row each tick** (the log's distance) adds about 2 us a sailing tick (1.7 to 2.2 us in each
  run) over the trace of the rudder, which nothing writes (under 1 us of it is the probe's,
  recording the statement). On the rule workload the fighter's `Spot.x` costs about 70 us a tick
  more than its `Health.hp` (0.3 to 1.2 % in each run, probe and trace alike), of the order of the
  spread between runs.
- **Switching the trace on or off** reloads the program at the boundary (instrumented, then
  stock), once at each end of a traced step: about 2.1 ms on the sailing sample (the first tick of
  `probe Sloop.Boat.rudder`, which follows `off`, and of `trace Sloop.Model`, which follows an
  instrumented mode) and 2.1 to 2.2 ms on the rule workload (the first tick of `off`, which follows
  an instrumented mode, and of `probe 2.Health.hp`, which follows `off`). On the sailing sample a
  `debug.writes` of 60 ticks spends more on its two reloads (about 4 ms) than the trace adds to its
  ticks (under 1 ms).

## With no trace

The hooks the trace needs stay in the tick when no trace runs: two calls of the step hooks'
`world` (a default method that does nothing) per system, a check per script call of whether a
recorder is armed, the per-tick question whether the program is to be instrumented, and a copy of
every write applied at a boundary (kept for a trace's boundary rows). A scratch example that times
the sailing tick as `off` does (20 rounds of 600 ticks), built unchanged at the branch's base
(`b7be59a2`) and on the branch, run alternately in three processes each under the lock (CPU load
36.5 % to 40.5 %, `bench3/off-vs-base/`): base 84.5, 77.9, 76.7 us; branch 79.7, 75.6, 76.9 us.
The difference is below the run-to-run spread; this bench cannot resolve it. This comparison ran on
the loaded machine and was not repeated in the quiet re-measurement (it is not part of the recipe).

## Quiet re-measurement

On an idle machine, from the merged checkout or the branch's worktree (`$MAIN` is the main
checkout's path), the example built:

```sh
python tools/write_trace_bench.py build
python tools/write_trace_bench.py run $MAIN/out/writes/quiet --runs 5 \
  --lock $MAIN/out/locks/bench_lock.py
python tools/write_trace_bench.py table $MAIN/out/writes/quiet
```

The `table` command prints the tables above (each run's median, the ratio to off, the first tick),
the conditions of each process and each trace's last row. The comparison with the base needs a
build of the base commit and is not part of it.

The results above (2026-10-15, the branch's worktree at `10b9cf27`) ran the whole `run`, without
`--lock`, inside one hold of the bench lock: `python $MAIN/out/locks/bench_lock.py run --label
quiet-writes --max-hold-min 25 -- python tools/write_trace_bench.py run $MAIN/out/writes/quiet
--runs 5`. The lock covered the gaps between processes as well, and each process's conditions
were still sampled inside it; the ten processes took 5 min 16 s.
