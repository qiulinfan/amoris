# The local check command: slice 1 decisions

Status: Draft, slice 1

The decisions slice 1's implementation (`xtask`, the crate `pocket-check` and the `pocket` binary's
`check` and `replay`) took where [checks.md](checks.md) was ambiguous or wrong for the code. They
are open choices 7 to 22 of checks.md 14, which lists them by title and keeps their numbers here.
They moved here unchanged when checks.md passed the 800-line limit that charter 3.9 set before
version 0.4, as simulation.md's did.

## 7. One step per charter check

The `projects` step of 4 is four steps, `determinism`, `fork`, `replay` and `reload`, after `types`
and in that order, because charter 3.7 names four checks and charter 10's slice 1 acceptance asks
each to pass: each gets its own verdict row, and `--only reload` reruns one. Each step runs
`pocket check <project> --json --only <check>` (with `--seeds <first>` under `--quick`) for every
project, `--jobs` at a time, and merges the step of that name from the project's report, each
problem tagged with `project`; `types` runs the same way with `--only types`. The report is 10.1's
in serde's default JSON form (`"verdict": "Fail"`), of which `format`, `verdict` and `steps` (each
with `name` and `verdict`) are required. `pocket check` reports raw results and `xtask` judges the
controls of 8.6: a project with `[expect] fail` runs only in the step its code's family names
(`reload.*` in `reload`, `types.*` and `lint.*` in `types`, `web.*` on the page), and a malformed or
misspelt `check.toml` is `check.config_invalid` before any step runs. A row is judged by its verdict
as well as its problems (slice 1 review): a `Fail` or `Inconclusive` row without a problem adds
`check.command_failed {verdict, project}` with that verdict, the row's `more_errors` is added to the
step's, and the exit status must be the one 11 gives the row's verdict (0 for `Pass` or `Skipped`, 1
for `Fail`, 3 for `Inconclusive`), else the project's result is
`check.command_failed {command, status, tail}`, so a failing check never merges as a pass.

## 8. Steps that wait for crates

A step that needs `pocket check` is `Skipped` with the reason "pocket-check not built yet" until the
release build's `pocket` answers `pocket check --json` (no project) with a report, which every
implementation prints for its `check.usage` refusal (2, 11); `perf` waits on the same probe and then
on `xtask` running `bench/budgets/`, which slice 1 has not built (budgets.md 8). `contract` is
`Skipped` while `shared/SYNC.toml` records no commit and the conformance checks are not built (8.7).
`web` is `Skipped` until `crates/pocket-web/check/index.html` exists. Amended in the slice 1 review:
`Skipped` holds only while no project takes part in the step. Once a project under `samples/` or
`tests/fixtures/` with a `check.toml` does, a missing `pocket` binary or a probe without a report
fails the step with `check.command_failed {command, status, tail, projects}`, the probe's exit
status and the last 20 lines of its stderr, since otherwise the four checks of charter 3.7, and a
control that must never pass, would vanish with exit 0 whenever `pocket check` regressed.

## 9. `docs`, a step after `fmt`

AGENTS.md rule 6 wraps documentation at 100 columns, and nothing checked it: `docs` runs
`tools/wrap_docs.py --check` over the Markdown under `docs/` and `shared/`, each file it would
change being `docs.unwrapped {path}`; no Python is `check.tool_missing`.

Amended 2026-10-09 (Pioneer, the xtask track). The rule is now AGENTS.md rule 7 (rule 6 became the
language rule), and the step checks a width, not a canonical form: `--check` had demanded the exact
greedy refill of every paragraph, which 67 of the 85 files failed although most were wrapped by
hand below 100 columns, and it counted code points and joined lines with spaces, so Chinese prose
(two columns a character, no spaces) became one unbreakable line. `wrap_docs.py` now refills only
a paragraph or list item that has a line wider than 100 display columns (East Asian wide characters
count two), breaks Chinese between two CJK characters (never before closing or after opening
punctuation) and joins such a break back without a space, keeps hard line breaks and link
definitions, and has a `--selftest`. The purpose is unchanged: a search returns a line, not a
paragraph. The step reads `docs/` only, without `docs/evidence/`, whose transcripts are kept as
recorded; `shared/` is the contract's text, which changes only through its record
(`gen.shared_modified`), so this step does not reformat it. A paragraph with an unbreakable token
wider than the limit (a long command in a code span) passes once it is refilled.

