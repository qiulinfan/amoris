# Spike: the native script host (`script-native`)

- Slice 0, charter 4.2 (scripting) and 3.7 (reload equivalence). Code: `spikes/script-native/` (its
  README says how to build and run each part).
- Date: 2026-10-03. Machine: Windows 11, AMD Ryzen 9 270 (8 cores, 16 threads), 15 GB RAM, shared
  with other agents' builds while measuring.

## Question

Can the native script host of charter 4.2 be built as the charter describes it, and what does it
cost? Specifically:

1. A Rust program embedding QuickJS-ng through rquickjs that transpiles TypeScript in process with
   oxc (JavaScript plus a source map) and loads the modules of a project one by one through
   rquickjs's resolver and loader, with no bundler.
2. The determinism setup of 4.2.4, each piece shown to hold by a test: no `std` or `os`;
   `Math.random` from a seeded PCG32 in the host; no `Date`, `WeakRef` or `FinalizationRegistry`;
   transcendental functions from an engine library built from `+ - * /` and `sqrt`; a frozen
   `globalThis`; a memory limit; an execution budget counted in work that stops the same script at
   the same point on every run.
3. A `bevy_ecs` world that scripts read and write through a batch host API (typed arrays, never a
   call per entity), and the rule workload of 4.2.7 timed per tick, against the same workload
   through one host call per entity, with the runtime's memory.
4. Structured errors `{code, message, detail}` naming the TypeScript file, line and column, the tick
   and the entity.
5. The lint of 4.2.5 against module-level mutable bindings.
6. Reload equivalence: reloading unchanged scripts mid-run leaves every following tick's world hash
   unchanged.

## Verdict

**Works, with caveats.** Every piece works and is checked by a test (21 tests,
`cargo test --release`, all pass, also in a debug build). The batch rule, the per-entity rule and
the same rule written in Rust build bit-identical worlds tick after tick, so the boundary loses
nothing.

The caveats:

- **Speed is the interpreter's.** The rule workload costs about 2.0 to 2.5 ms per tick at best and
  about 3 ms typically for 1,000 entities (21 to 27 times the same rule in Rust, by minimum). The
  eight batch API calls take about 0.1 ms of it; the rest is QuickJS interpreting the rule. It fits
  a 60 Hz tick, but scripts at this scale take a fifth of it; the charter's rule that hot logic
  moves into Rust primitives (4.2.4) is load-bearing.
- **The budget's stopping point needs a workaround.** QuickJS counts interrupt polls down in a
  counter that the public API cannot reset, so where a budget stops a script depends on what the
  context ran before. Spinning an empty loop to the counter's boundary before every tick makes the
  stop point a function of the tick alone, at 20 to 35 µs a tick. A one-line QuickJS API would
  remove the spin.
- **QuickJS reports an error at the last located expression** before the failing call, not at the
  `throw`: the line is exact, the column points into the statement.
- **The memory limit and the stack limit cannot be deterministic across platforms**: QuickJS counts
  allocator overhead and pointer-sized structures against the limit and checks the C stack pointer
  for depth (read from its source; only one platform was run). Hitting either must fail the tick as
  an error, never be part of the simulation's semantics.

## What was built

