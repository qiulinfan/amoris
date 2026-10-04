# The local check command

Status: Draft, slice 0

Charter: 3.7 (determinism, fork consistency, replay, reload equivalence, one local command), 3.9
(clean structure with no line limit; the crate boundaries, which the check verifies), 3.10
(performance benchmarks whose numbers the check reports and never fails on), 4.1 (the `wasm32` build
in the checks from day one), 4.2.5 (the lint in the local checks), 4.4 (a changed shader checked in
a browser), 9.4 (the crate dependency check recorded with every commit).

This specification fixes `cargo xtask check`, the one command that says whether the repository is
correct: its steps and their order, what each verifies and how, the per-project checks of charter
3.7, the web build checked in headless Chrome, the output agents read, and the exit codes. CI, if
there is one, runs the same command (charter 3.7). The crates are those of `architecture.md`; the
performance step's workloads and reference figures are in `budgets.md`; the threads' own tests are
in `threads.md`, 11. Performance is measured, not gated (charter 0.4, 3.10): the `perf` step reports
its measurements against their reference figures and baselines with warnings, and never fails the
check, makes it inconclusive or changes its exit code on a number (9, 11).

Concepts used by name and owned elsewhere: Tick, boundaries, the numeric rules, the disallowed lists
and iteration order (spec-sim, [simulation.md](simulation.md), [numeric.md](numeric.md),
[rng.md](rng.md)); `TickHash`, `Snapshot`, fork, `Recorder`, `Replay`, `verify`, `lockstep`,
`first_divergence`, `Divergence` and bundle hashes (spec-persist, [persistence.md](persistence.md),
[versions.md](versions.md)); the type check, the stateless lint, hot update (spec-script,
[script-host.md](script-host.md), [hot-update.md](hot-update.md)); commands, players and the error
protocol and the conformance checks (spec-contract,
[shared/contract/](../../shared/contract/README.md)); MCP tools and the benchmark harness (spec-mcp,
[mcp.md](../../shared/contract/mcp.md), [shared/benchmark/](../../shared/benchmark/README.md)). Each
of those specifications lists its own tests; this command runs them (6.2, 7.2) and adds the
whole-run checks of charter 3.7 (8).

## 1. Why one command, and why this one

Master's suite (`pocket test`) did not build or run the web configurations, Linux, iOS or the agent
benchmark (`docs/development.md`, Tests), its CI was switched off, and its rules lived in
documentation that agents had to remember (master AGENTS.md rules 3, 10 and 12; PocketEngine's
charter 8.2 lists among master's failures that CI was switched off when its quota ran out and work
was verified by hand). The rebuild turns each rule into a step of one command that fails, so
correctness does not depend on discipline (charter 3.7).

## 2. Entry points

