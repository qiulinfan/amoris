# Spike: native breakpoint debugging (quickjs-ng PR #1421)

- Date: 2026-10-03. Slice 0. Code: [spikes/debugger/](../../spikes/debugger/README.md).
- Charter: 4.2.6 (native breakpoint debugging), 4.2.7 (risks), 12 open question 5 (maintain the PR
  #1421 patch or wait for upstream).

## Question

Can TypeScript rule scripts on QuickJS-ng, embedded through `rquickjs` and transpiled by oxc, be
debugged with breakpoints from a Chrome DevTools Protocol (CDP) client such as VS Code's js-debug,
using quickjs-ng PR #1421? What does the patch cost at run time, and what does it cost to carry it
until upstream merges it?

## Verdict

**Works with caveats.** The PR applies cleanly to the QuickJS-ng that `rquickjs-sys` 0.14.0 vendors
(0.16.2) and builds through an unchanged `rquickjs` with a `[patch]` section. A CDP endpoint on top
of it passes every step the charter names, driven by a Python client with no UI: a breakpoint on a
TypeScript line, the pause with the right file, line, caller frame and locals, an evaluation on the
frame, a step over, a conditional breakpoint, a `debugger;` statement and the resume to the end (19
of 19 checks). The patched engine also builds for `wasm32-unknown-unknown`, and its trace handler
sees the same statements there as natively.

The caveats:

1. **Cost while instrumented.** Compiled in but with no handler installed, the patch costs nothing
   measurable (0.98 and 1.00 times the stock engine on the rule workload). With the handler
   installed, every statement pays: 2.04 times with no client, 2.08 to 2.09 times with a client
   attached. About 90 percent of that is the PR searching the function's line table from its start
   at every statement. An experiment that stores the line and column in the `OP_debug` instruction
   (21 lines added and 12 removed on top of the PR) brings it to 1.08 and 1.19 times. Either way the
   engine should instrument scripts only while a debugger is attached, recompiling them at a tick
   boundary on attach and detach, which stateless scripts and hot update make cheap.
2. **VS Code and Chrome DevTools were not run.** The endpoint follows what js-debug 1.140.0's source
   requires (discovery, the attach sequence, the `urlRegex` it sends on Windows, `callFunctionOn`
   answers); a session in a real VS Code is still to be seen.
3. **A latent defect in the PR.** It defines `OP_debug` after the temporary opcodes but emits it in
   the parser, so compilation passes 1 to 3 look it up as `put_var_ref2`'s table entry. Both are one
   byte without operands, so it works, by coincidence; any change to `OP_debug`'s format breaks (the
   experiment crashed until the opcodes moved before the temporary block). Worth reporting on the
   PR.
