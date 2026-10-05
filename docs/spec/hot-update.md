# Hot update

Status: Draft, slice 0

Charter: 3.7 (reload equivalence), 4.2.2 (hot update without a new release), 4.2.5 (the backstop for
stateless scripts), 4.2.6 (scripts swapped at a tick boundary, the old ones kept on failure with a
structured error, new ones tried in a fork first; schema changes through versions and migrations),
5.1 (commands applied at a tick boundary), 7 item 14 (hot-update semantics).

This specification fixes how a running game's scripts are replaced: the request, where each stage of
the work runs, the tick boundary at which the swap lands, what happens to the world's project
components, how a candidate is tried in a fork first, what is kept when anything fails, how replays
reproduce a swap, and the reload-equivalence property the local check holds the engine to. The
script host itself is [script-host.md](script-host.md).

## 1. Related specifications

| Tag | Owner and files | Concepts used here |
|---|---|---|
| [script] | spec-script: [script-host.md](script-host.md), [script-sandbox.md](script-sandbox.md) | `ScriptSource`, `compile`, `CompiledSet`, `instantiate`, `Program`, `check_project`, `ScriptError`, the lint, faults, project components and their `migrate` steps |
| [sim] | spec-sim: [simulation.md](simulation.md), [rng.md](rng.md) | boundaries, boundary writes, hot update as a boundary write (simulation.md 4.7), run conditions, the event inbox, RNG streams |
| [persist] | spec-persist: [persistence.md](persistence.md), [replay.md](replay.md), [versions.md](versions.md) | `Bundle` and its hash, fork, `lockstep`, `Divergence`, the recorder and replays, `SeekScripts`, schema versions and fingerprints, migrations of the live world (versions.md 8.5), retirements, run configuration, the bundle bound to each world (versions.md 7.5) |
| [arch] | spec-arch: [threads.md](threads.md), [checks.md](checks.md), [architecture.md](architecture.md), [budgets.md](budgets.md) | the game thread, sources (`Host`, `Editor`, `Developer`), Writes, Requests and their order at a boundary, worlds and branches, work off the game thread, the reload-equivalence check, the `transpile` feature, budgets |
| [contract] | spec-contract: [time.md](../../shared/contract/time.md), [errors.md](../../shared/contract/errors.md) | time modes and halts, the error protocol `{code, message, detail}` (`Problem`) |
| [mcp] | spec-mcp: [mcp.md](../../shared/contract/mcp.md) | the apply tool through which agents submit scripts |

## 2. What master teaches

Master's `project.apply` bundled, type-checked and then reloaded the project, which meant a fresh
world from the scene with the scripts started over it (master `docs/mcp.md`, Applying an edit).
Three of its benchmark failures came from that reload (master `docs/agent-eval.md`, Results):

- **`frozen_coins`**: a script error stopped the simulation, and nothing was dispatched to scripts
  while one stood, including the unload a reload sends; the old handlers stayed, the new bundle's
  joined them, and the old tick threw again. The agent spent 35 calls finding out why. Here a swap
  replaces the whole program at once, whatever state the session is in (4, 7).
- **`dodge`**: the reload did not reseed the random numbers, so the restarted game differed from a
  fresh run. **`coin_chime`**: a held action outlived the reload. Master then made a restart reseed,
  reset the clock and release held actions. Here the two operations are kept apart (3): a hot update
  changes only the scripts, and a restart is a new run.

Master's reload also dropped whatever its scripts held in module variables, which was game state
(master ADR 0007). Charter 3.2 moves that state into the world, which is what makes a swap cheap:
"stateless scripts mean a reload migrates no state inside the VM" (charter 4.2.2).

## 3. Terms

- **Hot update**: the world, its tick, its RNG seed, the event inbox and every held control stay;
  only the `Program` running the script systems changes, at one boundary. Project components whose
  schema changed are migrated at the same boundary (6). The bundle is a binding the runtime keeps
  per world (versions.md 7.5): a hot update addresses one world, `main` by default or a branch, and
  changes no other.
