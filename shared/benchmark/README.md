# The shared benchmark

Status: Draft, slice 0. Proposed for PocketEngine's `shared/benchmark/` (charter 6.2); not yet
synced, so no PocketEngine commit is recorded.

Charter: 2.2 (two agent roles), 3.1 (perception, the omniscient view for checkers), 6.1 item 2 (a
shared benchmark), 6.3 (what is compared), 8 (material brought from `master`), 9 (evaluation), 10
(slice 0: the harness skeleton; slice 2: the first player benchmark), 12 item 7 (sandboxing agents).

The benchmark turns charter 9 into numbers. It has two suites, run by one harness against either
engine through the MCP tools of [the shared contract](../contract/mcp.md):

- **The developer suite**: an agent changes or builds a game through the developer tools and the
  project's files, from "edit through a command" to "build a whole game from a brief". It ports
  `master`'s 72 tasks and harness (master `tools/scripts/agent_eval.py`, `docs/agent-eval.md`). The
  same model's pass rate, calls and tokens on both lines compare TypeScript with Lua as the language
  agents write (charter 6.3).
- **The player suite**: an agent plays the sailing game through the player tools with limited
  perception. It measures completion, decision calls and tokens, the difference fork lookahead
  makes, and the difference between perception and the omniscient view (charter 9, item 2).

[tasks.md](tasks.md) lists the tasks, the fixtures they run on and how each of `master`'s tasks
ports. This file covers the harness, the runners, the metrics, the statistics, the reports and the
skeleton slice 0 builds.

## 1. Principles

