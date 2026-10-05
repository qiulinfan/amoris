# Spike: QuickJS-ng through rquickjs in the browser (`script-web`)

- Slice 0 check, charter 4.2.7 ("how `rquickjs` builds for `wasm32`"), with 3.3 (determinism), 4.2.3
  (one interpreter everywhere), 5.1 (the game thread is a Web Worker on the web) and 3.10
  (performance).
- Date: 2026-10-03. Code: `spikes/script-web/` (its `README.md` says how to build and run each
  part).

## Question

Can the same QuickJS-ng that the native engine embeds through `rquickjs` run in a browser, on the
game thread's Web Worker, and give the same bits as the native build at every tick? What build route
gets it there, can one wasm module also hold the renderer (`wgpu` on WebGPU, which needs
`wasm-bindgen` on `wasm32-unknown-unknown`), and what does it cost in time per tick, module size and
start-up?

## Verdict

**Works, with caveats.** rquickjs 0.14.0 builds for `wasm32-unknown-unknown` with `wasm-bindgen` out
of the box: its `rquickjs-sys` compiles QuickJS-ng 0.16.2 with any clang that targets wasm32,
against a subset of wasi-libc it vendors, and the module imports nothing but `wasm-bindgen`'s own
glue. A deterministic rule workload run for 600 ticks in a module Web Worker in headless Chrome
gives the same world hash as the native Windows build at every tick, for two worlds (300 entities,
seed 1; 1,000 entities, seed 7), with and without cross-origin isolation, and with QuickJS compiled
natively by MSVC and by clang-cl. One module holds QuickJS and `wgpu` together: the same worker ran
a WebGPU compute shader through `wgpu` 30 and read its results back.

The caveats a later slice must carry (none blocks):

1. On a **Windows host**, `rquickjs-sys` 0.14.0's build script passes a verbatim `\\?\C:\...`
   include path that breaks nested headers; a copy of the headers on the include path works around
   it (Problems, 1). Linux and macOS hosts should not be affected (canonicalizing gives plain paths
   there; not tried).
2. **libm differs** between the native C library and wasi-libc's musl, by one or two ulps in up to
   27 percent of the results (the probe below); the determinism setup removes those functions from
   `Math` and the engine library replaces them, as the charter already requires. The `**` operator
   also calls libm's `pow` and cannot be removed from the language: it needs the lint rule (4.2.5).
3. **A cold browser start is slower than a native one**: 67 to 88 ms from creating the worker to the
   first tick done, against 14 to 25 ms for a whole native process. The first tick alone takes 18 to
   27 ms (3 to 4 ms in steady state) while Chrome compiles the module lazily and tiers it up; steady
   speed arrives after about ten ticks.

## What was built

One crate, `spikes/script-web`, with a native binary and a `cdylib` for the browser:

- **The host** (`src/sim.rs`): a QuickJS runtime with a 256 KiB stack limit (mandatory on wasm32,
  see Recipe), a 64 MiB memory limit and an interrupt handler that counts polls (QuickJS polls about
  every 10,000 interpreted calls and backward jumps), so a script's execution budget is counted in
  work. The context has `Eval`, `JSON`, `Map`/`Set`, typed arrays and `RegExp`, and no `Date`,
  `performance`, `WeakRef`, `FinalizationRegistry` or `Promise`.
- **The determinism prelude** (`js/prelude.js`): checks those globals are absent, deletes the 22
  libm-backed `Math` functions, replaces `Math.random` with the host's PCG32, freezes `Math` and the
  host object; after the scripts load, the host freezes `globalThis`.
- **The engine math library** (`js/emath.js`): `sin`, `cos`, `atan`, `atan2`, `wrap`, `length` from
  `+ - * /`, `Math.sqrt` and `Math.floor` only (series in Horner form with argument reduction).
- **The workload** (`js/workload.js`): one stateless `tick(t, s, host)` over a host-owned world of
  six f64 components per entity, passed in and out as `Float64Array`s each tick. Per tick it builds
  a spatial grid in a `Map` keyed by strings, steers every entity toward its nearest enemy with
  `emath.atan2`, `cos` and `sin` (or wanders by `host.random()`), resolves hits sorted by target
  (damage and critical hits from `host.below` and `Math.random`), integrates, respawns, allocates
  one object per entity for a ranking sorted on three keys, formats numbers with `toFixed` and
  returns a JSON summary plus a log line per hit.
- **The world hash** (FNV-1a over canonical bytes): the tick number, every component's bits, the
  summary string, the PCG32 state and the tick's interrupt-poll count.