| Piece | Where | How |
|---|---|---|
| Transpiler | `src/transpile.rs` | oxc per module: parse, semantic, transformer (TypeScript stripped, `**` lowered to `Math.pow`), codegen with a source map |
| Script host | `src/host.rs` | One rquickjs `Runtime` and `Context`, a custom `Resolver` and `Loader`, the interrupt handler, systems called per tick, exceptions made structured |
| Determinism setup | `src/prelude.rs`, `ScriptHost::load` | The context's intrinsics, Math replaced, the global graph frozen |
| Host API | `src/hostapi.rs`, `sdk/pocket.ts` | The native module `pocket:host` (only the SDK may import it) and the SDK `pocket` that scripts import |
| Engine math | `src/math.rs`, `math-sweep/` | sin, cos, tan, asin, acos, atan, atan2, exp, log, pow, cbrt, hypot from `+ - * /`, `sqrt`, `floor` and bits |
| RNG | `src/rng.rs` | PCG32 (XSH RR 64/32) as a world resource |
| World | `src/world.rs`, `src/game.rs` | `bevy_ecs` components, a registry scripts reach by name, a single-threaded `Schedule`, the canonical hash |
| Lint | `src/lint.rs` | oxc AST and scopes; three rules |
| Rule workload | `project/` | `rules/damage.ts` (batch), `rules/damage_loop.ts` (batch, plain loop), `rules/damage_each.ts` (per entity), `rules/combat.ts`; the Rust twin is `game::damage` |
| Checks | `tests/determinism.rs`, `tests/host.rs`, `tests/projects/` | 21 tests |
| Measurements | `src/bench.rs`, `examples/qjs_speed.rs`, `bench/math/` | `script-native bench`, `script-native mem` |

### The rule workload

1,000 entities, each with `Health {hp, max}`, `Armor {value}`, `Attack {power, target}`,
`Position {x, y}` and `Stats {kills}` (an integer component). Every tick each living entity hits its
target: damage falls off with distance, passes the target's armor, swings by `Math.sin` of the tick
and doubles on a critical roll from the engine RNG; every hit emits `Damaged`, every kill emits
`Died` and counts a kill; then the dead respawn, the living regenerate and everyone drifts by
`Math.cos` and `Math.sin`. About 990 `Damaged` and 10 to 30 `Died` events a tick.

The batch rule makes eight API calls a tick: one `query` of the five components (ids as a
`Uint32Array`, each field as a `Float64Array`, sorted by entity id), one `randomFill`, four column
writes (`Health.hp`, `Stats.kills`, `Position.x`, `Position.y`) and two `emit` calls with
typed-array columns. `Math.sin` and `Math.cos` are host calls too, three per entity. The per-entity
rule reads and writes one field per call (`entity.get`, `entity.set`, `entity.damaged`,
`entity.died`, `Math.random`): about 19,000 API calls a tick.

### Structured errors

A script exception during a tick comes back as, for example (from `tests/projects/broken`):

```json
{"code": "script.exception", "message": "armor 48 over 1000",
 "detail": {"file": "util.ts", "line": 8, "column": 102, "tick": 3, "entity": 8, "system": "inspect",
            "name": "RangeError",
            "stack": ["checkArmor (util.ts:8:102)", "<anonymous> (main.ts:9:37)", "each (pocket:53:23)", "inspect (main.ts:8:10)"]}}
```

The file, line and column are the TypeScript's, through each module's oxc source map (the thrown
line holds `as number` casts, so the JavaScript columns differ). The SDK's `each(rows, fn)` catches
an exception, names the entity it was thrown for and rethrows it; the host adds the tick and the
system. Host refusals carry their own codes with the same location: `host.component-range` (a value
written to an integer field that is fractional, negative or too large; nothing of the call is
applied), `host.unknown-component` and `host.unknown-field` with a suggestion (`"Helth"` →
`"Health"`), `host.missing-component`, `host.unknown-event`; loading gives `script.syntax`,
`script.module-not-found` (with a suggestion), `script.forbidden-import` and the lint's codes; the
sandbox gives `script.budget-exceeded`, `script.out-of-memory` and `math.not-deterministic`.

### The lint

Three rules, each a structured error with file, line, column and binding:

- `lint.module-mutable-binding`: `let` or `var` at module scope.
- `lint.module-binding-assigned`: any assignment (`=`, `+=`, `++`) to a module-scope binding, from
  anywhere.
- `lint.module-object-mutated`: a write through a module-scope binding: a property assignment or
  increment, `delete`, a mutating method (`push`, `set`, `add`, `delete`, `clear`, `splice`, `sort`,
  `fill`...), `Object.assign` and `Object.defineProperty`. A method call on an imported binding is
  let through (`entity.set(...)` is the SDK, not a mutation).