1. **Checkers are code that reads the omniscient world.** A task passes or fails on what the world
   holds and does, read through a checker's session (a developer's, where the check must play the
   game) that the agent never sees (charter 9: "checkers use the omniscient view; player agents
   cannot"). Nothing is scored by a model or on style.
2. **One harness, two engines.** The harness talks to an engine only through the shared MCP tools
   and three commands each line provides (section 3.2). A task's text, checker, reference solution
   and fixture description are shared; each line implements the fixtures in its own scene format and
   script language. The per-line parts are named: the fixtures, the script solutions under
   `solution/<line>/`, and, for the few developer tasks whose checker must read an engine component
   (`sailboat`, `raft`, `calm_sea`, `sinking_crate`: a collider, the water's fields, the wind), a
   `check_<line>.py` beside the shared `task.py`, reviewed with the task. Every other checker reads
   only contract types (3.5).
3. **Every task is checked both ways before it counts.** Its reference solution passes and the null
   runner fails, on both lines (master `docs/agent-eval.md`, Adding a task).
4. **Every number is measured and repeated.** Tokens and cost come from the agent's own accounting;
   calls and answer sizes from a proxy on the MCP connection; results from several runs with their
   mean and variance (charter 9, item 1). `master` ran each configuration once and saw runs of the
   same set differ by up to twice (master `docs/agent-eval.md`, After the agent tools).
5. **The agent cannot reach the answers.** Tasks run outside the repository on copies whose names
   say nothing; touching the harness is recorded; agents run sandboxed (section 3.7).

## 2. Layout

```text
shared/benchmark/
  README.md, tasks.md     this file and the task catalogue
  harness/                Python 3.12 or later, standard library only; every file under 800 lines
    bench.py              the command line: run, self-test, compare, list
    engine.py             the engine adapter: reads engines/<line>.toml, copies fixtures, serves
    mcp_client.py         a small MCP client (JSON-RPC over HTTP and stdio) for checkers
    proxy.py              the recording MCP proxy between an agent and the engine
    runners.py            reference, null, random, and external commands
    stats.py              means, variances, Wilson intervals, paired differences
    report.py             rows, the report, its JSON Schema
    fake_engine.py        a stand-in engine for the self-test (section 8)
  engines/
    pocket3d.toml         how this line starts, serves and attaches (section 3.2)
    pocketengine.toml
  tasks/
    dev/<task>/           task.py (text, setup, solve, check); solution/<line>/ for script tasks
    play/<task>/          task.py; the reference policy
    selftest/<task>/      tasks for the fake engine
  agents/                 runner adapters for coding agents: opencode, claude, codex, omp
```

`master` kept its 72 tasks, solutions, checks and harness in one file of 4,384 lines; one directory
per task keeps every file under the limit (charter 3.9) and lets each line add its own solution
files beside the shared ones.

## 3. The harness

### 3.1 Language

The harness is Python, standard library only. `master`'s harness and its agent runners are Python
and port with the least change; the runners wrap agent command lines, which is shell work; the
harness must be one program for two Rust engines and sits on neither engine's build or run path.
Python 3.12 is the floor: the harness needs nothing newer, and the reference machine runs 3.14.3.

### 3.2 The engine adapter

Each line provides three commands, named in `engines/<line>.toml`:

```toml
name = "pocket3d"
script_language = "typescript"      # what the line's agents write
entry = "scripts/main.ts"           # the entry script a brief calls {entry}
binary = "{root}/target/release/pocket{exe}"   # frozen per run: copied before the run starts
# A fresh project from a fixture (tasks.md, Fixtures) or empty, in a directory of its own; the
# fixture's world parameters for this seed (tasks.md 6.1) go in with --params.
new = ["{binary}", "new", "{dir}", "--from", "{fixture}", "--params", "{params_file}"]
new_blank = ["{binary}", "new", "{dir}"]
# Serve a project paused on 127.0.0.1 with the given grants and the task's tick limit; prints
# {"url": ...} as its first line on stdout and runs until stdin closes.
serve = ["{binary}", "serve", "{dir}", "--paused", "--seed", "{seed}", "--tick-limit", "{ticks}",
         "--listen", "127.0.0.1:0", "--grants", "{grants_file}"]
# One stdio MCP session bound to a grant of a served runtime (mcp.md, section 10).
attach = ["{binary}", "mcp", "--attach", "{url}", "--grant-token", "{token}"]
docs = ["docs/agent/*.md"]          # the documentation set handed to agents (dev suite)
```

The grants file holds the grants of `mcp.md` (3.1) with their tokens. For a developer task: the
agent's developer grant and the checker's, also a developer grant because the checker plays the
result (holds actions, moves things, steps). For a player task: the agent's player grant (its seat,
`clock: true`, `max_wall_ms: 600000`, the fork policy and, in the omniscient condition, `profile`
set to `omniscient_player`, as the contract's README prescribes for this ablation), a developer
grant for setup that the harness closes before the agent starts, and the checker's `checker` grant,
which reads and never acts or steps. The harness writes the file, so the agent never sees the
checker's token. Until slice 2 gives the engine these commands, `engines/pocket3d.toml` holds them
as written above with `status = "planned"`, and the harness refuses to run against it.

### 3.3 A task's run

1. **Copy.** The fixture is copied with `new` into a directory of the run's own under the system's
   temporary directory, named `game-<pid>-<n>` (a name that says nothing of the task: `master`'s
   agents found checks by searching for a task's name, master `docs/agent-eval.md`, Forty-five).
   Developer tasks get the documentation set and an index of it beside the copy.
2. **Setup.** A task's `setup(project_dir, line)` writes what the task needs into the copy (a broken
   game for a diagnose task), from the task directory's `solution/<line>/` or `setup/<line>/` files.
3. **Serve.** `serve` starts the runtime with the seed of this repeat, the task's tick limit and the
   grants. The harness's checker session attaches over HTTP. For a player task the setup developer
   session then calls `step {ticks: 1}`, so the world the fixture parameters made has been perceived
   once, and closes; the episode's `Start` decision point and `sim_ticks` count from that boundary
   (tasks.md 6.1).
4. **Before.** `before(env)` runs through the checker session and must make the task real (enemies
   to clear, a hit to explain); a setup that fails it is a harness error, not a failed task.
5. **Agent.** The runner gets the task (section 3.6). Its MCP server is `proxy.py` wrapped around
   `attach` with the agent's grant token.
6. **Follow-ups.** A task's later requests go to fresh runner invocations on the same project, as
   `master`'s `followups` did.
7. **Load.** For a task that edits files, the checker's developer session calls
   `apply {restart: true}` (`mcp.md`, 6.2): a fresh world from the scene with the agent's scripts,
   reseeded and with held inputs released, as a fresh run would start (`master`'s `dodge` and
   `coin_chime` failed on state a reload kept: master `docs/agent-eval.md`, Thirty-nine and
   Forty-five).
8. **Check.** `check(env, answer)` returns `(ok, score, detail)` through the checker session. First
   the harness reads `session {all: true}`: a world that halted on a script error (time.md, Halts)
   is the engine's failure, not the agent's, so the row is marked `error: "engine_halt"`, excluded
   from pass rates and reported.
9. **Teardown.** The runtime is stopped and the copy removed; the row is appended to the rows file
   at once, so a run cut short keeps what it did (master `--rows`).

A task module is plain Python:

```python
TASK = {
    "name": "sailboat", "suite": "dev", "fixture": "blank", "ticks": 0, "tier": "S2",
    "text": "...",                         # the brief; {entry} and {lang} are filled per line
    "answer": "null",                      # what the agent answers: a JSON value's description
    "limits": {"wall_s": 600, "calls": 200},
}
def solve(env, ctx): ...                  # the reference solution, through env.call(tool, params)
def check(env, answer): ...               # -> (ok: bool, score: float, detail: str)
```

`env.call` is the checker session: `env.call("world_get", {"entities": ["Sloop"]})`. Script tasks'
reference solutions are files under `solution/<line>/` copied into the project before `apply`, one
per script language.

### 3.4 The recording proxy

`proxy.py --log <calls.jsonl> --allow <tools> -- <attach command>` is the agent's MCP server. It
relays newline-delimited JSON-RPC both ways unchanged and writes one line per `tools/call`:

```json
{"t": 12.41, "tool": "observe", "args_bytes": 61, "result_bytes": 2210, "text_tokens": 553,
 "is_error": false, "code": null, "ms": 38}
```

`text_tokens` is `mcp.md`'s token unit over the result's text. Because the proxy is shared code on
the agent's side of the connection, calls and answer sizes are counted the same way for every agent
and on both lines. The allowlist filters `tools/list` and refuses other calls as the engine would
(`session.policy_denies` for a tool the condition withholds, `permission.denied` for one the role
lacks): a second guard behind the engine's roles, and the way a condition withholds `fork` from the
tool list. The proxy enforces the task's call limit too: a call past `limits.calls` is answered with
`isError` and `session.limit_reached {what: "calls"}` and never reaches the engine. A call the
engine answers with a JSON-RPC error (an unknown tool, a malformed request) is logged as a failed
call with `error.data.code` as its `code`, so `failed_calls` counts it like an `isError` result.

### 3.5 Checkers

- A checker uses only shared MCP tools and reads only contract types, never an engine component's
  JSON form, which differs between the lines. A player task's checker has the `checker` role:
  `session {all: true}`; `observe`, `nearby` and `events` with `omniscient: true`, whose percepts
  always carry `pos_m` (perception.md, The omniscient view) and whose events carry spec-sim's
  `EventSeq`; `intents`; `why`; `replay`; and `world_get` of the fixture's own project components
  (`Course`), whose fields tasks.md fixes for both lines. A developer task's checker has a developer
  grant and adds `act`, `step`, `world_edit`, `apply` and `reset`; where it must read an engine
  component, the task's `check_<line>.py` does (1, item 2). These tools are in slice 2's rollout
  (`mcp.md`, 12).
- A checker measures what its own run makes: it reads events after a cursor it takes when the check
  starts, not the log the agent left (`master`'s `wolves` checker counted the agent's own tries:
  master `docs/agent-eval.md`, Sea, grass, a sailboat).
- A tolerance a checker applies is stated in the brief (`master`'s `key_door` failed an agent on a
  door width the brief never gave: Five games again).
- A checker that plays a game (holds actions, moves things with `world_edit`) does so after the
  agent has finished; the brief says what the game must read from the world for that to work, as
  `master`'s briefs came to say that games read positions from their Transforms (Whole games).

### 3.6 Runners

| Runner | What it is |
|---|---|
| `reference` | The task's `solve` through the checker session: the harness checking itself. |
| `null` | Does nothing: the floor. |
| `random` | Player suite: at each decision point, a seeded random choice among the seat's intents and controls (`describe`), with seeded random arguments in their schemas' ranges. A floor above `null`. |
| `reference-omniscient` | Player suite: the reference policy under the omniscient view. |
| any other string | A shell command run once per task, the protocol below. |

The protocol of an external runner (version 1): the task arrives as JSON on stdin and the answer
leaves as the last JSON line on stdout.

```json
{"protocol": 1, "suite": "play", "name": "beat_to_windward", "line": "pocket3d",
 "task": "...", "answer": "null", "project_dir": "/tmp/pocket-bench-x/game-412-07",
 "mcp": {"command": ["python", ".../proxy.py", "--log", "...", "--", "..."], "env": {}},
 "docs": [".../docs/INDEX.md"], "limits": {"wall_s": 900, "calls": 300}}
```

```json
{"answer": null, "metrics": {"agent": "claude-code", "agent_version": "...", "model": "...",
 "tokens": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "total": 0},
 "cost_usd": 0.0, "turns": 0, "tool_calls": 0, "tools": {}, "mcp_protocol": "2025-11-25"}}
```

- The runner registers `mcp.command` as an MCP server named `pocket` with its agent, starts the
  agent in `project_dir` (developer suite) or an empty directory (player suite), gives it the task
  text and the docs (developer suite only), and reports the agent's own token counts. In the player
  suite the agent gets the MCP tools and nothing else: Claude Code runs with
  `--allowedTools "mcp__pocket__*"` and its file, shell and web tools disallowed, and the Codex,
  opencode and omp runners use their equivalents, since an agent that can read files could read
  tasks.md's seed formulas and recompute where the crates are. A trace that shows any call other
  than an MCP tool fails the row as `peeked`. Cache reads are reported apart: over nine tenths of
  `master`'s tokens were cache reads of the conversation so far (master `docs/agent-eval.md`,
  Context is the bill).
- The harness owns the wall limit: it kills the runner at `limits.wall_s` plus 60 seconds and passes
  `wall_s` to the runner, which passes it to its agent. In `master` the runner's limit and the
  harness's disagreed, and the shorter one decided silently (Running and recording a full run).
- `agents/` ports `master`'s `opencode_agent.py` and `pi_agent.py` (as `omp`) and adds Claude Code
  (`claude -p --output-format stream-json --mcp-config <file>`) and Codex (`codex exec --json`). The
  PocketEngine line's TypeScript agent host can be a runner too, through the same protocol.
- With `BENCH_TRACES=<dir>` the runners keep each agent's event stream, and the row gains `master`'s
  trace summary (turns, tokens, calls, kilobytes read, failed calls, calls before and after the
  first edit, `stuck`).

### 3.7 Isolation

- Task copies live outside the repository; the documentation set is copied with them, and nothing of
  the harness is beside it (master `docs/agent-eval.md`, Out of the repository).
- The engine binary is copied when a run starts and that copy serves every task and every agent's
  tools, so a build during the run changes nothing (master's `POCKET_EVAL_RUNTIME`; an agent's own
  tool once built the engine mid-edit: Fifty-five on opencode).
- Agents start in the task directory or an empty one, never in a repository, with extensions and
  configuration given by absolute path (an agent whose extension failed to load explored the home
  directory and sent what it read to its provider: master `docs/agent-eval.md`, Setting up the
  agents).
- Runners report files the agent touched under the harness or the evidence directories as `peeked`,
  and the row carries them.
- Model runners use their agent's own sandbox (Claude Code's sandbox settings,
  `codex --sandbox workspace-write`), confined to the task directory. Isolation at the operating
  system's level (a separate account, a container) needs administrator rights on the reference
  machine and is charter 12's open question 7.
