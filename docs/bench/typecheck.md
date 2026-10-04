# Script type checking (TypeScript 7)

- Date: 2026-10-04. Apple M5, macOS; `pocket` release build of feat/sdk; TypeScript 7.0.2 (the
  native compiler, `sdk/node_modules/@typescript/typescript-darwin-arm64/lib/tsc`, found by
  `pocket-server`'s `typecheck.rs`); median of 15 runs (21 for the round-trip comparison), warm file
  cache, wall clock around each process.
- **Under contention**: other agents were compiling and training on the same machine (load average
  23 to 46 over the runs), so these are upper bounds on an idle machine's figures.
- Project: `samples/sailing` (3 script files) with the generated `.pocket/types/pocket.d.ts` (18.8
  KB) and `components.d.ts` (10.1 KB: 4 engine and 4 game components); `tsc --extendedDiagnostics`:
  66 files (the es2023 lib included), 12,412 lines, 10,667 types.

| What | Median | Range |
|---|---|---|
| `tsc -p samples/sailing --noEmit`, one process | 40.8 ms | 35.3-49.2 ms |
| of which tsc's own parse + bind + check (`--extendedDiagnostics`) | 23 ms | |
| `tsc_ms` inside `scripts.check` (the child process, timed by the server) | 37.6 ms | 27.6-46.4 ms |
| `pocket scripts types` (CLI round trip: compile, instantiate in a throwaway host, emit, write) | 12.1 ms | 10.4-14.4 ms |
| of which on the game thread (`scripts types` minus `status`, both CLI round trips) | about 3 ms | |
| `pocket scripts check` (CLI round trip: types, dry-run compile, tsc beside it) | 50.0 ms | 38.4-60.6 ms |
| `pocket scripts apply`, scripts unchanged (the apply answers after tsc) | 47.7 ms | 40.6-57.5 ms |
| `pocket check samples/sailing --only types` (no host: lint, declarations, tsc) | 55.3 ms | 48.1-61.4 ms |
| TypeScript 5.9.3 (`node tsc.js`, the JavaScript compiler) on the same project, for reference | 344.3 ms | 317.5-393.1 ms |

The type check runs in a child process of the server (or of `pocket check`), never on the game
thread; only `scripts.types`, a compile and an instantiation, runs there. `scripts.apply` sends the
swap to the game first and runs `scripts.types` and `tsc` beside it, so a check never delays a swap;
the apply's answer waits for both. Reproduce: `pocket serve samples/sailing`, then time the commands
above from the project directory.
