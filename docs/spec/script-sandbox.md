# Script sandbox: lockdown, the lint, budgets and errors

Status: Draft, slice 0

Charter: 3.2 (state in the world, enforced), 3.3 (determinism), 3.4 (structured errors), 4.2.1 (an
execution budget counted in work), 4.2.4 (the determinism setup), 4.2.5 (enforcing stateless
scripts), 7 item 7 (the global-object freeze and lint rules, the execution budget).

This specification fixes how the script host keeps a QuickJS-ng context free of state that outlives
a call and of results that differ between targets: the intrinsics it offers, the globals it removes
and replaces, the lockdown, the harden epilogue, the stateless-script lint, how work, call depth,
memory and stack are limited, the QuickJS-ng patches that make those limits deterministic, and how
failures come back as structured errors naming the TypeScript line. It was split from
[script-host.md](script-host.md), which keeps the API scripts call, components, module loading, the
Rust API, the tests and the debugging hooks; the tags of script-host.md 1 ([sim], [persist],
[contract], [mcp], [arch]) are used here too.

## 1. Related specifications

| Tag | Owner and files | Concepts used here |
|---|---|---|
| [host] | spec-script: [script-host.md](script-host.md), [hot-update.md](hot-update.md) | the system context and its transaction (5), numbers at the boundary (6), module loading and transpiling (8), the Rust API (9), the tests (12), the debugger (13) |
| [sim] | spec-sim: [simulation.md](simulation.md), [numeric.md](numeric.md) | invocations as transactions, the poisoned world (simulation.md 4.6), the `math` library |
| [persist] | spec-persist: [persistence.md](persistence.md), [replay.md](replay.md), [versions.md](versions.md) | run configuration, the `Fault` record, `verify` |
| [contract] | spec-contract: [shared/contract/](../../shared/contract/README.md) | the error protocol `{code, message, detail}` (`Problem`), halts (time.md), derived facts, instruments and predicates |
| [arch] | spec-arch: [architecture.md](architecture.md), [threads.md](threads.md), [checks.md](checks.md) | threads and their stacks, the vendored QuickJS-ng (architecture.md 7.5), the check command |

## 2. The sandbox

### 2.1 Intrinsics

Each `Program` gets a new QuickJS-ng context in the host's runtime (`Context::custom`) with, beside
the base objects, `Eval` and `Promise` (module compilation and evaluation need them; their globals
are deleted), `RegExpCompiler`, `RegExp`, `Json`, `MapSet`, `TypedArrays` and `Proxy`. Not added:
`Date`, `Performance`, `WeakRef` (with `FinalizationRegistry`), `DOMException`; QuickJS's `std` and
`os` are never loaded. This is rquickjs 0.14.0 with QuickJS-ng 0.16.2, as the spikes ran; on the web
the shim's one clock import, `__rquickjs_host_now_us`, returns 0 (script-web spike, Recipe 3).

### 2.2 Removed and replaced globals