- A full run takes the machine to itself: no builds, no test suites, no GPU measurements while it
  runs, and the report records whether the machine was quiet (`master`'s run of 67 tasks lost three
  tasks to builds sharing the machine: Sixty-seven on opencode).

## 4. Metrics

| Metric | Suite | Source | Meaning |
|---|---|---|---|
| `ok` | both | checker | The task passed. |
| `score` | both | checker | 0 to 1: the share of the goal reached (crates aboard, marks rounded); 0 or 1 for a developer task. |
| `seconds` | both | harness | Wall time of the runner. |
| `calls`, `calls_by_tool` | both | proxy | MCP tool calls the agent made, in all and by tool. |
| `failed_calls`, `error_codes` | both | proxy | Calls answered with `isError`, and their codes: the friction to turn into engine or documentation changes (charter 9, item 5). |
| `result_tokens` | both | proxy | Tokens of the text of every answer the agent received: what the engine's answers cost. |
| `agent.tokens`, `agent.cost_usd`, `agent.turns` | both | runner | The agent's own accounting: input, output, cache reads and writes; dollars; model turns. |
| `decision_points` | player | engine | Decision points that arose for the seat on main (time.md, Decision points: the unit charter 9 counts), from the session's statistics: a function of the world and the filter, whatever the agent's stepping. |
| `decisions_answered` | player | engine | Of those, the ones the agent answered; a `step` without `until` passes over the rest (`passed_decisions`). |
| `acts`, `branch_acts` | player | engine | `act` calls on main, and on branches (lookahead). |
| `calls_per_decision` | player | both | `calls` over `decision_points`: what the agent spent per decision the game posed. |
| `wall_limit_stops` | player | engine | Time requests that stopped on `max_wall_ms` (the session statistics' count); expected 0 with the benchmark's 600,000. |
| `sim_ticks` | player | checker | Ticks of main from the start to completion, or to the task's limit. |
| `forks`, `branch_ticks` | player | engine | From the player session's statistics, read by the checker (`session {all: true}`). |
| `peeked`, `trace` | both | runner | Harness files touched; the trace summary. |

"Decision calls" in charter 9 are reported as `decision_points` and `decisions_answered` with
`calls_per_decision` beside them, since an agent that observes ten times per decision spends its
tokens there.

## 5. Repeats, seeds and statistics

- **Repeats.** A model runner runs each task `n` times per condition; `n` is 5 by default
  (`--repeats`). Repeat `i` uses world seed `i`, the same list for every condition, every model and
  both lines, so results pair by `(task, condition, seed)`: common random numbers, the established
  way to compare two treatments on the same instances.
- **Summary per task and condition.** `n`; the pass rate with its Wilson 95% interval; for each
  numeric metric the mean, the sample variance (divisor `n - 1`), the minimum and the maximum. Over
  a suite: tasks passed by majority, the mean pass rate, and the totals.
- **Comparisons.** `bench.py compare A.json B.json` pairs rows and reports, per metric, the mean of
  the paired differences with its sample variance and `n`, and for passes the counts of pairs passed
  on one side only (the two discordant counts of McNemar's test, which the report leaves to the
  reader to test). Comparisons of the two lines use the same model, agent, agent version, task set
  and seeds; the report refuses to compare rows that differ in any of these.
- **Deterministic runners.** `reference`, `null`, `random` and `reference-omniscient` run each task
  twice; their rows must be equal in everything but wall time. A difference is a determinism failure
  of the whole stack (engine, MCP tools, harness) and fails the run. Their time requests pass
  `max_wall_ms: 600000`, and a row with `wall_limit_stops` above 0 is a harness error, not a result:
  where a step stops must not depend on how fast a line runs ticks.

## 6. Baselines

| Suite | Baseline | What it shows |
|---|---|---|
| Developer | `reference` passes every task in the suite | The tasks are solvable through the tools; the harness and checkers work. |
| Developer | `null` passes none | No checker passes an untouched project. |
| Player | `null` | What doing nothing scores (a boat with its sail furled drifts). |
| Player | `random` | What blind action scores. |
| Player | `reference` under perception | A scripted sailor solves the task with limited perception, so the task is fair. |
| Player | `reference-omniscient` | What full knowledge is worth to the same policy: the upper reference for the perception gap. |

A task joins a suite only when, on both lines, `reference` passes it, `null` fails it, and a second
`reference` run gives identical rows.

## 7. Reports

A report is one JSON object:

```json
{"schema": 1, "suite": "play", "line": "pocket3d",
 "engine": {"commit": "...", "version": "...", "binary_sha256": "...",
            "script_language": "typescript"},
 "shared": {"commit": "..."}, "harness": {"commit": "..."},
 "runner": {"name": "claude-code", "command": "...", "agent_version": "...", "model": "..."},
 "machine": {"os": "...", "cpu": "...", "cores": 16, "ram_gb": 15, "quiet": true},
 "conditions": [{"view": "perception", "fork": false, "projection": "text"}],
 "started": "2026-10-03T08:00:00Z", "seconds": 0,
 "rows": [], "summary": {}}
```

A row holds the task's identity (`task`, `suite`, `tier`, `condition`, `seed`, `world_params`,
`repeat`), its outcome (`ok`, `score`, `answer`, `detail`, `error`, among them `engine_halt`, 3.3)
and the metrics of section 4 that apply. `harness/report.py` holds the report's JSON Schema, and
`bench.py` validates every report it writes against it. Each line keeps its reports as evidence
(Pocket3D under `bench/reports/`, `docs/spec/architecture.md` 3), with a results entry saying what
the run showed and which failures led to which changes, as `master`'s `docs/agent-eval.md` did.

