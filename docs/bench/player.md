# The player layer: decisions per second and token sizes (Pioneer)

- Date: 2026-10-09. Branch `explore/player`. Charter 2 (proof target 2: "a scripted decision model
  makes hundreds of decisions a second"; LLM agents play through MCP under limited perception), 3
  (principle 6: decision models may run in the tick, the LLM never does). Design:
  [spec/player.md](../spec/player.md).
- Machine: ASUS ROG Zephyrus G14 (budgets.md R1): AMD Ryzen 9 270 (16 threads), 15 GiB, Windows 11
  Pro 10.0.26200; release build (`opt-level = 3`, `codegen-units = 16`, no LTO), Rust 1.98.1. CPU
  only: nothing here touches the GPU.
- **Every timing in sections 1 and 2 is provisional, superseded by section 4** (the quiet
  re-measurement of 2026-10-10). Three other agents built and ran work on the machine the whole
  time; the same bench run minutes apart differed by up to 2x (the agent's decision cycles per wall
  second were 10.7 to 12.0 in one run and 20.0 to 21.3 in the next, the one below). Token sizes and
  tick counts do not depend on the machine: they are exact.
- Commands:
  `cargo run --release -p pocket-runtime --features transpile --example player_bench out/bench-runs/player/bench.json`
  (the run below, in full) and `cargo run --release -p pocket-interface --example perception_bench`
  (out/bench-runs/player/perception-bench.txt).

## 1. The reference skipper through the player tools

The reference skipper (`pocket_runtime::skipper`, spec/player.md 8) plays `samples/sailing-course`
as `Source::Player(0)` through `player.describe`, `player.observe`, `player.act` and `player.wait`
alone, on a game driven directly (no server, no MCP transport: the cost of the engine's side). Each
decision cycle observes three times (text at the default budget, for the token count; JSON at the
default budget; JSON at 1500 tokens, which the policy reads), decides, acts when it decides to, and
waits for the next decision point.

| Seed | Decisions | Acts | Ticks (game time) | Finished | Wall | Decision cycles per wall second | Ticks per wall second inside waits |
|---|---|---|---|---|---|---|---|
| 1 | 12 | 9 | 5148 (85.8 s) | yes, 3 marks | 0.586 s | 20.5 | 8909 |
| 2 | 12 | 9 | 5148 | yes | 0.584 s | 20.5 | 8928 |
| 3 | 12 | 9 | 5148 | yes | 0.600 s | 20.0 | 8680 |

The three seeds give the same run: nothing in the course draws from the seeded streams. A decision
cycle's own cost, the three observations and the act, is 690 us at the median (364 to 856 us, seed
1): about 1 ms of engine time per decision, against the hundreds of milliseconds to seconds an LLM
takes to answer one. The wall time of a run is the simulation inside the waits: 86 s of game time at
about 145 times real time. A decision arises every 429 ticks on average (1 to 1862; median 315),
about every 7 s of game time, with the sailing decision filter.

What the agent reads, in tokens (`ceil(bytes / 4)`, README, Token estimates; exact, seed 1, 12
decisions):

| Answer | Median | Min | Max |
|---|---|---|---|
| `player.observe`, text, the profile's default budget (400) | 222 | 162 | 235 |
| `player.observe`, JSON, default budget | 361 | 326 | 371 |
| `player.observe`, JSON, budget 1500 | 697 | 518 | 741 |
| `player.wait`'s answer (JSON, events delta included) | 232 | 122 | 428 |
| `player.describe` (the whole definition: beyond 2000 tokens, names only) | 268 | | |
| `player.describe {"part": "intents"}` (three intents with their JSON Schemas) | 1554 | | |

So one decision costs an LLM agent about 450 tokens of input in text (an observation and a wait's
answer), plus whatever it chooses to describe. The text projection is 0.6 of the JSON at the same
budget. An example text observation (seed 1, tick 890, just after the first crate came aboard):

```text
tick 890 t=14.8s seat=skipper observer=Sloop#3
self heading_deg=077 course_deg=077 speed_mps=2.7 wind_from_deg=270 wind_mps=6.0 twa_deg=-167 awa_deg=-151 aws_mps=2.8 point_of_sail=running tack=port hoist=1.00 sheet=0.88 trim=overtrimmed drive=0.99 rudder=0.54 heel_deg=7 pos_m=(43.3,0.1,-12.0) afloat=true next_mark=Mark1 taken=1 left=3
decision 890.skipper event:9:crate.taken
see Mark1#5 mark brg 069 rng 50.0m color=yellow round_to=port order=1 next=true
see Mark2#6 mark brg 108 rng 134m color=red round_to=port order=2 next=false
see Isle#8 island brg 084 rng 107m radius_m=10 shore_m=97 shore_brg_deg=084
see Crate2#10 crate brg 099 rng 85.5m alongside=false
chart Mark3#7 mark brg 082 rng 209m color=yellow round_to=starboard order=3 next=false
```

Mark3 is on the chart only: the Isle hides it (perception's ray casts).

## 2. Decision models in the tick

N boats, 30 m apart in a column north of the course, steered every tick toward a point 400 m east of
where each starts, so decisions per second = N x ticks per second. Three variants on the same scene
(the course's own Sloop and rules included), 600 ticks after 30 of warm-up:

- **baseline**: the boats sail with nothing deciding;
- **native**: every boat a seat (`boat_k`, the skipper's profile: 2 km of sight all round,
  occlusion) with a `sail_to` intent; its Rust executor decides the rudder and the sheet every tick
  from the seat's `PerceptionView`, and perception runs for every boat;
- **scripted**: a TypeScript system (`pilot`, in the bench) steers every boat with a proportional
  helm and the apparent-wind trim rule, reading the world in columns (no perception).

System times are means per tick from the simulation's step hooks (`Game::step_with`).

| Boats | Variant | Tick | `interface.intents` | `interface.perception` | `script.update` | Decisions per second |
|---|---|---|---|---|---|---|
| 1 | baseline | 115 us | 1 us | 13 us | 45 us | |
| 1 | native | 148 us | 13 us | 32 us | 45 us | 6,756 |
| 1 | scripted | 139 us | 1 us | 12 us | 71 us | 7,207 |
| 8 | baseline | 225 us | 1 us | 14 us | 46 us | |
| 8 | native | 732 us | 94 us | 402 us | 58 us | 10,931 |
| 8 | scripted | 259 us | 1 us | 14 us | 83 us | 30,880 |
| 32 | baseline | 603 us | 1 us | 18 us | 53 us | |
| 32 | native | 6,477 us | 417 us | 5,199 us | 135 us | 4,941 |
| 32 | scripted | 708 us | 2 us | 20 us | 132 us | 45,178 |

Both kinds of decision model clear "hundreds of decisions a second" by an order of magnitude or more
while the world runs well above real time (a 60 Hz tick has 16.7 ms). What they cost:

- A native executor decision is 12 to 13 us (`interface.intents` per boat); the scripted pilot's is
  about 2.5 us (the `script.update` difference over the baseline, per boat, at 32 boats), since it
  reads columns and no perception.
- The native variant's cost is perception's: every observing boat tests every candidate within its 2
  km with two ray casts against the occluders, so the update grows with observers times candidates
  (8 boats: 50 us each; 32 boats: 160 us each). Most of those rays cross open water: a first test of
  the ray against the occluders' bounds is the obvious next step (spec/player.md 10).
- perception_bench's rows (perception.md, Performance), same run: `perception.tick` 12.8 us a tick
  (the sailing probe, one skipper); `perception.npc` 1,794 us a tick (100 observers over 1,000
  perceivable entities among 20 walls); `observe.sail` 62 us a call (text, default budget).

## 3. What was not measured

- An LLM playing through MCP: the transport and the seat-bound session were checked live (a stdio
  `pocket mcp --seat skipper` session observed, acted, waited and was refused an unavailable
  affordance, `world` and the omniscient view: `tools/player_mcp_probe.py`, transcript in
  out/bench-runs/player/mcp-session.txt), but no model was run, so there is no tokens-per-course or
  success rate for an LLM yet.
- Real time with pause-on-decision under load: the thread test checks that the world holds at a
  decision and resumes on an answer, not the latency of the resume (time.md's `resume.latency`).
- Script executors, which are not built (spec/player.md 10).

## 4. Quiet re-measurement (2026-10-10)

The same benches with no other agent running, on AC power
([quiet-2026-10-09.md](quiet-2026-10-09.md), session 2), on master `0871920d`'s release build:
`player_bench` three times and `perception_bench` three times, one after the other ([bench-r1.json
to
bench-r3.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/player),
`perception-r*.txt`; the JSON still says `provisional: true`, the bench's fixed label). Cells:
median of the three runs (minimum to maximum). Token sizes and tick counts did not change (they are
exact).

The reference skipper (nine runs: three seeds in each of three processes): 12 decisions and 9 acts
over 5148 ticks in 0.565 s of wall time (0.559 to 0.570), **21.2 decision cycles per wall second**
(21.0 to 21.5; section 1: 20.0 to 20.5), a decision cycle's own cost 633 us at the median (601 to
663; section 1: 690), and 9,224 ticks per wall second inside the waits (9,142 to 9,334; section 1:
8,680 to 8,928).

Decision models in the tick:

| Boats | Variant | Tick | `interface.intents` | `interface.perception` | `script.update` | Decisions per second |
|---|---|---|---|---|---|---|
| 1 | baseline | 102 us (102-105) | 0 us | 11 us | 38 us | |
| 1 | native | 136 us (135-137) | 12 us | 29 us | 40 us | 7,370 (7,283-7,410) |
| 1 | scripted | 132 us (130-132) | 1 us | 11 us | 66 us | 7,601 (7,564-7,704) |
| 8 | baseline | 205 us (203-206) | 1 us | 12 us | 40 us | |
| 8 | native | 668 us (666-682) | 86 us | 375 us | 46 us | 11,984 (11,725-12,012) |
| 8 | scripted | 248 us (241-250) | 1 us | 13 us | 78 us | 32,254 (32,043-33,142) |
| 32 | baseline | 608 us (578-613) | 1 us | 16 us | 49 us | |
| 32 | native | 5,055 us (5,039-5,240) | 359 us | 4,056 us | 74 us | 6,330 (6,107-6,350) |
| 32 | scripted | 624 us (621-626) | 1 us | 14 us | 107 us | 51,245 (51,124-51,497) |

perception_bench: `perception.tick` 12.5 us a tick (12.0 to 12.5; section 2: 12.8),
`perception.npc` 1,734 us (1,664 to 1,776; 1,794), `observe.sail` 60.0 us a call (59.9 to 60.7; 62).

What changed against sections 1 and 2: the runs now agree within 1 to 7% (the provisional ones
differed by up to 2x), and the costs are 2 to 22% lower, the most where the most work runs per
tick (32 native boats: tick 5,055 against 6,477 us, perception 4,056 against 5,199 us; 32 scripted
boats: 51,245 against 45,178 decisions per second); only the 32-boat baseline's tick is level (608
against 603 us). Both kinds of decision model clear hundreds of
decisions a second by an order of magnitude or more, as before; the conclusions of section 2
stand.

## 5. Fork lookahead (2026-10-15; wall times re-measured quiet 2026-10-16)

- Branch `explore/lookahead`. Charter 2 (proof target 2: fork lookahead) and 6 (its measurement:
  "fork 前瞻的收益"); the model is [spec/player.md](../spec/player.md) 8.1, the benchmark's fork
  condition shared/benchmark/tasks.md 6.1 (4 live branches, 600 branch ticks per decision answered
  on main), the budgets `fork.sail`, `branch.sail` and `fork.sail.memory`
  ([spec/budgets.md](../spec/budgets.md) 5.1).
- Machine: R1 as above. The track's runs of 2026-10-15 shared it with five other agents' tracks
  building and testing; every timed command held the machine-wide bench lock (no other track timed
  at once) and recorded the machine's conditions inside the hold, but other builds ran, so their
  wall times were provisional. **Every wall time in this section is the quiet re-measurement of
  2026-10-16** (01:50 to 01:55, after every other track had finished: on AC power, no other cargo,
  rustc or Chrome process, CPU load 5 to 10% at the start of every timed command inside the hold),
  on the release build of `c232957e` (the code of `c8ba00ee`). Ticks, crates, decisions, acts,
  forks and branch ticks are exact: the runs are deterministic (the same variant gives the same
  numbers in any run, with kept or fresh branch games; `tests/lookahead.rs`), and the quiet run
  repeated all 186 episodes of `run-20261015c` exactly, world hash, weighed branch and branch value
  for value; only the wall times changed.
- Commands (from the worktree; raw output in the main checkout's ignored `out/lookahead/`):
  `python tools/lookahead_bench.py build`, then
  `python tools/lookahead_bench.py run <out> --fork-runs 3 --seeds 0-30 --chunk 8 --lock out/locks/bench_lock.py`
  (the models `greedy`, `lookahead` and `nofork` by default) and
  `python tools/lookahead_bench.py table <out>`, which prints every table and figure of this
  section. The quiet run made the same steps (three fork runs, then the course in chunks of 8
  variants), each in its own hold of the lock under the label `quiet-lookahead`.
- Runs: `out/lookahead/quiet` (2026-10-16, the quiet re-measurement: every wall time below; CPU
  load 5 to 9% at the start of the course chunks, 7 to 10% at the fork runs). It replaces the
  provisional wall times of `run-20261015c` (commit `c8ba00ee`: the course with the fork-off
  control, CPU load at the start of the course chunks 8 to 58%, and three fork runs at 56 to 76%)
  and `run-20261015d` (the same commit, three fork runs at 8 to 14%, the quietest of the provisional
  sets). The greedy and lookahead episodes repeat those of the earlier run `run-20261015b`
  (commit `00b7dd3e`) world hash for world hash, 124 of 124; the first run, `run-20261015` (commit
  `735293b6`), is the "before" of section 5.2 and found the two faults `7c7b3119` fixed (section
  5.1, last item).

### 5.1 The greedy skipper, the lookahead, and the lookahead with forks off

The course's 31 variants 0 to 30 (spec/player.md 8.1: Pioneer's own variants, not the shared
benchmark's seeds; variant 0 is the course as shipped, the others move the wind and the four
crates), each played to the end under the seat's perception and with the seat bound to the
omniscient profile, through `player.observe`, `player.act` and `player.wait` as
`Source::Player(0)`, at most 36,000 ticks, by three decision models: the reference skipper
(`greedy`), the fork lookahead around it (`lookahead`), and the same lookahead with forks off
(`nofork`, spec/player.md 8.1: the same candidates, rules and margin, each valued from the chart
alone), which separates what forking adds from what the lookahead's wider candidates and its value
add. The score ranks the two goals together, lower better: ticks to finish less 1000 per crate
aboard. That weight is the lookahead's own (its value counts a crate 1000 ticks, 58.3 m of course),
so the score measures both lookahead models on their own objective; the third table shows how the
verdicts move with the weight. The switch margin (15 m against 5 m) was chosen and the land charge
added on the variants 100 to 119 (three tuning runs, `out/lookahead/tune*.json`), which the
comparison does not use; the model was first explored on variant 1
(`out/lookahead/explore-course.json`), which it does use, as do the tests.

| Condition | Model | Finished | Ticks to finish, mean (median) | Crates aboard, mean | Score, mean | Decisions, mean (median) | Forks, mean | Branch ticks, mean | Overrides, mean | Wall s, median |
|---|---|---|---|---|---|---|---|---|---|---|
| perception | greedy | 31/31 | 7,501 (6,881) | 1.87 | 5,630 | 110.0 (11) | | | | 0.75 |
| perception | lookahead | 31/31 | 6,671 (5,936) | 2.35 | 4,316 | 78.9 (12) | 24.0 | 4,703 | 1.6 | 1.35 |
| perception | nofork | 31/31 | 6,613 (6,513) | 3.16 | 3,452 | 40.5 (14) | 0.0 | 0 | 3.0 | 0.71 |
| omniscient | greedy | 31/31 | 6,461 (6,123) | 1.58 | 4,881 | 7.2 (6) | | | | 0.64 |
| omniscient | lookahead | 31/31 | 6,651 (5,921) | 2.26 | 4,393 | 75.5 (10) | 16.6 | 3,290 | 1.0 | 1.02 |
| omniscient | nofork | 31/31 | 6,706 (6,525) | 3.19 | 3,513 | 54.8 (10) | 0.0 | 0 | 2.3 | 0.67 |

Paired by variant, the first model minus the second:

| Condition | Pair | Score, mean (median) | Crates, mean | Ticks, mean (median) | Variants better / equal / worse | More / fewer crates | Faster / slower |
|---|---|---|---|---|---|---|---|
| perception | lookahead - greedy | -1,314 (-957) | +0.48 | -830 (-83) | 22 / 6 / 3 | 12 / 3 | 16 / 9 |
| omniscient | lookahead - greedy | -487 (-948) | +0.68 | +190 (+0) | 19 / 10 / 2 | 15 / 2 | 10 / 11 |
| perception | nofork - greedy | -2,178 (-1,501) | +1.29 | -888 (-135) | 29 / 0 / 2 | 23 / 3 | 16 / 15 |
| omniscient | nofork - greedy | -1,368 (-1,404) | +1.61 | +245 (+103) | 26 / 0 / 5 | 27 / 2 | 12 / 19 |
| perception | lookahead - nofork | +864 (+205) | -0.81 | +58 (-2) | 6 / 4 / 21 | 1 / 15 | 16 / 11 |
| omniscient | lookahead - nofork | +881 (+16) | -0.94 | -55 (-231) | 9 / 5 / 17 | 1 / 18 | 20 / 6 |

The crate's weight in the score, ticks a crate is worth (the bench and the lookahead's value use
1000); cells: variants better / equal / worse:

| Condition | Pair | 0 | 250 | 500 | 1000 | 2000 |
|---|---|---|---|---|---|---|
| perception | lookahead - greedy | 16 / 6 / 9 | 21 / 6 / 4 | 22 / 6 / 3 | 22 / 6 / 3 | 22 / 6 / 3 |
| omniscient | lookahead - greedy | 10 / 10 / 11 | 16 / 10 / 5 | 18 / 10 / 3 | 19 / 10 / 2 | 19 / 10 / 2 |
| perception | nofork - greedy | 16 / 0 / 15 | 20 / 0 / 11 | 26 / 0 / 5 | 29 / 0 / 2 | 28 / 0 / 3 |
| omniscient | nofork - greedy | 12 / 0 / 19 | 16 / 0 / 15 | 22 / 0 / 9 | 26 / 0 / 5 | 27 / 0 / 4 |
| perception | lookahead - nofork | 16 / 4 / 11 | 13 / 4 / 14 | 9 / 4 / 18 | 6 / 4 / 21 | 6 / 4 / 21 |
| omniscient | lookahead - nofork | 20 / 5 / 6 | 17 / 5 / 9 | 13 / 5 / 13 | 9 / 5 / 17 | 8 / 5 / 18 |

Without the variants where an episode livelocked aground (more than 300 decisions: 18 and 28
under perception, 18 omniscient):

| Condition | Pair | Score, mean (median) | Ticks, mean (median) |
|---|---|---|---|
| perception | lookahead - greedy | -1,001 (-913) | -484 (+0) |
| perception | nofork - greedy | -1,392 (-1,500) | -82 (+22) |
| perception | lookahead - nofork | +392 (+205) | -401 (-4) |
| omniscient | lookahead - greedy | -1,113 (-969) | -447 (+0) |
| omniscient | nofork - greedy | -1,511 (-1,450) | +89 (+97) |
| omniscient | lookahead - nofork | +398 (+12) | -535 (-342) |

What forking adds, what the lookahead gains, and what it costs:

- **The measured benefit of fork lookahead, fork on minus fork off, is negative on this course's
  score.** Valued by forks rather than from the chart, the same candidates took 73 crates against
  98 over the 31 variants under perception and 70 against 99 omniscient, and scored worse on 21
  variants and better on 6 (omniscient: 17 and 9). Forks made the boat faster, not richer: on time
  alone (a crate weight of 0) fork on beats fork off on 16 variants and loses on 11 under
  perception, 20 and 6 omniscient, with paired medians of -2 and -231 ticks (means without the
  livelocked variants -401 and -535). The two break even at about 250 ticks a crate under
  perception and 500 omniscient.
- **Why: a branch is too short to see a chase pay.** In 23 of the 31 variants under each condition
  the two models first part where fork off sets out for a crate and the lookahead keeps the greedy
  act, 18 of them under perception at tick 1, the start. There the chart puts the chase 18 to 58 m
  ahead of the greedy act, while 150 to 300 ticks of a branch (2.5 to 5 seconds from the start)
  put it -5 to +14 m ahead, most within a metre, short of the 15 m margin: in so short a branch
  the boat has not closed on the crate enough for the value to tell the plans apart (it credits a
  chase only at a measured closing speed, spec/player.md 8.1, step 3). Under the contract's bound
  of 600 branch ticks a decision shared by up to four branches, this value cannot see the chases
  the chart shows. A longer horizon, a chase credited by its bearing rather than a measured
  closing speed, or forks used only to veto a candidate (land, the wind's eye) are the next
  trials; none was run.
- **Against the reference skipper** the lookahead gains (lookahead minus greedy), but that
  difference mixes forks with a wider candidate set (crates to 120 m at any angle to the wind, and
  the mark, where the greedy skipper turns only for a crate within 45 m and more than 70 degrees
  off the wind's eye) and a value aligned with the score; fork off minus greedy shows those two
  alone, +1.29 crates a variant under perception and better on 29 variants of 31. The lookahead
  took 73 crates to the greedy skipper's 58 under perception, better on 22 variants and worse on 3,
  and 70 to 49 omniscient, better on 19 and worse on 2. It is not faster at the median: the paired
  medians are -83 ticks under perception and 0 omniscient, and it was slower on 9 and 11 variants
  of 31; the perception mean of -830 ticks owes -10,657 to variant 28, where the greedy skipper
  livelocked aground (-484 mean and 0 median without variants 18 and 28).
- **Examples against the greedy skipper.** Variant 0 at tick 3,349: sailing on scored 13.2 m, the
  chase for Crate3 30.7 m; the lookahead finished with three crates to two, 87 ticks later.
  Variant 28 under perception: the lookahead chose to chase Crate2 at three decisions in a row
  (ticks 1,410 to 1,889, by 45 to 53 m) and finished in 5,142 ticks, where the greedy skipper ran
  aground and took 15,799; at tick 4,378 a chase for Crate3 worth 133.7 m before its land charge
  against the greedy act's 137.6 m was not chosen either way. Omniscient variant 18: a chase at
  tick 1,611 (by 31 m) left the boat where its course to the mark crossed the Isle; from tick
  3,217 every candidate but those at ticks 21,962 and 22,999 was charged for land, and the boat
  went aground and finished at 24,475 ticks with two crates against the greedy skipper's 5,180
  with one. Under perception variant 7 cost 1,837 ticks for one crate more (score +837), variants
  2 and 21 33 and 549 ticks for nothing.
- **The land charge changed no choice.** 91 branches were charged for a land warning: 68 crate
  chases and 23 greedy acts, all of the latter on omniscient variant 18 from tick 3,217. In every
  decision with a charged branch, that branch was already the worse by its own value, so the
  lookahead would have chosen the same without the charge. Fork off, which knows no land, ran
  aground and livelocked on variant 18 alone, as the lookahead did.
- **The bound decides how far a branch sees.** Every weighed decision spent exactly the 600 ticks
  between about three branches (3.06 forks a weighed decision): 150 to 300 ticks, 2.5 to 5 seconds
  of sailing, 8 to 17 m. The value extrapolates the rest with the closing speed a branch measured,
  which is the lookahead's whole model of the future; the contract's bound keeps a branch from
  scouting the course, and the losses above are what it hides.
- **Cost** (quiet re-measurement): branches simulated 0.71 of main's ticks under perception (0.49
  omniscient), 24 forks an episode; a weighed decision took 82 ms of wall time under perception and
  104 ms omniscient (the provisional runs gave 103 and 146 ms in `run-20261015c` and 169 and 200 ms
  in `run-20261015b`, under other tracks' load). The branch ticks are probably most of it: 600 ticks
  at the fork bench's 107 us, measured in the same quiet run, is 64 ms, and a decision's three
  forks into kept branches 0.75 ms; the observations and acts were not timed apart. Fork off decided
  in 0.02 ms under perception and 0.07 ms omniscient. An episode took 1.35 s against 0.75 s for the
  greedy skipper at the median under perception (1.02 s against 0.64 s omniscient; 1.53 s against
  0.90 s under perception in the provisional run).
- **The omniscient condition measures the decision filter as much as the information.** Bound to
  `omniscient_player`, the seat never meets a `sighted` decision point, so the greedy skipper
  decides 6 times at the median against 11 and turns for fewer crates: 49 against 58 (variant 0:
  none against two). Its mean score is better omniscient only because two variants ran it aground
  under perception (18 and 28: 1,997 and 1,105 decisions, a grounded boat's every other tick,
  spec/player.md 10). Both lookahead models make decision points of their own with their chases, so
  the condition changes their crates and score little (lookahead: crates -0.10, score +77; fork off:
  +0.03 and +61). The shared benchmark's paired difference "omniscient minus perception" (tasks.md
  6.4) will carry this filter effect for every policy whose decision points the filter makes.
- **The first run** (`run-20261015`, the same variants) found two faults, fixed in `7c7b3119`:
  `play` stopped at 2,000 decisions, which cut the lookahead's variant 18 under perception at tick
  20,901 while the greedy skipper finished 3 decisions under the cap, and the omniscient lookahead
  weighed 2,000 grounded decisions on variant 18 for nothing (5,673 forks, 827 s). A grounded boat
  now gets the greedy act without branches, and `play` stops only at the episode's end or its tick
  limit. No other variant changed.

### 5.2 The cost of a fork

The course at tick 300 with the reference skipper sailing it (14 entities, a 66,405-byte snapshot;
the budgets' `sail` workload is not built, section 5.3), three runs of the `fork` bench; cells: the
median of the runs' medians (minimum to maximum), 100 calls a run (20 for a fresh branch) after 10
(3) unmeasured; bytes: what the median call allocated through Rust's allocator (a counting global
allocator in the bench binary). Before: commit `735293b6` (`run-20261015`); after: `00b7dd3e`
(`run-20261015b`, CPU load 35 to 52%), which keeps the player declarations across a restore of
the same spec. In these two runs the bench dropped each fresh branch inside its timed call, so
that row includes the teardown. Both are provisional runs beside other tracks' builds, kept for
the size of the change; the quiet re-measurement (the second table) did not repeat them.

| Operation | Before, us | Before, bytes | After, us | After, bytes |
|---|---|---|---|---|
| snapshot | 60.2 (55.9 to 64.3) | 241,134 | 74.7 (69.9 to 117.3) | 241,134 |
| restore into a kept branch | 21,191 (17,940 to 23,049) | 16,965,893 | 282.3 (278.8 to 441.1) | 332,229 |
| fork into a kept branch (`Game::fork_into`): `fork.sail` | 19,916 (18,273 to 22,072) | 17,210,641 | 411.9 (363.6 to 546.7) | 576,977 |
| fresh branch with its own script host (`Game::fork`), teardown included | 44,752 (40,047 to 46,767) | 36,480,502 | 36,819 (24,420 to 38,728) | 19,891,870 |

The bench at commit `c8ba00ee` keeps what each timed call returns until all are timed, times
dropping a fresh branch on its own, and measures what a call leaves allocated (allocated less
freed). Three runs in the quiet re-measurement (2026-10-16, `out/lookahead/quiet`, CPU load 7 to
10%); the snapshot's bytes left allocated are the snapshot it returns:

| Operation | Wall time, us | Bytes allocated (median call) | Bytes left allocated after the call (median) |
|---|---|---|---|
| snapshot | 81.8 (78.8 to 84.0) | 241,134 | 70,908 |
| restore into a kept branch | 184.7 (184.2 to 185.9) | 332,229 | 0 |
| fork into a kept branch (`Game::fork_into`): `fork.sail` | 249.8 (249.8 to 250.1) | 576,977 | 0 |
| fresh branch with its own script host (`Game::fork`): `branch.sail` | 21,028 (20,921 to 21,256) | 19,892,174 | 1,735,167 |
| dropping a fresh branch | 765 (751 to 812) | | |
| a branch tick | 107.1 (104.7 to 117.7) | | |

The provisional runs of the same commit agree with these within about 2% at 8 to 14% CPU load
(`run-20261015d`: `fork_into` 252.7 us, a fresh branch 21,061 us, a branch tick 108.8 us) and took
1.7 to 3.1 times as long under 56 to 76% (`run-20261015c`: 772.2 us, 36,570 us and 219.7 us).

- **A restore re-derived the player layer.** Every restore parsed the perception declarations again,
  validated them and built the action catalog from `PlayerSpec`. The world now remembers the spec
  its declarations came from and keeps them when the restored spec is the same (spec/player.md 3);
  that change alone took a restore from 21.2 ms and 17.0 MB of allocations to 0.28 ms and 0.33 MB in
  the provisional runs (0.18 ms in the quiet one), so re-deriving was nearly all of it. A fork into
  a kept branch then costs 0.25 ms in the quiet re-measurement (0.41 ms in the provisional
  `run-20261015b`, 0.77 ms in the loaded `run-20261015c`), 2.3 ticks of the course measured in the
  same run (2.3 to 3.5 in the provisional runs): within the figure of `fork.sail`'s ceiling (a tick,
  `tick.sail`'s 4.2 ms), but more than one of the course's measured ticks. The course's checks, the
  lookahead's determinism tests and a test that a restore of another spec derives that spec's
  declarations pass.
- **A fresh branch** still derives the declarations once when its world is built and instantiates
  the bundle in a new QuickJS-ng host: 21.0 ms in the quiet re-measurement (21.1 ms in the
  provisional runs at 8 to 14% load, 36.6 ms loaded) without its teardown, which takes 0.8 ms more
  (1.7 ms loaded; the first runs' figures included it); more than a frame (`branch.sail`'s ceiling
  is 16.7 ms, spec/budgets.md 13), which the quiet run confirms. The lookahead therefore keeps
  its branch games and forks into them again; caching the derived declarations with the game's setup
  would bring a fresh branch near the host's own cost.
- **Memory** (`fork.sail.memory`): a fork into a kept branch allocates 0.58 MB and leaves none of
  it allocated after the call (median 0 bytes: the restored world replaces the branch's); a live
  fresh branch holds 1.74 MB of Rust heap and 0.31 MB of QuickJS-ng heap (the script host
  allocates through the C runtime, so its own count is read), about 33 MB for 16 branches.

### 5.3 Not measured

- An LLM agent with fork tools: `fork` and `discard` for players (mcp.md 5.5) are not built
  (spec/player.md 10), so the lookahead is a decision model in process, not an agent's session.
- `branch.main_lateness`: no branch steps on the game thread beside main in real time yet.
- The budgets' own workload: `fork.sail` and `branch.sail` name `sail` at tick 300, which is not
  built; the course stands in, but it is a different and smaller world, not a superset: one Isle
  of radius 10 where `sail` (the benchmark's `island` fixture) has an Isle of radius 150 and two
  islets, and the marks, crates, course rules and player layer that `sail` has none of.