## 10. `gen` without the build

The generators that need no engine build run even when `build` failed: every `clippy.toml` from
`tools/clippy-determinism.toml` (whose `crates` list names the six crates of 5.3 and the lint
fixture; the copies are identical, `pocket-runtime` included, its loop clock and game thread taking
`#[allow]` with their reasons), the `gen.orphan` scan (`Cargo.lock` files are cargo's own) and
`gen.shared_modified`, which also names a file added under `shared/` or removed from it
(`{path, recorded, actual}`). `cargo xtask gen` writes the outputs, `--check` compares them, and
`--shared` alone rewrites the hashes of `shared/SYNC.toml`, keeping its comment and `commit`, since
a change to the shared contract is deliberate (charter 6.2). clippy 1.98.1 ignores a listed path
that does not resolve in a crate, without a warning (checked with a scratch crate on 2026-10-03), so
one file serves crates that do not link `bevy_ecs`.

## 11. The lint fixture lives in `tools/lint-fixture/`

It is a Rust crate that proves the configuration under `tools/`, not a project, and
`tests/fixtures/` is where the project checks look for `check.toml`; its own empty `[workspace]`
table keeps it out of the repository's workspace without a `[workspace] exclude`. It depends on
`bevy_ecs`, `bevy_platform` and `hashbrown` directly, and clippy reported all 81 entries of the
lists on 2026-10-03. The fixture proves the lists only for the versions it links, and its
`Cargo.lock` is its own, so before linting it the `clippy` step compares, for each package the
fixture depends on directly, the versions its lock reaches from `lint-fixture` with those the
workspace's `Cargo.lock` reaches from the tick-code crates of 5.3 (the `crates` of
`tools/clippy-determinism.toml`); a difference is `gen.stale {path, package, fixture, workspace}`
with the path `tools/lint-fixture/Cargo.toml`, and the fixture is not linted (slice 1 review).
Comparing what the tick-code crates reach, not the whole lock, keeps a `hashbrown` that only the
renderer links from failing it, while a tick-code crate that starts linking another version does
fail it. `cargo xtask gen` does not write the fixture's pins: `bevy_platform` and `hashbrown` are
not in `[workspace.dependencies]`, and refreshing a lock is `cargo update`'s job.

## 12. Codes added

`check.command_failed {command, status, tail}` when a tool exits with a failure and gave no finding
the step reads (a stale `Cargo.lock` under `--locked`, a failing build script, a report missing from
`pocket check`), so such a failure never shows as a pass or an empty list; `docs.unwrapped {path}`;
and the warning `check.not_implemented {what}`, given while `third_party/rquickjs-sys-0.14.0/`
exists and `cargo xtask vendor --check` does not (`deps.vendor_stale` is not checked yet). Exit code
2's report holds one step named `check` with the refusal. The human form labels an inconclusive step
`INCO`. 2026-10-09 (Pioneer): `deps.vendor_stale` is checked (13), and the warning
`deps.vendor_unchecked {tail}` replaces `check.not_implemented` when the pinned crate is not in
cargo's cache.

## 13. What `deps` reads

Workspace edges come from each member's declared dependencies of every kind and target; forbidden
crates and features from `cargo metadata --filter-platform` for the host and the web target.
`tools/crate-graph.toml` lists every crate of architecture.md 5, those not created yet with
`planned = true`, so their absence is not `deps.crate_missing`. `CFLAGS` in the environment is
checked too, since cargo's `[env]` does not override a variable that is already set.

2026-10-09 (Pioneer): the vendored QuickJS-ng is compared too. Until `cargo xtask vendor` exists,
`deps` runs `python third_party/vendor.py --check --offline`, which rebuilds the directory from the
pinned crate and `third_party/patches/` into a temporary directory (about 7 s) and names each file
that differs, `deps.vendor_stale {path}`. `--offline` takes the crate from cargo's cache only, since
the check fetches nothing; when it is not there (a fresh machine: the `[patch]` means cargo never
downloads it) the step passes with the warning `deps.vendor_unchecked`, and one
`python third_party/vendor.py --check` with the network fills the cache. No Python is
`check.tool_missing`, as for `docs`.

## 14. Running `xtask` while a manifest is broken

