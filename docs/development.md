# Developing Pocket3D

How the engine is developed day to day, on any machine: setting a machine up, the build, test and
evidence loop, the agent benchmark's operation, the gotchas that have cost time, and where the work
stands. `AGENTS.md` holds the rules for every change and the daily commands; `docs/status.md` what
exists; this file what a developer or a coding agent needs to keep going after moving to another
machine. Everything here was true on 2026-10-02 at commit `60b757ee` on the reference machine
(below); a new machine should check it rather than assume it.

## Starting a session

1. Read `AGENTS.md`, then `docs/status.md` (the Built table and Next), then the Current state at the
   end of this file, then `git log --oneline -20`.
2. Bring the tool up to date if `tools/pocket/` changed since it was installed (Installing the tool,
   below), and bundle what you will run (`./.pocket/pocket ts samples/<name>`).
3. Check nothing long is running before starting builds or suites:
   `ps -ax -o pid,etime,command | grep -E 'pocket test|_tests|agent_eval|pocket_runtime' | grep -v grep`
   (on Windows: `tasklist | grep -i -E 'pocket|_tests'`; a running executable cannot be relinked
   there, so a leftover runtime fails the next build's link).

The owner's standing directives for this repository (given in conversation, dated):

- 2026-09-29: the engine is developed autonomously by the agents; the owner does not supervise each
  step.
- 2026-10-01: the branch being developed is `master`, kept current: every bit of progress is
  committed and pushed (`git push -q origin master`), with the agent's co-author trailer.
- 2026-10-01: helper scripts are kept in the repository (`tools/scripts/dev/`), never only in a
  session's scratch directory, which is wiped. Research, extension and implementation come first;
  testing stays light (the module or tag a change touches while working, the whole suite before a
  commit to shared code); performance, reach and the agent-first idea matter most.
- 2026-10-01: GitHub Actions is off (`gh workflow disable ci`); all verification is local.
  `.github/workflows/ci.yml` stays as a list of what a full verification covers; do not re-enable
  it.
- 2026-10-01: gameplay and benchmark runs use opencode with GLM 5.3 Flash; oh-my-pi with DeepSeek is
  the second opinion.
- 2026-09-17 on: developer tools and toolchains may be installed without asking (brew, rustup,
  cargo, npm, pip, emsdk, simulator runtimes). Ask the owner for anything that needs sudo or a
  password, changes system security settings, costs money, or accepts a licence on their behalf (the
  Android SDK licence, the Windows SDK's terms through xwin).
- Public-facing positioning (`docs/positioning.md`, a README pitch) is written slowly with the
  owner, never drafted in one turn; no filler, no unverified claims.

## Repository

- `git@github.com:qiulinfan/aipocket.git` (private), default branch `master` (renamed from the
  orphan branch `agent-first` on 2026-09-30). The other branches are archives: `pocket3d` (the
  PocketEngine base) and `pocket3d-graphics-physics` (an OpenGL, Metal, Physics3D and Lua reference
  implementation of 2026-09-17, what `AGENTS.md` rule 6 calls aipocket). Read them; never merge,
  rebase or cherry-pick them into `master`. PocketEngine is `github.com:qiulinfan/pocketEngine`.
- Never committed, machine-local: `.pocket/` (fetched toolchains, dependencies, caches, the
  installed tool), `build/` (outputs, benchmark traces), `.venv/`, `.codex/` (a project-level Codex
  MCP registration mirroring `.mcp.json`; Codex users make their own, `docs/mcp.md`),
  `compile_commands.json` (written by every build with the machine's absolute paths), the agent
  CLIs' configuration and keys.
- `README.md` belongs to the owner and is edited only when they ask (its status line is behind).

## Setting up a machine

Supported hosts: Windows 11 on x86-64 with Direct3D 12 (native, where development continues from
2026-10-02, ADR 0008), macOS on Apple silicon (native, the reference until then), Linux aarch64 and
x86_64 (verified in containers; a native host recipe below), browsers through the web build, the iOS
Simulator. There is no Android target yet (research only).

### Windows (x86-64)

What the owner installs (licences and administrator rights): Visual Studio 2022 or later with the
"Desktop development with C++" workload and a Windows 10/11 SDK (Community or Build Tools), Git for
Windows (Git Bash is the agents' shell), Python 3.14 from python.org, and CMake and Ninja on PATH
(Strawberry Perl's or Visual Studio's do). The rest needs no administrator:

```bash
curl -sSL -o rustup-init.exe https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe
./rustup-init.exe -y --default-toolchain 1.98.1 --profile minimal --no-modify-path   # then add ~/.cargo/bin to PATH
git clone git@github.com:qiulinfan/aipocket.git && cd aipocket
python tools/scripts/dev/fetch_llvm.py     # LLVM 23.1.2's release archive, unpacked to ~/.pocket-tools/llvm-23.1.2
./scripts/bootstrap.sh                      # builds tools/pocket, installs .pocket/pocket.exe, runs pocket setup and pocket doctor
./.pocket/pocket build && ./.pocket/pocket build --config release
python -m pip install pillow fonttools websocket-client
```

- The compiler is clang++ for `x86_64-pc-windows-msvc` (not clang-cl), linking with lld against the
  newest Visual Studio's C++ library and Windows SDK, which clang finds itself. A "Developer
  PowerShell" for an older Visual Studio changes which toolset clang picks; start the tool from an
  ordinary shell.
- `pocket setup` builds the CMake dependencies with that clang and links JavaScriptCore into
  `pocket_jsc.dll` (`tools/pocket/src/windows.rs`, ADR 0008); `pocket build` copies it, the Windows
  SDK's `dxcompiler.dll` (Direct3D 12's shader compiler) and, for debug, AddressSanitizer's runtime
  beside the executables.
- The checkout is LF (`.gitattributes`); Git for Windows' `core.autocrlf=true` would otherwise make
  every file CRLF and rewrite the generated files on each build.
- Executables embed a manifest that makes UTF-8 their code page and allows long paths
  (`tools/pocket/src/windows.manifest`).
- `POCKET_GPU_BACKEND=vulkan` runs wgpu's Vulkan backend instead of Direct3D 12, to compare.
- The `pocket` MCP server (`.mcp.json`, `.codex/config.toml`) starts only once `.pocket/pocket.exe`
  exists: an agent session opened before `bootstrap.sh` reports it as failed (Claude Code's log says
  "The system cannot find the path specified", cmd's words for the missing `.pocket`); reconnect it
  or open a new session after bootstrapping.

### macOS (Apple silicon)

```bash
xcode-select --install                      # Command Line Tools (Apple clang 21); native builds use them
curl https://sh.rustup.rs -sSf | sh         # rustup; rust-toolchain.toml pins the version
brew install cmake ninja                    # CMake builds the foreign dependencies (SDL3, FreeType, HarfBuzz, ...)
git clone git@github.com:qiulinfan/aipocket.git && cd aipocket
export PATH="$HOME/.cargo/bin:$PATH"        # agent shells often lack it
./scripts/bootstrap.sh                      # builds tools/pocket, installs .pocket/pocket, runs pocket setup and pocket doctor
./.pocket/pocket build && ./.pocket/pocket build --config release
python3 -m pip install --break-system-packages pillow fonttools websocket-client   # Homebrew Python is PEP 668 managed
```

`pocket` points `DEVELOPER_DIR` at `/Library/Developer/CommandLineTools` whenever that directory
exists (`tools/pocket/src/toolchain.rs`), so native builds never need Xcode or its licence. Python
packages: Pillow for every evidence script (they import PIL under plain `python3`), fontTools for
`pocket pack --web` (font subsetting runs `python3 -m fontTools.subset` with the first `python3` on
`PATH`), websocket-client for `tools/scripts/web_offline.py`. If a `python3` without them shadows
Homebrew's (uv's `~/.local/bin/python3` did), put a venv with all three first on `PATH` instead.

Optional, per feature:

| For | Install | Then |
|---|---|---|
| Web builds | `git clone https://github.com/emscripten-core/emsdk ~/.pocket-tools/emsdk && cd ~/.pocket-tools/emsdk && ./emsdk install 6.0.9 && ./emsdk activate 6.0.9` (1.9 GB; or `POCKET_EMSDK`) | `./.pocket/pocket setup --target wasm` (rebuilds FreeType, HarfBuzz, libwebp and Draco for wasm; the first wasm build fetches the SDL3 and emdawnwebgpu ports, so it needs the network) |
| iOS Simulator | Xcode at `/Applications/Xcode.app` (or `POCKET_XCODE`), its licence accepted by the owner (`sudo xcodebuild -license accept`), a runtime: `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild -downloadPlatform iOS` (about 8 GB) | `./.pocket/pocket setup --target ios-sim`; no device signing (no developer account) |
| Linux in a container | `brew install colima docker docker-buildx`, then `mkdir -p ~/.docker/cli-plugins && ln -sfn "$(brew --prefix)/opt/docker-buildx/bin/docker-buildx" ~/.docker/cli-plugins/docker-buildx`; `colima start --cpu 8 --memory 20 --disk 80` (a 10 GB VM OOM-killed the debug renderer tests) | `tools/scripts/linux.sh test --config release`; `POCKET_LINUX_ARCH=amd64` for x86_64 (QEMU unless the VM is recreated with `--vm-type vz --vz-rosetta`) |
| Model import from `.blend`, `.fbx`, ... | Blender at `/Applications/Blender.app` (or `POCKET_BLENDER`, or `[assets] blender`) | without it `runtime_tests` `[import]` and `[blenderlevel]` skip and the `blender_level` benchmark task cannot pass |
| `web_offline.py` | Google Chrome (or `POCKET_CHROME`) | |
| `wasm_core_tests.py` | Node on `PATH` | |
| The agent benchmark | opencode and oh-my-pi (`docs/agent-eval.md`, Coding agents) | |
| Reference sources (read-only, `AGENTS.md` rule 6) | `git clone --filter=blob:none --sparse -b release git@github.com:EpicGames/UnrealEngine.git ~/Reference/UnrealEngine && git -C ~/Reference/UnrealEngine sparse-checkout set Engine/Source Engine/Build Engine/Config` (a GitHub account linked to Epic); `git clone https://github.com/Unity-Technologies/UnityCsReference ~/Reference/UnityCsReference` | |

`pocket doctor` checks Ninja, the native dependencies, emsdk and fontTools only; a green doctor does
not mean CMake, Xcode, the iOS runtime, Blender, Chrome, Docker or the agent CLIs are there.

### Linux host (no container)

From `tools/docker/linux/Dockerfile` (Ubuntu 26.04):
`apt install clang lld libclang-rt-21-dev libstdc++-15-dev cmake ninja-build pkg-config make curl git python3 xz-utils unzip libjavascriptcoregtk-4.1-dev mesa-vulkan-drivers libvulkan1`
and the X11, Wayland, ALSA, Pulse, udev, dbus, drm, gbm and EGL development packages that file
lists; rustup; `export CC=clang CXX=clang++` so the CMake dependencies use clang; then
`./scripts/bootstrap.sh` (nothing in it is macOS-only). Links use lld; C compiles as gnu17. Not yet
run outside a container.

### Known-good versions (the reference machine)

Apple M5, 10 cores, 32 GB, macOS 27.0. Command Line Tools with Apple clang 21.0.0; Xcode 27.0
(27A266a, licence accepted) with the iOS 27.0 simulator runtime and the device "iPhone 18 Pro";
rustc 1.98.1; CMake 4.4.0; Ninja 1.13.2; Homebrew Python 3.14.7 (Pillow 12.3, fontTools 4.65,
websocket-client 1.9.2); Node 24.18; Emscripten 6.0.9; Blender 4.5.12 LTS; Docker 29.7.1, colima
0.10.3, buildx 0.37.2; opencode 1.18.2; oh-my-pi 18.4.3. Performance numbers in the docs were
measured here; measure again on a new machine before comparing (`tools/scripts/dev/gpu_probe.py`).

### Moving from another machine

- Copy `.pocket/cache/` to skip about 140 MB of downloads (draco 60 MB, harfbuzz 19 MB, Noto Sans
  CJK 16 MB, SDL3 16 MB, wgpu 14 MB, TypeScript 9 MB, ...; every file is checked against its
  sha256). Do not copy `build/` (its `build.ninja` holds absolute paths) or `.pocket/src` and
  `.pocket/build` (CMake trees); `pocket setup` and `pocket build` make them again.
- The two Noto fonts are fetched from moving `main`-branch URLs with pinned sha256: if upstream
  changes either file, `pocket setup` fails on a machine without the cached copy.
- `build/agent-traces/` (the benchmark's raw per-task event streams, input to `trace_report.py`) is
  only on the machine that ran them; the committed record is `tests/evidence/agent-eval/*.json`.
  Copy the traces by hand if they matter.
- Single files over 20 MiB (high-poly Blender models and other large art) are not in the clone but
  on the owner's Google Drive, pinned in `tests/data.json`: install rclone, set it up once and run
  `python3 tools/scripts/data.py fetch` (AGENTS.md, Test data).
- Disk: the clone is about 143 MB; `.pocket/` about 0.9 GB with every target set up;
  `tools/pocket/target` 0.6 GB; `build/debug` 2.2 GB, `build/release` 0.85 GB, `build/ios-sim` 0.57
  GB; emsdk 1.9 GB; the iOS runtime about 8 GB; each Linux image about 2.2 GB.
- The previous machine's agent memory (Claude Code's auto-memory) does not travel; what it held that
  a developer needs is in this file.

## The daily loop

### Installing the tool

The tool does not rebuild itself. After any change under `tools/pocket/` (a `git pull` that touches
it included):

```bash
cargo build --release --manifest-path tools/pocket/Cargo.toml && rm -f .pocket/pocket && cp tools/pocket/target/release/pocket .pocket/pocket
cargo build --release --manifest-path tools/pocket/Cargo.toml && rm -f .pocket/pocket.exe && cp tools/pocket/target/release/pocket.exe .pocket/pocket.exe   # Windows
```

Remove before copying: macOS kills a signed binary overwritten in place, and on Windows a running
`pocket.exe` (an MCP server, a watch) cannot be removed at all: stop it, or move it aside first.
Compiled into the tool and so stale until this: the web page shell (`web_shell.html`,
`include_str!`), the MCP tool descriptions, the guide a new project gets (`commands.rs`), code
generation (`gen.rs`, `sdkdoc.rs`: an old tool run by `pocket build` writes the generated files back
the old way) and the `pocket.toml` schema (an old tool rejects a new field). Never reinstall while
an agent benchmark runs: the harness and every agent's MCP server run `.pocket/pocket`.

### Bundles and the release runtime

Test binaries run directly, `tools/scripts/dev/runtime.py`, every evidence script and the benchmark
harness read `build/ts/<name>.js` and `build/release/bin/pocket_runtime` as they are; they do not
bundle or build. After an SDK change, bundle everything again:
`for s in samples/*/; do ./.pocket/pocket ts "$s" >/dev/null || echo "failed $s"; done; ./.pocket/pocket ts editor`,
then `./.pocket/pocket build --config release`. A stale bundle failed a benchmark task's reference
solution; a stale release runtime failed with `__pocket.clock is not a function`. `pocket ts <dir>`
names its output after the `name` in the project's `project.toml`, not after the directory (`--out`
to choose).

### Tests

What `./.pocket/pocket test --json` runs (`tools/pocket/src/commands.rs`, `test`), in the debug
configuration: it bundles every sample and the editor, then (1) the nine Catch2 binaries in
`build/<config>/bin` (`core`, `world`, `physics`, `nav`, `assets`, `audio`, `renderer`, `ui`,
`runtime` `_tests`; the editor and input-map tests are inside `runtime_tests`); (2)
`tests/ts/*.test.ts(x)`, twelve files, one headless frame each; (3) `types` (`pocket check` over the
workspace); (4) `scenarios:<sample>` for every sample with `scenarios/`, three seeds and a 900-frame
budget (`pocket scenario` defaults to release and 1800 frames, so a scenario longer than about
fifteen seconds fails only in the suite); (5) `benches:<sample>` (a failing bench does not fail the
run); (6) `pocket_tool` (`cargo test --release`, skipped silently when cargo is not on `PATH`); (7)
`python` (`sdk/python/test_pocket_env.py`). The summary counts 24 modules. A full run takes 8 to 12
minutes on the reference machine. Read the JSON from stdout only.

- `--filter` must match a C++ module name; `--filter scenarios`, `types` or `python` answer "no test
  modules matched", and `--filter ts` matches every `_tests` module. Use `pocket scenario <sample>`,
  `pocket bench`, `pocket check` instead.
- A test binary run directly needs `POCKET_ROOT` (every `root()` requires it; without it every case
  fails at `REQUIRE( r != nullptr )`) and current bundles:
  `POCKET_ROOT=$PWD ./build/debug/bin/runtime_tests "[tag]"`, after
  `./.pocket/pocket ts samples/<name>` (and `./.pocket/pocket ts editor` for `[editor]`).
- The whole suite in the background: `tools/scripts/dev/suite.sh <name>` writes
  `build/test-reports/<name>.json` (empty until the run ends: an empty file means running, not
  failed) and `<name>.done`. In an agent harness whose background shells end with the call, start
  `POCKET_ROOT=$PWD ./.pocket/pocket test --json > build/test-reports/<name>.json` with the
  harness's own background execution instead;
  `python3 tools/scripts/dev/test_summary.py build/test-reports/<name>.json` reads either.
- Never run two suites at once (they share `build/`); look first with `ps` (BSD `pgrep -f 'a\|b'`
  matches nothing). Kill leftover runtimes before a suite: an idle one held a suite for two hours.
- While a suite or a benchmark runs, do not edit engine sources or leave draft `.cpp` files under
  `engine/*/src`: module sources are globbed and the scenario phase rebuilds the runtime.
- Never chain `git commit` after a test run piped through `tail` or `head`: the pipe hides Catch2's
  exit status (two commits went in with a failing assertion that way).
- Not covered by `pocket test`: the web configurations, Linux (`linux.sh`), iOS (`ios_evidence.py`),
  `tools/scripts/wasm_core_tests.py` (core tests under node) and the agent benchmark. After touching
  engine C++ (`engine/app/src/session.cpp` above all), build `--config wasm` too: the web
  configurations compile with `-fno-exceptions`, so a `try`/`catch` must sit inside
  `#if defined(__cpp_exceptions)` (as in `Session::command`, where native builds turn JSON type
  errors into `bad_args` and web builds abort).

The hello golden hash (`tests/evidence/hello-golden.json`) changes whenever a hashed component gains
a field or the per-tick hash changes: run
`POCKET_ROOT=$PWD ./build/debug/bin/runtime_tests "golden state hash for hello"`, put the new value
in `state_hash`, add a dated sentence to `note` (which component gained which fields), keep the
file's indentation and final newline, and say why in the commit message. `core_tests` `[repro]` pins
a hash of its own.

Tests lean on the samples:

- adding entities, layers or scenarios to a sample shifts counts asserted in `renderer_tests`,
  `runtime_tests` and the editor tests (meshes, skinned and morphed counts, the physics scene's
  entity count, the sprites sample's scenario count, tile bodies, layers);
- editor tests click hierarchy rows that must stay visible at 1024x640 (`Ramp` is the fifteenth row,
  children listed under their parents; after the 2026-10-02 restyle it sits at y 372 to 394 in a
  pane reaching 414; nothing above the rows may be taller than about 24 px); scroll the inspector
  (`ui.wheel` dy -6) before clicking lower fields;
- the hello and assets samples' scripts break after `world.clear` and clearing the sprites world
  stalls its script, so tests place their entities far away (x + 200, y = 50) instead;
- in a paused session `frame()` still ticks: use `time.scale 0` and `world.update_transforms` for
  pixel probes;
- the event log survives `env.reset` (count from `events.lastSeq()`) and body velocities persist
  (zero them after driving);
- TypeScript tests run without a project directory, so asset paths are repository-relative;
- the sprites level: plank x -6..-3, ledge x 3..5, hill x 6..9, lift at x -8.5, no solid border;
- known flake: `renderer_tests` decal normals and terrain layers write PNGs into
  `samples/playground/assets` and once failed together, passing on a rerun.

Six samples' scenes are generated: `defense`, `farm`, `fps`, `island`, `topdown` and `village` each
from `python3 tools/scripts/dev/<name>_scene.py`. Change the script and run it; never edit those
`scene.json` files by hand.

### Writing code

- Recurring C++ pitfalls: `std::format` and `fail()` strings need `{{` and `}}` for literal braces;
  nested `Json` initializer literals in tests mismatch braces easily (build them from named parts);
  a streamed voice's SDL converter must be destroyed before `SDL_QuitSubSystem(AUDIO)`; entity-typed
  component fields hold numbers, and scene JSON names entities through string fields resolved by
  name.
- The renderer embeds its WGSL in raw strings (`R"WGSL(...)WGSL"`) in
  `engine/renderer/src/renderer.cpp`; its binding budget is in `docs/design/rendering.md`, Bindings
  and limits.
- The SDK: modules import `./registry`, not `./pocket`, to register tick handlers (the other import
  is a cycle); per-bundle module state lives on the shared registry, because only the last-loaded
  bundle's dispatcher runs; host globals (the timer family, `performance`, `console`) are declared
  in `sdk/runtime/timer.ts` and `pocket.ts`. oxc only transforms, so run `./.pocket/pocket check`
  after any SDK, editor or sample TypeScript change: the workspace stays at zero type errors.

### Committing

```bash
git add <paths>                     # never .codex/; compile_commands.json is ignored
git -c user.name=bluesamoyed commit -q -F - <<'EOF'
<subject>

<body>

Co-Authored-By: <the agent's trailer>
EOF
git push -q origin master           # GitHub has answered 500 on push: retry
```

Quote the heredoc (`<<'EOF'`): an unquoted one runs backticks in the message.

## Checking work

### Pictures

The evidence scripts start the release runtime headless on a sample (`tools/scripts/dev/runtime.py`)
and capture; they need `./.pocket/pocket build --config release` and the sample's bundle first,
Pillow, and loopback access (outside an agent sandbox). GPU numbers are worthless while a benchmark
or a browser page drawing a pack runs (a 2.85 ms measurement taken beside a suite was 0.49 ms
alone).

| Evidence | Made by |
|---|---|
| `tests/evidence/rendering/` clouds, grass, height-blend, horizon, island, materials, ocean, river, ssr | `tools/scripts/dev/<name>_evidence.py` (horizon: `--before` an older runtime) |
| `tests/evidence/rendering/specular-metals.png`, `specular-aa.png` | `tools/scripts/dev/specular_evidence.py --before <a runtime built before the change>` |
| `tests/evidence/rendering/` cloth, crowd, gpu-particles (and -collide), humanoids, irradiance, irradiance-walls (`probe_visibility_evidence.py`), patterns, presets, props, ssgi, taa-samples, taa-skinned (`--before` an older runtime), terrain-lod, toon, trails, village | `tools/scripts/<name>_evidence.py` (dashes as underscores) |
| `tests/evidence/characters/ragdoll.png`, `tests/evidence/assets/voxels.png`, `tests/evidence/water/` | `ragdoll_evidence.py`, `voxel_evidence.py`, `water_evidence.py` in `tools/scripts/` |
| `tests/evidence/assets/obj-import-findings.png` | `tools/scripts/dev/objstress/` (`docs/research/2026-10-02-rendering-and-import-assessment.md`) |
| `tests/evidence/ios/` | `tools/scripts/ios_evidence.py` (`run()` and `screenshot()` for single samples) |
| `tests/evidence/web/offline-*` | `tools/scripts/web_offline.py`; the other web evidence was made by driving a page by hand and posting to `web_evidence.py` (`docs/web.md`, Evidence) |
| `tests/evidence/agent-eval/*.json` | `tools/scripts/agent_eval.py` (`docs/agent-eval.md`) |

Seventeen rendering images have no script; they were made by hand through commands, described in the
design docs where they appear: animation, defense, dungeon, farm, footprints, fps, hills-props,
particles, pbr, storm, topdown, view-texture, village-day (png and gif), village-sample,
water-rings, weather. Give the next one a script.

### Web

`AGENTS.md` rule 10: after a shader change, pack and load.
`./.pocket/pocket build --config wasm && ./.pocket/pocket pack <sample> --web` (unset
`DEVELOPER_DIR` first if it points at the Command Line Tools); start
`python3 tools/scripts/web_evidence.py` once, outside an agent sandbox (a sandboxed server is
unreachable from a browser; a second server exits at once if port 4718 is taken: `lsof -i :4718`);
open `http://127.0.0.1:4718/<sample>/?v=N` (bump N past the cache) in a WebGPU browser; after about
five seconds read the console for `Validation Error`, `WGSL` or `gpu error`. A black page means Tint
rejected the whole mesh shader module. Drive it with `window.pocket.command(...)` and
`await pocket.commandAsync('capture', {path})`. In the Claude desktop app's built-in browser pane: a
hidden pane gets no animation frames (time things with `pocket.command('step', {ticks: 600})`, not
waits), it cannot register service workers (use `web_offline.py`, headless Chrome) or take pointer
lock, and a hidden pane stalls a web lockstep peer. `input.map {}` with empty parameters clears the
action map. Wasm compile errors:
`./.pocket/pocket build --config wasm --generate-only; source ~/.pocket-tools/emsdk/emsdk_env.sh; ninja -C build/wasm -k 0`.

### iOS and Linux

`python3 tools/scripts/ios_evidence.py [--device "iPhone 18 Pro"]` packs, installs and drives
samples in the simulator; unattended, take screenshots with
`xcrun simctl io <udid> screenshot f.png`. On iOS keep each fragment stage at four storage buffers
or fewer. `tools/scripts/linux.sh` subcommands: `shell`, `raw "<cmd>"` (no build), `build`,
`test [--config release]`, `exec "<cmd>"`, or any `pocket` command; debug sanitizers need more than
20 GB for `runtime_tests`, so test with `--config release`. The x86_64 release suite took about nine
hours under QEMU and passed 23 of 24 modules (`runtime_tests` failed, a rerun was inconclusive).

## The agent benchmark

`docs/agent-eval.md` is the reference: the tasks, runners, coding agents, results, and (Running and
recording a full run, Adding a task) the exact recipe for a full run, the rules while one is going,
and how to add a task.

## Agent harness notes

- Claude Code's Bash sandbox blocks loopback connections and `pkill`: run outside it anything that
  talks to a runtime (`pocket test`, `pocket rpc`, `runtime.py`, `gpu_probe.py`, the evidence
  scripts, `agent_eval.py`, `ios_evidence.py`, `web_evidence.py`).
- The tool shell is zsh (the owner's login shell is fish): quote globs (`--include='*.cpp'`; an
  unmatched one aborts the command), use `<<'EOF'` heredocs, beware words starting with `=`. BSD
  `sed` has no `\b`; use `tools/scripts/dev/edits.py` or Python. macOS has no `timeout`:
  `perl -e 'alarm 60; exec @ARGV' ./cmd`.
- A long process started in one session (a benchmark, a suite) can outlive it and no completion
  notice arrives in the next: poll its output files.
- The iOS Simulator MCP tool needs the owner's per-device grant; `simctl` does not.
- `git checkout -- docs` once threw away uncommitted doc wrapping: look before discarding.
- On macOS the tool shell above is zsh; on Windows it is Git Bash (with PowerShell beside it). In
  Git Bash a heredoc fed to `python -` lost or doubled backslashes in Python string literals more
  than once (`\\n` came out as a line break): write such scripts to a file first, or use
  `tools/scripts/dev/edits.py`. Run Python with `PYTHONUTF8=1` (the console's code page is not
  UTF-8). A Catch2 test name with a comma is cut there: pass a wildcard (`"script.profile*"`) or the
  test's tag.
- Windows: a directory junction (`mklink /J`, how a git worktree can share the main checkout's
  `.pocket`) is followed by `git worktree remove`, which deleted the shared dependencies once
  (`pocket setup` rebuilt them in four minutes). Remove the junction itself first
  (`cmd //c rmdir <worktree>\.pocket`), then the worktree. Agents working in worktrees now copy the
  dependencies instead
  (`mkdir -p .pocket && cp -r <main>/.pocket/deps .pocket/ && cp <main>/.pocket/pocket.exe .pocket/`,
  about 1 GB; their stamps hold no paths), so no link is left for a removal to follow, and build
  with `pocket build --config release --generate-only && ninja -C build/release -j 5` when several
  build at once on this 15 GB machine.

## Environment variables

| Variable | Read by | Meaning |
|---|---|---|
| `POCKET_ROOT` | test binaries, scripts, the SDK's Python env | the workspace root |
| `POCKET_RPC_URL` | `pocket rpc`, `pocket mcp` | the control server to attach to |
| `POCKET_RUNTIME` | `pocket` (`commands.rs`) | a runtime binary to launch instead of the built one |
| `POCKET_CONFIG` | `sdk/python/pocket_env.py` | the build configuration whose runtime it starts (release) |
| `POCKET_EVAL_RUNTIME`, `POCKET_AGENT_TRACES`, `POCKET_EVAL_TARGET`, `POCKET_EVAL_DEVICE` | `agent_eval.py` | the frozen runtime, where traces go, `ios-sim` and a simulator's name |
| `POCKET_JOBS` | `pocket scenario`, `pocket bench` | runtime processes at once |
| `POCKET_THREADS` | `engine/core/src/parallel.cpp` | worker threads (`1` keeps all work on one) |
| `POCKET_CXX`, `POCKET_CC`, `POCKET_AR` | `toolchain.rs` | compilers and archiver for native builds |
| `POCKET_LLVM` | `toolchain.rs` (Windows) | an LLVM directory to take clang++ from when it is not on PATH or under `~/.pocket-tools/llvm-*` |
| `POCKET_PYTHON` | `toolchain.rs` | the Python the tool runs (font subsetting, the web packer, the Python tests) |
| `POCKET_GPU_BACKEND` | `engine/rhi/src/device.cpp` (Windows) | `vulkan` for wgpu's Vulkan backend instead of Direct3D 12 |
| `POCKET_EMSDK` (or `EMSDK`), `POCKET_XCODE`, `POCKET_LINUX_ARCH` | `pocket`, scripts | Emscripten, Xcode's developer directory, `amd64` for x86_64 Linux containers |
| `POCKET_DATA_REMOTE` | `tools/scripts/data.py` | where large test files are stored (default `pocketdata:pocket-data`; AGENTS.md, Test data) |
| `POCKET_BLENDER` | `engine/assets/src/import.cpp`, `agent_eval.py` | Blender's executable |
| `POCKET_CHROME` | `web_offline.py` | Chrome's executable |
| `POCKET_FONT` | `engine/app/src/session.cpp` | the UI font when neither `--font` nor the project names one |
| `POCKET_SAVE_DIR` | `session.cpp` | where save slots go |
| `POCKET_GPU_REPORT` | `engine/rhi/src/device.cpp` | print GPU objects a subsystem forgot to release |
| `POCKET_TOOL` | `session.cpp` | the `pocket` binary `project.apply` bundles with (set by `pocket run`) |

## Current state (2026-10-02, evening, Windows)

- **The move to Windows** (ADR 0008, Proposed): development continues on the owner's Windows 11
  laptop (RTX 5060 Laptop, Direct3D 12), from `f32da248` on. Everything below the tool builds there
  with LLVM 23's clang for the MSVC ABI; the release and sanitized debug configurations, the web
  build and Windows packs work; core, world, physics, nav, assets, audio, ui, renderer and runtime
  tests pass (the runtime ones after the fixes in `6b4b0ae8`), and so do all 18 samples' scenarios;
  hello's state hash is the macOS golden. Evidence: `tests/evidence/windows/README.md`. Not yet run
  on Windows: the agent benchmark (the coding agents and their keys are not installed here), the
  whole `pocket test` in debug (more memory than this machine's 15 GB may be needed: test in release
  first), the iOS and Linux checks (they need a Mac and Docker).
- **Direct3D 12 startup**: wgpu compiles each pipeline's HLSL through DXC in the process on every
  run, so the renderer now makes each pipeline the first time a frame draws with it (`add42838`;
  docs/design/rendering.md, Pipelines made on first use): the first frame of hello came down from
  2.05 to 1.15 s, showcase from 3.13 to 2.42 s, village 2.90 to 1.61 s, fps 3.13 to 2.05 s (0.8 s on
  Vulkan). What is left is the pipelines a scene draws with (the lit mesh pipeline about 0.4 s,
  probe capture 0.5 s); a feature first turned on mid-run pays for its pipeline then. With the 5 MB
  stack (`47b3081c`) and this, runtime_tests runs in 259 s (about 19 minutes before).
- **The whole suite on Windows** (`pocket test --config release`, 2026-10-02): every module, the 20
  scenario sets, the 12 TypeScript tests, types and python pass, after Blender 5.1's FBX importer
  was worked round (`b69d6620`); the tool's cargo tests are skipped when cargo is not on the suite's
  `PATH` (Git Bash here lacks `~/.cargo/bin`: run them with `~/.cargo/bin/cargo test` in
  `tools/pocket`). The debug (AddressSanitizer) suite has not been run whole; its renderer_tests
  pass.

- **Last benchmark**: the full 72-task run on opencode with GLM 5.3 Flash, 71 of 72 on the runtime
  frozen at `b0616f2` (`tests/evidence/agent-eval/opencode-glm-full72.json`; `slow_swarm` failed at
  the time limit and passed when run again; docs/agent-eval.md, Results). `reference.json` and
  `null.json` cover the 72 tasks. Commits after `b0616f2` (crouch, nested spawn children, shove,
  mantle and later) are not measured by it.
- **Fixed after the move** (Windows, 2026-10-02): the failure the pause left, `scenarios:arena`. Red
  runs the ball at Blue, who stands in its way, and no goal comes within 5 s; Blue is pushed from z
  -6 to -9.9. Cause, from 0db76ac5 (character shoves): a dynamic body that runs into a character now
  has its speed toward it braked, so the ball stops against Blue instead of glancing off; and the
  skin distance a character gives way by each tick (`kSkin * 0.5` in `move_characters`, step 2) is
  not scaled by the mass share, so a light ball pressed on by Red walks Blue along whatever Blue's
  mass (shown with `Character.mass` at 1e9). The skin is now given way to by the same share; the old
  scenario had passed only because the ball barged Blue into the goal (a goal resets the players),
  which the shove commit meant to end, so the scenario now has Blue stand aside for the goal and a
  second one checks that Blue, in the ball's way, stays put (`tools/scripts/dev/arena_probe.py`
  prints the three places tick by tick).
- **OBJ import** (the assessment's Plan item 1, `2b94b7a6` and a review's fixes after it): crease
  normals, ear clipping, a reader ten times faster with a cache in `.imported/` (which packs now
  carry), double precision, unit and up-axis settings; a 5M-triangle scan spawns in 2.2 s (34.5
  before) and is read from the cache in 0.8 s. Not done: reading on a worker thread (a mesh's
  collider and bounds must land on a known tick).
- **Visible quality** (the assessment's Plan item 2, 2026-10-02 evening): the ocean's horizon dashes
  and the sky's below-horizon band are fixed (`1c39bbfd`), skinned and morphed meshes write motion
  vectors for TAA and motion blur (`1b048f43`; TAA left off by default in the samples, with the
  blockers in docs/design/rendering.md, TAA in the samples), and specular anti-aliasing,
  multiple-scattering GGX and specular occlusion are in (`38a2f0fc`). Each was reviewed by a second
  agent; the paused session left these findings open, to do next:
  - `38a2f0fc`: the far normal-mapped floor sparkles a little more than before (specks 0.64 to
    0.67%, flicker 4.0 to 5.9 levels; the lobe width varies pixel to pixel: take the derivative
    kernel from the smooth geometric normal, or coarse derivatives); Toksvig rarely acts (a 1% dead
    zone for 8-bit maps, and not gated on minification: store the factor at upload from
    float-averaged mips, scale by `saturate(lod)`); the lights' multiple-scattering term uses
    Karis's fit for E, so a white metal under a light reflects up to 1.18 of what it receives at
    roughness 0.2 to 0.5 (fit E to the engine's own direct BRDF; keep A + B for image-based light);
    unlit pixels now pay for normal maps, decals and weathering; `render.ssr` max_roughness now
    compares the filtered roughness (say so in its help); the anisotropic branch widens masking too.
  - `1b048f43`: tests for a texture camera's main view keeping object motion and for a part leaving
    the view or switching LOD; a `static_assert` on `Mat4`'s size beside the joint copies;
    `read_motion`'s comment; the TAA evidence scripts' `render: True` (wants `"each"`); split-screen
    views after the first get no motion of their own (the doc says otherwise).
  - Not yet verified after merging `38a2f0fc` onto the other two (the owner deferred it): the web
    build loaded in a browser (the specular agent compiled all 16 WGSL modules under Chromium's Tint
    on its branch, and the merge only added the water's horizon clamp) and runtime_tests
    (renderer_tests pass: 53 cases, 5048 assertions).
- **Next work**, in order: the open findings above; the rest of the assessment's Plan item 2
  (reversed-Z with a 32-bit float depth, GTAO-style AO with bent normals, probe blending and larger
  probe faces); the editor's look is restyled (`78926036`, docs/editor.md, Look) and waits on the
  owner's taste; Grass on `samples/hills` (deferred because benchmark tasks copy that sample, and
  the `meadow` task asks an agent to add it: add it only between runs, with that task changed).
- **Waiting on the owner**: their own Google OAuth client for the large-file store (AGENTS.md, Test
  data; nothing is stored there until it exists); accepting the Android SDK licence (Android is
  blocked until then); accepting ADR 0008's technical choices (clang++ for the MSVC ABI rather than
  clang-cl, Bun's JavaScriptCore in `pocket_jsc.dll` rather than V8, the dynamic C runtime); how
  Windows packs carry the shader compiler and the Visual C++ runtime (the Windows SDK's
  `dxcompiler.dll` is copied beside development builds; a pack for players needs a redistributable
  DXC, from Microsoft's DirectXShaderCompiler releases, or FXC's 20-second start, and either the
  Visual C++ runtime DLLs beside the game or the redistributable installed).