4. **Not in the patch**: pausing on exceptions, and the location of caller frames (taken here from
   QuickJS-ng's backtrace). The web build has the hook but no CDP transport.

**Open question 5: maintain the patch, unmodified.** It is 620 added lines in four files: 303 of
them a self-contained inspection API, 84 in the interpreter, the parser, three bytecode passes and
the context, the rest declarations and tests. It applied with line offsets only, no fuzz and no
conflict, to QuickJS-ng 0.16.2, 0.17.0 and master of 2026-10-02. Upstream's design converged in
review (a maintainer proposed the `OP_debug` scheme it uses), but the PR has waited for a maintainer
since July. Carrying it as the upstream diff, applied by a script to a pinned `rquickjs-sys`, costs
a rerun of that script per `rquickjs-sys` update and ends when upstream merges it. The operand form
and the opcode placement belong in the PR rather than in a local fork.

## What was built

All under `spikes/debugger/` (its README says how to build and run each part).

- **The patched engine.** `vendor.py` extracts `rquickjs-sys` 0.14.0 from the cargo cache or
  crates.io (pinned by SHA-256), applies `patches/quickjs-ng-pr1421.diff` (the PR's diff at head
  `0e7a5e08`, pinned by SHA-256) with `git apply`, and leaves the result in `rquickjs-sys/` (not
  committed), which `Cargo.toml` substitutes through `[patch.crates-io]`. `rquickjs` itself is
  unchanged. The patch adds six functions; the five the spike calls and their struct are declared by
  hand in `src/ffi.rs`, because the crate ships pregenerated bindings and regenerating them needs
  libclang.
- **TypeScript.** oxc 0.152 strips types per module and writes a source map (`src/ts.rs`). The
  debugger presents the TypeScript file itself to the client: the script URL is the `.ts` file's
  URL, `Debugger.getScriptSource` returns the TypeScript text, and every location the endpoint sends
  or receives is a TypeScript line and column, mapped through oxc's source map. QuickJS-ng runs the
  JavaScript. The test script `scripts/game.ts` has interfaces and annotations, so its JavaScript
  lines differ from its TypeScript lines.
- **The game thread** (`src/debugger.rs`, `src/inspect.rs`) owns the runtime. The trace callback
  returns at once unless a client is attached; otherwise it checks a per-line bitmap of breakpoint
  lines, then the source map, then the step state. A pause happens inside the callback, on the game
  thread's own stack: it clears the handler (so evaluations are not traced), builds the call frames,
  sends `Debugger.paused`, and serves the CDP thread's requests (`Runtime.getProperties`,
  `Debugger.evaluateOnCallFrame`, `Runtime.callFunctionOn`) from a channel until a resume or step
  command. Values handed out as `objectId`s are held for the pause and released after it. Callers'
  positions come from QuickJS-ng's own backtrace (`JS_NewError` and its `stack`), locals from the
  patch's `JS_GetLocalVariablesAtLevel`, evaluation and breakpoint conditions from its
  `JS_EvalInStackFrame`.
- **The CDP thread** (`src/cdp.rs`) never touches the context. It serves `/json/version` and
  `/json/list` over HTTP and one WebSocket client at a time (tungstenite) on 127.0.0.1, keeps
  breakpoints in shared state, and forwards requests that need the context to the paused game
  thread. Methods: `Debugger.enable`, `disable`, `setBreakpointByUrl` (url or urlRegex, with a
  condition), `setBreakpoint`, `removeBreakpoint`, `setBreakpointsActive`, `getPossibleBreakpoints`,
  `getScriptSource`, `pause`, `resume`, `stepOver`, `stepInto`, `stepOut`, `evaluateOnCallFrame`;
  `Runtime.enable`, `runIfWaitingForDebugger`, `getProperties`, `callFunctionOn`, `evaluate` (an
  `undefined` answer). Events: `Debugger.scriptParsed`, `paused`, `resumed`,
  `Runtime.executionContextCreated`. Other methods get an empty result, as QuickJS-Debugger does,
  because clients send several at attach. On disconnect the breakpoints go and a pause resumes.
- **The test** (`cdp_test.py`) is the CDP client. It discovers the WebSocket the way js-debug does,
  sets the first breakpoint with a `urlRegex` built by a port of js-debug's `urlToRegex` for
  Windows, renders an object the way js-debug's variables view does, and checks each pause.
- **The benchmark** (`bench.py`, `bench-stock/`, `scripts/workload.js`): the charter's rule workload
  (1,000 entities each tick read components, compute damage, write back, emit events), on the stock
  `rquickjs-sys` and on the patched one in each mode, as separate processes in a shuffled order per
  round.
- **The experiment** (`patches/experiment-op-debug-operands.diff`, applied by
  `vendor.py --experiment`): `OP_debug` and `OP_debugger_stmt` carry their line and column as two
  32-bit operands and sit before the temporary opcodes; the interpreter reads them instead of
  calling `find_line_num`.
- **The wasm probe** (`wasm-probe/`): a module that installs a counting trace handler and evaluates
  a small script, built for `wasm32-unknown-unknown` with clang 23 and loaded in headless Chrome by
  `tools/webcheck.py`, and run natively by `cargo test`.

## Numbers

Machine: the reference laptop (16 cores); other agents were building at the same time, so single
runs vary by up to a factor of two and the figures below are medians over interleaved rounds.
Release builds, MSVC (QuickJS-ng then uses its switch dispatch, as on the web, where `EMSCRIPTEN` is
defined).