`cargo xtask` cannot start while any member's `Cargo.toml` fails to load, since cargo loads the
whole workspace before it runs `xtask` (seen in this wave, while agents edited crates in parallel).
The built binary still runs as `CARGO_MANIFEST_DIR=<root>/xtask <target>/debug/xtask check` and
reports the manifest error under `deps`.

## 15. The web check's pieces

`xtask` serves the page itself on 127.0.0.1 (COOP and COEP only in the isolated run) and drives
`tools/webcheck.py` without `--serve`;
`cargo xtask webcheck <page> --serve DIR [--isolation on|off] [--timeout S]` is one such load. The
page's title JSON may carry `errors` (problems with the codes of 7.2), `warnings` and
`measurements`; other JSON is one `web.test_failed`. The CDP client in Rust of 7.4 (open choice 4)
is not built, so the check still needs Python and `websocket-client`. The `wasm` step checks the web
game crates in one `cargo check` rather than one per crate: their features are off by default
(architecture.md 7.4), so unification adds none, and one run is cheaper on this machine.

## 16. `check.toml` as built

`[expect]` takes `lint = false` beside `fail`: a control whose defect the lint would refuse (a
module-level counter, a closure made at load time) compiles with the lint off, so it reaches the
check that is its backstop (`module-state` and `closure-state` under `tests/fixtures/controls/`,
both `reload.diverged`); only a control may turn the lint off. The ticks of `[fork] branch_inputs`
count from the fork point (tick 1 is the first tick after it), so one file serves every fork point.
An input line's `params` is an object, empty when left out. A project is `project.toml` (`name`,
`rate`, `seed`), `scene.json` and `scripts/`, read by `pocket_runtime::Project`.

## 17. How the four checks run

