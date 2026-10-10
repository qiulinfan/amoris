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