- **Restart**: a new world from the scene and the seed, starting at tick 0. It is a new run, not a
  hot update: [mcp]'s `reset`, or its `apply {restart: true}` with new scripts. Because all state is
  in the world, a restart has nothing to reset beyond building a new world, so master's half-reset
  (2) cannot recur.
- **Reload**: a hot update whose bundle is unchanged. It must change nothing (9).
- **Candidate**: a compiled and instantiated bundle not yet installed.

## 4. Requests and stages

### 4.1 Commands

Hot update uses commands of the runtime's catalog ([arch], threads.md 2 and 5), with JSON parameters
refused on unknown keys like every command:

| Command | Kind, sources | Parameters | Effect |
|---|---|---|---|
| `scripts.apply` | Request (threads.md 2 and 5.5): routed to a runtime worker, never recorded; Editor, Developer | `world?` (default `main`), `files?` (path to text; default: the project's `scripts/` read from disk), `trial?` (`{ticks, compare?}`), `dry_run?`, `force?`, `types?` (`"report"`, `"refuse"`, `"skip"`) | Runs the stages of 4.2 and answers with an `UpdateReport` |
| `scripts.revert` | The same | `world?` | `scripts.apply` with the previous bundle the runtime kept for that world (`scripts.no_previous` when there is none) |
| `scripts.status` | Read; any | `world?` | The world's current and previous bundle hashes, the systems, the last report, the latest script failure |
| `scripts.swap` | Write; Host only | `{world, bundle}` | The swap itself (5), recorded in that world's recording; produced by the runtime from a prepared candidate |

[mcp]'s apply tool and the editor's watcher send `scripts.apply`. The debugger's attach and detach
are Controls (threads.md 3.5) that, at a boundary, reinstantiate the current bundle with or without
instrumentation through steps 1 and 4 of the swap (5); they are not recorded, because they change
nothing (9).

```rust
pub struct ScriptUpdate {
    pub world: WorldRef,             // the world whose bundle changes; default main
    pub source: UpdateSource,        // Disk | Files(ScriptSource) | Compiled(CompiledSet)
    pub trial: Option<Trial>,
    pub dry_run: bool,               // stop after the trial; no swap
    pub force: bool,                 // swap even when the bundle hash is unchanged (9)
    pub types: TypeCheck,            // Report (default) | Refuse | Skip
}
pub struct Trial { pub ticks: u32, pub compare: bool }  // ticks at most 3,600 (chosen)
pub struct UpdateReport {
    pub outcome: UpdateOutcome,      // Applied | Unchanged | DryRun
    pub applied_at: Option<Tick>,    // the first tick the new scripts run
    pub bundle: ContentHash, pub previous: ContentHash,
    pub systems: SystemsDiff,        // added, removed, kept: system names
    pub components: Vec<SchemaChange>, // Added, Migrated { from, to }, Retired, Unchanged
    pub warnings: Vec<ScriptError>,  // script.module_unused and the like
    pub type_errors: Vec<ScriptError>,
    pub trial: Option<TrialReport>,
    pub timings: UpdateTimings,      // per stage, microseconds, by the runtime's injected clock
}
```

### 4.2 Stages

| # | Stage | Runs on | Fails with (`scripts.refused`, `detail.stage`) |
|---|---|---|---|
| 1 | Gather: read `scripts/` (or take `files`) into a `ScriptSource` | a runtime worker thread (threads.md 6) | `script.source_encoding` and file errors |
| 2 | Compile: parse, lint, transform ([script] 8.2) | the worker; needs the `transpile` feature | `script.syntax`, `lint.*` |
| 3 | Type check: `check_project`'s `tsc` step ([script] 7.4), natively when `tsc` is installed | the worker | with `types: "refuse"`, `types.error`; otherwise reported in `type_errors` |
| 4 | Unchanged? The bundle hash equals the current one and `force` is false: answer `Unchanged`, no write | the worker | |
| 5 | Prepare: instantiate in a host of the worker's own ([script] 9) and plan the schema changes (6) against a copy of the live registry | the worker | `script.*` load codes, `version.*`, `migrate.missing_step`, `migrate.missing_removal` |
| 6 | Trial, when asked (8) | the worker, natively | `scripts.trial_failed` |
| 7 | Dry run: answer with the report, no write | | |
| 8 | Enqueue `scripts.swap` as a Host write; the reply waits for it | | |
| 9 | Swap at the boundary (5) | the game thread | `script.*` load codes, `migrate.failed` |

Stages 1 to 7 never touch the world, so any failure there leaves the session exactly as it was, and
a slow compile or trial never stalls a tick (charter 5.1). Requests are handled one at a time in
arrival order; a second `scripts.apply` waits for the first to answer. In the headless, batch and
check forms, which drive `Game` without a thread, `Game::apply` runs a Request's stages inline on
the caller's thread before it returns (threads.md 5.5), so the reload check of checks.md 8.5 sees
the same stages and the same `scripts.swap` write.

The shipped web build has no `transpile` feature ([arch], architecture.md 4.6): its worker accepts
only `Compiled` sources (packed natively, or sent by a browser editor built with `transpile`),
refuses TypeScript with `scripts.transpile_unavailable`, and runs stages 5, 7, 8 and 9 in the game
worker before the next tick, so ticks wait for them (budget `web.swap`, budgets.md 5.4). Trials
(stage 6) are refused on the web in slice 1 with `scripts.trial_unavailable`: a trial of up to 3,600
ticks run in the game worker would hold main's ticks for seconds, which threads.md 7.3's 8 ms slices
forbid (open choice 4). A recorded `scripts.swap` replayed in a build without `transpile`
instantiates the bundle's embedded compiled modules (replay.md 2.4).

**Lockstep.** In lockstep pacing a swap lands on every peer or on none: a `scripts.swap` that does
not arrive through the lockstep input is refused with `time.wrong_mode`. A swap in lockstep is
broadcast as a lockstep input with `at: Some(T)` and applied by every peer at boundary T - 1
(threads.md 5.2; shared/contract/time.md, Lockstep), each peer preparing the same `Compiled` bundle
first.

## 5. The swap

`scripts.swap {world, bundle}` is a Host write: it applies at a boundary of `world` before the
Editor's, Developers' and players' commands of that boundary (threads.md 5.2), so the next tick runs
the new scripts and every later command of the batch is validated against them. Applying it on the
game thread:

1. Instantiate the candidate in the game thread's `ScriptHost` (a `Program` cannot cross threads:
   its QuickJS-ng context belongs to one runtime). Stage 5 has already done this once, so a failure
   here is rare, but it is handled the same way.