| Global | Treatment | Why |
|---|---|---|
| `Date`, `performance` | Absent | A clock breaks replay (charter 4.2.4); use `ctx.time` or `ctx.tick` |
| `Promise`, `queueMicrotask` | Deleted; the job queue must be empty after every call (`script.async_work`), and a job that throws, overruns its budget or faults fails the call that queued it | Pending work would outlive the tick; the lint refuses async code. The script-native spike's verification found a throwing `.then` and an endless loop in a job reported as success, because its host drained jobs without checking them |
| `WeakRef`, `FinalizationRegistry` | Absent | They depend on when garbage is collected |
| `SharedArrayBuffer`, `Atomics` | Deleted | No threads in a script; `Atomics.wait` blocks |
| `eval`, `Function` constructors | `eval` deleted; `Function` and the `constructor` of function, generator and async function prototypes replaced by functions that throw `script.code_generation` and keep `prototype` (`instanceof Function` works) | Code must come through the loader to be linted, mapped and transpiled: the script-native spike's verification found `**` in code made by `eval` or `new Function` calling the platform's `pow`, which gave other bits than the engine's in 18,056 of 20,000 cases; SES's `lockdown()` tames these the same way |
| `RegExp.prototype.compile` | Deleted | It replaces a regular expression's pattern and bytecode in place: on a frozen regex without `g` or `y`, QuickJS-ng's `js_regexp_compile` writes `pattern` and `bytecode` before its `lastIndex` write throws, so `try { RE.compile(String(n + 1)) } catch {}` keeps a counter that `RE.source` reads later. SES's `lockdown` removes it too |
| `Math.random` | Host function over the system stream (script-host.md 5.6) | Seeded by the world |
| `Math.sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `exp`, `expm1`, `log`, `log1p`, `log2`, `log10`, `pow`, `cbrt`, `hypot` | Host functions over [sim]'s `math` (numeric.md 6.3, with ECMAScript's special cases) | QuickJS-ng calls the platform's libm, which differs between MSVC's C runtime and the web's musl by 1 or 2 ulps in up to a quarter of results (script-web spike, numeric probe) |
| `Math.abs`, `floor`, `ceil`, `round`, `trunc`, `sign`, `sqrt`, `fround`, `f16round`, `min`, `max`, `imul`, `clz32`, `sumPrecise` | Kept | Exact operations. QuickJS-ng computes some through the platform's C library (`floor`, `ceil`, `trunc` and `sqrt` directly, `min` and `max` through `fmin` and `fmax`, the `%` operator through `fmod`; UCRT natively, wasi-libc's musl on the web), exact in a correct library, which the cross-target sweeps of script-host.md 12, test 2, check |
| `console` | Added (script-host.md 5.7) | Output only |

### 2.3 Lockdown

Before any project module evaluates, the host locks the context down as SES's `lockdown()` (Agoric's
Hardened JavaScript) does, for the same purpose: no mutable state reachable from the intrinsics.

1. Replace the `Math` functions and delete the globals of 2.2, `RegExp.prototype.compile` included.
2. Set `Error.prepareStackTrace` to the host's mapper (5.2) and `Error.stackTraceLimit` to 64 (the
   debugger spike's Recipe 7); both are accessors whose setters write context-wide state
   (`ctx->error_prepare_stack`), so each is then redefined as a non-writable data property.
3. Apply override taming to the properties of SES's moderate list that QuickJS-ng has (among them
   `Object.prototype.toString`, `valueOf`, `constructor`, `Error.prototype.name` and `message`):
   each becomes an accessor whose setter defines an own property on the receiver, so
   `this.name = "GameError"` in an `Error` subclass still works.
4. Deep-freeze `globalThis`: every object reachable through own property values, accessor functions
   and prototypes, the prelude's exports included.
5. In debug builds and the lockdown test (script-host.md 12), walk the graph again and fail on any
   extensible object, writable or configurable property, or setter not on the reviewed list (steps 2
   and 3), so a QuickJS-ng upgrade that adds one is caught.

### 2.4 Module state: the harden epilogue

Freezing intrinsics does not freeze a module's own values. The transpiler appends to every project
module a call to the private native `harden` with every value the module holds at its top level:
every top-level binding it declares, exported or not (`const`, function, class and enum bindings,
read from the transformed AST), and every value of its module namespace, the `default` export
included (the `*default*` binding `export default <expression>` creates, which has no name in the
source); imports are hardened by their own module. After the module evaluates, `harden` deep-freezes
each value and refuses the load with `script.module_state` if it reaches state freezing cannot fix:
a `Map`, `Set`, `WeakMap`, `WeakSet`, `ArrayBuffer`, typed array, `DataView`, generator, iterator,
`Proxy`, or a `RegExp` with `g` or `y` (its `lastIndex` must change). Plain objects, arrays,
functions, classes, errors, primitive wrappers, other regular expressions and the prelude's values
pass. The error names the binding:
`scripts/ai.ts: 'cache' holds a Map, which cannot be frozen; keep the data in a component`. So a
top-level `const SPEEDS = [1, 2, 3]` and an `export default { count: 0 }` can be read, while
`SPEEDS.sort()` or `cfg.count++` in a system throws with the hint to copy first; functions and
classes are frozen too. The freeze reaches string and symbol keys only, never private names, which
is why the lint refuses static private members (3).

## 3. The lint

The lint is a pass over oxc's TypeScript AST and scoping inside `compile`, so whatever compiles has
passed it; `check_project` reports the same diagnostics (script-host.md 7.4). Every rule is an error
and there are no suppression comments: charter 3.2 asks the runtime to enforce the rule, not advise
it. The one switch, `CompileOptions::lint_off`, exists for the check command's `module-state`
negative control (checks.md 8.6), which needs a stateful script to reach the reload-equivalence
check; sessions, the editor and the apply path cannot set it.

Load time is what runs while a module evaluates: initializers of top-level `const`s,
`export default` expressions, enum member initializers, class static field initializers, top-level
computed keys and destructuring defaults.

| Code | Rule |
|---|---|
| `lint.module_let` | No `let` or `var` at the top level, exported or not |
| `lint.module_reassign` | No assignment, `++`, `--`, compound assignment, destructuring target or `for (x of ...)` target naming a top-level binding (functions and classes included), anywhere in the module |
| `lint.module_statement` | Top-level statements are only imports, exports, `const`, function, class, enum and type-only declarations |
| `lint.load_time_code` | Load-time expressions run no user code. Allowed: literals (regular expressions without `g` or `y`), untagged templates, identifiers, member access other than of `prototype`, object literals with data properties and methods (no `get`/`set`), array literals, spread, function, arrow and class expressions (not called), unary operators except `delete`, binary, logical and conditional operators, TypeScript `as`, `satisfies` and `!`, and calls to the pure functions below with arguments that follow this rule. No other call, no `new`, no tagged template, no class static block or static accessor |
| `lint.load_time_hook` | In an object literal that is part of a load-time expression, and among the static members of every class: no property or method whose key is `valueOf`, `toString`, `toLocaleString` or `toJSON`, and no computed key (`[Symbol.iterator]`, `[Symbol.toPrimitive]`, `[Symbol.hasInstance]` and the rest). These are the hooks through which a load-time conversion (`x + 1`, a template substitution, `Number.parseFloat(x)`, `String.fromCharCode(x)`, `Math.max(x)`), an iteration (spread, destructuring) or an `instanceof` would call project code |
| `lint.static_private` | No static private field, method or accessor in a class (`static #n = 0`, `static #cache = {}`): the freeze cannot reach private names (QuickJS-ng's `js_object_seal` lists only the keys of `JS_GPN_STRING_MASK` and `JS_GPN_SYMBOL_MASK`), so `static next() { return ++Spawner.#n; }` would count across calls. Instance private fields are allowed: instances live within one call |
| `lint.async` | No `async` functions or generators, `await`, `for await`, dynamic `import()` or `import.meta` |
| `lint.removed_global` | No reference to a removed global (2.2), `require`, `process`, `window`, `document` or `fetch`; no assignment to `globalThis`; no use of `RegExp.prototype.compile` |
| `lint.decorator` | No decorators (they run at load time) |
| `lint.namespace` | No value `namespace` (it compiles to load-time code); modules replace it |
| `lint.private_module` | No import of `pocket:host` |