- **The numeric probe** (`src/probe.rs`, `js/probe.js`): libm functions through QuickJS's `Math` and
  through Rust's `f64` methods, the engine library, a pure `+ - * /` expression, and number to
  string and back, over 4,096 inputs (half in [-10, 10], half in [-1e6, 1e6]); the browser compares
  every result with the native one and reports how many differ and by how many ulps.
- **The web side**: `web/worker.js` is a module Web Worker that loads the `wasm-bindgen` output
  (`--target web`), runs the workload and the probe and posts hashes and timings; `web/main.js`
  starts it, compares with `expected.json` (written by the native binary) and reports through
  `document.title` for `tools/webcheck.py`.
- **`--features gpu`** (`src/gpu.rs`): `wgpu` 30.0.1 with only its `webgpu` backend in the same
  module; `gpu_check()` requests an adapter and a device from the worker, runs a 256-invocation
  compute shader and checks the values it reads back.
- **Tools**: `build.sh` (both builds and the reference data), `scripts/measure.py` (native and
  browser timings interleaved round by round under the same machine load, with CPU load recorded),
  `scripts/wasm_info.py` (imports, section sizes, raw and gzip sizes, Rust versus C code bytes).

## Numbers

Machine: Windows 11, 16 cores, a laptop sharing the CPU with other agents' builds during every
measurement (session A below ran beside two compiler processes; session B's CPU load at round starts
was 4 to 46 percent, recorded by `measure.py`); headless Chrome 154 on the AMD integrated GPU.
Release builds throughout (`opt-level = 3`, LTO, one codegen unit; QuickJS at `-O3` for wasm, `/O2`
for MSVC and clang-cl). Commands run from `spikes/script-web` unless they name `tools/`; `$T` is
`$CARGO_TARGET_DIR`.

### Determinism

| Check | Command | Result |
|---|---|---|
| 300 entities, seed 1, 600 ticks, 5 runs per side | `./build.sh`, then `python tools/webcheck.py http://127.0.0.1:8701/index.html --serve spikes/script-web/web --port 8701` | DONE: 600 of 600 hashes equal; final `89924e8a1a629493` |
| The same, QuickJS built by clang-cl natively | `CC_x86_64_pc_windows_msvc=<llvm>/bin/clang-cl.exe cargo build --release --bin native`, then `$T/release/native --runs 3` | final `89924e8a1a629493`, as with MSVC |
| 1,000 entities, seed 7, 600 ticks, 3 runs per side | `$T/release/native --seed 7 --entities 1000 --runs 3 --expected web/expected.json`, then webcheck with `?runs=3` | DONE: 600 of 600 equal; final `4c5b822279cac1a3` |
| Without cross-origin isolation (plain `python -m http.server`), the 1,000-entity world | README, Run, with `?runs=1&ticks=200` | DONE: 200 of 200 equal, `crossOriginIsolated` false |
| With `wgpu` linked in (`FEATURES=gpu ./build.sh`) | webcheck with `?runs=1&gpu=1` | DONE: 600 of 600 equal; compute shader read back 256 of 256 values |
| Same platform, fresh runtime per run | every run above | each run's 600 hashes equal run 0's |

The interrupt-poll count is in the hash (3,719 polls over the 600 ticks of the first world), so the
work-counted budget also agrees across platforms.

### The numeric probe (4,096 inputs, native MSVC build against the browser)