2. Re-check the schema plan against the live registry, which another write in the same batch cannot
   have changed (only swaps change project schemas, and requests are serialized).
3. Migrate the world when the plan says so (6).
4. Install: the `Program` replaces the current one; the previous bundle is kept for
   `scripts.revert`; components the new bundle adds are registered, reusing the existing
   `ComponentId` of every component whose fingerprint is unchanged.
5. Hand the swap to the recorder: `Recorder::bundle_loaded` with the new `Bundle`, then the applied
   write (replay.md 2.3), and the new format table when a project component was added or changed
   (versions.md 4).
6. Answer the requester with the `UpdateReport`, `applied_at` being the next tick.

If step 1 or 3 fails, step 4 has not happened and a failed migration is undone (versions.md 8.5
restores the pre-migration snapshot): the old `Program` stays installed, the world is unchanged, and
the write is refused with the errors, so it is not replayed (threads.md 5.3). The new scripts'
systems take their places in `Update` from the next tick, in the new `systems` order with their run
conditions (simulation.md 4.7); a `when: "start"` system does not run again, because run conditions
depend on the tick alone.

No tick ever runs half on old scripts and half on new: a swap is applied only between ticks, and the
`Program` is read once at the start of the `Update` phase.

## 6. Project components across a swap

The candidate's project components are compared with the live registry by [persist]'s rules
(versions.md 3.5 and 5), for each name:

| Candidate against live | Plan |
|---|---|
| Same version and fingerprint | Unchanged |
| A component the world has never registered | Added: registered at the swap, version as declared |
| Fingerprint differs, version equal | Refused: `version.unbumped`, naming the changed field and the step to add |
| Version higher, every step present | Migrated: versions.md 8.5 (snapshot, migrate the sections, restore), the `migrate` steps running in the candidate's host ([script] 7.3) |
| Version higher, a step missing | Refused: `migrate.missing_step` |
| Version lower | Refused: `version.downgrade` |
| A live component the candidate lacks | Retired when `game({retired})` names it (its rows dropped or renamed), else refused: `migrate.missing_removal` |

A component's `doc` and defaults are not part of its fingerprint: changing them needs no version,
and new defaults apply to components inserted after the swap. Engine components never change in a
hot update; they change with the engine build. bevy_ecs cannot unregister a component, so a retired
project component stays registered with no instances, and a later bundle may add it again only as a
new version.

## 7. Failures and faults

- **A refused update** keeps the old scripts and the world as they were and answers
  `scripts.refused` with `detail.stage` and `detail.errors`, the list of [script]'s and [persist]'s
  structured errors, each with its TypeScript location where there is one (charter 4.2.6).
- **A script failure before the update.** A running game halts after a failed system
  (script-sandbox.md 5.4; shared/contract/time.md, Halts). A hot update applies all the same
  (master's `frozen_coins`, 2), and a successful swap ends the halt, since the code that failed is
  gone. The failed tick itself is history: to rerun it with the new scripts, restore an earlier
  snapshot first; versions.md 7.5's `seek` with `SeekScripts::Current` loads the session's current
  bundle at the restored boundary, as a hot update would.
- **A failure after the update** is an ordinary script failure (script-sandbox.md 5.4). Nothing
  reverts by itself; `scripts.revert` is one call away, and a trial (8) is how a caller learns of a
  failure before it reaches the main world.
- **A fault** (`script.out_of_memory`, `script.stack_overflow`) in stage 5 or 6 refuses the update
  like any error; during a swap's migration it refuses the swap and leaves the world unmigrated
  (script-sandbox.md 4.3). A refused write is not replayed, so a run that faulted there and its
  replay agree; a replay on a target that faults where the recording did not stops with the fault's
  code (replay.md 3.4). `script.call_depth` is not a fault: it fails the same way on every target.

## 8. Trying scripts in a fork first

With `trial: {ticks, compare}`, stage 6 runs the candidate on a fork before anything reaches the
main world (charter 4.2.6):

1. The worker asks the game thread for a fork (a Read applied at the next boundary whose reply is
   `Reply::Fork`, threads.md 5.5; persistence.md 6.3's `fork`, which yields a world that can move to
   another thread, threads.md 6). The game thread pays only for the fork (the budget `fork.sail`,
   budgets.md 5.1); the trial's own start, with the worker's host instantiating the candidate, is
   the budget `branch.sail`.
2. On the worker, a new `ScriptHost` instantiates the candidate, migrates the fork when the plan
   says so, and steps the fork `ticks` ticks with no boundary writes: what the scripts do from this
   state if no one acts.
3. With `compare`, a second fork from the same boundary runs the current scripts the same way, the
   two in [persist]'s `lockstep`, and the report carries the first divergence (tick, sections and
   fields, replay.md 3).
4. The trial fails, and with it the update, if any script system fails or faults on the fork;
   otherwise the update goes on to stage 7 or 8.