The host runs it on every module before transpiling (it can be turned off) and refuses to load one
that fails. It cannot see state behind an alias or in a closure; the reload-equivalence check
catches those when they reach the world (`tests/projects/hidden`, a counter in a closure, diverges
on the tick after the reload).

### The world hash

FNV-1a 64 over a canonical walk: every registered component in registry order, its entities in id
order with every field's IEEE bits, then the tick, the RNG's state and increment, and the tick's
events in order. It never reads memory layout or storage order. It is the simplest thing that
answers the spike; the canonical serialization of charter 7.5 replaces it.

## Determinism setup, piece by piece

| Requirement | How | Test |
|---|---|---|
| No `std` or `os` | rquickjs never compiles `quickjs-libc.c`; the resolver accepts only `"pocket"` and relative paths | `no_std_or_os_modules`: `import * as std from "std"` is `script.module-not-found`; `std`, `os`, `console`, `print`, `scriptArgs`, `setTimeout` are undefined; importing `pocket:host` outside the SDK is `script.forbidden-import` |
| `Math.random` from a seeded PCG32 | `Math.random` and the SDK's `randomFill` draw from the world's `Rng` resource | `pcg32_matches_the_reference_implementation` (seed 42, stream 54: `0xa15c02b7 0x7b47f409 ...`); `math_random_draws_from_the_world_rng` (one stream, and its state is the world's) |
| No `Date` | The context is built without the Date intrinsic | `date_weakref_finalization_registry_and_performance_removed` |
| No `WeakRef`, `FinalizationRegistry` | Built without the WeakRef intrinsic (it holds both); also without `performance` | same test |
| Engine math | `Math.sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `exp`, `log`, `pow`, `cbrt`, `hypot` replaced by `src/math.rs`; `x ** y` lowered to `Math.pow` by oxc; `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `expm1`, `log1p`, `log2`, `log10` throw `math.not-deterministic` | `script_math_is_the_engine_library_bit_for_bit` (a script's results over a sweep hash to the Rust library's bits, `**` included); `math_sweep_hash_is_pinned`; `math_accuracy_against_the_platform_libm`; `math-sweep` natively and in wasm32-wasip1 |
| Frozen `globalThis` | After the setup, a walk freezes `globalThis` and every object reachable from it (values, accessors, prototypes) | `global_object_is_frozen`: writes to `globalThis`, `Math`, `Math.random`, `Array.prototype`, `Object.prototype`, `JSON.parse` all throw `TypeError` |
| Memory limit | `Runtime::set_memory_limit` | `memory_limit_stops_a_script_and_the_host_survives`: 32 MiB, `script.out-of-memory` on tick 1, tick 2 runs and the heap is back under 4 MiB |
| Budget counted in work | The interrupt handler counts its calls (one per 10,000 polls; a poll is a function call or a jump) and stops the script past the tick's limit | `budget_stops_the_same_script_at_the_same_point_every_run`, `budget_interrupt_cannot_be_caught_by_the_script`, `budget_stopping_point_needs_the_phase_sync` |

### The execution budget

QuickJS-ng decrements `ctx->interrupt_counter` on every function call and every jump and calls the
handler when it reaches zero, then sets it back to 10,000 (`JS_INTERRUPT_COUNTER_INIT`). The handler
here counts its calls; past the tick's limit it returns true and QuickJS throws an uncatchable error
(a script's own `try`/`catch` cannot swallow it). With a limit of 20 units the spin script of
`tests/projects/budget` stops after iteration 69,997 on every run: three polls per iteration (the
loop's jump, the SDK call, the host call), 21 × 10,000 = 210,000 polls.

The counter lives in the context and the public API cannot reset it, so without help the stop point
depends on everything the context ran before: the same spin stopped at iteration 67,084 on tick 1 of
one project and 69,592 on tick 2 of another that did light work on tick 1
(`budget_stopping_point_needs_the_phase_sync`). A reload, a fork on another host or a different
earlier tick would move it. The host therefore runs an empty `for (;;) {}` before every tick with
the handler set to stop at its next call, which leaves the counter at exactly 10,000; then both
cases stop at 69,997. A unit is coarse against the workload (the batch rule uses 6 units a tick),
which suits a safety limit.

## Measured numbers

All in release builds (`cargo build --release`, Rust 1.98.1, `debug = "line-tables-only"`). The
machine was shared with other agents' builds (CPU load 11 to 34 % before the runs), so every variant
was ticked in the same rounds in a rotating order, and the tables give each run's median and minimum
per tick; the minimum is the best estimate of the uncontended cost.

### Rule workload per tick (1,000 entities)

Command: `script-native bench` (50 warm-up ticks, then 1,000 measured ticks per variant; two runs
per build). Times include every boundary call: the Rust schedule, the query, the script, the writes
and the events. QuickJS compiled by MSVC 14.50 (the default) and by clang-cl 23.1.2
(`CC_x86_64_pc_windows_msvc=clang-cl`, README).

| Variant | MSVC median (run 1 / 2), µs | MSVC min, µs | clang-cl median, µs | clang-cl min, µs |
|---|---|---|---|---|
| Batch, SDK `each` (`main.ts`) | 3,031 / 3,317 | 2,517 / 2,545 | 3,169 / 2,985 | 2,289 / 2,018 |
| Batch, plain loop (`main_loop.ts`) | 2,872 / 3,237 | 2,421 / 2,395 | 2,818 / 2,884 | 1,982 / 1,983 |
| Batch without the phase sync | 2,950 / 3,325 | 2,488 / 2,609 | 2,915 / 2,912 | 2,011 / 1,990 |
| Per entity (`main_each.ts`) | 7,659 / 10,147 | 6,035 / 5,989 | 8,889 / 9,175 | 5,827 / 5,808 |
| The same rule in Rust | 135 / 156 | 92 / 93 | 177 / 156 | 104 / 98 |

The batch rule's p99 was 4,198 / 4,175 µs (MSVC) and 4,678 / 5,172 µs (clang-cl). All variants built
the same world at every one of the 1,050 ticks (`sameHashesAllVariants: true`).

Where the batch tick goes:

- **The eight API calls**: 103 / 115 µs a tick (MSVC), 149 / 122 µs (clang-cl), measured inside the
  host functions (`batchTimed.inApiCallsUs`): 3 to 5 % of the tick.
- **The 3,000 `Math.sin` and `Math.cos` host calls**: about 0.36 to 0.42 ms a tick, derived from the
  per-call cost below rather than measured in the tick: 12 to 17 %.
- **The rest, about 80 %, is QuickJS interpreting the rule.** `examples/qjs_speed.rs` (no host at
  all) measures QuickJS at 38.6 ns per iteration of `s += i * 0.001`, 51.3 ns for an integer loop,
  79.4 ns with a function call per iteration and 85.5 ns for a `Float64Array` read-modify-write
  (MSVC); clang-cl gives 38.3, 37.3, 61.7 and 53.9 ns. QuickJS turns off its computed-goto dispatch
  under `_MSC_VER`, so both Windows builds use the switch.
- **The SDK's `each`** (a closure call and a `try` per entity, for the entity in error reports)
  costs 2 to 15 % against the plain loop by minimum, inside the noise by median.
- **The phase sync**: 21 / 25 µs per call (MSVC), 35 / 31 µs (clang-cl), mean of 1,000 calls
  (`phaseSyncUs`); the tick times with and without it do not separate in this noise.
- **Per entity against batch**: about 19,000 API calls a tick (18,966 in the measured tick) make the
  tick 2.4 to 2.9 times slower by minimum.
- **QuickJS built by clang-cl** is 15 to 21 % faster than MSVC by minimum (the batch rule: 2,018 to
  2,289 µs against 2,517 to 2,545); the medians do not separate on this machine.
- Work per tick: 6 units (60,000 polls) for the batch rule, 5 for the per-entity rule.

### Math per call

Command: `script-native bench` (`math`): loops of 100,000 calls in the hardened context, less the
same loop without the call; nanoseconds per call, two runs per build.

| Function | MSVC | clang-cl |
|---|---|---|
| `Math.sin`, the engine's (Rust through the host) | 140 / 123 | 134 / 120 |
| `repro.sin`, the same algorithm in TypeScript on QuickJS | 1,063 / 1,100 | 904 / 740 |
| `Math.sin` of QuickJS itself (the platform's libm; not used) | 53 / 53 | 17 / 34 |
| `Math.atan2`, the engine's | 160 / 159 | 160 / 214 |
| `repro.atan2` in TypeScript | 1,242 / 1,303 | 1,126 / 1,053 |
| `math::sin` called from Rust | 17 / 17 | 16 / 14 |
| The empty loop, per iteration | 43 / 41 | 49 / 51 |

### The engine math library on two targets

Command: `math-sweep` natively and as wasm32-wasip1 under Node 25.8.1's WASI (README). The sweep is
4,001 arguments through 13 functions.

| | x86_64-pc-windows-msvc | wasm32-wasip1 |
|---|---|---|
| Engine library (`math::sweep_hash`), release and debug | `63a6df8bf5dd011c` | `63a6df8bf5dd011c` |
| Platform libm (`f64::sin` and the rest), release | `52dfbe5cfda405c2` | `f31cff9626d1eda8` |
| Points where the engine's `sin` differs from the platform's | 639 of 4,001 | 638 of 4,001 |

The platform's functions disagree between the two builds; the engine's agree to the bit. Against the
MSVC libm (`math_accuracy_against_the_platform_libm`, 200,001 arguments) the largest errors are, in
units in the last place: sin 1, cos 1, atan 4, atan2 4, exp 1, log 2, pow 16, cbrt 2. `pow` is
`exp(y log x)` in plain doubles, so its error grows with the result.

### Loading, reloading, transpiling

Command: `script-native bench`.

- Loading the project (create the runtime and context, the setup and the freeze, then resolve, lint,
  transpile, compile and evaluate `main.ts`, `rules/damage.ts`, `rules/combat.ts`, the SDK and
  `pocket:host`): 4.2 to 5.8 ms.
- Reloading it mid-run (drop the runtime, load again from disk): 5.0 to 5.5 ms;
  `script-native check-reload` reported 4.3 ms.
- oxc per module (parse, semantic, transform, codegen with map): 6 to 7 µs for `main.ts` (4 lines),
  17 to 24 µs for `rules/combat.ts` (29 lines), 59 to 76 µs for `rules/damage.ts` (59 lines).

### Memory

Command: `script-native mem [--native | --entry main.ts | --entry main_each.ts]`: one game alone in
the process for 1,000 ticks; Windows' counters for the process and QuickJS's own
(`Runtime::memory_usage`).

| Rules | Working set after, MiB | Peak, MiB | Private bytes after, MiB | QuickJS heap |
|---|---|---|---|---|
| Rust | 4.6 | 4.7 | 0.9 | none |
| Batch scripts | 7.3 | 7.3 | 1.8 | 238,355 bytes in 2,722 allocations: 619 objects, 37 functions, 704 atoms |
| Per-entity scripts | 7.1 | 7.1 | 1.5 | 236,288 bytes |

The script host adds about 2.7 MiB of working set (mostly the pages of QuickJS's and oxc's code) and
0.9 MiB of private memory. The typed arrays a query returns are built from Rust vectors
(`ArrayBuffer::new`), so they use Rust's allocator and do not count against QuickJS's heap or its
limit; they are freed when the tick drops them.

### Checks

| Command | Result |
|---|---|
| `cargo test --release` | 21 passed (12 in `determinism`, 9 in `host`) |
| `cargo test` (debug) | 21 passed |
| `cargo clippy --release --all-targets` | no warnings |
| `script-native check-reload --ticks 60 --at 20` | `{"equivalent": true, ...}` |
| `script-native check-reload --project tests/projects/hidden --ticks 60 --at 20` | `{"equivalent": false, "firstDivergentTick": 21, ...}` |
| `script-native check-reload --project tests/projects/counter ...` | refused at load: `lint.module-mutable-binding` (`let n`, line 4); with `--no-lint`, first divergent tick 21 |
| `script-native lint --project tests/projects/lint` | 8 problems in `bad.ts`, none in `clean.ts`, exit 1 |
| `script-native lint` (the workload) | no problems, exit 0 |

Every source file is under 800 lines (the largest, `src/host.rs`, has 508).

## Decision: the math library lives in Rust, behind the host API

The library is `src/math.rs`, installed as `Math.sin` and the rest before the global object is
frozen, rather than a TypeScript module in the SDK. The reasons, in order:

1. **One implementation for scripts and engine.** The showcase's buoyancy, wind and physics are Rust
   systems (charter 4.3) and need the same functions; a script that predicts what a Rust system
   computes gets the same bits. Two implementations would each need their own pinned test and could
   still disagree, as on `master`, where the C++ library worked in float and the TypeScript one in
   double.
2. **Bit-identity rests on Rust alone.** Rust's `f64` arithmetic is IEEE 754 on every target and is
   never contracted into fused multiply-adds; the sweep hash is equal natively and in wasm32-wasip1.
3. **It is faster**: 120 to 140 ns a call against 740 to 1,300 ns for the same algorithm
   interpreted.
4. **Scripts keep writing `Math.sin` and `x ** y`**, as the corpus does; nothing new to learn and
   nothing to forget.

The costs: each call crosses the boundary (70 to 100 ns more than QuickJS's own C `Math.sin`); and
the fallback debugger of charter 4.2.6, which runs the same TypeScript in Node against a host-API
shim, needs the same functions in that shim. `math-sweep` already builds the library alone, so the
shim can load it as WebAssembly.

## Recipe for slice 1

1. **Crates**: `rquickjs` `=0.14.0` with `default-features = false, features = ["loader"]`; the oxc
   crates at one version (`oxc_allocator`, `oxc_ast`, `oxc_ast_visit`, `oxc_parser`, `oxc_semantic`,
   `oxc_transformer`, `oxc_codegen`, `oxc_span`, `oxc_syntax`, `oxc_diagnostics` `=0.152.0`,
   `oxc_sourcemap` `=9.0.0`); `bevy_ecs` `=0.19.1` with
   `default-features = false, features = ["std"]`; `serde_json` with `float_roundtrip`.
2. **Context**:
   `Context::custom::<(Eval, RegExpCompiler, RegExp, Json, MapSet, TypedArrays, Promise, Proxy)>`.
   That leaves out Date, WeakRef (with FinalizationRegistry), `performance` and DOMException; `std`
   and `os` never exist because rquickjs does not build `quickjs-libc.c`.
3. **Before any module loads**: replace Math's transcendental functions with the engine library
   (with `Coerced<f64>` arguments, so `Math.sin("1")` behaves as JavaScript says), `Math.random`
   with the world RNG, and the rest of the libm-backed functions with throwing stubs; then freeze
   the whole graph reachable from `globalThis` (`src/prelude.rs`).
4. **Transpile per module** (`src/transpile.rs`): `Parser` with
   `SourceType::ts().with_module(true)`, `SemanticBuilder`, `Transformer` with
   `env.es2016.exponentiation_operator = true`, `Codegen` with `source_map_path`; keep each map as
   `SourceMap::into_owned()` by module name.
5. **Load one by one** (`src/host.rs`): one type implementing rquickjs's `Resolver` and `Loader`;
   module names are paths relative to the project; `"pocket"` is the SDK (TypeScript, transpiled
   like the rest), `"pocket:host"` a native `ModuleDef` that only the SDK may import, whose
   functions find the host's state through `Ctx::userdata`. Lint, then transpile, inside
   `catch_unwind`: a panic in a loader callback aborts the process. A failure is stored as a
   structured error and the load returns `Error::new_loading_message`; the host reports the stored
   error.
6. **Batch API**: a query returns `{count, ids: Uint32Array, Component: {field: Float64Array}}`
   sorted by entity id; a write takes one field of one component for a list of ids and checks every
   value (integer fields take whole numbers in range) and every entity before applying any; events
   cross as typed-array columns, one call per kind. Entity ids from scripts arrive as numbers and
   are checked as whole numbers (rquickjs's own `u32` conversion truncates 1.5 to 1 silently).
7. **Errors**: on `Error::Exception` take `ctx.catch()`, read `message`, `name`, `stack`, the host's
   `code` and `detail` and the SDK's `entity`; parse QuickJS's frames (`at name (file:line:col)`)
   and map each through its module's source map.
8. **Budget**: count interrupt-handler calls per tick, sync the counter's phase before each tick (or
   patch QuickJS, Open), and treat budget, memory and stack overflows as errors that fail the tick.
9. **Ticks**: the `Schedule` with `SingleThreadedExecutor`, then the systems the entry module
   exports as `systems`, in order; drain QuickJS's job queue after them.
10. **Carry the tests over**: `tests/determinism.rs` and `tests/host.rs` are the local checks this
    host needs (charter 3.7); add the rule-workload benchmark to the performance checks.
11. **On Windows, build QuickJS with clang-cl** (`CC_x86_64_pc_windows_msvc=clang-cl`, LLVM 23.1.2):
    15 to 21 % faster on the workload by minimum, with no source change.

## Versions and sources

| What | Version | Source |
|---|---|---|
| Rust | 1.98.1 (`rust-toolchain.toml`) | https://www.rust-lang.org |
| rquickjs, rquickjs-core, rquickjs-sys | 0.14.0 | https://crates.io/crates/rquickjs/0.14.0, https://github.com/DelSkayn/rquickjs |
| QuickJS-ng, as vendored by rquickjs-sys 0.14.0 | 0.16.2 (`QJS_VERSION_*` in its `quickjs.h`) | https://github.com/quickjs-ng/quickjs |
| oxc crates | 0.152.0 | https://crates.io/crates/oxc_transformer/0.152.0, https://github.com/oxc-project/oxc |
| oxc_sourcemap | 9.0.0 | https://crates.io/crates/oxc_sourcemap/9.0.0 |
| bevy_ecs | 0.19.1 | https://crates.io/crates/bevy_ecs/0.19.1 |
| serde_json | 1.0.151 | https://crates.io/crates/serde_json |
| cc (builds QuickJS) | 1.6.0 | https://crates.io/crates/cc |
| MSVC | 14.50.35717 (Visual Studio 18 Community) | installed on the machine |
| clang-cl | 23.1.2 | `C:/Users/rynne/.pocket-tools/llvm-23.1.2`, https://github.com/llvm/llvm-project |
| Node (WASI runner for `math-sweep`) | 25.8.1 | installed on the machine |
| PCG32 | reference `pcg32_srandom_r` and `pcg32_random_r` | https://www.pcg-random.org |
| Math algorithms | ported from Amoris Pioneer `master` 50514a2d `sdk/runtime/repro.ts` | this repository's `master` |

Nothing was downloaded beyond crates. The clang-cl build used its own target directory outside
the checkout, because the C compiler is an input of rquickjs-sys's build script and switching it in
the shared directory would rebuild the other agents' QuickJS.

## Problems met

- **Entity ids are not spawn order.** A `bevy_ecs` 0.19 `World` holds an entity of its own at index
  0, so the first spawned entity is 1. The scenario first set targets by spawn index; the batch and
  Rust rules silently skipped the missing targets and still agreed with each other, and the
  per-entity rule reported it as `host.missing-component` ("entity 0 has no Position") through a
  structured error. Targets now come from the spawned ids. The world hash uses raw entity indices,
  which depend on how many entities the engine makes before the scenario; `master`'s networking
  lesson (hash entities by their place) applies.
- **A panic in a loader callback aborts the process.** The lint asked oxc's semantic for AST nodes
  it had not built and panicked inside rquickjs's loader trampoline, which cannot unwind through
  QuickJS's C frames. Fixed by building the nodes and by `catch_unwind` around lint and transpile in
  the loader.
- **The lint flagged SDK calls.** `entity.set(...)` on an imported binding looked like a mutating
  method; method calls on imports are now let through, property writes on them still reported.
- **The budget's stop point moved** with earlier work; see the phase sync above.
- **A double lost its last bit through JSON.** A probe returned `0.9276185126548473` and
  `serde_json` parsed it as `...472`; fixed with `serde_json`'s `float_roundtrip`. Anything that
  carries doubles as JSON (snapshots, the MCP contract) needs exact parsing.
- **QuickJS's error position** is the last located expression before the failing call (here the
  template's `limits.max`), not the `throw`; the test pins that and checks the line.
- **The benchmark was noisy** on a machine shared with other agents: means drifted upwards within a
  run of one variant after another. The variants now tick in the same rounds in a rotating order.
- **Clippy** rejected the approximate constants (`PI`, `FRAC_PI_2`, `LN_2`, `FRAC_1_SQRT_2`); they
  come from `std::f64::consts` now (the same values).

## Open

- **Interrupt counter**: ask quickjs-ng for a way to reset `interrupt_counter` (or carry a one-line
  patch with the debugger patch of charter 4.2.6), which removes the 20 to 35 µs spin per tick.
- **A failed tick keeps its partial writes.** Writes reach the world as the script makes them;
  rolling a failed tick back needs the snapshot and restore of charter 3.3, or writes buffered until
  the systems return.
- **Platform-dependent limits.** Where a memory limit or the C stack stops a script differs between
  platforms (QuickJS counts allocator overhead, pointers are 4 bytes in wasm32, and the stack check
  reads the C stack pointer). Stack depth was not tested.
- **Math coverage and accuracy**: `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `expm1`,
  `log1p`, `log2` and `log10` throw for now; `pow` is within 16 ulps and would need an
  extended-precision `log` to do better.
- **Not tried here**: the debugger patch (PR #1421), building this host for `wasm32` (only the math
  library ran as wasm32-wasip1), precompiling modules to bytecode, `.d.ts` generation and `tsc`
  checks (the SDK's types are loose; `rows.Health as Columns` casts), entity generations (nothing
  despawns).
- **Speed**: interpretation is 80 % of the tick. What would cut it, unmeasured: bulk math over typed
  arrays (one host call for a column of `sin`), fewer function calls per entity in rule code,
  bytecode precompilation (load time only). The performance budgets of charter 3.10 should take the
  numbers above as the interpreter's baseline on this machine.
- **The lint is heuristic**: an alias (`const t = TABLE; t.x = 1`) or state in a closure escapes it,
  and state that never reaches the world or the events is invisible to the reload check (and
  harmless to it).
- **Freezing the intrinsics** makes a strict-mode assignment to a property that a frozen prototype
  defines (`obj.toString = f` on a plain object) throw, the "override mistake"; class syntax and
  object literals are unaffected. `eval` and the `Function` constructor remain (no determinism
  risk).
- **QuickJS seeds its own hash and random state from the clock** (`ctx->hash_seed`,
  `ctx->random_state`); the hash only places Map entries in buckets (iteration follows insertion)
  and `Math.random` is replaced, so neither shows, but a QuickJS upgrade should be checked for new
  uses.
