# The engine fixes the debug evaluation asked for

[debug-eval.md](debug-eval.md) gave an LLM agent (opencode, GLM 5.3 Flash) a planted script bug in
the sailing game and the host's CLI, 23 times, and ranked what the engine should add by what it
cost the agents. This page says what Pioneer built of that list on 2026-10-09 (branch
`explore/agentdebug`), how it was checked, what it measures, and what was left.

- Machine: Windows 11 Pro, AMD Ryzen 9 270 (16 threads), 15 GB. Three other agents built and ran
  GPU work at the same time: **every timing here is provisional**; the held host's latencies were
  measured again on a quiet machine (the last section), which supersedes them.
- Build: `cargo build --release`, Rust 1.98.1; QuickJS-ng compiled by the clang-cl the build
  script found (its warnings name it), with P1 to P11.
- The LLM evaluation itself was not run again: its harness is macOS only (`sandbox-exec`, the
  owner's opencode runner). The fixes are checked by tests and by driving the real binary
  ([smoke-server.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/smoke-server.txt),
  [held-host.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/held-host.json));
  whether agents now finish faster is a question for the next evaluation run.

## What was built, by finding

| Finding (debug-eval.md) | What it cost the agents | Built | Checked by |
|---|---|---|---|
| 1. A breakpoint stalls the agent's own tools | 28 calls in 11 runs hung until the agent's tool gave up, 2,238 s, 21 % of all agent time | (a) `time.step` returns when a breakpoint, data watch, `debugger;`, exception or debugger step stops the game, with `stopped_by: {reason, tick, system, location, breakpoint?, watch?}`; the step ends with the stopped tick. (b) While held, `status`, `world.get/tree/query/schema`, `scripts.list/read/status` and `snapshot` answer from the last publication (marked `paused_at`), `catalog.list`, `docs.search`, events, logs and `debug.*` as always. (c) Every other call that needs the game thread is refused at once with `debug.paused {location, reason, tick, system}`; the CLI waits 60 s at most (`--timeout`, `POCKET_TIMEOUT`) and says `host.timeout` | pocket-app `tests/paused_host.rs`; the smoke test's section 4; server.md 3.4 |
| 2. A restore silently reverts the scripts | 11 runs; 3 reported a fix live on a host running the bug | `snapshots.restore` and `debug.rewind` keep the applied scripts unless `{bundle: "snapshot"}`: the world is restored under its own bundle and swapped to the applied one by the hot update's recorded Host write; the answer and the status line name the bundle (`restored tick 0; scripts dd3ac0de7414 (applied)`, `... | scripts dd3ac0de7414`) | pocket-runtime `tests/agent_reads.rs` (restore on the thread, a recording through restore and swap replays identically); smoke test; server.md 3.3 |
| 3. `debug.eval` cannot see most block-scoped locals | 7 runs, 37 failed evaluations | P11, a QuickJS-ng patch (`third_party/patches/p11-debug-eval-scope.diff`, applied by `vendor.py`): an evaluation resolves names in the lexical scope of the statement its frame stands at, in the stopped frame and in callers' (below) | pocket-app `debug_agent.rs` `eval_sees_the_block_scoped_locals_in_scope` (fails without P11's lookup); CDP end to end 20/20 ([cdp-e2e-p11.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/cdp-e2e-p11.txt)); pocket-script's tests, the wasm build's included |
| 4. The debugger is not described where agents look | 34 refusals of guessed parameters in 9 runs | a catalog entry with kind, aliases and JSON Schema for every `debug.*` method (`pocket help debug.watch` lists `entity, component, field`); the MCP `debug` tool takes all 16 methods as actions and lists their 18 parameters; `debug.watch` takes entity names; aliases `debug.break`, `debug.watch.clear`; a module's short path (`helm.ts`); CLI short forms (`pocket debug break scripts/helm.ts:67 --if <expr>`, `debug eval <expr>`, `debug watch Sloop.Boat.hoist`, `debug set`, `step`, `clear`, `unwatch`, `rewind`, ...); `debug state` as text (the place, the innermost frame's locals one per line, the other frames' places) and `debug.state {brief}` as JSON | `paused_host.rs` (catalog entries equal `pocket_debug::methods()`; the MCP tool covers every method and parameter); unit tests of the short forms and text forms in `client.rs` |
| 5. A write trace ("why did this field change") | | not done | |
| 6. Reads come whole | 12 refusals of field reads; median 29 k characters of tool output per run | `world get Sloop Boat.heading_deg,Boat.rudder` (`world.get {fields}`, paths, `null` where the entity lacks the component, a wrong field refused with the component's fields and a suggestion); `scripts read <p> --lines 50-80` (numbered); `step 300 --sample Sloop.Boat.heading_deg,Sloop.Boat.rudder --every 30` (a table); arrays past 8 items cut in the CLI's text | `agent_reads.rs`; smoke test |
| 7. Smaller fixes | | `pocket scripts status`, `pocket docs <words>` | |

## Measurements (provisional, superseded by the quiet re-measurement)

From
[held-host.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/held-host.json)
(`python tools/held_host_bench.py`, ten rounds, medians) and the smoke run:

| What | Before (debug-eval.md, macOS) | Now (Windows, loaded machine) |
|---|---|---|
| `time.step` into a breakpoint, `POST /api/call` round trip | did not return until the resume | 18.0 ms (3.7 ms in-process in `paused_host.rs`, a smaller game) |
| `status` while held | waited for the resume | 3.0 ms |
| `world.get {fields}` while held | waited | 2.5 ms |
| `scripts.apply` while held | waited (one agent's waited behind a breakpoint that re-fired each tick) | refused in 1.8 ms |
| The same four through the CLI, process start included | the agent's tool timed out (15 to 120 s) | 62 to 75 ms (`pocket status` while running: 48.5 ms in the same session) |
| `world get Sloop`, text / exact JSON | 11 KB | 3,143 B / 11,091 B; two fields alone 54 B |
| `debug.state` at `rules.ts:27` | 9.5 KB over 412 lines in the evaluation's `helm.ts` frames | full JSON 3,629 B, `{brief}` 677 B, CLI text 306 B |
| MCP `tools/list` | 4.4 KB, 1,236 tokens (cl100k) | 6,645 B, about 1,660 tokens at four bytes a token: the debugger's description costs about 400 tokens a session |

The step into a breakpoint over HTTP (18 ms) was mostly the server's look for a stop: it checked
every 5 ms, and a tokio timer on Windows fires at the system timer's 15.6 ms granularity. Since the
review below the game thread answers the step from inside the stop, and the same step takes
0.39 ms over a kept-alive connection (median of 20; reads while held 0.13 to 0.17 ms). Later runs
of `held_host_bench.py` on this machine gave about 16 ms for every call over HTTP, `status` while
held included, which never reaches the game thread: `urllib` opens a connection per call, and that
connection, not the host, took a timer quantum then (the earlier run's 2 to 3 ms reads show it did
not always). HTTP figures from new connections are not comparable across runs here.

## The review of the held host (2026-10-09)

An adversarial review of this branch confirmed two defects in server.md 3.4 as first built; both are
fixed, and `tools/held_host_review.py` runs the reviewer's reproductions against `pocket serve`
([held-host-review.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/held-host-review.json),
provisional).

| Finding | Reproduction before the fix | Fix | Now |
|---|---|---|---|
| **The editor's Stop while paused was refused**, and Play kept running. The editor continued the pause, read `time.control {}` and sent `play.stop`; in real-time Play the next tick's breakpoint held the game again within milliseconds of the continue, so `time.control` and `play.stop` met a held game and were refused at once (`queued: false`); `played` was undefined, so Stop also skipped applying the edited scripts to the edit world. The same for any "continue, then act" call: 59 of 60 `world.edit`s after a continue refused | debugme copy, breakpoint in the gauge loop, Play, `debug.wait`, 1 s, `debug.continue`, `play.stop`: `debug.paused`, 3 of 3 runs, also with three gauges | `play.stop` in Play crosses the hold: the server has the hub pass over every pause (`DebugHub::pass`, counted, ended by a guard also when the caller goes away) until the Stop lands. `time.control {}` answers while held (the status). `time.control` with changes is queued for the boundary after the held tick (`debug.paused {queued: true}`): Pause, then Continue leaves Play resting at the tick's end. The editor's Stop sends `time.control {}` and `play.stop` and continues nothing itself; a queued call shows as an information toast | `play.stop` answers Edit mode in 6 of 6 runs (one and three gauges), 1.6 to 9.8 ms; the editor's end-to-end run passes every step on Windows, Edit mode 0.1 s after Stop, the edited bundle on the edit world ([debug-host.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/debug-host.txt)); Pause while held, Continue: Play rests at the held tick (61), an edit works, a step stops at the breakpoint again in 0.7 ms |
| **A queued `time.step` was told it had stopped, then ran its ticks.** The server answered every waiting `time.step` at a stop, but the loop ended only the front step | step A of 4000 ticks (breakpoint at `g.level[r] === 3000`), step B of 50 ticks 10 ms later: both answered `paused: true, tick: 3001`; after the continue the world stood at 3051 | The game thread answers the step whose tick stopped, from inside the stop: before each tick of a step the loop hands the step's reply to the loop state (`Publisher::hold_step_reply`), `StateHandle::stopped` answers it and then counts the stop, and the loop takes the reply back after the tick (`take_step_reply`; gone: the step ends with the tick). A step queued behind it answers `debug.paused {queued: true}` and runs all its ticks after the resume, as that says | A answers `paused: true, tick: 3000`, B `debug.paused {queued: true}`; held at 2999; after the continue 3050 (`paused_host.rs` `a_stop_answers_only_the_step_it_ends`, and through `pocket serve`) |

What the review called "any continue, then act flow" stays a race by design: a call made while the
game is held is refused at once, which is what keeps agents from hanging (28 calls in the
evaluation), and `debug.continue` cannot promise a boundary before the next stop. Measured on
sailing in Play with a breakpoint in the log system (60 rounds each): `world.edit` sent right after
`debug.continue` over a kept-alive connection landed at the boundary 60 times; over a new
connection per call 15 times, 39 were refused at once and 6 answered `debug.paused {queued: true}`
(sent while running, met by the next stop; they applied after the following continue). The CLI
starts a process per call and almost always loses. The refusal now says how to act between ticks:
clear the breakpoint first, or Stop; the queued Pause is the third way.

Checked by: `cargo test -p pocket-app --test paused_host` (4 tests, 15 repeated runs without a
failure), pocket-runtime's and pocket-server's tests, the editor's end-to-end driver on the real
host, and the review tool above.

## How it works

- pocket-debug records each stop in the loop state with a summary (`Pause::summary`, pocket-link
  `StateHandle::stopped`), so both the game loop and the server see where the game stands without
  asking the hub (threads.md 3.5).
- Before each tick of a `time.step` the game loop hands the step's reply to the loop state; a stop
  inside the tick answers it with the summary as `stopped_by` and the loop ends that step with the
  tick (server.md 3.2), so `debug continue` finishes the tick and the world stands at its end.
- The server's `Host::call` (server.md 3.4): a held game's reads are answered from the latest
  `WorldSnapshot` through pocket-link's `SnapshotView`. The registry's format of each section
  decodes to exactly the JSON the game's own `world.get` builds (checked on all 61 components of
  the sailing scene, and call by call in `paused_host.rs`). A call waiting on the game looks for a
  new stop every 5 ms and answers `debug.paused {queued: true}` at it, unless the game thread
  answered it there (the stopped step); `play.stop` in Play instead has the hub pass over every
  pause until it lands.
- Restores (server.md 3.3) reuse the hot update: `Game::restore_any` puts the snapshot's world and
  bundle back, `Game::swap_bundle` applies the `scripts.swap` Host write to the applied bundle,
  which the recorder records with the bundle, so a replay sees what ran.
- P11 (debugger.md 2; script-sandbox.md's patch table): QuickJS's direct `eval` resolves names
  from a variable index whose `scope_next` chain walks the block's earlier variables and then the
  enclosing blocks'; PR #1421's `JS_EvalInStackFrame` picked the first variable of the function's
  deepest block, wherever the frame stood. While a trace handler is set, the parser now emits a
  temporary `OP_debug_scope` holding `fd->scope_first` (what a direct `eval` at that statement
  would use) before each statement opcode; phase 3 moves it into a per-function (offset, scope)
  table, shifting the offsets when jumps shrink as it does for the line table; `OP_debug` stores
  the frame's PC beside P10's stack pointer; the evaluation looks the frame's statement up there
  and keeps PR #1421's choice where nothing is recorded. Uninstrumented bytecode is unchanged.
  The first attempt put the scope in a `u16` operand of `OP_debug`: the program failed to load
  (`invalid opcode (op=0, pc=14)`), because the compiler's passes size phase 1 and 2 opcodes
  through `opcode_info[op]`, which for the opcodes after the short ones (`OP_debug` is the last) is
  another opcode's entry; a temporary opcode has its own entry and renumbers nothing the bytecode
  keeps, so the precompiled builtins still load (the constraint that shaped P9). A control run with
  the lookup disabled fails the new test where PR #1421 shows a sibling block's `hidden`.

## Not done, and why

- **The write trace** (finding 5) and the smaller fixes of finding 7 other than the two short
  forms: engine readings zero before the first tick, events carrying the world epoch, `null` for
  clearing an entity field, the externally tagged op's decode error, `scripts check` naming whose
  bundle it compiled.
- **Kept snapshots, the history and `project.info` while held** are refused with `debug.paused`:
  they live on the game thread. Publishing the kept ticks in the link would make `snapshots list`
  answer too.
- **Reads while held show the boundary before the stopped tick**: the paused tick's staged writes
  are visible only to `debug.state` and `debug.eval`. Each answer says so (`paused_at` with
  `snapshot_tick`, the CLI's "the world as of tick 57's end").
- **A call sent before a stop** answers `debug.paused {queued: true}` and runs after the resume:
  the second step in `paused_host.rs` covers it for `time.step`; for other calls only the race
  measurement above saw it (6 of 60 edits over new connections). Its answer is not delivered, and
  neither is a queued `time.control`'s (bad parameters included).
- **A step answered at the stop has no samples** and `world_hash: null`: the loop's own answer,
  with them, comes after the resume and nobody waits for it.
- The editor's mock host (`editor/mock/`) does not model any of this, except that its Stop now
  resumes a pause as the host's does; its types (`editor/src/host/protocol.ts`) do.

## Found on the way

- `tools/smoke_server.py` waited for the edit's `history` push after reading the edit's answer, but
  the host pushes the history before it answers, so a fast push was consumed with the answer's
  frames and the wait timed out (seen once in five runs here). It now looks among those frames too.
- `pocket serve` on Windows had no way to be stopped gracefully by another process (SIGTERM is
  POSIX): it now stops on Ctrl-Break, which a parent can send a child started in its own process
  group, and the smoke test runs on Windows.
- `cargo clippy -D warnings` failed on master in pocket-runtime's render-feed extractor (the tick
  code's determinism lists applied to presentation code) and in pocket-app's `main`; both fixed.
- A refused compile (`scripts.apply` while held) first ran the type check's `tsc` lookup, 17 ms of
  nothing; a held refusal now skips it.
- **`debug.wait` waited out its timeout after a `debug.continue` whose next stop came at once.** The
  hook raised the hub's pause count when a stop began and published the pause only after
  capturing its frames, so a wait that came in between read the new count, waited for a later pause
  and answered at its timeout (10 s by default) with the right state. The count now rises with the
  publication; pocket-app's `debug_agent` tests went from 10 s to 0.7 s. The evaluation's agents
  called `debug wait` in one run.
- On this machine pocket-script's `depth.rs` and `web.rs`, expected to fail without a clang that
  has a wasm32 backend, passed (7 and 3 tests): a clang-cl was found by the build script.
- **The editor ignored `world.changed {reset: true}`.** Play, Stop and restores replace the world
  and the host pushes a reset with no lists; the editor's handler read the lists and did nothing,
  so the old world's values stayed until a later diff named them. The end-to-end run caught it
  once Stop took 0.1 s: the inspector still showed the Play world's Log in Edit mode. A reset now
  reads the tree and every entity again (editor.md 6).
- The end-to-end driver (`editor/tools/debug-host.ts`) pressed Cmd+S for the editor's Mod+S, which
  is Ctrl+S off macOS; it does now. It runs under Node 25 with `--experimental-transform-types`
  and a preload defining its three Bun calls, and the editor builds with Vite under Node, from the
  dependencies Bun had installed in the main checkout (linked, nothing installed).

## Quiet re-measurement (2026-10-10)

`python tools/held_host_bench.py` three times (three `pocket serve` sessions of ten rounds each),
with no other agent running, on AC power ([quiet-2026-10-09.md](quiet-2026-10-09.md), session 2),
master `0871920d`'s release build ([held-host-r1.json to
held-host-r3.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/agentdebug)).
Median of the 30 calls (minimum to maximum), ms:

| What | Kept-alive connection | New connection per call (urllib) | CLI, process start included |
|---|---|---|---|
| `time.step` into the breakpoint | 0.64 (0.46 to 15.43) | 11.36 (1.12 to 24.19) | 24.91 (23.35 to 30.04) |
| `status` while held | 0.25 (0.16 to 0.36) | 15.12 (0.52 to 20.24) | 23.82 (22.51 to 29.23) |
| `world.get {fields}` while held | 0.26 (0.17 to 0.39) | 15.38 (0.60 to 23.96) | 24.02 (22.89 to 28.20) |
| `scripts.apply` refused | 0.20 (0.12 to 0.28) | 15.43 (0.50 to 23.27) | 23.87 (22.77 to 28.51) |

- **Kept alive, the held host answers in a quarter to two thirds of a millisecond**: reads 0.25
  ms, the refusal 0.20 ms, the step that stops at the breakpoint 0.64 ms (one call of 30 took a
  timer quantum, 15.4 ms). The review's figures above (0.39 ms and 0.13 to 0.17 ms, one run of 20)
  were of the same order.
- **A new connection per call is bimodal**: 30 to 40% of the calls answered in 0.5 to 4.5 ms and
  the rest in 6 to 24 ms, most of them near the system timer's 15.6 ms quantum, which the
  connection's set-up takes, as the section above suspected. Its medians say which mode won, not
  what the host costs.
- **Through the CLI every call costs 23 to 25 ms** (`pocket status` while running: 23.8 to 24.9),
  against 62 to 75 ms on the loaded machine: the process start, not the host.
- Sizes are unchanged: `world get Sloop` 3,143 B as text and 11,091 B as JSON, two fields 54 B;
  `debug.state` 3,629 B, `{brief}` 677 B, the CLI's text 306 B.
