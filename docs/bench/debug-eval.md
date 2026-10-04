# Agent-native debugging: an agent finds a planted defect

Charter 6, the debugging row: "an agent locates a planted defect". An LLM agent gets a player's
description of a symptom, nothing about the code, and works through the host's CLI only (MCP in
three runs). It has to find the defect in the game's TypeScript, fix it through the host, and the
fix is then checked objectively on a fresh host.

- Date: 2026-10-04. Apple M5 (Mac17,2), macOS 27.0.1, with other agents' builds and games running
  (load average 40 to 60), so wall times are loose.
- Host: `pocket` release build of main `1e5b94f` (`cargo build --release -p pocket-app`).
- Agent: opencode 1.18.2, model `zai-coding-plan/glm-5.3-flash` (the owner's runner), driven by
  `opencode run --format json --auto` with stdin closed.
- Harness: [tools/eval/debug_eval.py](../../tools/eval/debug_eval.py) (standard library); every
  run's prompt, raw event stream, readable transcript, diffs and result are in
  [docs/evidence/debug-eval/](../evidence/debug-eval/), all runs in
  [summary.json](../evidence/debug-eval/summary.json).

## Method

**The game.** `samples/sailing` copied to a temporary directory, plus the evaluation's helm layer
([tools/eval/debug_eval/scripts/](../../tools/eval/debug_eval/scripts/)): a `Helm` component on the
Sloop (`steer` -1..1, `sail`, `anchor`, `goto` an entity) and a `helm` system that turns it into the
engine's `Boat` controls (rudder from the wheel or from a pursuit autopilot, hoist, sheet trimmed to
the apparent wind). The sample's `rules.ts` (muster, log, take_aboard) is unchanged. The layer
exists because the sample's boat physics is Rust (pocket-physics); a TypeScript defect needs
TypeScript that steers. The samples themselves are not modified.

**The bugs.** One per run, each a small mistake that leaves the game running and wrong:

| Bug | Where | Mistake | What the player is told |
|---|---|---|---|
| `steer-sign` | helm.ts:54 | `rudder = -clamp(steer) * GAIN`: a stray minus | the wheel turns the boat the wrong way |
| `goto-radians` | helm.ts:66 | `Math.atan2(dx, -dz)` compared with `heading_deg` without `* 180 / Math.PI` | steering for a crate, the boat sails off elsewhere, whichever crate |
| `reach-squared` | rules.ts:57-61 | squared distance compared with `REACH` (3 m): the reach is 1.7 m | the crew calls crates two or three metres away out of reach |
| `anchor-stale` | helm.ts:71-78 | `b.hoist[r] = wanted`, the value read before the anchor rule set `sail = 0` | at anchor the boat never slows down |
| `tally-off-by-one` | rules.ts:63 | `left` counted before `taken` is incremented | after the last crate, one is still left and "all aboard" never comes |
| `goto-wrap` | helm.ts:28 | `angleDiff` wraps above 180 but not below -180 | steering for a crate behind to starboard, the helmsman turns the long way |