## 8. The slice 0 skeleton

Slice 0 builds the harness and proves it against a stand-in engine, since the engine's MCP tools
arrive in slice 2.

- `harness/` as in section 2, each module under 800 lines.
- `fake_engine.py`: a small engine in Python that serves the three commands of section 3.2 (`new`,
  `serve` on 127.0.0.1 with grants, `attach` over stdio) and the tools `session`, `describe`,
  `observe`, `act`, `step`, `fork`, `discard`, `world_get`, `world_query`, `world_edit` and `events`
  with the shapes of `mcp.md` and the contract, the three roles, strict parameters and budgets, over
  a toy world: a point boat with a heading, a speed and a sail, a steady wind, marks, and named
  entities. It exists to test the harness; its world is not the engine's.
- Two self-test tasks: `selftest_reach` (player: reach a mark 50 units downwind with limited
  perception) and `selftest_spawn` (developer: spawn a named entity at a place).
- `engines/pocket3d.toml` with the planned commands and `status = "planned"`.

Acceptance, all from one command, `python shared/benchmark/harness/bench.py self-test`:

1. `reference` passes both self-test tasks and `null` fails both.
2. Two `reference` runs give identical rows outside wall time.
3. `random` with `--repeats 3` gives a summary with means, variances and Wilson intervals that
   `stats.py`'s unit tests check against hand-computed values.
