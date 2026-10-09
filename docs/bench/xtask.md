# Local checks: method, coverage and platform limits

`cargo xtask check` checks dependency boundaries, source quality, builds, generated files and
runtime behaviour. Performance is recorded separately and is not a pass condition. A successful
check must distinguish executed checks from skipped ones and record the actual platform.

Specifications: [checks](../spec/checks.md), [first slice](../spec/checks-slice1.md),
[architecture](../spec/architecture.md), [versions](../spec/versions.md) and
[script sandbox](../spec/script-sandbox.md). Script usage: [SDK](../sdk.md).

## Check method

Run the full gate on a stable tree after the relevant dependencies are installed. It builds native
and wasm targets, checks generated declarations, and verifies the same deterministic workload
through native debug/release builds, forks, recordings, reloads and the browser worker. The browser
step exercises isolated and ordinary loading rather than accepting a page that merely starts.

Targeted checks help during development; the full gate is required for changes to shared code.
Each step reports its own verdict. Unavailable platform-specific tests or external-asset smoke
fixtures must be explicitly conditional, not reported as passed. Avoid editing the checked tree
or running competing builds during the gate.

```sh
cargo xtask check
cargo xtask check --only types
cargo test --release -p pocket-script --features transpile --test depth
```

Use a worktree-local cargo target for parallel development. Windows builds require the native
TypeScript 7 executable through `POCKET_TSC`; a launcher shim is not an equivalent timing path.
QuickJS-ng's wasm build needs a clang with the wasm backend and its configured toolchain.

## Latest source-side Windows result

The final Pioneer check recorded on 2026-10-10 at source revision `5919744f` executed 689 tests:
14 check steps passed, two were skipped, and the command exited successfully. This is a historical
Windows result for the source implementation, not validation of a later Amoris integration or
another platform. Run the gate on the integrated tree to establish those results.

| Step | Latest source result | Coverage |
|---|---|---|
| deps | PASS | Allowed crate edges, deterministic dependencies and offline vendor verification |
| fmt | PASS | Rust formatting |
| docs | PASS | Document width and wrapping rules |
| clippy | PASS | Native and wasm lint checks |
| build | PASS | Native workspace |
| gen | PASS | Generated outputs match their sources |
| test | PASS | 689 tests, including applicable ignored tests |
| wasm | PASS | Game-side wasm compilation |
| types | PASS | Real native TypeScript 7.0.2 |
| determinism | PASS | Native workload parity and cross-build comparison |
| fork | PASS | Fork isolation and continuation |
| replay | PASS | Recording and deterministic replay |
| reload | PASS | Project reload behaviour |
| contract | SKIP | Shared conformance harness and synchronized reference absent |
| perf | SKIP | Budget workloads, calibration and reporting harness absent |
| web | PASS | Browser-worker parity with native recordings |

Machine: Ryzen 9 270, 16 threads, 15 GiB RAM, Windows 11 Pro 10.0.26200; RTX 5060 Laptop
and Radeon 780M. Rust 1.98.1, LLVM 23.1.2 with clang-cl, Chrome 155, Python 3.14,
TypeScript 7.0.2. Gate durations during the earlier parallel work are not calibrated benchmarks.

## Integrity checks delivered with the gate

Offline vendor verification reconstructs the 306 files of the patched QuickJS-ng binding from
the pinned crate and patch set. A byte difference is `deps.vendor_stale`. If the pinned archive
is absent from the local cargo cache, the check reports `deps.vendor_unchecked`; it fetches nothing.

Native and web hosts install the compiled engine version at startup. Recorded compiled modules
are reused only by a compatible engine; otherwise they are compiled again, or refused when
transpilation is unavailable. Replay verification rejects an incompatible version with
`version.mismatch`. This prevents a cached module from silently bypassing the implementation it
was recorded against.

A held command whose scheduled tick has already passed now returns `command.tick_passed`.
A targeted test advances the clock under 4,095 held commands and verifies that all are answered
and the queue remains reusable. It tests a queue invariant rather than relying on a wall-time score.

Document checks count display columns, including East Asian wide characters, preserve hard
breaks and link definitions, and wrap only overlong paragraphs. Shared contract text is governed
by its synchronization rules; a local formatter must not rewrite it independently.

## Windows script-stack limits

QuickJS-ng's depth check evaluates `down(1000)` with a configured maximum call depth of 200.
The MSVC fallback needs more stack than clang-cl. These are stack measurements, not performance
measurements:

| C compiler | Profile | Stack used | Configured stack | Result |
|---|---|---:|---:|---|
| clang-cl 23.1.2 | release | 368,468 B | 1 MiB | Pass |
| MSVC cl 19.50 | release | 1,170,892 B | 2 MiB | All seven depth tests pass |
| MSVC cl 19.50 | debug | 3,479,908 B | 6 MiB | Pass |

The cl fallback emits a build warning naming the actual compiler. Validate both the sandbox's
script-depth limit and the host thread's native stack when changing compilers or optimization.
A compiler label alone does not prove stack safety on another target.

## Remaining coverage

`contract` needs executable conformance cases, a benchmark-harness self-test and a synchronized
reference commit. `perf` needs workload files, calibration, the benchmark command and reporting
against budgets. Until those exist, their verdict remains SKIP. The existing web measurements
can inform future budgets but do not implement that missing gate.

Metal-only tests establish Metal behaviour only when run on macOS. Asset smoke checks require
their project fixture. The Windows gate does not establish Apple GPU correctness, packaging or
performance. Keep per-run logs and detailed check receipts local under ignored `out/`.

Historical final Windows check: [Pioneer check
result](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/xtask).