**End-to-end** (`PYTHONUTF8=1 python cdp_test.py`): 19 of 19 checks passed in each of four runs of
the PR build and in one run of the experiment build (`--exe .../debugger-spike-exp.exe`). From
`Runtime.runIfWaitingForDebugger` to the first `Debugger.paused`: 2.6 to 3.6 ms; a step over,
request to `Debugger.paused`: 2.9 to 3.5 ms (the CDP thread polls its socket every millisecond).

**Slowdown** (`PYTHONUTF8=1 python bench.py --rounds 10 --experiment <experiment build>`, the
experiment built as the README says; 1,000 entities, 100 warm-up ticks, 1,000 timed ticks per run;
each figure is the median over 10 rounds of a run's median tick; every run emitted the same 201,175
events):

| Mode | What runs | us per tick | Versus stock |
|---|---|---|---|
| stock | crates.io `rquickjs-sys` 0.14.0 | 319.5 | 1.000 |
| off | PR build, no handler: the bytecode is the stock engine's | 314.3 | 0.984 |
| detached | compiled with the handler, then the handler cleared: every `OP_debug` stays | 319.5 | 1.000 |
| idle | handler installed, no client: the callback returns at once | 650.9 | 2.037 |
| enabled | client attached, no breakpoint | 669.0 | 2.094 |
| armed | client attached, a breakpoint on a line that never runs | 664.8 | 2.081 |
| exp-off | experiment build, no handler | 316.3 | 0.990 |
| exp-idle | experiment build, handler installed, no client | 346.1 | 1.083 |
| exp-enabled | experiment build, client attached | 381.1 | 1.193 |
| exp-armed | experiment build, a breakpoint that never hits | 381.2 | 1.193 |

The workload passes 9,472 statements per tick, so the PR's idle handler costs (650.9 - 319.5) /
9,472 = 35 ns per statement, the experiment's 2.8 ns, and an attached client with the experiment 6.5
ns. The line lookup's share: (650.9 - 346.1) / (650.9 - 319.5) = 92 percent by the experiment; an
earlier probe (eight interleaved rounds of `bench-stock`, `bench --mode idle` and a build with only
`find_line_num` replaced by constants) gave 312.4, 631.9 and 343.7 us per tick, 90 percent.

**Patch size** (`patches/quickjs-ng-pr1421.diff`): 620 lines added and 2 removed in 4 files.

| File | Hunks | Added | Removed | What |
|---|---|---|---|---|
| `quickjs.c` | 19 | 387 | 2 | 303 lines of inspection API in one hunk (stack depth, locals, setting a variable, evaluating in a frame); 29 in the interpreter's `OP_debug` case; 14 for the emit helpers; 18 in 11 places of the statement parser (one of them `debugger;`); 19 in three bytecode passes (`code_match`, `get_label_pos`, `resolve_labels`); 4 for the context's fields and a declaration |
| `quickjs.h` | 1 | 91 | 0 | Declarations and their documentation |
| `quickjs-opcode.h` | 1 | 3 | 0 | `OP_debug` and `OP_debugger_stmt`: 252 to 254 of the 256 one-byte opcodes |
| `api-test.c` | 2 | 139 | 0 | Tests (not compiled by `rquickjs-sys`) |

The experiment adds 21 lines and removes 12 (`quickjs-opcode.h` +4 -2, `quickjs.c` +17 -10).

**Merge** (`git apply --check -v` of the PR diff): QuickJS-ng 0.16.2 as vendored by `rquickjs-sys`
0.14.0: all 23 hunks apply at offsets of -1 to 312 lines (0 to 4 in `quickjs.c`). 0.17.0 and master
`90c3922da0` (2026-10-02): all apply, `quickjs.c` at offsets of -5 to 42 lines. No fuzz, no
conflicts. The PR's base `82626eac` is 12 commits behind 0.16.2; GitHub reports it mergeable. The
opcode count is 252 in all three, so the PR's numbering holds.

