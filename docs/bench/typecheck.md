# Script type checking (TypeScript 7)

- Date: 2026-10-04. Apple M5, macOS; `pocket` release build of feat/sdk; TypeScript 7.0.2 (the
  native compiler, `sdk/node_modules/@typescript/typescript-darwin-arm64/lib/tsc`, found by
  `pocket-server`'s `typecheck.rs`); median of 15 runs, warm file cache, wall clock around each
  process (CLI rows) or each call (game-thread rows).
- **Under contention**: other agents were compiling and training on the same machine (load average
  20 to 28 over these runs), so these are upper bounds on an idle machine's figures.
- Projects: `samples/sailing` (3 script files; with the generated `.pocket/types/pocket.d.ts`, 20.3
  KB, and `components.d.ts`, 10.1 KB: 4 engine and 4 game components; `tsc --extendedDiagnostics`:
  66 files with the es2023 lib, 12,425 lines, 10,914 types, check 20 ms) and a generated project of
  40 components of 8 fields and 40 systems in 8 files (`generate` in
  `crates/pocket-runtime/tests/host.rs`; 73 files, 14,359 lines, 18,591 types, check 28 ms).

## On the game thread

What `scripts.check` costs the game thread: the server sends `scripts.apply {dry_run, types}`, one
compile, then (scripts that are not the running bundle) an instantiation in a throwaway host, the
emitter and the writes under `.pocket/types/`. `tsc` runs after it answers, in a child process.
Timed around `Game::apply` in the ignored test `script_types_cost`:

| What | sailing | generated |
|---|---|---|
| `scripts.types`, scripts unchanged (compile; the running program answers the components) | 0.59 ms (0.23-14.6) | 1.58 ms (0.91-2.11) |
| `scripts.apply {dry_run, types}`, scripts changed (compile, instantiate, emit, write) | 3.76 ms (2.87-8.12) | 8.30 ms (5.01-21.3) |

Before this layout, `scripts.check` compiled twice (the dry run, then `scripts.types`) and
instantiated once, and `scripts.apply` compiled twice and instantiated twice (the swap, then the
throwaway host); now each compiles once and instantiates once.

## Round trips and `tsc`

| What | sailing | generated |
|---|---|---|
| `tsc --noEmit -p`, one process | 41.5 ms (35.4-80.9) | 68.1 ms (58.2-124) |
| `tsc_ms` inside `scripts.check` (the child process, timed by the server) | 38.3 ms (one run) | |
| `pocket scripts types` (CLI round trip) | 11.5 ms (7.5-18.4) | |
| `pocket status` (CLI round trip, for scale) | 12.4 ms (10.0-16.3) | |
| `pocket scripts check` (CLI round trip: dry run with types, then tsc) | 57.3 ms (49.6-62.4) | |
| `pocket scripts apply`, scripts unchanged (the apply answers after tsc) | 55.8 ms (47.4-61.6) | |
| `pocket check <project> --only types` (no host: lint, declarations, tsc) | 70.8 ms (61.3-86.0) | 93.1 ms (67.4-142) |
| TypeScript 5.9.3 (`node tsc.js`, the JavaScript compiler), earlier run, for reference | 344.3 ms (317.5-393.1) | |

A CLI round trip is dominated by starting the `pocket` process and its HTTP call: `scripts types`
and `status` cost the same within noise, so the game-thread table above is the measure of the work.

## Windows R1 on a quiet machine (2026-10-10)

The cheap parts again, on the Windows laptop (budgets.md R1: Ryzen 9 270, 15 GB) with no other agent
running ([quiet-2026-10-09.md](quiet-2026-10-09.md), session 2), master `0871920d`'s release build
and TypeScript 7.0.2 from `sdk/node_modules`. Another machine than the M5 above, so these do not
supersede its figures; they are this machine's
([script-types-cost.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/typecheck/script-types-cost.txt),
[cli.json](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/quiet/typecheck/cli.json)).

| What | sailing | generated |
|---|---|---|
| `scripts.types`, scripts unchanged (game thread, median of 15) | 0.72 ms (0.65-10.21) | 2.02 ms (1.92-9.95) |
| `scripts.apply {dry_run, types}`, scripts changed (game thread, median of 15) | 12.68 ms (12.07-15.65) | 18.72 ms (16.85-20.95) |
| `tsc --noEmit -p`, the native binary (median of 15) | 53.4 ms (50.9-58.0) | |
| `pocket check <project> --only types`, with the native binary | 114.1 ms (109.0-119.8) | |
| `tsc --noEmit -p` through `node_modules/.bin/tsc.exe` | 134.1 ms (130.6-142.4) | |
| `pocket check <project> --only types` with `POCKET_TSC` at that `.bin/tsc.exe` | 278.6 ms (271.9-295.2) | |

- The dry run with declarations costs the game thread 2.3 to 3.4 times the M5's (12.7 against 3.8 ms
  on sailing, 18.7 against 8.3 generated) while the compile alone is close (0.72 against 0.59 ms);
  the difference is in the instantiation or the writes under `.pocket/types/`. Not investigated.
- `node_modules/.bin/tsc.exe` is bun's shim, which starts `node` and the package's launcher before
  the native compiler: about 80 ms per `tsc` run, and 165 ms per `pocket check` (which runs it
  twice, `--version` first). `pocket` prefers the native binary of the platform package when it
  finds `sdk/node_modules` itself (typecheck.rs); a `POCKET_TSC` should name that binary,
  `sdk/node_modules/@typescript/typescript-win32-x64/lib/tsc.exe`, not the shim.

## The editor's worker

`editor/tools/sdk_check.ts` against `pocket serve` of a sailing copy: Monaco's TypeScript worker
(TypeScript 5.9, monaco-editor 0.57) loaded the host's declarations (4 engine and 4 game components)
and reported the same code and position as `tsc` 7.0.2 for each of the 15 mistakes of
`editor/tools/sdk_mistakes.ts.txt` (docs/sdk.md's table).

## docs/sdk.md's size

1,663 tokens by cl100k and 1,667 by o200k (5,435 bytes, 819 words), counted with the `gpt-tokenizer`
npm package (4.0.0, its encodings bundled): `bun add gpt-tokenizer`, then `encode(text).length` from
`gpt-tokenizer/encoding/cl100k_base` and `.../o200k_base`. Claude's own tokenizer was not available
here; code-heavy text tends to cost it more tokens than cl100k, so the guide is kept about 15% under
the 2,000-token budget.

## Reproduce

```sh
cargo test --release -p pocket-runtime --features thread,transpile --test host script_types_cost \
  -- --ignored --nocapture        # the game-thread table; prints the generated project's path
pocket serve samples/sailing --port 7911   # then, in samples/sailing, time the CLI rows
```