```rust
pub struct TrialReport {
    pub from_tick: Tick,                  // the boundary the fork was taken at
    pub ticks_run: u32,
    pub failures: Vec<ScriptError>,       // every failed invocation, in tick and system order
    pub steps: u64,                       // script steps over the trial (exact)
    pub world_hash: WorldHash,            // the fork's after its last tick
    pub divergence: Option<Divergence>,   // with compare: new against current scripts
}
```

The main world keeps ticking during a trial, so the swap that follows lands at a later boundary than
the fork was taken at; the report says both. The fork is discarded and never recorded: a trial
leaves no trace in the main world or its recording, which is the fork-consistency property applied
to scripts (checks.md 8.3). A `compare` trial of a pure refactoring (renamed locals, reformatted
code) must report no divergence; this is the forward-looking counterpart of versions.md 7.3, which
replays a recording under new scripts and names the first tick it departs.

## 9. Reload equivalence

**Property.** For any run, applying `scripts.apply {force: true}` with the unchanged bundle at any
boundary leaves every later tick hash equal to the run without it (charter 3.7). The local check
holds every project to it (checks.md 8.5): run A has no reloads; run B reloads at the boundaries the
project's check file lists, through the same command an edit uses; the two run in [persist]'s
`lockstep`, and a difference fails with `reload.diverged`, naming the first diverging tick and the
component that differs. `force` exists for this check: a reload the engine skipped because the
bundle was unchanged would make the check vacuous, so B's recording must show every reload
(`reload.not_performed` otherwise).

**Why it holds when scripts are stateless.** A swap replaces only the `Program`. Everything a tick
reads is in the world, which the swap does not touch when the bundle is unchanged: no schema change,
no migration, the same `ComponentId`s (5, step 4). RNG streams are derived from the world seed, the
tick and the system key (rng.md), so the new program draws what the old would have. The event inbox
is world state (simulation.md 5.2), so the new systems see the same events. Step budgets are set at
every call (script-sandbox.md 4.1), so the new context's history cannot move an abort. Run
conditions depend on the tick alone.

**What it catches.** State kept outside the world: a module-level variable the lint missed or that a
fixture runs with the lint off (the `module-state` negative control, checks.md 8.6), a closure made
at load time through one of the paths script-sandbox.md 3 names as outside the lint, and host-side
state kept per `Program` by an engine bug (a cache that survives between calls). It cannot catch
state that a reload happens to rebuild identically, which is harmless by definition.

The debugger's instrumenting reload ([script] 13) is a reload too: attaching and detaching must
leave the hashes and the step counts unchanged ([script] 12, test 3).

## 10. Replays and recordings

A swap is a recorded Host write (5, step 5), so a replay re-applies it at its boundary with the
recorded bundle (embedded by default, versions.md 7.4) and reproduces the run, migration included
(versions.md 8.5). A refused update, a dry run and a trial leave nothing in the recording. Replaying
one recording under other scripts and naming the first divergence is versions.md 7.3's `verify` in
`Compare` mode with a substituted bundle; seeking across a swap and choosing the recorded or the
current scripts is versions.md 7.5.

## 11. What is deterministic and why

The swap is an input of the run: it lands at a boundary that the recording names, and its content is
named by the bundle hash. The candidate is built deterministically from that content ([script] 8,
10), migrations are deterministic script calls ([script] 7.3), and the order of a swap among the
boundary's writes is fixed by its source (threads.md 5.2). Which boundary a live request reaches
depends on timing, like any command's, and the recording keeps it (threads.md 5.2). Trials run on
forks the main world never sees. Nothing of the old `Program` carries over, so nothing of it can
leak into the new one.

## 12. Errors

| Code | When | `detail` |
|---|---|---|
| `scripts.refused` | A stage of 4.2 failed | `{stage, errors: [ScriptError or persist error]}` |
| `scripts.trial_failed` | A script system failed or faulted during the trial | `{trial: TrialReport}` |
| `scripts.transpile_unavailable` | TypeScript sent to a build without `transpile` | `{}` |
| `scripts.trial_unavailable` | A trial asked of the web build in slice 1 | `{}` |
| `scripts.no_previous` | `scripts.revert` before any swap | `{}` |