The pure functions callable at load time are the `pocket` exports tagged `@pure` in their
declaration (`game`, `system`, `component`, the `field` builders, `freeze`), the `Math` functions
except `random`, the static functions of `Number`, `String.fromCharCode`, `String.fromCodePoint`,
`Object.freeze`, `Object.keys`, `Object.values`, `Object.entries`, `Array.isArray` and `Array.of`.
`Object.fromEntries` is not among them: it builds an object from computed keys, `valueOf` among
them, which `lint.load_time_hook` could not see.

So no project function body runs at load time: with the hooks refused, every implicit conversion,
iteration or `instanceof` a load-time expression causes runs only built-in code, which lockdown
froze (2.3), and `prototype` is never read, so a class's instance methods are not reached. No
closure with private mutable variables can be made then, and the epilogue freezes all a module
keeps. Charter 4.2.5's backstop still runs, for engine bugs and for a rule this table misses: the
reload-equivalence check ([hot-update.md](hot-update.md), 9) and the fork-consistency check
(checks.md 8.3).

## 4. Execution budget and memory

### 4.1 Steps

Work is counted in steps. QuickJS-ng polls its interrupt counter at every function call
(`JS_CallInternal`), every bytecode jump (`OP_goto`, `OP_if_true`, `OP_if_false` and variants) and
every element of builtin loops that could run unbounded (`Array.prototype` iteration methods,
prototype-chain walks), calling the interrupt handler when it runs out; one poll is one step. A
call's steps depend only on the bytecode it executes, the same for one QuickJS-ng build natively and
on the web (the script-web spike's handler-call counts matched over 600 ticks), so they are
deterministic where milliseconds are not (charter 4.2.1, criterion 3). Steps never enter the world
hash: the spike's verification moved its hashed count by running unrelated code first.

Upstream keeps the counter per context, resets it to 10,000 (`JS_INTERRUPT_COUNTER_INIT`) whenever
the handler runs and cannot read or set it, so where a budget ends would depend on all the context
ran before, a fork's or a reload's history included. The host therefore sets the counter to the
call's budget at the start of every call of every kind (4.4) and reads it afterwards (P1, 4.2). The
script-native spike reached the same stop points without a patch by spinning the counter to its
boundary before every tick (69,997 iterations on every run, against 67,084 or 69,592 unsynced), at
up to 35 µs a tick; that is the fallback until P1 is carried. A spent budget aborts the call with
QuickJS-ng's uncatchable `InternalError: interrupted`, reported as `script.budget_exceeded` and
handled as the call's kind says (4.4).

```rust
pub struct ScriptLimits {
    pub steps_per_system: u32,   // 1,000,000; at most i32::MAX
    pub steps_per_tick: u64,     // 2,000,000, all script calls of a tick (4.4)
    pub load_steps: u32,         // 1,000,000, evaluating a program's modules
    pub steps_per_call: u32,     // 100,000: a derived fact, an instrument or a predicate (4.4)
    pub max_call_depth: u32,     // 200 nested calls of any kind (P5, 4.3)
    pub memory_bytes: usize,     // 64 MiB per QuickJS-ng runtime
    pub stack_bytes: usize,      // per target and profile, from the depth test (4.3)
    pub log_lines_per_tick: u32, // 100; changes no result (script-host.md 5.7)
}
```

The limits are chosen, not measured: the charter's rule workload took 60,000 polls a tick at 18,000
to 30,000 polls per ms (script-native spike, 6 units of 10,000 in 2.0 to 3.3 ms), so the tick limit
is 33 times a heavy tick and stops a runaway loop within about 110 ms; its heap was 238,355 bytes
against 64 MiB. A system's budget is the smaller of `steps_per_system` and what is left of
`steps_per_tick`. The step limits and `max_call_depth` are run configuration that [persist] records
with a replay (versions.md 3.7). `SystemStats.steps` goes to the perf tools ([mcp]); a fixed
workload's exact step count is a regression check in [arch]'s framework that machine noise cannot
disturb.

### 4.2 The QuickJS-ng patch set

This line already carries quickjs-ng PR #1421 for the debugger, applied unmodified to a pinned
`rquickjs-sys` and substituted through `[patch.crates-io]` (debugger spike, Recipe 1); the patched
crate is committed under `third_party/` and rebuilt by `cargo xtask vendor` (architecture.md 7.5).
The script host needs these further changes, carried the same way and proposed upstream:

| Patch | Change | Why |
|---|---|---|
| P1 | `JS_SetInterruptCounter(JSContext *, int32_t)` and `JS_GetInterruptCounter(JSContext *)`, declared by hand in the host's own `extern "C"` block | Deterministic budgets and exact step counts (4.1) |
| P2 | `JS_ThrowStackOverflow` and `JS_ThrowOutOfMemory` make their exception uncatchable (`JS_SetUncatchableError`), as interrupts already are | Stack depth depends on native frame sizes and memory on allocation history; a script must not catch either and carry on differently per platform (4.3) |
| P3 | Seed `random_state` and `hash_seed` from a constant, not `js__gettimeofday_us()` | Unobservable today (`Math.random` is replaced; the hash seed only places `Map` and `Set` entries in buckets, and iteration follows insertion), but it removes the runtime's only clock read; Amoris's predecessor charter fixes Lua's string hash seed for the same reason (its 4.2.3) |
| P4 | Compile QuickJS-ng's C with `-ffp-contract=off` on clang and GCC | No fused multiply-add on one platform and not another (master `docs/design/networking.md`, Determinism) |
| P5 | A per-context call-depth counter, incremented on entry to `JS_CallInternal` for every function kind (bytecode, C functions, bound functions, generators) and per nesting level in QuickJS-ng's C-recursive builtins (`JSON.parse` and `JSON.stringify`, `Array.prototype.flat`, `join` and `toString` over nested arrays, regular expression compilation), decremented on exit; past `max_call_depth` it throws an uncatchable `InternalError` that the host reports as `script.call_depth`, with `JS_SetCallDepthLimit(JSContext *, uint32_t)` to set it | Where the stack runs out depends on the target and the build profile: the script-native spike's verification measured 50 nested calls under 256 KiB with MSVC-built QuickJS-ng, 228 with clang-cl and 907 in Chrome, and Rust's frames above a call are larger in debug. A depth counted in calls is the same everywhere (4.3) |
| P6 | Every float store into a typed array or a `DataView` (`Float16`, `Float32`, `Float64`) writes the canonical NaN for any NaN (`0x7FF8000000000000` for f64, `0x7FC00000` for f32, `0x7E00` for f16), and a float16 or float32 element read as a number canonicalizes a NaN | QuickJS-ng NaN-boxes values only on 32-bit targets (`JS_NAN_BOXING` when `INTPTR_MAX < INT64_MAX`, quickjs.h), where every NaN becomes `0x7FF8000000000000`; 64-bit native builds keep the payload, and 0/0 is `0xFFF8000000000000` on x86. Without P6, `f[0] = 0/0; new Uint8Array(f.buffer)[7]` reads 0xFF natively and 0x7F on the web, and a script that hashes floats through an aliased typed array diverges |
| P7 | `rquickjs-sys`'s build script makes the wasi-libc include path absolute with `std::path::absolute` instead of `canonicalize`, and emits `cargo:rustc-env=POCKET_QJS_CC=<compiler id>` | `canonicalize` gives a verbatim `\\?\` path on Windows that hides nested wasi-libc headers (script-web spike, Problems 1), which the spikes worked around with a copied header directory; the compiler id lets `EngineVersion` and the perf step see which C compiler built QuickJS-ng (architecture.md 7.3) |

| P9 | Each function the debugger traces builds a table of its statement positions on its first trace; `OP_debug` reads it instead of searching the line table from the start | The PR's search was about 90 percent of the instrumented cost (debugger spike); the spike's alternative, line and column as `OP_debug` operands with the opcodes moved before the temporary block, renumbers the short opcodes and breaks the precompiled builtin bytecode QuickJS-ng embeds (debugger.md 2) |
| P10 | The trace handler also hears catchable exceptions, once per throw, with a catch prediction from the catch offsets on the value stacks (`OP_debug` records each frame's stack pointer); `JS_GetDebugTraceException`; `JS_GetStackFrameInfo(level)` | Pause on uncaught exceptions at the throw, and callers' positions without parsing `Error().stack` (debugger.md 2, 3) |

A call's steps must not change while the debugger instruments it (script-host.md 12, test 3).

### 4.3 Call depth, memory and stack

**Call depth is a deterministic limit.** Every script call runs with P5's counter at 0 and
`max_call_depth` as its limit, so a recursion 60 deep works or fails the same way on every target
and in every build profile. Exceeding it throws an uncatchable `script.call_depth`, an ordinary
deterministic failure handled as the call's kind says (4.4; for a system, 5.4).

**The stack is sized so the depth limit always fires first.** `set_max_stack_size` (`stack_bytes`)
and the stack of every thread that runs scripts (the game thread, `ThreadOptions::stack_bytes`; the
runtime workers that run hot-update stages, trials, forks and `eval`; threads.md 3.1 and 6) are set
per target and profile from the depth test (script-host.md 12, test 9): a script recursing to
`max_call_depth` through the deepest frame shape (a JS function, a host native, a JS callback,
repeated) in native debug, native release and web, measured on each thread kind; `stack_bytes` is
that use times 1.5 (chosen), and each thread's stack is `stack_bytes` plus the Rust frames below the
first call, measured the same way. The C stack check can therefore never fire before the depth
counter on a correct build. On wasm32 the stack limit is 256 KiB (script-web spike), which the depth
test must show suffices for `max_call_depth`, else `max_call_depth` is lowered for every target,
never raised for one.

**Faults.** Reaching the memory limit (`Runtime::set_memory_limit`) or the stack limit despite the
above depends on the platform and on garbage-collection history, so it is a fault, not a game
outcome, and it is never discarded and continued. Inside a tick (a system, a derived fact or a
predicate evaluated in `Finish`), the call fails with `script.out_of_memory` or
`script.stack_overflow` (uncatchable through P2), the tick stops there, and the world is poisoned:
`step` refuses with `sim.world_poisoned` until a restore (simulation.md 4.6), the recorder writes a
`Fault` record instead of the tick's record (replay.md 2.2), and `verify` reports `Stopped` with the
fault's code rather than a hash divergence (replay.md 3.4). Outside a tick a fault refuses what it
was part of and leaves the world as it was: an instantiation or a hot update (hot-update.md 7), a
migration (versions.md 8.5), a query (the fact absent, 4.4), a trial (`scripts.trial_failed`). The
checks count every fault as a failure. Host buffers a call sizes from script values are allocated
only after those values pass their checks (the script-native spike's host grew by 383 MiB for one
refused write naming entity 100,000,000). Garbage collection is otherwise unobservable: no `WeakRef`
or `FinalizationRegistry`, and host finalizers never touch the world.

**Panics stay in Rust.** Every native of `pocket:host` runs under `std::panic::catch_unwind`: a
panic becomes an uncatchable JavaScript error, `sim.internal` in the step report and a poisoned
world (simulation.md 4.6), so nothing unwinds through QuickJS-ng's C frames, which would abort the
process, editor included (the script-native spike's verification hit this). A native reads its
arguments, which can run getters, before it borrows the world, and reads typed arrays without
panicking on a detached buffer.

### 4.4 Script calls and their budgets

Script functions run in several places besides script systems. Every kind is listed here, with its
budget, its counter and what a throw or an overrun does; the counter is set at the start of every
call (P1) and P5's depth counter starts at 0.

| Call | Where it runs | Budget | Counts against `steps_per_tick` | A throw, an overrun or `script.call_depth` |
|---|---|---|---|---|
| System, script intent executors included (script-host.md 5.5) | `script.update` in `Update`, the game thread | the smaller of `steps_per_system` and what is left of the tick's | yes | the system fails (5.4) |
| Derived fact or instrument (shared/contract/perception.md) | `interface.perception` in `Finish` for memory, the game thread; a query at a boundary for instruments and the facts it shows | `steps_per_call` | in a tick, yes | the fact or reading is absent for that observer and tick; the error goes to the step report in a tick, to the session log in a query |
| Predicate (`quiet`, `done`; shared/contract/time.md) | `interface.turns` in `Finish`, the game thread | `steps_per_call` | yes | the predicate counts as false; the error goes to the step report |
| Module evaluation | `instantiate`, on the thread that builds the host | `load_steps` | no | the load is refused (`script.load_exception`, `script.budget_exceeded`) |
| Migration step (versions.md 8.3) | a swap, a load or a restore, on the game thread or a worker | `steps_per_system` per value | no | `migrate.failed`; the swap, load or restore is refused |
| Debugger evaluation, breakpoint condition (script-host.md 13) | the paused game thread | `steps_per_call` | no: the tick's counter is saved and restored around it | an error to the debugger client |
| `eval` (shared/contract/mcp.md 6) | a throwaway fork's host on a runtime worker | `steps_per_system` | the fork's own | an error answer |

Script functions that perception or the time controller calls are evaluated only on the game thread,
in the tick or at a boundary, never off it. A fault in any of them follows 4.3.

## 5. Errors

### 5.1 The error record

Every failure reaches agents and tools as [contract]'s `{code, message, detail}`:

```rust
pub struct ScriptError {
    pub code: &'static str, pub message: String, pub detail: ScriptErrorDetail,
}
pub struct ScriptErrorDetail {
    pub phase: ErrorPhase,                 // Compile | Lint | Load | Run | WriteBack
    pub location: Option<SourceLocation>,  // the first frame in project code
    pub stack: Vec<StackFrame>,            // project and prelude frames, innermost first
    pub tick: Option<u64>, pub system: Option<Box<str>>,
    pub entity: Option<f64>,               // the entity concerned, when known
    pub component: Option<Box<str>>, pub field: Option<Box<str>>,
    pub value: Option<serde_json::Value>, pub hint: Option<Box<str>>,
    pub js_error: Option<JsError>,         // { name: "TypeError", message } from QuickJS-ng
    pub suggestions: Vec<Box<str>>,        // nearest names for an unknown one
}
pub struct SourceLocation { pub file: ModulePath, pub line: u32, pub column: u32 }
pub struct StackFrame { pub at: SourceLocation, pub function: Option<Box<str>> }
```

`message` is one plain sentence. Lines and columns are 1-based, columns in UTF-16 code units, as
`tsc` and editors count. `entity` comes from a host call naming an entity, a write-back check (the
row's entity) or `QueryResult.each` (the row being processed when `fn` threw).

**Validation order.** A host call validates its arguments in a fixed order and reports the first
problem only: positional arguments left to right; within an object, keys in ECMAScript's own-key
order (integer-like keys ascending, then strings in insertion order); within a `spawn`, components
in that order and each component's fields in that order. Column write-backs are checked query by
query in creation order, rows in ascending id, slots in declaration order. So which code a call with
several problems reports is the same in every build.

### 5.2 Source mapping

QuickJS-ng calls `Error.prepareStackTrace` (2.3) with V8-style call sites (`getFileName`,
`getLineNumber`, `getColumnNumber`, `getFunctionName`); the host's mapper looks each position up in
its module's source map, so `error.stack`, the structured `stack` and the debugger show TypeScript
lines. Prelude frames are kept and marked `pocket`; compile and lint errors take their location from
the oxc span. Modules a release precompiles to bytecode keep their line tables (script-host.md 8.2),
so locations are the same in every build.

### 5.3 Codes

| Code | Phase | When |
|---|---|---|
| `script.syntax`, `script.source_encoding` | Compile | The module does not parse, or is not UTF-8 |
| `lint.*` | Lint | A rule of 3 |
| `script.module_not_found`, `script.module_case`, `script.module_outside_root`, `script.module_extension` | Load | Resolution (script-host.md 8.1) |
| `script.module_unused` | Load | Warning: a module not reachable from the entry |
| `script.module_state` | Load | A top-level value cannot be frozen (2.4) |
| `script.entry_not_game`, `script.unknown_key`, `script.system_duplicate`, `script.component_name`, `script.bad_definition` | Load | The game definition (script-host.md 4, 7.3) |
| `script.load_exception` | Load | Module evaluation threw |
| `script.exception` | Run | `run` threw; `js_error` holds the JavaScript error |
| `script.unknown_component`, `script.unknown_field`, `script.entity_missing`, `script.component_present`, `script.component_missing`, `script.single_count`, `script.event_reserved`, `script.bad_data` | Run | A host call refused its arguments (script-host.md 5.3 to 5.5, 5.8) |
| `script.restricted`, `script.undeclared_code` | Run | An intent executor called what its restricted context lacks, or rejected with a code the game does not declare for `Fail` (script-host.md 5.5) |
| `script.write_not_finite`, `script.write_not_integer`, `script.write_out_of_range`, `script.write_type`, `script.write_too_long` | Run, WriteBack | script-host.md 6 |
| `script.budget_exceeded`, `script.call_depth` | Run, Load | 4.1, 4.3 |
| `script.out_of_memory`, `script.stack_overflow` | Run, Load | Faults (4.3) |
| `script.async_work` | Run | Jobs were pending after the call |
| `script.random_outside_system`, `script.code_generation`, `script.no_system`, `script.debug_read_only` | Run, Load | Sandbox refusals (2.2; script-host.md 5.6 and 13); a native called with no system running |
| `migrate.failed`, `rng.stream_expired`, `rng.bound_invalid`, `rng.key_invalid` | Run | [persist]'s and [sim]'s codes, raised through the host with the TypeScript location |

Hints come from a fixed table keyed by the JavaScript error and its context, covering the mistakes
master's agents made and those this design invites: a `ReferenceError` for a removed global
(`Date is not available in game scripts: use ctx.time or ctx.tick`) or for a `pocket` export used
without importing it; a `TypeError` from writing a frozen module value (copy it first, or keep the
state in a component) or a `world.get` copy (use `ctx.world.set`).

### 5.4 What a failure does

A failed system (an exception, a refused write, a budget overrun, `script.call_depth`) leaves no
effect (script-host.md 5.4) and the tick completes ([sim]'s errors inside a tick): the failure is an
entry of the step's report, with its location, and the host emits a `script.failed` event whose data
is the `PlainData` `{system: "<name>", code: "<code>"}` (the system's name as `system(...)` declares
it, not its `script:` key), with no subject. The location stays out of the event, which is hashed
world state, and goes to the step report, the halt's problem and the session log; so the world is
the same whether a release kept debug information or not. The event's kind is declared `Hidden`
(shared/contract/perception.md, Events and their scope), so it reaches developers and checkers,
never players. The system runs again on its next scheduled tick. The contract halts a running game
at the boundary after a script failure until a developer resumes or a hot update lands
(shared/contract/time.md, Halts), since master found that a stopped game with the error in its state
let agents find script bugs (master `docs/agent-eval.md`, Results). The policy is run configuration
recorded with a replay (versions.md 3.7). A fault is not a failure of this kind: it poisons the
world (4.3).

## 6. Open choices

1. **Slice 1: the vendoring script.** `third_party/vendor.py` (standard library, with git as a plain
   patch tool) builds `third_party/rquickjs-sys-0.14.0/` from the pinned crate (SHA-256
   `cee271d0...`) and the diffs in `third_party/patches/`, each pinned by its SHA-256 in the script:
   quickjs-ng PR #1421 unmodified, then `p1-interrupt-counter.diff`, `p2-uncatchable-faults.diff`,
   `p3-constant-seeds.diff`, `p5-call-depth.diff`, `p6-canonical-nan.diff`, `p4-p7-build.diff` (P4
   and P7 both change the build script, so they are one diff), `p8-discard-jobs.diff` (choice
   14), `p9-debug-line-cache.diff` and `p10-debug-exceptions-frames.diff` (the debugger,
   debugger.md 2). `--check` rebuilds into a temporary directory and fails on any byte that differs, as
   `cargo xtask vendor --check` will (architecture.md 7.5); the script stays until xtask's Rust
   version replaces it.
2. **Slice 1: P7 picks the pinned compiler.** When a build names no C compiler for the target
   (`CC_<target>`, `TARGET_CC`, `CC`), the vendored build script uses `$POCKET_LLVM`, else
   `~/.pocket-tools/llvm-23.1.2`: `clang-cl` for `x86_64-pc-windows-msvc` and
   `aarch64-pc-windows-msvc`, `clang` and `llvm-ar` for `wasm32-unknown-unknown`. Every build of a
   checkout then compiles QuickJS-ng the same way whether `cargo xtask` started it or not, the
   shared target directory does not rebuild QuickJS-ng each time a different caller builds, and
   `cargo build --target wasm32-unknown-unknown -p pocket-script` works with no variable set.
   `rquickjs_sys::POCKET_QJS_CC` (`pocket_script::QJS_CC`) reports the compiler, such as
   `clang-cl: clang version 23.1.2 (...)`.
3. **Slice 1: P4 in the build script.** On the MSVC target rquickjs-sys's build script replaces
   `CFLAGS`, so `.cargo/config.toml`'s `-ffp-contract=off` never reached it; P4 passes it in the
   build script: `/clang:-ffp-contract=off` to clang-cl (which ignores the plain flag),
   `-ffp-contract=off` to compilers that take it, and nothing to MSVC's `cl`, which contracts only
   under `/fp:contract`. The build script decides clang-cl from the compiler's name, since asking cc
   for the compiler would cache `CFLAGS` before the MSVC branch sets them.
4. **Slice 1: what P5 counts.** Every `JS_CallInternal` (bytecode, C, bound functions, generator
   resumptions) is one level; so is each nested value of `JSON.parse` and `JSON.stringify`, each
   array `Array.prototype.flat` descends into, each group and class the regular-expression parser
   nests (past the limit `js_compile_regexp` throws the call-depth error rather than a
   `SyntaxError`), and each internal method of a Proxy (`get`, `set`, `has`, `defineProperty`,
   `deleteProperty`, `getOwnPropertyDescriptor`, `ownKeys`, `getPrototypeOf`, `setPrototypeOf`,
   `isExtensible`, `preventExtensions`, and `Array.isArray`'s look through it), since it forwards to
   its target and reads its trap from its handler, either of which can be another Proxy, without a
   call. Before that last item a chain of 2,000 Proxies faulted natively on the stack check and ran
   on wasm32 (the review's probe); now it fails with `script.call_depth` on both (tests/depth.rs and
   the web report's `proxies` lines). `join` and `toString` over nested arrays and a `JSON.parse`
   reviver count through the calls they make. The thrower lifts the limit while it builds the error,
   since `Error.prepareStackTrace` is a call.
5. **Slice 1: P6's reach.** Canonical NaNs are written by element stores, `fill` and `DataView`'s
   setters for float16, float32 and float64, and float16 and float32 elements read as numbers
   (element reads, `at`, the sort comparator's arguments, `DataView`'s getters) are canonicalized;
   float64 reads keep their bits, which no script can see without a store.
6. **Slice 1: generators.** `lint.async` refuses async functions and generators, `await`,
   `for await`, `import()` and `import.meta`; synchronous generators are allowed, since a generator
   object made in a call lives only in that call. `harden` refuses a generator or built-in iterator
   object kept at the top level by its class id, sampled at the lockdown, rather than by
   `%IteratorPrototype%` in its prototype chain, which would also refuse a generator function's own
   `prototype` object.
7. **Slice 1: the lint's removed globals** add `setTimeout`, `setInterval`, `clearTimeout` and
   `clearInterval` to 2.2's list; `RegExp.prototype.compile` is refused on a regular-expression
   literal, a top-level const bound to one, and `RegExp.prototype` itself (the runtime has deleted
   it anyway).
8. **Slice 1: the lockdown's roots and reviewed setters.** The deep freeze starts from `globalThis`
   and from the hidden intrinsics: objects no own property or prototype chain from `globalThis`
   reaches but builtins hand out (`%ArrayIteratorPrototype%`, `%MapIteratorPrototype%`,
   `%SetIteratorPrototype%`, `%StringIteratorPrototype%`, `%RegExpStringIteratorPrototype%`,
   `%IteratorHelperPrototype%`, `%WrapForValidIteratorPrototype%`, the prototypes of
   `Iterator.concat()` and `Iterator.zip()`, `%GeneratorFunction.prototype%` and
   `%GeneratorPrototype%`, their async function and async generator counterparts with
   `%AsyncIteratorPrototype%`, `%ThrowTypeError%`), and the objects whose globals it deletes
   (`%Promise%` and its prototype, `Array.fromAsync`, `AsyncDisposableStack`). The lockdown samples
   them before deleting anything and returns them as `[name, value]` pairs; the freeze records them
   as intrinsics, and the verification walk starts from the same list, `globalThis` first, so a path
   is named from `globalThis` when it has one. Without them a system kept a counter on
   `Object.getPrototypeOf([].values())` across calls and reloads (the review's probe;
   tests/lockdown.rs keeps it). `Array.fromAsync` and `AsyncDisposableStack` are deleted beside
   `Promise` (and the lint refuses them, `lint.removed_global`): they were the ways left for
   lint-clean code to make a promise, its constructor and a queued job. The walk finds no extensible
   object and no writable or configurable property; its setters are the override taming's (named
   `tamed <key>`, defining an own property on the receiver), `__proto__`'s,
   `Error.prototype.stack`'s (it defines `stack` on the receiver), %ThrowTypeError%
   (`Function.prototype.arguments` and `caller`) and the ES2025 `Iterator.prototype` accessors. The
   deep freeze is the host's walk (`freeze.rs`), which records the intrinsics it froze so a module's
   `harden` walks only the module's own values. Reading an error's `name`, `message` and `stack`
   calls a getter only when it belongs to an intrinsic, never a project object's.
9. **Slice 1: stack sizes from the depth test.** `tests/depth.rs` measured the stack the deepest
   frame shape (a JS function, `Math.sin` coercing its argument, a `valueOf` callback, repeated)
   uses at `max_call_depth` 200: 356,186 bytes in a release build and 3,467,626 in a debug build, on
   `x86_64-pc-windows-msvc` with clang-cl. `stack_bytes` defaults to 1 MiB in release and 6 MiB in
   debug (at least 1.5 times the use), 256 KiB on `wasm32`, where the web report shows the depth
   limit firing first; `pocket_script::host::THREAD_STACK_BYTES` is `stack_bytes` plus 2 MiB for the
   Rust frames below the first call, the stack the game thread must have (threads.md 3.1).
10. **Slice 1: where a write-back is reported.** A column write-back is checked after `run` returns,
    so no line caused it; its error is placed at the system's `run`, which `compile` records for
    every `system({ name: "...", run })` with a literal name (`SystemInfo::defined_at`).
11. **Slice 1: pending work.** After `run` the job queue is drained under the call's budget; a job
    that overruns or faults fails the call with its code, a promise rejected with no handler fails
    it with the rejection's reason (the host's rejection tracker), and jobs still pending after the
    drain are `script.async_work`. When a call fails or faults, the jobs it queued are dropped
    (choice 14), as are those of a failed instantiation and of any `eval_json`.
12. **Slice 1: console lines** carry the tick and the system but no location yet.
13. **Slice 1: P2 without allocating.** Memory can run out on an allocation too small to leave room
    for the error object: `JS_ThrowInternalError` then fails and QuickJS-ng throws `null`, which no
    flag makes uncatchable, so a `try` around a growing linked list caught it and the call ended
    well (the review's probe, with `memory_bytes` at 4 MiB). P2 therefore makes one uncatchable
    `InternalError("out of memory")` per context with the intrinsics (`JSContext.oom_error`, its
    stack preset to the empty string so throwing it never builds a backtrace) and throws a reference
    to it; it also counts every out-of-memory error in the runtime (`JS_GetOutOfMemoryCount`) and
    zeroes the context's interrupt counter, so the next poll asks the host's handler. The host notes
    the count when a call begins: a changed count makes the call `script.out_of_memory` (a fault)
    whatever happened to the error, a builtin swallowing it or replacing it with its own included,
    and the handler stops the call at that poll without counting it as an overrun.
    `error_from_value` maps `out of memory` only for an uncatchable error or a changed count, so
    `throw new InternalError("out of memory")` is an ordinary `script.exception`. tests/depth.rs
    runs four allocation shapes inside `try`.
14. **Slice 1: P8, dropping a failed call's jobs.** `JS_DiscardPendingJobs(JSRuntime *)` frees every
    queued job without running it. The job queue is the runtime's, so the jobs of a call that threw
    or ran out of its budget before draining ran in the next call, under its state and transaction,
    or crossed the tick boundary, and a new host (a fork) had none of them (the review's probe: a
    failed system's job committed `n = 999` through the next system). The host calls it when a
    system call fails or faults, when an instantiation fails, after every `eval_json` and when a
    panic abandons a call; tests/budget.rs checks that the next system's steps and the world are
    those of a run without the job, and that a rehost changes no later digest.
15. **Slice 1: host errors by identity.** A native's refusal is thrown as a plain `Error` with the
    refusal's message; the host keeps that error object for the call (a `Persistent` beside the
    `ScriptError`, cleared when the next call begins) and `error_from_value` recognises it by
    identity, so a script rethrowing a caught refusal keeps its code and detail, while an object
    with copied properties (or `{__pocketCode: "sim.internal"}`) is a `script.exception`. Faults
    come only from the host's own state: an uncatchable error, the out-of-memory count, a native's
    panic; `sim.internal` is never read from a script value.
16. **Slice 1: host buffers are charged before they are made.** `ScriptLimits` has
    `host_bytes_per_call` (16 MiB by default; run configuration like the step limits): query columns
    and ids (16 bytes per row and per row and numeric slot: the script's copy and the host's copy
    for the write-back), `shuffle`'s permutation (16 bytes per item), `weighted`'s weights and
    `fill`'s values (8 bytes each) are counted against it before they are allocated, and a call past
    it fails with `script.budget_exceeded` (`detail.budget` `"host_bytes_per_call"`), a
    deterministic failure. The columns, ids and permutations are copied into QuickJS-ng's own memory
    (`JS_NewArrayBufferCopy`), so `memory_bytes` counts what a script holds of them; before, 200
    held query results took 76 MB of host memory against a 16 MiB limit. Lengths a script gives are
    bounded before anything is allocated: `shuffle` and `weighted` take at most 2^24 items
    (`rng.bound_invalid`), a dense array's `length` is read and bounded before its keys are listed
    (stream keys 64 parts, query name lists 256, game-definition lists 4,096, plain data 8,192
    elements), and plain data keeps a running lower bound of its JSON size while it is read (key
    counts before the keys are listed), refusing past 16 KiB at once rather than after the whole
    tree is built and serialized. tests/limits.rs covers each.
17. **Slice 1: the epilogue's keys and names.** The harden epilogue passes bindings under computed
    keys (`{ ["__proto__"]: __proto__ }`): a plain `__proto__: __proto__` set the object's prototype
    and left the binding unhardened. Its own bindings start with `__pocket_` (`__pocket_harden`,
    `__pocket_self`), and `lint.reserved_name` refuses any binding or reference with that prefix in
    project code, since a top-level one collides with the epilogue's imports and a reference would
    reach the private native.