**Web** (`wasm-probe/build.sh`): the patched `rquickjs-sys` compiles for `wasm32-unknown-unknown`
(clang 23.1.2, the crate's vendored wasi-libc); the release module is 1,184,294 bytes without
wasm-opt. Natively and in headless Chrome the handler saw the same 14 statements with the same line
sum (59) and the script returned 45 in both.

## Recipe for a later slice

1. Pin `rquickjs`/`rquickjs-sys` and the PR diff by hash. A script extracts the crate, applies the
   diff unmodified and touches `build.rs`, and the workspace substitutes the result with
   `[patch.crates-io]` (as `vendor.py`). On each `rquickjs-sys` update, rerun it; when upstream
   merges the PR and `rquickjs-sys` ships it, delete the script, the diff and the hand-written
   declarations.
2. Declare the patch's functions and `JSDebugLocalVar` by hand (`src/ffi.rs`) until the bindings
   carry them.
3. Instrument only while a debugger is attached: on attach, install the handler and reload every
   script at the next tick boundary (the hot-update path); on detach, clear the handler and reload
   again. Code compiled without the handler is the stock engine's bytecode (but for `debugger;`
   statements, which the PR always compiles to a checked no-op), so the cost is zero when nobody
   debugs. Precompiled release bytecode is compiled without the handler (the PR does not bump
   `BC_VERSION`, so instrumented bytecode must never be written out).
4. Pause on the game thread, inside the trace callback, and serve inspection requests there; keep
   the CDP server on another thread that only edits shared breakpoint state and forwards requests
   (charter 5.1: one thread owns the world). Clear the handler while paused and drain stale commands
   before each pause.
5. Present TypeScript to the client: URLs and sources are the `.ts` files, every location is mapped
   through oxc's source map in the endpoint, breakpoints are matched by TypeScript line. Any CDP
   client then works without source-map support, agents included, and the same mapping serves the
   structured errors of 4.2.6.