4. The proxy's call counts equal the fake engine's session statistics.
5. A report validates against the schema; `compare` of a report with itself gives zero differences.
6. A runner that sends an unknown key gets the strict decoder's refusal (`request.unknown_field`)
   and one that calls a tool its role lacks `permission.denied`, through the proxy, one that calls
   an unknown tool a JSON-RPC error, and one that exceeds `limits.calls` `session.limit_reached`;
   the row counts all four in `failed_calls`.
7. It finishes in under 60 seconds on the reference machine (the budget `bench.selftest` of
   `docs/spec/budgets.md` in the Pocket3D line; proposed, not measured, since the harness is not
   built yet).

The self-test SHOULD join the local check command (spec-arch), so the harness stays working while
the engine grows. It needs loopback sockets, which some agent sandboxes block (master
`docs/development.md`, Agent harness notes).

## 9. Rollout

| Slice | The benchmark gains |
|---|---|
| 0 | The skeleton and the self-test; this file and `tasks.md`; `engines/pocket3d.toml` planned. |
| 1 | The engine's `new` and `serve`; the fixtures `blank`, `hello`, `bodies`, `arena`, `hopper`, and `open_sea` and `island` on slice 1's headless sailing scene (tasks.md, Fixtures). |
| 2 | The player suite on the sailing fixtures: the slice's acceptance (Claude completes a sailing task over MCP with limited perception) is the task `run_to_mark` passing with `claude-code`; the developer tasks of tier S2 once `apply`, `project_brief`, `docs_search`, `schema` and `checks` exist (`mcp.md`, open choice 2). |
| 3, 4 | Tiers S3 and S4 as rendering and the showcase land. |
| Later | Deferred tasks, each when a slice brings its feature (tasks.md). |

## 10. Open choices

1. **Sandboxing agents** (charter 12, item 7): the agents' own sandboxes now; an operating-system
   boundary needs administrator rights and is left to the owner.
2. **Repeats against cost.** Five repeats of the 27 developer tasks of slice 2 are 135 agent runs;
   `master`'s full run of 72 took about three and a half hours. Recommended: five repeats for the
   comparison runs that decide something, one for a quick look, and the report says which.
3. **Where reports live.** Each line's evidence directory (Pocket3D: `bench/reports/`), with the
   shared summary of a cross-line comparison under `shared/benchmark/results/` once both lines run.
4. **A model judge.** None: every check is code. A task whose result only a person can judge (how a
   level looks) stays out of the suites.
