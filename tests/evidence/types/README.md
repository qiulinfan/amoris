# Type checking evidence

- `watch.log` (2026-09-29): `pocket run checkdemo --watch -- --headless` on a fresh `pocket new` project. The start is checked clean; `position` misspelt as `positon` in `world.set` is caught on the next save, with its place and the compiler's suggestion, and reaches the runtime (`script.diagnostics` answered the same error over `/rpc`); the fix clears it. The scratch project was removed afterwards.
- The first `pocket check` of the workspace found 38 errors in 8 files before any fix: the SDK used `Vec3` in `world.ts` without importing it, defined `events.lastSeq` twice (the first reading a field the command does not return), left JSX `key` and `animationend` untyped and `Row` without `background`; a physics scenario passed its ray length where the options go (the SDK now takes either); the editor, `tests/ts` and the swarm sample used the host's `setTimeout` and `performance` undeclared. All fixed; `pocket test` keeps the workspace at zero as its `types` module.

Reproduce: `./.pocket/pocket check`, `./.pocket/pocket check samples/ui`, `runtime_tests "[types]"`.