Determinism: the same-process variant is spec-persist's `lockstep` over two `Game`s, after a
reference chain that also catches a refused input; the cross-process variant runs
`pocket check <project> --chain-only --seeds S --json` (which prints
`{seed, chain: [[tick, hash], ...]}`, tick 0 first) and, at a difference, `--snapshot-at T`
(`{seed, tick, snapshot}`, the snapshot's bytes in hex) to name fields. Fork: R and A run to F, B
and C are forks of A, and all four are stepped one after another each tick, R kept as a game so
`fork.original_changed` names the differing fields. Replay: recordings go to `out/check/replays/`
(`--out DIR` elsewhere) and are verified by `pocket replay --verify <file> --json` (which prints
`{ticks_run, identical, divergence, stopped}`); localization makes two copies, the first write at or
after `ticks / 2` dropped and moved one tick later, and the moved one must diverge exactly at its
tick T. Reload: B records with keyframes off and gets the reload before the inputs of its boundary,
as a Host swap applies first.

## 18. Codes added

`<check>.run_failed {project, seed, error}` when a run of `determinism`, `fork`, `replay` or
`reload` stops before it compares anything (a fault, a project that does not load), carrying the
problem that stopped it; `replay.perturbation_undetected {tick, perturbation}` when a perturbed copy
replays without naming a tick (the localization would otherwise pass vacuously); the warning
`replay.nothing_to_perturb` when no write is recorded at or after `ticks / 2`. The `types` step runs
the lint (by compiling with it on) and gives the warning `check.not_implemented {what: "tsc"}` until
the type check of script-host.md 7.4 is built.

## 19. The Rust-side controls

`shared-static`, `process-random` and `unrecorded-write` (8.6) are
`crates/pocket-check/tests/controls.rs`, each a test-only system that emits an event carrying the
defect (the event inbox is in the world hash) added through `GameBuilder::system`; `process-random`
draws from one `RandomState` per process, so only the cross-process variant can see it, and its
child is the test binary run again (`POCKET_CHECK_CHILD`). The same file proves the moved-input
localization through `verify_bytes`.

## 20. The cross-build variant as built (slice 1 review)

`cargo xtask check`'s `determinism` step, outside `--quick`, builds the debug `pocket`
(`cargo build -p pocket-app --bin pocket --locked`) once the release one has answered the probe,
runs `pocket check samples/sailing --chain-only --seeds <first> --json` with both builds and
compares the chains tick by tick (`xtask/src/steps/cross_build.rs`). A difference is
`determinism.diverged {project, seed, variant: "cross_build", divergence, snapshots}`, where
`divergence` names the tick and both hashes and `snapshots` the two `--snapshot-at T` commands whose
outputs `diff` compares: `xtask` links no engine crate, so it does not name fields itself. A debug
build or a chain that fails is `check.command_failed`; the log is
`out/check/determinism-cross-build.log`.

## 21. What `pocket check` refuses and what its children may answer (slice 1 review)

A `check.toml` that does not parse or decode is `check.config_invalid {path, line, message}` with
exit code 2 and one step named `check`, before any check runs (11; `--only types` alone does not
read it); the line is where a typed parse of the file puts the misspelt or mistyped key. A child
process that exits with a status its request does not allow (anything but 0 for a chain or a
snapshot; 0 or 1 for `replay --verify`), prints JSON with an `error`, or prints a chain with an item
that is not `[tick, hash]`, is `check.command_failed {command, status, error, tail}` carrying the
child's own error, which the check reports as such, never as a divergence. When the child cannot
give its snapshot at a divergence's tick, the divergence is still reported, without fields, and the
child's problem is a warning.

## 22. The web step as built (slice 1, the web task)

The page is `web/` (`index.html`, `pocket.js`, `game.js`, `form.js` and the check's `check.js`), not
`crates/pocket-web/check/`: it is the shipped page of the browser-local run form, and the query
`?check` is the runtime flag of 7.1 that turns the check on, so the check loads the page users load.
The step (`xtask/src/steps/web.rs`, `web_pack.rs`) runs for the projects whose `check.toml` has
`[web] workload = true` (the sailing sample), the web page's own negative controls among them
(`[expect] fail` with a `web.` code; another check's controls are not workloads), and is `Skipped`
when none has; it needs the release `pocket` (`check.command_failed` without it). It builds the
module once (`cargo build -p pocket-web --target wasm32-unknown-unknown --profile web --locked`),
binds it with `wasm-bindgen --target web --no-typescript` into `out/check/web/pkg/` and, when
`wasm-opt` is found (`POCKET_WASM_OPT`, else emsdk's under `~/.pocket-tools/`), writes the `-O3`
variant into `out/check/web-opt/`; it writes each workload's web package
(`cargo run -p pocket-web --example pack --release`, threads-slice1.md 14), copies its inputs, runs
`pocket hashes <project> --seed S --ticks N --inputs <file>` for every seed of `[run]` (every seed
rather than open choice 6's first: a 600-tick sailing run takes under a second in the worker),
copies the replays the `replay` step wrote under `out/check/replays/` (a missing one is the warning
`web.replay_missing`; a control has none, since the `replay` step does not run it), and lists them
in `expected.json`, a control with its `expect`.

It loads `index.html?check` three times through `tools/webcheck.py` on its own server: the plain
module isolated and not (7.3; passing only isolated is `web.isolation_required`), and the `-O3`
variant once, isolated, whose measurements carry the suffix `.O3`; timings are taken from the
isolated loads only, since without isolation Chrome coarsens `performance.now()` to 0.1 ms. The page
runs, per workload: every seed stepped through its inputs, sent as commands with `at`, the second
seed by a page that acknowledges snapshots 25 ms late, so the worker skips publications; the first
seed in real time at 8 times; each run's world hash after every tick against the native chain
(`web.cross_target_diverged {project, seed, tick, native, web, variant}`); every snapshot rebuilt
and checked, versions one apart, the hash stream whole, never more than two snapshots unacknowledged
(`web.threads_failed {test}`, `test` one of `rebuild`, `publication_order`, `hash_stream`,
`flow_control`, `ordering`); threads.md 11's ordering test through messages, on the sailing sample
(slice 1, review: the sample's only same-tick inputs from two sources touch different entities, so a
wrong canonical order changed no hash the comparison sees): six arrival orders of four `world_edit`s
of `Sloop.Boat.rudder`, from `"editor"`, `{developer: 0}`, `{developer: 1}` and `{player: 0}` with
one `at`, each in a fresh worker, must give one world hash and the player's rudder
(`web.threads_failed {test: "ordering"}`; `tests/worker.rs` runs all 24 orders natively); a refused
input (`web.test_failed {test: "inputs"}`); and each replay in the worker (`web.replay_diverged`). A
control runs each seed stepped; its problems are not the page's errors but its outcome,
`controls: [{project, expected, got, caught, errors}]`, and a control not caught fails the page; the
step judges each load's outcome with the project steps' `merge` (8.6): caught is the measurement
`<dir>/control_caught`, not caught `check.control_passed {control, expected}`, caught by another
code `check.control_wrong_failure {control, expected, got}`, and a control the page did not report
`web.test_failed {test: "controls"}`. Warnings: `web.step_errors` (failed invocations during a run)
and `web.flow_control_idle` (the late page never made the worker skip). Measurements:
`web.size.wasm`, `web.size.wasm.gz` (gzip level 9 through Python, which the step needs anyway; no
Brotli, whose crate is not pinned), `web.size.glue`, `web.size.page` (the files the page fetches
before its project), the start-up marks since navigation, the tick, hash, publication, rebuild and
transfer times. Not built: the `tests` page step (no crate registers web tests yet), `shaders` and
`frame` (no renderer), the `perf` step's web budgets and `build.web.memory`; and two of 7.2's
`determinism` workloads, whose fixtures do not exist: the `bodies` workload of numeric.md 5 (200
bodies) and the `physics-platform-math` control (8.6), so the step has no automated negative control
yet (the web task's controls below were run by hand). The physics task (`pocket-physics`, which owns
numeric.md 5's glamx and parry patch) adds both under `tests/fixtures/` with
`[web] workload = true`, the control with `[expect] fail = "web.cross_target_diverged"`; the step
runs them as they are.

Two runs on R1 on 2026-10-03 gave identical hashes at every tick of the three seeds natively and in
the worker, isolated and not, with and without `-O3`; changing one native hash made the page fail
with `web.cross_target_diverged` naming that seed and tick, and sending the inputs one tick late
made every run diverge at tick 40, the first input's tick. After the slice 1 review (R1,
2026-10-03): the page passed isolated and not with the ordering run (six orders, one world hash); a
module built without the canonical sort still matched the native chain on every run and failed only
`ordering` (four world hashes across six orders), which is why the ordering run exists; and a
scratch `expected.json` holding two copies of the sailing workload as controls, one with a native
hash changed, reported that one `caught` and the other not, which the step's judgement turned into
`<dir>/control_caught` and `check.control_passed`.

## 23. A full run on Windows (Pioneer, 2026-10-09)

With every tool of 6.3, 6.4 and 22 installed on R1 (the pinned LLVM, wasm-bindgen 0.2.129, wasm-opt
132, Chrome 155, Python with websocket-client, TypeScript 7.0.2 through `POCKET_TSC`), every step
but `contract` and `perf` runs, and the check passed in full for the first time since the old
rebuild branch once `docs/evidence/xtask/render-gi-lint.diff`, applied in the working tree, cleared
the clippy findings of the files other tracks own this wave and compiled their Metal test on macOS
only (docs/bench/xtask.md, with both runs' reports beside the diff). What it took: the `docs`
step's width rule (9); clippy 1.98.1's findings, with `pocket_sim::math::min_f32` and `max_f32` for
the determinism lists' `f32::min` and `f32::max`; the comparison of the vendored crate (13); ignored
tests that skip what a machine lacks (checks.md 6.2); and the engine version that `pocket-app` and
`pocket-web` now install (versions.md, open choice 6), under which the `web` step's replays pass
because the worker, built from the same tree, reports the native build's source.

A finding in one crate hides every crate above it: `-D warnings` fails that crate, and cargo then
lints none of its dependents, so the first run's nine findings in pocket-assets and pocket-physics
hid pocket-render's 43, and pocket-render's hid pocket-viewport's two on wasm32. Fixing by rounds,
or `cargo clippy --no-deps -p <crate>` for the crates above a failing one, finds them all.

`contract` stays skipped: it needs the contract's conformance checks as code
(`contract.perception.*`, `contract.projection.golden`, `contract.actions.*`), the benchmark
harness's self-test (shared/benchmark/README.md 8), and, for `contract.sync`, a reference commit in
`shared/SYNC.toml` with a way to read the other line's `shared/` at it. `perf` stays skipped: it
needs `bench/budgets/`, `pocket bench <workload> --json`, `cargo xtask perf --calibrate` and
`--set-unset` (budgets.md 8), the page's `perf` step and `build.web.memory`; the `web` step already
measures the module's sizes and the start-up, tick, hash, publication and transfer times it would
report.