6. For js-debug: `/json/version` without `webSocketDebuggerUrl`, `/json/list` with it and
   `"type": "node"`; responses without `sessionId`, as Node sends; `urlRegex` compiled with the
   `regex` crate (it accepts js-debug's escapes); `Runtime.callFunctionOn` answered with a `result`
   always (js-debug dereferences it to list an object's properties); other unknown methods answered
   empty.
7. Raise `Error.stackTraceLimit` to 64 (the backtrace's maximum) for callers' positions, or add a
   frame-location function to the patch upstream.

## Versions and URLs

| Item | Version | Where |
|---|---|---|
| quickjs-ng PR #1421 "Add Debugging Interface" | head `0e7a5e08` (2026-08-13), base `82626eac`; open, last maintainer comment 2026-07-31 | https://github.com/quickjs-ng/quickjs/pull/1421; diff https://patch-diff.githubusercontent.com/raw/quickjs-ng/quickjs/pull/1421.diff, SHA-256 `b97233ca42edcd180f50df5a0b53bd33cc1264fa9aaf3b3680c42545fae8df66` |
| G-Yong/QuickJS-Debugger | master (pushed 2026-08-13), MIT; read, not built | https://github.com/G-Yong/QuickJS-Debugger |
| rquickjs, rquickjs-core, rquickjs-sys | 0.14.0 (2026-09-18); vendors QuickJS-ng 0.16.2 | crates.io; `rquickjs-sys-0.14.0.crate` SHA-256 `cee271d0eeba64f0915b846cb7ae02e16faf3dfdffdca91731101d9d30fe3423` |
| QuickJS-ng | 0.16.2 (vendored); 0.17.0 and master `90c3922da0` for the merge check | https://github.com/quickjs-ng/quickjs |
| oxc_allocator, oxc_parser, oxc_semantic, oxc_transformer, oxc_codegen, oxc_span | 0.152.0; oxc_sourcemap 9.0.0 | crates.io |
| tungstenite | 0.30.0 | crates.io |
| regex | 1.13.1 | crates.io |
| serde_json | 1.0.151 | crates.io |
| VS Code js-debug | 1.140.0 (2026-09-23); source read, not run | https://github.com/microsoft/vscode-js-debug |
| Rust | 1.98.1, `x86_64-pc-windows-msvc` and `wasm32-unknown-unknown` | `rust-toolchain.toml` |
| C compilers | MSVC 14.50 (Visual Studio 18) natively; clang 23.1.2 for wasm32 | `~/.pocket-tools/llvm-23.1.2` |
| Python | 3.14 with websocket-client | |

## Problems met

- **`git apply` inside the checkout** resolves a patch's paths against the repository root, and with
  Git for Windows' `core.autocrlf=true` it wrote CRLF into the patched files. `vendor.py` sets
  `GIT_CEILING_DIRECTORIES` to the crate and passes `-c core.autocrlf=false`.
- **Cargo does not rebuild the C.** `rquickjs-sys`'s build script declares only
  `rerun-if-env-changed`, and the crate's files carry the archive's fixed timestamps, so new C
  sources were ignored. `vendor.py` touches `build.rs`, which makes cargo rebuild and rerun it.
- **A shared target directory mixes patched crates.** Cargo hashes a path dependency by its path
  relative to the workspace root, so two workspaces with a patched `rquickjs-sys/` at the same
  relative path shared one build-script output, and the experiment's C briefly ended up in the main
  binary. The experiment copy now uses another directory name; every timing above comes from builds
  whose linked library was checked. Other spikes with `[patch]` path crates in the shared target
  directory can meet the same thing.
- **Bindings.** `rquickjs-sys` includes pregenerated bindings per target that lack the patch's
  functions; declaring them by hand avoided the `bindgen` feature and libclang.
- **Callers' lines.** The patch reports the paused statement's line but has no call for other
  frames. QuickJS-ng's backtrace has them, so the pause builds an `Error` from Rust and parses its
  `stack`; the default limit of 10 frames was raised to 64.
- **js-debug's variables view** calls `Runtime.callFunctionOn` and dereferences `result` in the
  answer; an empty answer would make listing any object fail. Reading its source found this; the
  endpoint implements `callFunctionOn` on the paused frame (and answers `undefined` otherwise).
- **The idle handler doubled the workload.** Replacing the line lookup with constants located the
  cost; storing line and column in the instruction removed it, after the opcode-table defect above
  was found by the experiment's crash.
- **wasm32**: the module imports the shim's clock (`env.__rquickjs_host_now_us`) and needs a smaller
  stack limit (256 KiB, as `spikes/script-web` found).
- **Stale commands.** A `Debugger.resume` sent twice would resume the next pause at once; each pause
  drains the command channel first.

## What remains open

- Attaching VS Code and Chrome DevTools for real. From js-debug's source the remaining risks are
  cosmetic (it logs and carries on when `Runtime.evaluate` or its telemetry finds no `process`), but
  only a session shows it.
- A CDP transport for the browser build: the hook works in wasm32, but a page cannot listen on a
  port; a relay through the page or a development server is unbuilt.
- Pausing on exceptions: the patch has no hook for thrown exceptions.
- The debugger and determinism: evaluating an expression, a condition or `callFunctionOn` while
  paused can change the world, so a session in which the debugger wrote anything is not replayable
  and should be marked. Evaluation while paused also runs under the execution budget's interrupt
  handler.
- A pause stops the game thread inside a tick: the editor keeps its last snapshot, and its commands
  and agents' `step` wait for the resume. The time-model and threads specifications need a state for
  this.
- Hot update: on reload the endpoint must announce new scripts and drop its cache of filename atoms.
- Locals: QuickJS-ng lists every variable of a function, including block-scoped ones not yet in
  scope (shown as `undefined`); a caller's position points at the call's last argument, not the
  callee.
- Upstream: report the opcode placement and propose the operand form on PR #1421 (the owner's call;
  nothing was posted). For comparison, PocketEngine's Lua line needs no engine patch: Lua's debug
  hook already exists (its charter names LuaPanda over DAP).