The codes inside `errors` are [script]'s (`script.*`, `lint.*`, `types.error`) and [persist]'s
(`version.*`, `migrate.*`). They follow [contract]'s error protocol, whose families table lists
`scripts` as this specification's.

## 13. Performance budgets

| Budget | Value | Measured by |
|---|---|---|
| Swap at the boundary for the sailboat scene's scripts, without migration (game thread) | 6.9 ms on the reference laptop: the script-native spike reloaded its rule workload's five modules (a new runtime and context, setup, freeze, resolve, lint, transpile, evaluate) in 4.3 to 5.5 ms, times 1.25 (budgets.md 8.1); the sailboat scripts do not exist yet | the script host bench |
| `scripts.apply` from request to reply for one changed file, without trial (native) | not measured in slice 0 (no spike ran `tsc` or the worker path) | the script host bench |
| A trial: the fork, plus `ticks` ticks on the worker | the budget `fork.sail` on the game thread; `branch.sail` for the fork made ready to step; the rest off it | budgets.md 5.1 |
| Instantiating a `Compiled` bundle at a boundary in the web worker, cold and warm | the budget `web.swap` (not measured; the script-web spike's context, prelude, scripts and freeze took 0.7 to 1.6 ms warm and 13.6 to 20.6 ms cold) | budgets.md 5.4 |

The swap's cost on the game thread is a context with lockdown and module evaluation ([script] 11).
For a smaller workload the script-web spike measured its context, prelude, scripts and freeze at
0.76 to 0.93 ms natively and 0.7 to 1.6 ms warm in the browser (13.6 to 20.6 ms cold).

## 14. Tests and checks

1. **Boundary**: a swap requested during a long `step` lands between two ticks; tick T runs old
   systems only, T + 1 new ones only; a `when: "start"` system does not rerun.
2. **Kept on failure**: for each stage of 4.2, a candidate failing there leaves the world hash and
   the installed bundle as they were, answers `scripts.refused` with that stage and a TypeScript
   location where one exists, and leaves the recording without a write.
3. **Schemas**: an added component registers; a version 1 to 2 change migrates the live world and
   the result equals migrating a save of it; an unbumped change, a missing step, a downgrade and an
   unretired removal are refused with their codes; a step that throws refuses the swap and leaves
   the world unmigrated.
4. **Trial**: a candidate that throws on its third tick fails its trial without changing the main
   world's hash or recording; a `compare` trial of a reformatted bundle reports no divergence; one
   that changes a constant reports the first tick and field it changes.
5. **Reload equivalence** (checks.md 8.5) on every project, with the `module-state` and
   `module-state-lint` controls (checks.md 8.6), and the debugger's instrumenting reloads.
6. **Replay**: a recording with two swaps and one migration verifies tick by tick (versions.md 7.2,
   `Verify`), natively and in the web worker.
7. **After a failure**: a session paused by a failed system takes a hot update that fixes it and
   runs on; the old failing system never runs again.
8. **Web**: the shipped worker swaps a `Compiled` bundle at a boundary, refuses TypeScript with
   `scripts.transpile_unavailable` and a trial with `scripts.trial_unavailable`.
9. **Unchanged**: `scripts.apply` of the current bundle answers `Unchanged` and records nothing;
   with `force` it swaps and records the write.
10. **Worlds**: a swap into branch `b1` changes `b1`'s scripts only; `main`'s hashes equal a run
without it, and `b1`'s recording holds the swap; a fork made after a swap on `main` runs the new
scripts, one made before runs the old.
11. **Lockstep**: a swap sent outside the lockstep input is refused with `time.wrong_mode`; one sent
through it lands on both peers of a two-peer lockstep at the same boundary, with equal tick hashes
after it.

## 15. Open choices

1. **Type errors.** Settled with [mcp]: `types: "report"` by default, as master's apply did (its
   answer listed the first ten type errors and reloaded); `"refuse"` is one parameter away for an
   agent that wants it, and the MCP `apply` tool takes the same parameter.