**The agent's sandbox.** A project-level `opencode.json` in an empty scratch directory (the user's
opencode configuration untouched): bash may run only `pocket ...` and
`grep head tail wc sort jq echo cat sleep`; file tools reach only the scratch directory; no web, no
subagents. The game's files are therefore reachable only through `pocket scripts read/write/apply`.
`POCKET_HOST` points the CLI at the host. The prompt
([example](../evidence/debug-eval/goto-radians/cli-1/prompt.txt)) describes the game and its
controls as a player knows them, quotes the symptom, says that `pocket help` lists the host's
inspection, time, event, debugger and script commands, and asks for a fix in the code (not in the
scene or the rules' constants), confirmed in the running game, and a final JSON line naming the
cause, file and line. Budget: 60 tool calls, 15 minutes.

Three conditions: **free/cli** (the agent chooses its tools), **debugger/cli** (the prompt adds:
before changing code, confirm the cause in the debugger with a breakpoint or a watch and say what it
showed), **free/mcp** (the host's MCP endpoint is also attached as opencode tools).

**The check** (on a fresh `pocket serve` of a fresh copy whose `scripts/` are the agent's final
files, so world edits and a still-running old bundle do not count), through the CLI:

| Check | Pass |
|---|---|
| steer | from tick 0, 1.5 s of wheel -1 ends left of wheel 0 by more than 10 degrees, wheel 1 right of it by more than 10 |
| goto | `goto=Crate4`: the boat comes within 3 m of it in 15 s |
| goto_behind | a crate at bearing 225 (behind, to starboard) with the bow at 90: after 2 s the heading has turned more than 15 degrees to starboard |
| reach | crates at 2.5 m and 1.6 m are taken, one at 3.6 m is refused |
| anchor | 4 s under sail, then anchored: 8 to 10 s later the sail is down and the mean speed below 1.5 m/s |
| tally | four crates taken one by one: `left` 3, 2, 1, 0 and one `crates.all` |

**Fixed** means all six pass (the bug's own and no regression). **Located** means the agent's final
JSON names the planted file and a line within two of the planted lines. As a second, stricter
measure the harness compares the 720-tick hash chain of a scenario that exercises every feature
(`pocket hashes --inputs`) with the clean game's. `--selftest` shows the clean game passing every
check and each planted bug failing its own ([selftest.txt](../evidence/debug-eval/selftest.txt);
`goto-radians` also fails `goto_behind`).

## Results

23 runs: 21 fixed, 21 located; every fix is bit identical to the clean game over the scenario.

| Condition | Runs | Fixed | Median agent time | Median tool calls (`pocket` invocations) | Median tokens (93 % cached) | Used the debugger |
|---|---|---|---|---|---|---|
| free/cli | 13 | 11 | 444 s | 30 (61) | 634 k | 2 |
| debugger/cli | 7 | 7 | 477 s | 31 (47) | 404 k | 7 |
| free/mcp | 3 | 3 | 453 s | 32 (45) | 492 k | 2 |

By bug, free/cli: `steer-sign` 2/2, `goto-radians` 1/3, `reach-squared` 2/2, `anchor-stale` 2/2,
`tally-off-by-one` 2/2, `goto-wrap` 2/2. `goto-radians` with the debugger required: 2/2; over MCP:
1/1.

| Run | Bug | Fixed | Located | Hash = clean | Tool calls | `pocket` calls | Agent s | Tokens k (cached) | Debugger | Stopped by |
|---|---|---|---|---|---|---|---|---|---|---|
| [steer-sign/cli-debugger-1](../evidence/debug-eval/steer-sign/cli-debugger-1/transcript.md) | steer-sign | yes | yes | yes | 23 | 43 | 313 | 315 (287) | yes | - |
| [goto-radians/cli-debugger-1](../evidence/debug-eval/goto-radians/cli-debugger-1/transcript.md) | goto-radians | yes | yes | yes | 42 | 83 | 784 | 906 (863) | yes | - |
| [goto-radians/cli-debugger-2](../evidence/debug-eval/goto-radians/cli-debugger-2/transcript.md) | goto-radians | yes | yes | yes | 23 | 38 | 310 | 287 (260) | yes | - |
| [reach-squared/cli-debugger-1](../evidence/debug-eval/reach-squared/cli-debugger-1/transcript.md) | reach-squared | yes | yes | yes | 22 | 47 | 362 | 287 (261) | yes | - |
| [anchor-stale/cli-debugger-1](../evidence/debug-eval/anchor-stale/cli-debugger-1/transcript.md) | anchor-stale | yes | yes | yes | 31 | 44 | 477 | 404 (372) | yes | - |
| [tally-off-by-one/cli-debugger-1](../evidence/debug-eval/tally-off-by-one/cli-debugger-1/transcript.md) | tally-off-by-one | yes | yes | yes | 46 | 114 | 774 | 875 (831) | yes | - |
| [goto-wrap/cli-debugger-1](../evidence/debug-eval/goto-wrap/cli-debugger-1/transcript.md) | goto-wrap | yes | yes | yes | 42 | 106 | 759 | 973 (920) | yes | - |
| [steer-sign/cli-1](../evidence/debug-eval/steer-sign/cli-1/transcript.md) | steer-sign | yes | yes | yes | 40 | 76 | 511 | 635 (583) | - | - |
| [steer-sign/cli-2](../evidence/debug-eval/steer-sign/cli-2/transcript.md) | steer-sign | yes | yes | yes | 49 | 121 | 855 | 1361 (1277) | - | - |
| [goto-radians/cli-1](../evidence/debug-eval/goto-radians/cli-1/transcript.md) | goto-radians | NO | no | no | 53 | 81 | 901 | 917 (875) | yes | time budget |
| [goto-radians/cli-2](../evidence/debug-eval/goto-radians/cli-2/transcript.md) | goto-radians | NO | no | no | 48 | 124 | 900 | 758 (692) | yes | time budget |
| [goto-radians/cli-3](../evidence/debug-eval/goto-radians/cli-3/transcript.md) | goto-radians | yes | yes | yes | 16 | 35 | 144 | 188 (163) | - | - |
| [reach-squared/cli-1](../evidence/debug-eval/reach-squared/cli-1/transcript.md) | reach-squared | yes | yes | yes | 15 | 54 | 192 | 160 (133) | - | - |
| [reach-squared/cli-2](../evidence/debug-eval/reach-squared/cli-2/transcript.md) | reach-squared | yes | yes | yes | 16 | 21 | 121 | 156 (138) | - | - |
| [anchor-stale/cli-1](../evidence/debug-eval/anchor-stale/cli-1/transcript.md) | anchor-stale | yes | yes | yes | 14 | 21 | 134 | 152 (126) | - | - |
| [anchor-stale/cli-2](../evidence/debug-eval/anchor-stale/cli-2/transcript.md) | anchor-stale | yes | yes | yes | 15 | 25 | 146 | 159 (142) | - | - |
| [tally-off-by-one/cli-1](../evidence/debug-eval/tally-off-by-one/cli-1/transcript.md) | tally-off-by-one | yes | yes | yes | 47 | 146 | 478 | 842 (802) | - | - |
| [tally-off-by-one/cli-2](../evidence/debug-eval/tally-off-by-one/cli-2/transcript.md) | tally-off-by-one | yes | yes | yes | 50 | 125 | 472 | 818 (780) | - | - |
| [goto-wrap/cli-1](../evidence/debug-eval/goto-wrap/cli-1/transcript.md) | goto-wrap | yes | yes | yes | 30 | 61 | 444 | 660 (610) | - | - |
| [goto-wrap/cli-2](../evidence/debug-eval/goto-wrap/cli-2/transcript.md) | goto-wrap | yes | yes | yes | 28 | 54 | 362 | 566 (521) | - | - |
| [goto-radians/mcp-1](../evidence/debug-eval/goto-radians/mcp-1/transcript.md) | goto-radians | yes | yes | yes | 32 | 37 | 535 | 578 (530) | yes | - |
| [anchor-stale/mcp-1](../evidence/debug-eval/anchor-stale/mcp-1/transcript.md) | anchor-stale | yes | yes | yes | 32 | 55 | 453 | 492 (450) | yes | - |
| [goto-wrap/mcp-1](../evidence/debug-eval/goto-wrap/mcp-1/transcript.md) | goto-wrap | yes | yes | yes | 20 | 45 | 331 | 293 (255) | - | - |

"Tool calls" are the agent's tool invocations; one shell line often holds several `pocket` commands.
`python3 tools/eval/debug_eval.py --report` rebuilds this table and the tallies below from the
recorded transcripts.

## What the agents did

**They read the code first.** Every run began `pocket help`, `pocket scripts list`,
`pocket scripts read` of every module. In at least 16 of the 23 runs the agent named the defect from
the source alone before its first `pocket step` (a keyword match on its messages); the steps that
followed reproduced the symptom and verified the fix (`world set` the player's control, `step N`,
`world get`). The four-file game is small enough to read whole, so for `steer-sign`,
`reach-squared`, `anchor-stale` and `tally-off-by-one` the debugger added confirmation, not
discovery.

**The debugger decided the hard case.** `goto-radians` is the one bug the model often misread: in
two of three free runs it judged the `atan2` line "plausible", reproduced the symptom (the rudder
pinned at -1), and turned to the debugger on its own. Both runs then ran out of time on debugger
friction (below); `goto-radians/cli-2` found the cause at 867 s of 900. When the debugger worked, it
was the evidence: the paused frame's locals in `pocket debug state` showed `bearing = 1.344` where a
heading in degrees belonged
([cli-debugger-2](../evidence/debug-eval/goto-radians/cli-debugger-2/transcript.md) at 212 s,
[mcp-1](../evidence/debug-eval/goto-radians/mcp-1/transcript.md) at 328 s), and both fixed it. A
data watch did what it is for in
[tally-off-by-one/cli-debugger-1](../evidence/debug-eval/tally-off-by-one/cli-debugger-1/transcript.md)
(`debug.watch` on `Tally.taken` stopped at the increment, rules.ts:66).

**Commands used** (runs using it, of 23; the CLI's short forms, `pocket call` and MCP counted
together): `scripts list`, `scripts read`, `world get`, `world set`/`edit` and `step` 23 each,
`scripts apply` and `scripts write` 21 (always a whole file, from a heredoc or a file in the scratch
directory), `world tree` 18, `status` 17, `world query` 15, `world schema` 12, `snapshots restore`
11, `events` 11, `scripts.status` 9; `step --watch`/`--until` stopped on a field change in several
runs. Debugger methods in 11 runs: `breakpoints.set`, `state`, `continue` and `breakpoints.clear` in
all 11, `eval` in 10, `step` and `watch` in 4, `pause` in 3, `rewind`, `wait` and `unwatch` in 1. Of
the 1,556 `pocket` invocations, 590 (38 %) were `step`, `world get` and `world query`.
`events --why`, `logs` and `history` were hardly used: no bug produced an event chain worth
following.

**What worked.** Refusals that name the accepted fields
(`request.unknown_field: ... it takes condition, file, line, log`) let agents recover in one call,
and agents used that deliberately to learn undocumented parameters
(`pocket call debug.breakpoints.set '{"help":true}'`). The world hash printed by `step` let two
agents see that a replay after their fix was bit identical to the one before it, i.e. the fix was
not running; the bundle hash of `pocket call scripts.status` is how most of the others found out.
`step --watch <e>.<C>.<f>` replaced step-and-get loops where agents knew it.

## Where they got stuck

Measured over all 23 runs (10,758 s of agent time):

| Problem | Runs | Cost |
|---|---|---|
| A call blocked while the game stood at a breakpoint, until the agent's tool gave up (15 to 120 s each) | 11 | 28 calls, 2,238 s (21 % of all agent time) |
| `snapshots restore` after `scripts apply` put the old scripts back without saying so | 11 | 8 runs noticed and applied again (994 s from the restore); 3 ended with the host running the old code while their final message said the fix was live |
| `debug.eval` refused a local that `debug state` lists (`ReferenceError: bearing is not defined`) | 7 | 37 failed evaluations |
| Debugger parameters guessed by provoking refusals (`pocket help debug.eval`: "no command or method") | 10 | 34 `request.*` refusals on `debug.*` calls |
| Field-level reads refused (`world get Sloop Boat.heading_deg`, `Boat,Transform`), whole components piped through grep/jq instead | 11 | 12 `sim.component_unknown`; median 29 k characters of tool output per run |

Details, with the evidence:

1. **A breakpoint stalls the agent's own tools.** `pocket step 1` with a breakpoint set does not
   return: it waits for the resume. While the game stands, every command that goes through the game
   thread waits too: `status`, `world get/query/tree`, `scripts list/read`, `snapshots list`,
   `history`, even `catalog.list`; only `events`, `logs` and `debug.*` answer
   ([paused-host.txt](../evidence/debug-eval/paused-host.txt)). debugger.md 8 says "status and
   snapshots stay readable"; status does not. The CLI's HTTP client has no read timeout, so the
   agent's shell hangs until opencode kills it; over MCP the same `time step` failed with
   `MCP error -32001: Request timed out` after 60 s. Every debugger-variant run hit this at least
   once. Agents learned to write `pocket step 1 & sleep 2; pocket debug state`, which the sandbox
   then could not clean up (`kill` was not allowed), and a `scripts apply` waited behind a
   breakpoint that re-fired every tick
   ([goto-radians/cli-debugger-1](../evidence/debug-eval/goto-radians/cli-debugger-1/transcript.md),
   673 s).
2. **Restoring a snapshot silently reverts the scripts.** `snapshots.restore` switches back to the
   bundle the snapshot was kept with (server.md 3.3). The natural verification loop, fix,
   `scripts apply`, `snapshots restore 0`, replay the symptom, therefore replays the old code.
   Agents read the result as "my fix did not work", re-read the file, forced re-applies, tried Play
   ([steer-sign/cli-2](../evidence/debug-eval/steer-sign/cli-2/transcript.md) spent 275 s before
   finding it through `scripts.status`), and three agents ended their run on a restore and reported
   the fix live when the host was running the bug again. `scripts check` printing the bundle of the
   files on disk added to it: one agent took that hash for the running one.
3. **`debug.eval` cannot see most block-scoped locals.** At helm.ts:67 `debug state` lists `dx`,
   `dz` and `bearing`; `eval` sees `dx` and not the other two
   ([eval-block-scope.txt](../evidence/debug-eval/eval-block-scope.txt)). The cause is in
   `JS_EvalInStackFrame` (PR #1421, vendored QuickJS-ng): it starts the eval's scope chain from the
   first local of the deepest block and walks `scope_next`, which only reaches variables declared
   before that one. Agents then fell back to the locals listing, or to re-typing the expression with
   literals.
4. **The debugger is not described where agents look.** The catalog has one entry for the whole
   group (`debug.state`, "no parameters"), so `pocket help debug.eval`, `debug.watch`,
   `debug.breakpoints.set` answer "no command or method". The MCP `debug` tool's input schema has
   only `action`; GLM called it with `{"action": "breakpoints.set"}` and moved to the CLI.
   `debug.watch` takes an entity id only (`request.wrong_type: 'entity' must be integer`) where
   every other command takes names; there is `debug.unwatch` but agents asked for
   `debug.watch.clear`.
5. **Reads come whole.** `world get Sloop` prints 11 KB (the Floater's 70 buoyancy points, the
   collider hulls), `debug state` 9.5 KB over 412 lines (each frame's closure, `ctx` included), and
   there is no way to ask for `Boat.heading_deg` alone with `world get`; agents grepped JSON
   (`grep -o '"heading_deg":[^,]*'`) and stumbled over the `--json` shape (`.components.Boat`, and
   `pocket --json world get` refused). `pocket scripts read` prints no line numbers, so agents set
   breakpoints on lines counted by hand (helm.ts:36 for line 54 in
   [steer-sign/cli-debugger-1](../evidence/debug-eval/steer-sign/cli-debugger-1/transcript.md)).
6. **Smaller traps.** Engine readings are zero until the first tick (`heading_deg 0` at tick 0), so
   three agents read "0 to 86 degrees" as a turn; the boat's weather helm turned it to port with the
   wheel centred, which confounded heading tests until agents furled the sail or used the autopilot
   as a reference. After a restore, `events` mixes the old and the new timeline (`t=1545` then
   `t=1543`), and `--since` is exclusive where agents expected inclusive. Clearing an entity field
   by `goto=0` is refused with `sim.entity_id_invalid: 0 is not an entity id ...; the next is 0` (a
   wrong "next"), and `{"set": {..., "fields": ...}}` in `world edit` is refused as "ops[0] has no
   field 'set'; it takes spawn". `pocket scripts status` and `pocket docs` have no short form
   (`pocket call scripts.status`, `pocket call docs.search`).

## What the engine should add, ranked

Ranked by the time and correctness it cost in these runs.

1. **Never block an agent on a paused game.** (a) `time.step` returns when a script breakpoint or
   watch stops the game:
   `{tick, stopped_by: {reason: "breakpoint" | "data_breakpoint", system, location, watch?}}`, the
   rest of the step dropped or resumed by `debug continue`; the CLI prints
   `tick 6 | stopped at scripts/helm.ts:67 (bp1): pocket debug state`. (b) Reads answer while
   paused: `status` from the snapshot reader (as debugger.md 8 promises), `scripts read/list`,
   `catalog`, `docs.search` from files, and `world get/query/tree` from the last published snapshot,
   marked `paused_at`. (c) The CLI gets a default request timeout, and a call that would wait on the
   game thread while it is paused is refused at once with `debug.paused {location}` instead of
   hanging. This alone was 21 % of the agents' time.
2. **A restore keeps the scripts that are applied.** Programs hold no state (charter 3.2), so
   `snapshots.restore` and `debug.rewind` can load the snapshot's world under the current bundle by
   default and switch bundles only on request (`{tick, bundle: "snapshot"}`); whichever it does, the
   answer and the status line name the bundle (`restored tick 0; scripts 4381c2 (applied)`), and
   `status` always shows the running bundle. Today the most natural verification loop silently tests
   the old code, and three agents reported a fix live on a host running the bug.
3. **Fix `debug.eval` scoping** (a P11 for the vendored QuickJS-ng): resolve names in the block that
   holds the paused statement, from its last declared variable, using a pc-to-scope table built with
   P9's per-function statement table. Until then, `debug.eval` could fall back to the frame's locals
   listing for a bare identifier.
4. **Describe the debugger like every other command.** Catalog entries with JSON Schemas for each
   `debug.*` method (`pocket_debug::methods()` already has them; server.md 6 defers this "until the
   hub contributes its own entries"), so `pocket help debug.watch` and the MCP `debug` tool's
   `inputSchema` list `file, line, condition, log, expr, frame, kind, entity, component, field, id`;
   entity names accepted by `debug.watch`; short forms `pocket debug break scripts/helm.ts:67 [--if
   <expr>]`, `pocket debug eval <expr>`, `pocket debug watch Sloop.Boat.hoist`; and a compact text
form of `debug state`: the location, then one line per local (`bearing = 1.344`), closures and `ctx`
only on request.
5. **A write trace: "why did this field change".** A non-pausing form of the data watch:
   `pocket debug writes Sloop.Boat.rudder --ticks 60` listing each tick on which a script's staged
   write changed the field (system, file:line, before and after), from the probe data watches
   already run (debugger.md 6, `written_at`), together with the systems whose queries can write that
   column. In `steer-sign` it would answer `tick 1 helm scripts/helm.ts:76 rudder 0 -> 0.6` right
   after `steer=-1`; in `anchor-stale`, that no write to `hoist` follows the anchor and `helm` is
   its only writer. Reporting instead of stopping also avoids item 1. Events already have this
   (`events --why`).
6. **Field-level, compact reads.** `world get Sloop Boat.heading_deg,Boat.rudder` (paths and comma
   lists, as `world query --fields` takes); engine-internal bulk (Floater points, collider hulls)
   left out of `world get <entity>` unless named; `pocket scripts read <path> --lines 50-80` with
   line numbers; and a sampler,
   `pocket step 300 --sample Sloop.Boat.heading_deg,Sloop.Boat.rudder --every 30`, which returns a
   table and replaces the step-and-get loops behind 38 % of the `pocket` calls.
7. **Smaller fixes.** Engine readings written at load (or marked unmeasured) rather than zero before
   the first tick; events carrying the world epoch, with `events` defaulting to the current one
   after a restore; `null` documented (and the message fixed) for clearing an entity field; a decode
   error for an externally tagged op that names the inner field; `pocket scripts status` and
   `pocket docs <query>` short forms; `scripts check` saying that its bundle is the files', not the
   running one.

## Limits

- One model (GLM 5.3 Flash) and a game small enough to read whole; with four short modules, code
  reading finds most single-line defects without running anything. A larger project, or defects that
  need runtime values (a wrong value in data, an order-of-systems effect, a rare branch), would
  weigh the debugger more; the harness takes new bugs as `Bug(file, old, new, symptom, check)`.
- Two to three trials per bug at most; the success rates are indicative. Wall times include other
  agents' load on the machine and the model's queueing.
- The debugger variant tells the agent to use the debugger, so it measures the debugger's usability,
  not whether an agent would choose it.
- The sandbox forbids shell tools beyond the listed filters (no `sed`, `python3`, `kill`), which
  cost a few calls in some runs (`sed -n` for line ranges, killing a hung background step) and is
  stricter than an agent in a developer's checkout would be.