Bit-identical: `Math.sqrt`, the engine library's `sin`, `cos`, `atan` and `atan2`, a `+ - * /`
expression, `String(x)`, `toFixed`, `toPrecision`, `toExponential` (QuickJS-ng's own `dtoa`),
`parseFloat` and `Number("0x...")`, and Rust's `f64::sqrt`.

Different (results that differ, and the largest difference):

| Function | QuickJS `Math` | Rust `f64` |
|---|---|---|
| sin, cos, tan | 58, 67, 138 (1 ulp) | 58, 67 (1 ulp) |
| asin, atan, atan2 | 49, 12, 821 (1 ulp) | atan2: 821 (1 ulp) |
| exp, expm1 | 3 (1 ulp), 923 (2 ulps) | exp: 3 (1 ulp) |
| log, log2, log10 | 1, 5, 14 (1 ulp) | ln: 1 (1 ulp) |
| cbrt, tanh, hypot | 1,123, 659, 470 (1 ulp) | cbrt: 1,123 (1 ulp) |
| pow, `**`, `**` with an integer exponent | 2, 2, 5 (1 ulp) | powf: 2 (1 ulp); powi: 1,644 (5 ulps) |

QuickJS and Rust differ by the same counts because on each platform they compute with the same
algorithms: the Windows C runtime's natively, musl's in wasm (wasi-libc's `libm` for QuickJS; Rust
links musl-derived math on wasm32 as well). Rust's `powi`, whose precision Rust leaves unspecified,
differs most.

### Module size

`python scripts/wasm_info.py <module>` (gzip at level 9; the JavaScript glue is separate):

| Module | Raw bytes | gzip -9 |
|---|---|---|
| Release, after `wasm-bindgen --target web` | 1,243,991 | 460,733 |
| The same without the name section (`wasm-opt --strip-debug`) | 1,185,878 | 443,545 |
| `wasm-opt -O3` (binaryen 132) | 1,116,959 | 427,918 |
| `wasm-opt -Oz` | 1,111,611 | 427,917 |
| With `wgpu` (`--features gpu`) | 1,382,605 | 496,761 |
| With `wgpu`, `wasm-bindgen --remove-name-section` | 1,250,858 | 467,735 |
| With `wgpu`, `wasm-opt -O3` | 1,174,271 | 450,704 |

Of the release module's 1,037,011 code bytes, 929,116 are C (QuickJS-ng with its parser and
compiler, and the wasi-libc members it links) and 105,163 Rust; `wgpu` adds 46,490 bytes of Rust
code. The data section is 143,464 bytes (QuickJS's atoms and Unicode tables). The JavaScript glue is
8,122 bytes (2,387 gzipped), 40,195 (8,096) with `wgpu`. The module's imports are three
`wasm-bindgen` glue functions, nothing from `env` or WASI.

### Time per tick

The timings below were taken with the build before a final lint-only refactor of the Rust code (the
same scripts and hashes; its module was 302 bytes smaller).

Two interleaved sessions; each round runs the native binary, then the browser, with a fresh runtime
per run; the figure per side is the median over rounds of the median per-run mean tick time
(microseconds, 300 entities, 600 ticks):

| Session and command | Native, MSVC | Native, clang-cl | Browser | Browser / MSVC by round |
|---|---|---|---|---|
| A: `python scripts/measure.py --rounds 3` (5 runs a side) | 4,263 | | 4,220 | 1.05, 0.99, 1.01 |
| B: `python scripts/measure.py --rounds 4 --runs 3 --native native-msvc.exe --native-variant clangcl=native-clangcl.exe --variant O3=base-O3.wasm --variant Oz=base-Oz.wasm` (copies of the two native builds and the `wasm-opt` outputs) | 4,008 | 3,495 | 4,435 | 1.06, 0.94, 1.09, 1.32 |

In session B the `wasm-opt` modules ran at 4,253 (`-O3`) and 4,310 (`-Oz`), inside the noise of the
4,435 of the unoptimized one. Single runs range from 2,967 to 5,574 on the browser side and 3,409 to
5,064 natively; the first run of a round is usually the fastest on both sides (the laptop's boost
clock), so only same-round comparisons mean much.

So the browser runs this workload at about the speed of the native Windows build: 1.0 to 1.1 times
the MSVC build's time, about 1.27 times the clang-cl build's. The Windows builds and the wasm build
all use QuickJS's switch dispatch (`DIRECT_DISPATCH` is 0 under `_MSC_VER` and `EMSCRIPTEN`); a
native build on Linux or macOS uses computed goto and should be faster than both (not measured).

The 1,000-entity world (seed 7) took 20,941 to 27,503 per tick natively (3 runs,
`native --seed 7 --entities 1000 --runs 3`) and 19,187 to 23,735 in the browser (3 runs, run right
after), with the same load caveat. Neither world is the charter's reference rule workload (4.2.7,
measured by the native speed spike); these numbers compare the two platforms, not the budget.

### Start-up to the first tick

Browser, cold (a new Chrome profile, `Cache-Control: no-store`), from the page creating the worker,
seven rounds of sessions A and B (milliseconds):

| Step | Cold (run 0) | Warm (runs 1 to 4, same worker) |
|---|---|---|
| Worker script and glue loaded (worker created to its first message) | 10.9 to 16.6 | |
| `init()`: fetch, compile and instantiate the module | 18.8 to 24.7 | |
| `Sim::new`: runtime, context, prelude, scripts, freeze | 13.6 to 20.6 | 0.7 to 1.6 |
| The first tick | 17.6 to 27.2 | 3.2 to 8.3 |
| **Worker created to first tick done** | **66.8 to 87.8** | |