- **`cargo xtask check`**: the repository's check, the definition of done (AGENTS.md, rule 4). It is
  the established `cargo xtask` convention (matklad's xtask pattern, used by rust-analyzer): a
  workspace crate `xtask` run through the alias `[alias] xtask = "run --package xtask --"` in
  `.cargo/config.toml`, built by the pinned toolchain, nothing to install. `xtask` links no engine
  crate (`architecture.md`, 4.15); it drives `cargo`, the built `pocket` binary and Chrome as
  processes, so a compile error is a result it reports, not a crash.
- **`pocket check <project>`**: one project's checks (8) plus its type check and lint (6.4), in
  process, on the release build. It is what an agent runs after editing a game (the apply loop of
  charter 4.2.5); the developer-facing MCP tool `checks` returns its report (shared/contract/mcp.md,
  6). `cargo xtask check` runs it for every project that has a `check.toml`.

Both print the same report shape (10) and use the same exit codes (11).

## 3. Invocation

```text
cargo xtask check [--json] [--quick] [--only STEP,...] [--skip STEP,...] [--jobs N] [--record]
pocket check <project> [--json] [--seeds N,...] [--only CHECK,...]
```

| Flag | Meaning |
|---|---|
| `--json` | stdout carries exactly one JSON document, the report; progress goes to stderr |
| `--quick` | the code-health steps, build, tests (without those marked `#[ignore]`), `wasm`, `types`, and the project checks at the first seed only; no `web`, no `perf`. For an agent's inner loop |
| `--only`, `--skip` | step names (4); an unknown name is refused with a suggestion and exit 2 (charter 3.4) |
| `--jobs N` | project checks run at once (default 2, chosen for the about 4 GB this machine has free while other agents work; not measured) |
| `--record` | also prints the commit trailer line (12) |

The command honors `CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS`, needs no administrator rights, no
installer and no dialog, and every server it starts binds `127.0.0.1` on a port the system picks
(AGENTS.md, Working on this machine; several agents share the machine).

## 4. Steps and their order

| # | Step | Verifies | Needs | Skipped when |
|---|---|---|---|---|
| 1 | `deps` | the crate graph, external crates, features, build flags, the vendored QuickJS-ng (5.2) | `cargo metadata` | |
| 2 | `fmt` | `cargo fmt --all --check` | | |
| 3 | `clippy` | warnings and the determinism lints (5.3) | | |
| 4 | `build` | `cargo build --workspace --release --locked` | | |
| 5 | `gen` | generated files are up to date (5.4) | build | build failed |
| 6 | `test` | `cargo test --workspace --release --locked --no-fail-fast` (6.2) | build | build failed |
| 7 | `wasm` | every web crate builds for the web target (6.3) | | |
| 8 | `types` | type check and stateless lint of every project (6.4) | build | build failed |
| 9 | `projects` | determinism, fork, replay, reload per project, and the negative controls (8) | build | build failed |
| 10 | `contract` | the shared contract's conformance checks and the benchmark harness's self-test (8.7) | build | build failed |
| 11 | `web` | the web build in headless Chrome (7) | build, `wasm`, `projects` (whose hash chains and replays it compares) | any failed |
| 12 | `perf` | measures the workloads of `budgets.md` and reports them against their reference figures; never fails (9) | build, `web` | either failed; `--quick` |

Steps 1 and 2 compile nothing and step 3 compiles in check mode only, so a layout or style problem
is reported before the release build. Every step that can run does run after a failure; a skipped
step names the step it waited on. Steps run one after another, except that `projects` runs up to
`--jobs` project processes at once; `perf` runs alone and last, with nothing else of the check
running (9).

## 5. Code health

### 5.1 The files the steps read

There is no file-length limit and no step that counts lines: charter 0.4 replaced the 800-line rule
with code organized by responsibility (3.9), which reviews judge, and the check verifies the crate
boundaries (5.2). What remains of this section is shared by the steps that read files:

- **Files**: those git tracks plus untracked files git does not ignore
  (`git ls-files --cached --others --exclude-standard`), so a new file is checked before it is
  committed (`docs`, `gen`, and the discovery of projects, 8.1).
- **Generated files**: a file whose first five lines contain `@generated`, the marker `rustfmt`
  recognizes (its `generated_marker_line_search_limit` is 5; `rustfmt --print-config default` on
  1.9.0). The `gen` step proves each such file is produced by a registered generator (5.4), so a
  hand-written file cannot claim the marker.

### 5.2 `deps`: the crate graph and what may sit below each crate

Inputs: `cargo metadata --format-version 1 --locked`, run once with `--filter-platform` for the host
and once for `wasm32-unknown-unknown` (features resolve per target), and `tools/crate-graph.toml`,
which mirrors `architecture.md`, 5:

```toml
[crate.pocket-physics]
group = "game"
deps = ["pocket-sim", "pocket-assets"]
web = true

[crate.pocket-contract]
group = "contract"
deps = []
web = true
any = true     # every crate may depend on it (architecture.md, 4.16)

[group.game]   # forbidden anywhere in the transitive normal dependencies of the group's crates
forbid = ["wgpu", "winit", "egui", "egui-wgpu", "egui-winit", "tokio", "rmcp", "rayon", "web-sys"]
forbid_direct = ["wasm-bindgen", "js-sys"]   # bevy_ecs reaches them on wasm32 (architecture.md, 6)

[features.bevy_ecs]
forbid = ["multi_threaded", "serialize"]          # persistence.md 12, P6

[features.serde_json]
require = ["float_roundtrip"]
forbid = ["preserve_order"]                       # replay.md 2.1

[features.rapier3d]
require = ["enhanced-determinism", "serde-serialize"]   # architecture.md 6; the physics spike
forbid = ["parallel", "simd8"]

[features.rquickjs]
forbid = ["parallel"]     # it implements Send by pulling tokio/rt-multi-thread (architecture.md 4.6)

[build]
cflags_must_contain = "-ffp-contract=off"
rustflags_forbid = ["target-cpu", "+fma"]
```

| Verifies | Error code | Detail |
|---|---|---|
| every workspace member is in the graph file and every entry is a member | `deps.crate_unlisted`, `deps.crate_missing` | `{crate}` |
| every edge between workspace crates (normal, dev, build) is allowed | `deps.edge_not_allowed` | `{from, to, kind}` |
| no forbidden crate in a group's transitive normal dependencies, and none of `forbid_direct` as a direct one | `deps.external_not_allowed` | `{crate, dependency, chain}`: the path from the crate to it, so the fix is visible |
| resolved features per target | `deps.feature_not_allowed`, `deps.feature_missing` | `{package, feature, target}` |
| `.cargo/config.toml` sets `CFLAGS` with `-ffp-contract=off` | `deps.cflags_missing` | `{found}` |
| no `target-cpu` or `+fma` in the config's `rustflags` or the environment's `RUSTFLAGS` | `deps.rustflags_not_allowed` | `{source, value}` |
| `third_party/rquickjs-sys-0.14.0/` is exactly what `cargo xtask vendor` rebuilds from the pinned crate and the diffs (architecture.md 7.5) | `deps.vendor_stale` | `{path}`: the first file that differs |

Why the features and flags: `architecture.md`, 6 and 7.3 (master's fused multiply-adds broke
cross-target hashes, `docs/design/networking.md`, Determinism).

### 5.3 `fmt` and `clippy`

- `cargo fmt --all --check`; unformatted files: `fmt.unformatted {files}`.
- `cargo clippy --workspace --all-targets --release --locked --message-format=json -- -D warnings`
  for the host target, then
  `cargo clippy --target wasm32-unknown-unknown --no-default-features --profile web --locked -p <crate> ... -- -D warnings`
  for every crate with `web = true`, so `#[cfg(target_arch = "wasm32")]` code (the worker loop, web
  pacing, the injected clock) is linted too; each diagnostic:
  `clippy.warning {path, line, lint, message}`.
- The crates that hold tick code (`pocket-sim`, `pocket-persist`, `pocket-physics`,
  `pocket-interface`, `pocket-script`) and `pocket-runtime` each carry a `clippy.toml` (clippy reads
  the one in the crate's directory) with the `disallowed-methods` and `disallowed-types` lists of
  simulation.md 10 and numeric.md 5: the wall clock, the environment, thread spawning, storage-order
  queries, hash map types, and the platform's transcendental functions. spec-sim owns the lists and
  verified that each path resolves under clippy 1.98.1 with `bevy_ecs` 0.19.1; this step is the
  mechanism. The six files and the lint fixture's are generated from one
  `tools/clippy-determinism.toml` by `cargo xtask gen` (5.4), so a copy that drifts fails as
  `gen.stale`.
- A use whose order provably never reaches the world (a lookup-only cache) carries
  `#[allow(clippy::disallowed_types)]` with a comment saying why; reviews look for those.
  `pocket-runtime` applies the boundary writes, so it takes the hash-map and query-order lists; its
  loop-clock module alone reads `Instant::now`, under `#[allow(clippy::disallowed_methods)]` with
  its reason (`threads.md`, 3.2).
- A list that stops resolving after a dependency update would pass silently, so a fixture crate that
  uses every listed item must fail clippy (simulation.md 12, item 8; the `lint-fixture` control of
  8.6).

### 5.4 `gen`: generated files

`cargo xtask gen --check` runs every registered generator into a temporary directory and compares
byte for byte. Generators that need engine types run the built binary (`pocket gen --out <dir>`):
the `.d.ts` files (spec-script), the JSON Schemas of the command catalog and the shared contract
(spec-contract, spec-mcp), the budgets table in `budgets.md` from `bench/budgets/` (`budgets.md`,
8.2), every `clippy.toml` from `tools/clippy-determinism.toml` (5.3), and `schema.lock.json` with
the projects' locks (versions.md 5).

| Error code | When |
|---|---|
| `gen.stale` `{path}` | a generated file differs from what its generator writes now |
| `gen.orphan` `{path}` | a file carries `@generated` but no generator writes it |
| `gen.shared_modified` `{path}` | a file under `shared/` differs from the hash recorded in `shared/SYNC.toml` (charter 6.2: the contract changes on both sides first) |

`shared/SYNC.toml` holds `commit` (the PocketEngine commit the copy was synced from) and a `[files]`
table of each shared file's SHA-256. Until the first synchronization `commit` is `""` and the hashes
are those of this line's draft; `gen.shared_modified` compares the files with the recorded hashes
either way, so an edit to `shared/` without updating the record fails whether or not a commit is
recorded. The file is committed with the draft (README.md 1).

## 6. Build, tests, the web target, types

### 6.1 `build`

`cargo build --workspace --release --locked --message-format=json`. Compiler errors become
`build.failed {crate, errors}` with the first ten as `path:line: message`. Release because every
timing and the agents' runs use it (`architecture.md`, 7.2; master's debug build was 25 to 31 times
slower, `docs/decisions/0006-agent-runs-release.md`), and one profile keeps the build time and the
disk within this machine's means; only the full check's cross-build variant (8.2) builds `pocket` in
debug as well, with `pocket-sim`'s feature `invariants` (simulation.md 4.1), which the test builds
also enable through a dev-dependency.

### 6.2 `test`

`cargo test --workspace --release --locked --no-fail-fast`, plus `-- --include-ignored` in the full
check, which runs the long variants tests mark `#[ignore]` (threads.md 11: 20 producer timings
instead of 5); `--quick` leaves them out. The step parses libtest's stable text output
(`test <name> ... FAILED` and the failures section) into `test.failed {crate, test, message}` with
the message cut to 20 lines. The exit status of `cargo test` is authoritative; the step never reads
its result through a pipe (master once committed with a failing assertion because `| tail` hid the
exit status, `docs/development.md`, Tests). The tests include those the other specifications list
(simulation.md 12, rng.md 10, persistence.md 12, versions.md 10, script-host.md 12), the threads'
tests (`threads.md`, 11) and the controls that need a Rust-side defect (8.6).

### 6.3 `wasm`

`cargo check --target wasm32-unknown-unknown --profile web --locked --no-default-features -p <crate>`
for every game crate with `web = true` in the graph file, then `-p pocket-web` with the shipped
feature set (none) and with `editor`. Features are off by default everywhere (architecture.md 7.4),
so this is what the shipped page links. A failure is `wasm.check_failed {crate, errors}`. This is
the charter's "`wasm32` build in the local checks from day one" (4.1) at the cost of a type check;
the full web build is step 11. QuickJS-ng's C needs clang for this target: `xtask` sets
`CC_wasm32_unknown_unknown`, `CXX_wasm32_unknown_unknown` (meshoptimizer's C++ when `import` is on;
the render spike's `build_web.sh` set it to `clang++`) and `AR_wasm32_unknown_unknown` to the
`clang`, `clang++` and `llvm-ar` of an LLVM directory named by the environment (`POCKET_LLVM`), else
`~/.pocket-tools/llvm-23.1.2`, and reports a missing clang as `check.tool_missing`. The vendored
crate's P7 removes the Windows include workaround (architecture.md 8.1). Machine paths stay out of
committed files.

### 6.4 `types`

For every project: the TypeScript type check and the stateless-script lint, through the command
spec-script names (charter 4.2.4: `tsc` in CI and the apply command; 4.2.5: the lint in the local
checks). Each finding is `types.error {project, path, line, column, code, message}` or the lint's
own code (spec-script). When the lint passes, the step writes the project's declarations under
`.pocket/types/` (with the `tsconfig.json` `tsc` uses when the project has none; it never writes
into the source tree: `scripts.types`, script-host.md 7.4) from the project loaded into a game with
no entities, then runs TypeScript 7's `tsc --noEmit -p` over them as `scripts.check` does
(`pocket-app` hands `pocket-server`'s runner to `pocket-check`, which spawns no process itself),
with its time as the `tsc` measurement and its version in the summary; a run past 60 s is
`types.timeout`. `tsc` is found as the server finds it (`POCKET_TSC`, else the first TypeScript 7 in
`sdk/node_modules` after `cd sdk && bun install`, ...; server.md); `sdk/bun.lock` pins 7.0.2 by
version and integrity hash, so `bun install` replaces master's pinned download and nothing is
fetched by the check itself. Without a TypeScript 7 `tsc`, or when the declarations cannot be
written (`project.unwritable`), the step is inconclusive with `check.tool_missing {tool: "tsc"}`
(the lint alone is no verdict), and `pocket-app`'s test 7 fails unless `POCKET_NO_TSC=1`.

## 7. `web`: the web build in headless Chrome

### 7.1 Build and package

1. One web module, built once:
   `cargo build -p pocket-web --target wasm32-unknown-unknown --profile web --locked` with no
   optional feature, then `wasm-bindgen --target web` (the CLI version equal to the crate's pin,
   `=0.2.129`), once as it comes and once through `wasm-opt -O3`, since the spikes disagreed on what
   `-O3` does to speed (`architecture.md`, 8.1). The check page's entry and the game crates' web
   tests are compiled into this shipped module behind a runtime flag the check page sets, so the
   check needs no second fat-LTO build; their bytes count in `web.size.*`. Sizes, and the build's
   peak memory (`build.web.memory`), go to the `perf` step (`budgets.md`, 5.4 and 5.5).
2. The check page: the module, beside the page (`crates/pocket-web/check/`), in `out/check/web/`,
   with the packaged projects marked `[web] workload = true` (`pocket pack <project> --web`:
   transpiled scripts, scene, cooked assets), the native hash chains of those projects
   (`expected.json`) and the replay files, both written by the `projects` step, which `web` needs
   (4).

### 7.2 What the check page does

The page starts the presenter and the game worker as a shipped page does (`threads.md`, 7), runs
these steps in order, and sets `document.title` to `DONE <json>` or `FAIL <json>`, the protocol of
`tools/webcheck.py`:

| Page step | Verifies | Error code |
|---|---|---|
| `tests` | every test the other specifications mark web (rng.md's reference vectors, persistence.md's pinned vectors P4, script-host.md 12's lockdown, mathematics, step counts, NaN canonicalization and depth tests, versions.md's V9: the worker reports the native build's `EngineVersion.source`) runs in the worker. Each game-group crate lists its web tests as `(name, fn() -> Result<(), String>)` in a registry the check entry collects and runs | `web.test_failed {test, message}` |
| `determinism` | for each web workload and seed, the worker's per-tick `TickHash` chain equals the native chain; the first differing tick is named (charter 3.3). The workloads include `bodies` (200 bodies, numeric.md 5) and, as a negative control until the glamx and parry patch lands, the physics fixture with a hull given a density, a joint and a CCD body, which must fail with `web.cross_target_diverged` (8.6) | `web.cross_target_diverged {project, seed, tick, native, web}` |
| `replay` | each replay recorded natively replays in the worker, which has no transpiler, from its embedded compiled modules and data with the recorded hashes (replay.md 2.4; the browser-local form watches agents' replays, charter 5.1) | `web.replay_diverged {project, tick}` |
| `threads` | the ordering and snapshot-rebuild tests through messages (`threads.md`, 11) | `web.threads_failed {test, message}` |
| `shaders` | every WGSL module of `pocket-render` compiles under Tint (`createShaderModule`, `getCompilationInfo`) and every pipeline is created inside `pushErrorScope("validation")` without an error | `web.shader_rejected {module, messages}` |
| `frame` | the reference scene draws one 1280 by 720 frame offscreen whose pixels are not all the clear colour, with no validation error | `web.frame_empty {stats}` |
| `perf` | only when the `perf` step asks: web tick, fork, frame and start-up measurements (`budgets.md`) | |

Why shaders and a frame: Tint is stricter than naga, and "a module Tint rejects draws nothing"
(master AGENTS.md rule 10; `docs/web.md`, Shader note: uniform control flow around `textureSample`);
charter 4.4 requires the browser check after a shader change, and here it runs on every check. Why
cross-target hashes: a native build and a browser build are different builds, and master's lockstep
games agreed only after three fixes (`docs/design/networking.md`, Determinism);
`wasm32-unknown-unknown` also differs in pointer width and NaN payloads, which only a comparison
catches.

### 7.3 Runs

The step loads the page twice through the web checker on a free loopback port: with COOP and COEP
headers (cross-origin isolated) and without them. Both must pass; a page that passes only isolated
is `web.isolation_required`, because the Worker form must work on a shared link without those
headers (charter 5.1; `threads.md`, 7.5). Headless Chrome draws on this machine's integrated GPU
(AMD Radeon 780M), so web frame numbers are integrated-GPU numbers (`budgets.md`, 3). No Chrome:
`check.tool_missing {tool: "chrome"}`, inconclusive. The page's console and exceptions go to
`out/check/web.log`; a failure's detail carries the last 20 console lines.

### 7.4 The Rust successor of `tools/webcheck.py`

`cargo xtask webcheck <url> [--serve DIR] [--port N] [--timeout S] [--isolation on|off] [--loads N] [--json]`,
with the protocol and exit codes of `tools/webcheck.py` (title `DONE`/`FAIL`, console and exceptions
printed, 0 or 1), implemented in `xtask` with `tungstenite` (the Chrome DevTools Protocol over a
WebSocket; the debugger spike pins `=0.30.0`) and a `std::net` file server (the wasm MIME type, COOP
and COEP only with `--isolation on`, and `Cache-Control: no-store` except under `--loads`). It
starts Chrome headless with a throwaway profile, `--enable-unsafe-webgpu`, and never the owner's
profile. `--loads N` loads the page N times in the same profile with cacheable responses and reports
each verdict, which the start-up measurement needs (a cold and a warm load, `budgets.md`).

Until the successor lands (slice 1), the step runs `python tools/webcheck.py` with `PYTHONUTF8=1`; a
missing Python or `websocket-client` is `check.tool_missing`. `tools/webcheck.py --serve` always
sends COOP and COEP, so the run without isolation serves the page from a plain file server instead
and points `tools/webcheck.py` at its URL without `--serve`, as the script-web spike did with
`python -m http.server` (`docs/spikes/script-web.md`, Determinism). The warm start-up load waits for
the successor.

## 8. Project checks

### 8.1 `check.toml`

Every project under `samples/` and `tests/fixtures/` with a `check.toml` is checked:

```toml
[run]
seeds = [1, 2, 3]                    # each check runs once per seed
ticks = 600                          # ticks per run
inputs = "checks/inputs.jsonl"       # optional: commands applied during the run

[fork]
at = [120, 300]                      # fork points (ticks)
ticks = 120                          # ticks each comparison runs after its fork point
branch_inputs = "checks/branch.jsonl"

[reload]
at = [60, 240]                       # boundaries where the unchanged scripts are reloaded

[web]
workload = true                      # also run cross-target in the browser (keep it small)

[expect]                             # negative controls only (8.6)
fail = "reload.diverged"
```

An input file holds one command per line in the JSON form agents send over MCP, with the tick it is
an input of (applied at boundary `tick - 1`, `threads.md`, 2):

```json
{"tick": 30, "source": {"player": 0}, "name": "<command>", "params": {}}
```

Each line is applied through `Game::apply` with `at` set to its tick, the path of `threads.md`, 5.3.
So a failing agent session's commands can become a check's input unchanged. A line the command
refuses fails the check with `check.input_refused {line, error}`.

The runs use `Game` synchronously, with no thread, and hash every tick (spec-persist).

### 8.2 Determinism

| Variant | How | Catches |
|---|---|---|
| same process | two `Game`s from the same project and seed, stepped alternately on one thread with the same inputs; hashes compared after every tick | state shared between instances through process globals, which would also break parallel forks |
| cross process | `pocket check` runs itself again as a child process (`--chain-only`) per seed and compares the child's hash chain with its own | per-process variation: hash-map seeds (`RandomState`), addresses, thread identities |
| cross build | the sailing workload run by a debug build of `pocket` and compared with the release build's chain; full check only, since it needs a second build (simulation.md 12, item 10) | optimizer-dependent results, debug-only code paths |
| cross target | the web page (7.2) | compiler, target and engine differences |

The same-process variant is spec-persist's `lockstep` over two `Stepper`s (replay.md 3.3); the
others compare `TickHash` chains with `first_divergence` (10.1). A difference is
`determinism.diverged {project, seed, variant, divergence}`, where `divergence` is spec-persist's
`Divergence`: the tick, the differing sections, and with full state on both sides the entity, field
and both values. In the cross-process and cross-build variants the child is asked again for its
snapshot at that tick (`--snapshot-at T`) so `diff` can name fields.

### 8.3 Fork consistency

For each seed and each fork point F, with K = `[fork] ticks` (charter 3.7):

1. **Reference**: R runs to F + K with the run's inputs, never forked.
2. **Identical at the fork**: A runs to F; B = fork(A); hash(B) equals hash(A), else
   `fork.not_identical {tick, hash_a, hash_b}`.
3. **Same actions, same hashes**: A and B get the run's inputs for K ticks in spec-persist's
   `lockstep` (replay.md 3.3); their hashes are equal at every tick, else
   `fork.branch_differs {tick, divergence}`.
4. **Acting on the branch leaves the original alone**: from a fresh fork C = fork(A at F), C gets
   the run's inputs plus `branch_inputs` while A gets the run's inputs only, stepped alternately;
   A's hashes equal R's at every tick, else `fork.original_changed {tick, divergence}`. C must
   differ from A by the end, else `fork.branch_inputs_ineffective`: a branch that never diverges
   proves nothing.

### 8.4 Replay

1. **Record**: a run with spec-persist's recorder on writes a replay file (the inputs as applied,
   per-tick hashes, the script bundle hash and the engine version, charter 7 item 13).
2. **Verify in a fresh process**: `pocket replay --verify <file> --json`, which is spec-persist's
   `verify` (replay.md 3.4), replays it; every tick's hash equals the recorded one, else
   `replay.diverged {divergence}`, which names the first diverging tick (charter 3.3).
3. **Localization**: the check writes a copy of the recording without the first recorded write at or
   after tick `ticks / 2` (an input of tick T, applied at boundary T-1) and verifies the copy; a
   divergence reported before T is `replay.divergence_too_early {reported, expected_at_least}`,
   because nothing differed before T. The tick it finds is reported as a measurement.

The replay files are kept for the web page's `replay` step (7.2).

### 8.5 Reload equivalence

Run A has no reloads. Run B is the same run, but at each boundary in `[reload] at` it sends
`scripts.apply {force: true}` with the project's unchanged scripts, the command an edit uses, which
the runtime turns into a `scripts.swap` Host write at a boundary (hot-update.md 4, 5 and 9;
`threads.md`, 6). The two runs go through spec-persist's `lockstep`, and hashes are compared at
every tick: `reload.diverged {tick, reload_at, divergence}`. The divergence names the component
whose value differs, which points at the script that kept hidden state (charter 3.7, 4.2.5). Without
`force` an unchanged bundle is answered `Unchanged` and nothing is swapped (hot-update.md 4.2),
which would make the check vacuous, so B's recording must show a `scripts.swap` at every reload
point, else `reload.not_performed {at}`.

### 8.6 Negative controls

A check that never fails proves nothing. Each control is a small defect the check must catch with a
stated code; a control that passes is `check.control_passed {control, expected}`, one that fails
with another code is `check.control_wrong_failure {control, expected, got}`, and either fails the
step.

| Control | Defect | Must fail with | Runs as |
|---|---|---|---|
| `module-state` | a script keeps a module-level counter that feeds a component; the lint is off for this fixture | `reload.diverged` | project `tests/fixtures/controls/module-state` |
| `module-state-lint` | the same script with the lint on | spec-script's lint code | project |
| `process-random` | a test-only Rust system writes a value derived from `RandomState` | `determinism.diverged` (cross process) | `pocket-check` integration test |
| `shared-static` | a test-only system increments a process-global atomic and writes it | `determinism.diverged` (same process) and `fork.original_changed` | `pocket-check` integration test |
| `unrecorded-write` | a test-only path changes the world outside the command queue at one tick of the recording run | `replay.diverged` | `pocket-check` integration test |
| `tint-only` | a WGSL module naga accepts and Tint rejects (a `textureSample` under non-uniform control flow) | `web.shader_rejected` | web page, fixture module |
| `lint-fixture` | a fixture crate using every item of the disallowed lists (simulation.md 12, item 8) | `clippy.warning` for each item | `clippy` step: `tests/fixtures/lint-fixture` is outside the workspace (`[workspace] exclude`) and linted on its own with the same generated lists |
| `physics-platform-math` | the physics fixture of numeric.md 5: a dynamic hull with a density, a joint and a CCD body, whose mass properties, joint and CCD math call the platform's library natively | `web.cross_target_diverged` | web page, `tests/fixtures/controls/physics-platform-math` with `[expect] fail = "web.cross_target_diverged"`; when it reports `check.control_passed`, the glamx and parry patch is proved and the fixture becomes a positive workload (numeric.md 5) |
| `bad-edge`, `bad-feature` | an edge not allowed; `rapier3d` with `parallel` | `deps.edge_not_allowed`, `deps.feature_not_allowed` | `xtask` unit tests over fixture trees |

Controls that need a defect in Rust are integration tests of `pocket-check` that build a `Game` with
the extra system and run the same check functions (the cross-process one re-runs the test binary as
its child), so no fixture code ships in the `pocket` binary.

### 8.7 Contract conformance and the benchmark self-test

The shared contract names the checks both lines run, so their reports compare
(shared/contract/README.md, Conformance: `contract.sync`, `contract.perception.*`,
`contract.projection.golden`, `contract.actions.*` and the rest). The `contract` step runs each
under exactly that name, as a `StepResult` measurement row per check, and reports a failure with the
problem code the contract gives it. `contract.sync` compares `shared/` with the PocketEngine commit
`shared/SYNC.toml` records, in a checkout named by `POCKETENGINE_DIR`. While `commit` is `""` (not
yet synchronized) or no checkout is reachable, it is `Skipped` with that reason, never passed and
never inconclusive, as the contract asks, while `gen.shared_modified` (5.4) still holds the copy to
its recorded hashes.

The same step runs the benchmark harness's self-test (shared/benchmark/README.md, 8: a fake engine,
the proxy, the reports), which needs loopback sockets; its duration is reported as `bench.selftest`
(reference figure 60 s proposed; not measured, since the harness is not built yet).

## 9. `perf`

The step runs last and alone, on the release builds, as `budgets.md` specifies. It measures and
reports; it never fails and is never inconclusive, so no performance number changes the check's
verdict or its exit code (charter 3.10: the numbers are measurements, not pass conditions). Its
verdict is `Pass` once its measurements are recorded, whatever they are, and `Skipped` with the
reason when it cannot measure (a step it needs failed, `--quick`, or a tool or build it needs is
missing). Everything it finds about the numbers is a warning:

1. fingerprint the machine. The reference figures were measured on the reference machine R1
   (`budgets.md`, 3). On any other machine (a CI host, the owner's Apple M5) the step measures every
   workload just the same and compares each measurement with that machine's own
   `bench/baseline-<machine id>.json` rather than with R1's figures, and says so with the warning
   `perf.not_reference`;
2. on R1, check that QuickJS-ng was built by clang-cl (`EngineVersion.c_compiler`), else the warning
   `perf.wrong_compiler` (architecture.md 7.3), and calibrate the CPU and the GPU (`budgets.md`, 7);
3. run every budget's measurement: `pocket bench <workload> --json` natively, the page's `perf` step
   in the browser, and the sizes of 7.1. Each is reported as a `Measurement` (10.1) with its
   reference figure, and compared with the machine's accepted baseline for drift (`budgets.md`,
   8.3);
4. calibrate again; drift beyond the tolerance marks the run noisy (`perf.noisy`): its measurements
   are still reported, but they cannot become a baseline.

Two bootstrap commands give the step its reference values (`budgets.md`, 8.1):
`cargo xtask perf --calibrate` runs the CPU and GPU calibration workloads 7 times each on a quiet R1
and writes their medians to `bench/calibration-r1.toml`, and `cargo xtask perf --set-unset`, which
refuses until a calibrated run exists, measures every budget still "not measured" and writes its
reference figure by the rule of `budgets.md` 8.1. Until they run, the step still measures: it
reports each row without a figure with `perf.budget_unset`, and a run it cannot judge for noise
cannot become a baseline.

| Warning code | Meaning |
|---|---|
| `perf.budget_exceeded` `{budget, measured, limit, unit, stat, samples}` | a measurement worse than its reference figure: a possible regression to look into, with the profile of `budgets.md` 9 |
| `perf.scale_below_floor` `{budget, measured, floor}` | a benchmark scene smaller than its floor, so its measurements are not comparable with the reference |
| `perf.capacity_dropped` `{counter, value}` | a capacity counter reported a drop or an overflow (`budgets.md`, 6; each capacity's own test fails in `test`) |
| `perf.noisy` `{before, after, reference}` | calibration moved: the machine was busy; the run cannot become a baseline |
| `perf.not_reference` `{fingerprint}` | not the reference hardware: measurements are compared with this machine's own baseline |
| `perf.wrong_compiler` `{c_compiler}` | QuickJS-ng was not built by clang-cl on the MSVC target; the timings are not comparable with the reference figures (architecture.md 7.3) |
| `perf.on_battery` | the laptop is on battery power; the run cannot become a baseline (`budgets.md`, 7.5) |
| `perf.budget_unset` `{budget}` | a budget with no reference figure yet ("not measured" in `budgets.md`, 5), reported with its measurement until `cargo xtask perf --set-unset` sets one (`budgets.md`, 8.1) |
| `perf.drift` `{budget, measured, baseline, percent}` | more than 10% worse than the accepted baseline (`budgets.md`, 8.3) |

## 10. Output

### 10.1 The report

```rust
// xtask/src/report.rs, the same shape printed by `pocket check --json`
pub struct Report {
    pub format: u32,                  // 1
    pub command: String,              // the command line as run
    pub commit: Option<String>,       // HEAD, with "+dirty" when the tree differs from it
    pub machine: Machine,             // budgets.md, 3: the fingerprint
    pub started_utc: String,          // RFC 3339
    pub duration_ms: u64,
    pub verdict: Verdict,
    pub steps: Vec<StepResult>,
}

pub enum Verdict { Pass, Fail, Inconclusive, Skipped }

pub struct StepResult {
    pub name: String,                 // "deps", "fmt", ..., "perf"
    pub verdict: Verdict,
    pub duration_ms: u64,
    pub summary: String,              // one line, for people and agents alike
    pub errors: Vec<Problem>,     // spec-contract's {code, message, detail}, at most 20
    pub more_errors: u32,             // how many were left out
    pub warnings: Vec<Problem>,   // at most 20; never change the verdict
    pub measurements: Vec<Measurement>,
    pub skipped_because: Option<String>,
    pub log: Option<String>,          // the full output under out/check/, which git ignores
}

pub struct Measurement {
    pub name: String,                 // a budget's name (budgets.md) or a health number
    pub value: f64,
    pub unit: String,                 // "ms", "KiB", "count"
    pub stat: String,                 // "median", "p95", "p99", "max", "value"
    pub samples: u32,
    pub limit: Option<f64>,           // the reference figure (budgets.md 2), never a pass condition
    pub limit_kind: Option<String>,   // "ceiling" or "floor"
}
```

The check's findings use the same error protocol as the engine (charter 3.4), so an agent reads one
shape everywhere. Reports are cut for an agent's context: a passing step is its summary line and its
measurements; only failures carry details, at most 20 per step; full logs stay in files the report
names. Master cut its scenario tool's answer from about twenty thousand tokens to one thousand the
same way (`docs/mcp.md`, `pocket_scenario`).

### 10.2 Human output

Without `--json`, one line per step on stdout, then each failure as `code: message` with its detail
on one line, then the verdict (the figures below illustrate the form; nothing was measured):

```text
PASS  fmt       1.1s  formatted
FAIL  projects  41s   determinism.diverged in samples/sail seed 2 at tick 377 (cross process)
...
FAIL  (1 failed, 0 inconclusive, 10 passed) in 6m 12s; report: out/check/report.json
```

### 10.3 Error codes

All codes of this specification, by step: `deps.crate_unlisted`, `deps.crate_missing`,
`deps.edge_not_allowed`, `deps.external_not_allowed`, `deps.feature_not_allowed`,
`deps.feature_missing`, `deps.cflags_missing`, `deps.rustflags_not_allowed`, `deps.vendor_stale`;
`fmt.unformatted`; `clippy.warning`; `build.failed`; `gen.stale`, `gen.orphan`,
`gen.shared_modified`; `test.failed`; `wasm.check_failed`; `types.error`, `types.timeout`;
`determinism.diverged`; `fork.not_identical`, `fork.branch_differs`, `fork.original_changed`,
`fork.branch_inputs_ineffective`; `replay.diverged`, `replay.divergence_too_early`;
`reload.diverged`, `reload.not_performed`; `web.test_failed`, `web.cross_target_diverged`,
`web.replay_diverged`, `web.threads_failed`, `web.shader_rejected`, `web.frame_empty`,
`web.isolation_required`; the warnings `perf.budget_exceeded`, `perf.scale_below_floor`,
`perf.capacity_dropped`, `perf.noisy`, `perf.not_reference`, `perf.wrong_compiler`,
`perf.on_battery`, `perf.budget_unset` and `perf.drift` (every `perf` code is a warning, 9);
`check.tool_missing`, `check.input_refused`, `check.control_passed`, `check.control_wrong_failure`,
`check.usage`, `check.config_invalid`; and the `contract.*` names of 8.7. They follow the error
protocol's dotted `<family>.<reason>` form (shared/contract/errors.md).

## 11. Exit codes

| Code | Meaning |
|---|---|
| 0 | every step that ran passed, and none was inconclusive |
| 1 | at least one step failed |
| 2 | the command could not run as asked: an unknown flag or step (`check.usage`, with a suggestion), or a malformed `crate-graph.toml`, `check.toml` or `budgets.toml` (`check.config_invalid {path, line, message}`) |
| 3 | nothing failed, but at least one step was inconclusive (a tool missing) |

The codes are checked in that order: 2 before anything runs, then 1 if any step failed, then 3.
`--quick` skips `web` and `perf` by design, and skipping by request is not inconclusive. The `perf`
step is never `Fail` or `Inconclusive` (9), so no performance number sets the exit code, on R1 or on
any other machine: a CI host or another developer machine reaches exit 0 when everything else
passes, which is charter 3.7's CI form. An inconclusive check is not a pass, since a check without a
verdict does not meet charter 10 ("every check has a verdict"); no slice is accepted or refused on a
performance figure (charter 3.10).

## 12. Recording with every commit

Charter 9.4 asks for the crate dependency check's result with every commit. `--record` prints one
trailer line, which the committing agent puts at the end of the commit message (the form):

```text
Checked: pass full; deps ok; perf pass
```

`git log --format='%h %(trailers:key=Checked,valueonly)'` then lists the record per commit with no
file to keep in sync. The full report stays in `out/check/report.json` on the machine that ran it.

## 13. Duration and resources

- Measurements of the check itself: `check.quick`, `check.full` (every step but `perf`) and
  `check.perf` (the `perf` step, which runs alone) on the reference machine, and `build.web.memory`,
  the peak resident memory of the web-profile build, reported against the reference figures of
  `budgets.md`, 5.5, all first measured in slice 1's first run; master's whole suite took 8 to 12
  minutes (`docs/development.md`, Tests).
- Memory: about 4 GB of this machine's 15 GB are free while other agents work, so builds follow
  `CARGO_BUILD_JOBS`, project checks run `--jobs` at a time, and the browser runs alone.
- Every step's command, working directory and environment are written at the top of its log, so a
  failure can be rerun by hand.

## 14. Open choices

1. **`cargo xtask` or a separate binary.** Recommendation: `cargo xtask` (2), the convention, with
   nothing to install and no engine linked into the checker.
2. **Markdown under the line limit.** Withdrawn: charter 0.4 removed the line limit (3.9), so no
   file of any kind is counted (5.1).
3. **Overflow checks in tests.** Settled by numeric.md 8: the tick-code crates build with
   `overflow-checks = true` in the release profile too (a per-package override), so tests on the
   release profile catch an overflow there; the other crates keep the default.
4. **The web checker.** Recommendation: port `tools/webcheck.py` to `cargo xtask webcheck` in slice
   1 (7.4) so the check needs no Python and can measure the warm start-up.
5. **libtest text or `cargo-nextest`.** Recommendation: libtest's stable text output, no extra tool;
   nextest only if parsing proves fragile.
6. **Seeds in the full check.** Recommendation: every seed of every `check.toml` for the
   same-process and fork variants, the cross-process variant at every seed, the web at the first
   seed per workload, so the browser run keeps the full check's duration (`check.full`) short.
7. to 22. **Slice 1 decisions**, recorded in [checks-slice1.md](checks-slice1.md) under these
   numbers: 7, one step per charter check; 8, steps that wait for crates; 9, `docs`, a step after
   `fmt`; 10, `gen` without the build; 11, the lint fixture lives in `tools/lint-fixture/`; 12,
   codes added; 13, what `deps` reads; 14, running `xtask` while a manifest is broken; 15, the web
   check's pieces; 16, `check.toml` as built; 17, how the four checks run; 18, codes added; 19, the
   Rust-side controls; 20, the cross-build variant as built; 21, what `pocket check` refuses and
   what its children may answer; 22, the web step as built.
