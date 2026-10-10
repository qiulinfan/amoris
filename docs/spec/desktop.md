# Desktop editor

Amoris Desktop packages the existing React editor in an Electron window. Its viewport is the
engine's WebAssembly/WebGPU renderer. The authoritative world remains in a separate Rust
`pocket serve` process; native menus execute the same editor command registry used by the web UI.

## Build and run

From the repository root:

```sh
cd sdk && bun install --frozen-lockfile
cd ../editor && bun install --frozen-lockfile && bun run build
cd ..
cargo build --release -p pocket-app
cd desktop && npm ci
npm test
npm start -- ../samples/sailing
npm run package
```

The packaging command produces a platform-specific application under `out/desktop/`. The macOS
bundle is named `Amoris.app`. Open it and select a project folder containing `project.toml`; Open
Project also appears in the native File menu and the editor command palette. The generated bundle is
a local build, without distribution signing or notarization.

Packaging uses `target/release/pocket`, `editor/dist`, `web/viewport`, and the platform package
installed by `sdk/bun.lock`. Set `AMORIS_POCKET_BINARY`, `AMORIS_EDITOR_DIST`,
`AMORIS_WEB_VIEWPORT`, or `AMORIS_TSC` to select another verified build. The packaging tool requires
Node 22.12 or a current Node 24+ release.

## Runtime boundary

The main process starts `pocket serve PROJECT --port 0 --editor DIST` and reads its ready JSON. The
window loads that process's exact loopback URL with `?viewport=wasm`. The package contains the Rust
executable, editor distribution, `web/viewport` module, and the platform-native TypeScript 7
compiler with its library declarations. It sets `POCKET_WEB_VIEWPORT` and `POCKET_TSC` explicitly;
opening a project outside the repository needs no Bun, Node, or Rust installation at runtime.

The renderer uses a sandbox with context isolation and no Node integration. Its preload bridge
exposes project selection, recent-project selection, menu command delivery, and unsaved-change
reporting. IPC checks the owning main frame and current launcher or host origin. Navigation and new
windows are restricted; no arbitrary shell execution or filesystem API is exposed.

Project switching and application quit stop and await the owned host process. A startup failure or
unexpected host exit returns to the launcher with a readable error. Unsaved Monaco buffers and
edit-world changes trigger a Cancel/Discard dialog before switching or closing. Discarding Play
removes the host process, including its fork; scene saves retain the edit world through the existing
host protocol.

## Verification

`npm test` exercises host readiness, invalid projects, startup failures, shutdown, and desktop
security boundaries. Run `bun run typecheck` and `bun run build` in `editor/` for the shared UI. For
an application check, use a copy of a sample outside the checkout, launch the packaged app, confirm
**WebGPU (wasm)** in its status bar, edit and undo a component, then Play, Pause, Step, and Stop.
Apply a TypeScript script to verify the bundled checker, switch projects, and quit; the previous
host must exit and remove `.pocket/host.json`.

`node tools/desktop_smoke.mjs [--sample samples/sailing] [--out <dir>]` runs that check without a
person, up to the edit: it starts the app on a copy of the sample with Chromium's remote debugging,
and through the DevTools protocol checks the host's readiness identity, **WebGPU (wasm)** in the
status bar and a drawing canvas, Play (the world forks and ticks run) and Stop (the edit world's
hash is back), then closes the window as a person would (WM_CLOSE on Windows, SIGTERM elsewhere)
and checks that the host exited and removed `.pocket/host.json`. It writes a JSON summary and
half-size screenshots of the edit and play views.

### Windows (2026-10-09, Pioneer)

Run from source on Windows 11 with Electron 44.5.1 (`npm ci` in `desktop/`), the editor built in
`editor/`, a debug `pocket.exe` (`AMORIS_POCKET_BINARY`) and the SDK's `tsc.exe` (`AMORIS_TSC`):
`npm test` passes (18, the packaged-runtime test skipped without `AMORIS_TEST_RESOURCES`); that test
passes too with the resources assembled by hand; and `tools/desktop_smoke.mjs` passes every step on
samples/sailing
([evidence/polish/desktop/](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/polish/desktop)).
Electron chose the Radeon 780M (`amd rdna-3`), as Chrome does without
`--force_high_performance_gpu`.

Until then the app could not open any project on Windows: `pocket serve` canonicalized the project
to a verbatim path (`//?/C:/...`) and reported it in its readiness line, which `host.cjs` compares
with `fs.realpath` (`C:/...`), so every start ended in "Engine returned an invalid readiness
response". The host now reports the plain form (server.md 1, `hostfile::project_root`); master's
`pocket.exe` still fails the packaged-runtime test, this one passes it
([evidence/polish/desktop-node-tests.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/polish/desktop-node-tests.txt)).

Not verified on Windows: `npm run package` and a packaged app (the run used the checkout's files),
editing and applying a script through Monaco, and switching projects. On Windows the app stops its
host with `child.kill()`, which terminates the process rather than asking it to stop (Node has no
Ctrl-Break for a child); `host.cjs` then removes `.pocket/host.json` itself, as the smoke run saw.

The shell uses Electron's [security
guidance](https://www.electronjs.org/docs/latest/tutorial/security) and [application
packaging](https://www.electronjs.org/docs/latest/tutorial/application-distribution). The first
verified bundle targets macOS arm64; other operating systems require their own Rust host build and
native TypeScript package before packaging.