The cold steps are slow because Chrome compiles the module lazily with its baseline compiler and
optimizes hot functions later: in run 0, ticks 1 to 9 average 5.7 to 9.1 ms, ticks 10 to 99 3.2 to
4.6 ms, and from tick 100 on 2.8 to 4.3 ms. With the `wasm-opt -O3` module, worker created to first
tick was 50.1 to 63.0 ms in session B (the unoptimized module, timed last in each round, took 66.8
to 87.8).

Native (`native` binary, same rounds): `Sim::new` 0.76 to 0.93 ms, the first tick 3.5 to 4.7 ms with
no warm-up curve, `main` to first tick done 4.4 to 5.6 ms, and the whole process from spawn to exit
after one tick 14.2 to 25.1 ms (`measure.py`, five spawns per round, session B).

## Recipe

For slice 1's `wasm32` build of the script host (and slice 3's browser-local run form):

1. **Dependencies**:
   `rquickjs = { version = "=0.14.0", default-features = false, features = ["std"] }` (add `loader`
   for modules); `wasm-bindgen = "=0.2.129"` with `wasm-bindgen-cli` 0.2.129 installed at the same
   version; `crate-type = ["cdylib", "rlib"]`.
2. **C compiler**: point cc-rs at a clang with the wasm32 target:
   `CC_wasm32_unknown_unknown=<llvm>/bin/clang`, `AR_wasm32_unknown_unknown=<llvm>/bin/llvm-ar`.
   Nothing else is downloaded or needed; `rquickjs-sys` ships the wasi-libc headers and `libc.a`
   (from wasi-sdk 24) and its own shim for the clock, `abort` and stdio. On a Windows host, add
   `CFLAGS_wasm32_unknown_unknown="-isystem <copy of rquickjs-sys/vendor/wasi-libc/include>"`
   (`build.sh` makes the copy) until the upstream path fix lands.
3. **The one import**: QuickJS's shim imports `env.__rquickjs_host_now_us`. Define it in Rust,
   `#[unsafe(no_mangle)] pub extern "C" fn __rquickjs_host_now_us() -> f64`, and wasm-ld resolves it
   inside the module, so `wasm-bindgen`'s output imports nothing from `env`. Return 0 or simulation
   time: no script may see the wall clock.
4. **Runtime limits**: `set_max_stack_size(256 * 1024)` is mandatory on wasm32 (QuickJS's 1 MiB
   default equals the whole shadow stack, and its stack check then wraps, turning deep recursion
   into a trap instead of a `RangeError`; rquickjs README); set a memory limit and an interrupt
   handler that counts polls.
5. **Determinism setup**, identical natively and on the web: `Context::custom` without `Date`,
   `Performance`, `WeakRef` and `Promise`; delete libm-backed `Math` functions; replace
   `Math.random` (QuickJS seeds its own from the clock when a context is created); freeze `Math`,
   the host objects and `globalThis`; give scripts an engine math library from `+ - * /` and `sqrt`;
   forbid `**` by lint. Hash the interrupt-poll count with the world: it agrees across platforms.
6. **The game thread**: `wasm-bindgen --target web`, loaded by a module worker
   (`new Worker(url, {type: "module"})`, then `await init({module_or_path})`). It needs no
   cross-origin isolation, no `SharedArrayBuffer` and no wasm threads.
7. **The renderer can share the module**: `wgpu` with
   `default-features = false, features = ["webgpu", "wgsl"]` links beside QuickJS (C objects from
   clang, the vendored `libc.a`, Rust's own allocator and `wasm-bindgen`'s glue coexist) and reaches
   WebGPU from the worker. Whether the presenting thread instantiates the same module or a second,
   smaller one is the threads specification's call (charter 7.15); either builds.
8. **Shipping**: run `wasm-opt -O3` on the `wasm-bindgen` output (10% smaller raw, 7% gzipped, no
   measurable change in speed); `-Oz` saves nothing more.
9. **The check**: a page like `web/` (native reference hashes in, per-tick comparison, a verdict in
   `document.title`) runs under `tools/webcheck.py` in the local check command.

## Versions and sources