2. **Trial inputs.** Recommended: forward with no writes (8), which needs nothing but a fork. A
   rewound trial, which replays the last N recorded ticks under the candidate, is versions.md 7.3
   run from a keyframe and can be offered by [mcp] as a separate tool.
3. **Automatic revert** when new scripts fail soon after a swap. Recommended: no; an explicit
   `scripts.revert` keeps every change of scripts an input the caller chose and the recording names.
4. **Trials on the web.** Recommended: refused in slice 1 (4.2); later, trial ticks in 8 ms slices
   interleaved with main's ticks in the game worker, or a second worker with its own host, measured
   against `web.swap` and the web tick budgets.
5. **Slice 1: the swap without the request path.** `pocket_script::swap(world, set, force)` is stage
   5 and 9 of 4.2 on one thread: it instantiates the candidate in the world's host, plans its
   project components (6), registers the added ones and installs the program, keeping the previous
   bundle for `scripts.revert`; any failure leaves the installed program and the world as they were,
   and an unchanged bundle is `Unchanged` unless `force`. It answers a `SwapReport` (`Applied` or
   `Unchanged`, the bundle and the previous one, the systems added, removed and kept, the components
   `Added` or `Unchanged`). The commands of 4.1, the worker stages, trials and recording are
   `pocket-runtime`'s.
6. **Slice 1: schema changes.** Same version with the same field names, types and order is
   `Unchanged` (docs and defaults are not compared); a component the world never registered is
   `Added`; the same version with other fields is `version.unbumped`, a lower one
   `version.downgrade`, a live project component the candidate lacks `migrate.missing_removal`
   (`retired` is not read yet), and a higher version `migrate.missing_step`, since the live
   migration of versions.md 8.5 is not built: such a change needs a restart in slice 1. An
   `Unchanged` component takes the candidate's schema, docs and defaults, in the `ScriptAccess`
   resource beside its accessor (its `ComponentId` stays), which every script call resolves first,
   so components inserted after the swap get the new defaults (6; tests/hot_update.rs). The
   registry's entry keeps the first registration's doc and JSON Schema: `pocket-sim`'s
   `ComponentRegistry` has no way to replace an entry yet, and that crate is not the script host's.
7. **Slice 1: reload equivalence in the crate's tests.** `crates/pocket-script/tests/reload.rs` runs
   the regatta workload with forced reloads at ticks 30, 31 and 77 and with a new host on the same
   thread at tick 50, and compares every tick's digest (choice 14 of script-host.md) with a run
   without them; a fork is a fresh `Sim` on another thread with the same compiled set
   (script-host.md 14, choice 7), and its digests equal the original's at every tick. The
   `module-state` control (a module-level counter, the lint off) first diverges on the tick after
   the reload, and with the lint on it does not compile. The check command's run over whole projects
   with persistence's hash (checks.md 8.5) is the runtime's.
8. **Slice 1: `scripts.apply` and `scripts.swap` in the runtime.** `scripts.apply` takes `files?`,
   `force` and `dry_run` (an unknown key, `trial` and `types` among them, is refused with a
   suggestion until trials and the type check are built) and answers `{outcome, bundle, ...}`. With
   no `files` it compiles the project's own TypeScript again (the lint off only for a negative
   control) or, without the transpiler, reuses the compiled set the game started with. Its stages
   run inline at the boundary (threads.md 13, choice 9) and end in the Host write
   `scripts.swap {bundle}`, which always instantiates the named bundle: it is produced only when a
   swap is wanted, so replaying a forced reload reloads too. A swap names its bundle by hash; a game
   finds it among the bundles it prepared, or builds it from the compiled modules a replay embeds
   (`pocket_runtime::compiled_from_record`), whose systems carry no source sites (only error
   locations use them).