| What | Version | Source |
|---|---|---|
| rquickjs, rquickjs-core, rquickjs-sys | 0.14.0 (repository commit `d7ef5eea`) | https://crates.io/crates/rquickjs, https://github.com/DelSkayn/rquickjs |
| QuickJS-ng (vendored by rquickjs-sys) | 0.16.2 | https://github.com/quickjs-ng/quickjs |
| wasi-libc headers and `libc.a` (vendored by rquickjs-sys) | wasi-sdk 24, `wasi-sysroot-24.0.tar.gz` | https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-24 |
| Rust | 1.98.1 (LLVM 22.1.8), `wasm32-unknown-unknown` | `rust-toolchain.toml` |
| clang, llvm-ar, clang-cl | 23.1.2 | https://github.com/llvm/llvm-project/releases (already at `~/.pocket-tools/llvm-23.1.2`) |
| MSVC (native QuickJS; cc-rs's choice, confirmed by the object's `@comp.id`) | cl 19.50.35728, Visual Studio 18 | installed |
| cc-rs | 1.6.0 | crates.io |
| wasm-bindgen, wasm-bindgen-cli | 0.2.129 | crates.io (CLI already installed) |
| wasm-bindgen-futures, js-sys, web-sys | 0.4.79, 0.3.106, 0.3.106 | crates.io |
| wgpu | 30.0.1 | crates.io |
| binaryen `wasm-opt` | 132 (from emsdk 6.0.9) | already at `~/.pocket-tools/emsdk` |
| Chrome (headless, via `tools/webcheck.py`) | 154.0.8037.93 | installed |

Nothing new was downloaded or installed outside cargo's registry.

## Problems met

1. **Nested wasi-libc headers not found on Windows.** `rquickjs-sys`'s build script canonicalizes
   `vendor/wasi-libc/include`, which on Windows yields `\\?\C:\...`; clang finds `stdlib.h` through
   it but not `<bits/alltypes.h>`, because a verbatim path takes no `/` separator. Passing the same
   directory again in `CFLAGS_wasm32_unknown_unknown` does not help: cc-rs 1.6 appends it after the
   build script's flag and clang drops it as a duplicate directory. A copy of the headers at a plain
   path does work (searched after the verbatim one, it supplies the nested headers); `build.sh`
   makes it in the target directory. The upstream fix is a non-verbatim absolute path in `build.rs`
   (for example `std::path::absolute`, or the `dunce` crate); it is worth reporting to rquickjs.
2. **The `env` import.** The shim's clock import would leave `wasm-bindgen` an import from a module
   named `env`, which no browser can resolve; defining the symbol in Rust removes it at link time
   (Recipe, 3). Checked with `wasm_info.py`: no `env` or WASI import remains.
3. **`python3` on this machine** is the Microsoft Store alias, which fails; the scripts take a
   `PYTHON` override and `build.sh` falls back to `python`.
4. **Timing noise.** Other agents built in parallel throughout (up to 46 percent CPU load at round
   starts, and 57 percent at one spot check) and the laptop's clocks drop after the first seconds of
   load (the first run of each round is usually the fastest on both sides). `measure.py` therefore
   interleaves native and browser runs round by round and compares medians; the spread is reported
   with every figure.

## What remains open

- **Other browsers**: only Chrome 154 (V8) was run. Firefox and Safari run the same wasm, so the
  hashes should agree, but neither was tested, nor were their speeds.
- **Other native platforms**: Linux and macOS natively, and arm64 above all, were not built. On
  arm64 clang contracts `a * b + c` into a fused multiply-add by default within one C expression
  (the lesson in `master`'s `docs/design/networking.md`); build QuickJS there with
  `-ffp-contract=off` and run this check. Natively on Linux and macOS QuickJS uses computed-goto
  dispatch (`DIRECT_DISPATCH`), which MSVC, clang-cl and wasm builds do not, so native will be
  faster than measured here.
- **Code caching across visits**: every browser start here was cold (a throwaway profile and
  `Cache-Control: no-store`). Chrome caches compiled wasm for later visits; the warm-visit start-up
  was not measured.
- **The memory limit depends on the platform**: QuickJS's objects are smaller on wasm32 than on
  64-bit native (73 KB against 108 KB between ticks here), so a script near the limit fails at
  different points; the limit must stay a safety net, never part of the rules. Garbage-collection
  timing also differs, harmlessly while `WeakRef` and `FinalizationRegistry` stay out.
- **Not tried**: the `wasm32-wasip1` route (it needs a WASI shim in the worker and cannot share a
  module with `wgpu`'s `wasm-bindgen` backend, so it fits the charter worse); wasm threads;
  `rquickjs`'s `rust-alloc` feature (QuickJS on Rust's allocator instead of wasi-libc's dlmalloc);
  precompiled QuickJS bytecode; the debugger patch (PR #1421) on wasm; a transpiled TypeScript
  workload (plain JavaScript here).
- **Adapter info**: the worker's WebGPU adapter reported empty name, vendor and driver strings
  (browsers withhold them by default); the compute results were correct.
