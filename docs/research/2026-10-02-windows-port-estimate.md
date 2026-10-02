# A Windows port, estimated (2026-10-02)

What developing the engine natively on Windows would take, and the shortcut through WSL2. Five
read-only audits (the build tool, the engine's C++, the script host, the dependencies and scripts,
WSL2) at `60b757ee` to `edbfe1b1`, combined into one phased plan and then reviewed by a skeptical
agent that re-checked about forty of the cited sites and the external artifacts. Nothing has been
built on Windows; every number is an estimate in new or changed lines and focused agent-days (an
agent working on the Windows machine at this repository's pace), excluding wall-clock build,
download and benchmark time. The raw reports are in `2026-10-02-windows-port-sources.json` beside
this file.

## How complex the platform layer is

Not very: wide but shallow.

- The engine reaches the OS through two libraries (SDL3 for windows, input and audio, linked only by
  `pocket_platform`, `pocket_audio` and the runtime's entry point; wgpu-native for the GPU) and one
  script interface (`ScriptHost`, 14 methods; the native backend `engine/script/src/jsc_host.cpp` is
  343 lines). Of 84,446 lines of engine and test C++, about 160 to 180 call POSIX, Apple or Linux
  APIs, in four files (`engine/app/src/session.cpp`, `runtime.cpp`, `net.cpp`,
  `engine/assets/src/import.cpp`).
- What is tangled: there is no OS-services layer (`posix_spawn` is written out twice, in
  `session.cpp` and `import.cpp`; BSD sockets inline in `net.cpp`); `#ifndef __EMSCRIPTEN__` stands
  for "POSIX"; the window code's `#else` assumes X11 or Wayland
  (`engine/platform/src/platform.cpp`); the seven existing `_WIN32` guards only switch features off
  and have never been compiled.
- The build tool (6,437 lines of Rust) has no `cfg(windows)`, but its dependency tables already
  accept `windows-x86_64` and `windows` host keys (`tools/pocket/src/manifest.rs`). Its Unix
  assumptions are many and small: the archive rule needs a shell (`rm -f $out && $ar rcs`,
  `ninja.rs`), `which()` ignores `.exe`, `python3` is hard-coded at four sites, `canonicalize`
  (sixteen sites) yields `\\?\` paths, the diagnostics parser splits on `:` (breaks on `C:\`), the
  pack launcher is a shell script, and the debug configuration forces ASan and UBSan on every host.
- The previous ports were small (Linux 224 lines, `49fd3360`, plus a 74-line teardown fix found by
  its first real run; the iOS Simulator 629 lines, `4e71010b`), but both were POSIX siblings reusing
  `posix_spawn`, BSD sockets and a JavaScriptCore that came with the system. Windows changes the
  ABI, the C runtime, the script engine's source and the process and socket APIs at once.

## Native Windows, by phase

| Phase | Contents | Lines | Agent-days | Done when |
|---|---|---|---|---|
| 0. Decide on the Mac | a spike linking Bun's JavaScriptCore build on macOS against the test suite; an ADR amendment (clang++ targeting MSVC with lld, not clang-cl; JavaScriptCore from Bun or V8; the dynamic C runtime with JavaScriptCore in a DLL; Vulkan first); `.gitattributes` with `eol=lf` (without it the generated files are rewritten on every build) | 105-235 | 0.85-1.9 | the spike passes and the owner accepts the decisions |
| 1. The tool and libraries build | toolchain discovery (`.exe`, LLVM via PATH or vswhere, `llvm-lib`, `-dumpmachine` must say `windows-msvc`), a Windows branch of the Ninja generator (archive, link, `.exe` and `.lib`, defines `NOMINMAX`, `WIN32_LEAN_AND_MEAN`, `_CRT_SECURE_NO_WARNINGS`, quoting), `dunce` paths, the diagnostics parser, CMake dependency builds for MSVC, per-host configurations | 470-1,110 | 3.7-11.2 | `pocket doctor` green, `pocket setup` builds every dependency, the six test modules below `pocket_app` pass |
| 2. Scripts and the headless runtime | Winsock in `net.cpp`; JavaScriptCore from Bun's release lanes (a `pocket_jsc.dll` with an export list, which also settles Bun's static C runtime against wgpu's dynamic one and is the relinkable form the LGPL asks for), its mimalloc, and the ICU decompression hook Bun's Windows build expects from the embedder | 290-700 | 3-7.4 | `pocket run hello` headless reports JavaScriptCore; runtime, audio and renderer tests pass in release |
| 3. Window, input, rendering | the HWND surface for wgpu (`platform.cpp`, `engine/rhi/src/device.cpp`), precise sleeps (`SDL_DelayPrecise`), the first windowed run on a real driver (DPI, IME, present modes) | 35-145 | 0.85-2.5 | the editor and windowed runs work on Vulkan |
| 4. Edit loop, full suite, pack | one process API in `pocket_core` (`CreateProcessW`) replacing both `posix_spawn` copies (so `project.apply`, which agents use, and Blender import work), the first full run of the 405 test cases in debug and release, `pocket pack` for Windows (exe, launcher, runtime DLLs, the LGPL notice), a macOS-Windows lockstep check | 255-570 | 2.9-7 | `pocket test` passes, a pack runs on a clean Windows |
| 5. The agent benchmark | the runners' process-tree kill (Job Objects instead of `killpg`), `.exe` paths, prompts through stdin, the harness's paths and encodings | 75-210 | 1.25-3 | a reference and an opencode run finish on Windows |

Total: about 1,230 to 2,970 lines and 12.5 to 33 agent-days; the likely outcome about 1,700 to 2,000
lines and 16 to 20 days. The reviewer judged this somewhat optimistic and put the range at **15 to
41 agent-days**, for these reasons:

- the macOS spike cannot prove the Windows artifact: Bun's Windows build differs exactly in the
  risky parts (clang-cl, the static C runtime, a bundled ICU whose data is zstd-compressed behind a
  hook the embedder must define, without which `Intl`, `localeCompare` and `toLocaleString` break),
  its CI tests no Windows lane and builds no C-API test program, so a first-day Windows link of the
  C API we use comes before the DLL work;
- debug sanitizers under the MSVC ABI (ASan with non-ASan dependencies, a static-CRT JavaScriptCore,
  no arm64 ASan runtime in LLVM, more than 20 GB for `runtime_tests` on Linux) and the first full
  test run (x86-64 has never run on real hardware; under QEMU `runtime_tests` failed, undiagnosed)
  are under-budgeted;
- omitted: the web build from a Windows host (emsdk's `.bat` wrappers, `HOME` unset, cmd's
  8,191-character limit), Windows file locking (a running runtime or loaded DLL cannot be
  overwritten, so relinks fail while the MCP server keeps one alive), a GUI-subsystem entry point
  for packed games (else a console window opens), the benchmark scripts' 50 `open()` calls without
  an encoding (code page 936 on a Chinese Windows), Unicode paths (the UTF-8 code page manifest
  should ship early), and script debugging (Safari's inspector does not exist on Windows).

The script engine is the swing factor. Bun's JavaScriptCore runs in production on Windows x64 and
arm64 (`oven-sh/WebKit` release `autobuild-4fde1587`, 2026-10-01: 385 MB and 366 MB archives, LGPL-2
or later, JIT, DFG and FTL on). If it does not work through the C API, V8 (ADR 0005's plan) replaces
it at roughly +380 to 620 lines and +2.5 to 6 days, plus multi-hour V8 builds: the prebuilt
monolithic V8 libraries are stale (kuoruan/libv8's last release is 2025-01) and rusty_v8's use
Chromium's libc++.

## WSL2 instead

The existing Linux build in WSL2 with Ubuntu 26.04 (24.04 lacks clang 21 and libstdc++ 15): about 50
to 580 lines and 2 to 6.75 agent-days, nearly all verification (a host recipe, the first run on real
x86-64, explicit GPU backend selection, the first windowed run through WSLg, the benchmark and the
web build from WSL). Clone into the Linux filesystem, not `/mnt/c`; raise the VM's memory in
`.wslconfig` for debug with ASan, or test in release.

- It gives: the tool, debug and release builds, `pocket test`, headless runs and captures, the
  control server and `pocket mcp`, watch mode, the agent benchmark (opencode itself recommends WSL
  on Windows), the web build opened in a Windows browser, and the editor through WSLg on CPU
  rendering (lavapipe).
- It does not give: a Windows `.exe` for players (a pack is a Linux pack; Windows players get the
  web build), representative GPU or D3D12 performance, cursor lock in WSLg (microsoft/wslg#376), or
  any coverage of Windows-only code.
- To settle on the first run: `device.cpp` enables every wgpu backend, so with Mesa's d3d12 driver
  the engine may pick GL over lavapipe and break a renderer test's backend assertion;
  `GALLIUM_DRIVER=llvmpipe` is the zero-code workaround.
- The backend selection and the first real x86-64 run (about 20 to 310 lines, 0.75 to 3.25 days) are
  needed by a native port anyway.

## What only the owner can do

- A Windows machine: x64, Windows 10 1903 or later or Windows 11, a GPU with a current Vulkan and
  D3D12 driver (a Windows-on-Arm VM on the Mac builds and runs headless but likely has no hardware
  GPU).
- Accept Microsoft's licence for Visual Studio Build Tools or Community (the MSVC STL, the C
  runtime, the Windows SDK; also needed by Rust's MSVC toolchain; Community is free only within its
  eligibility terms), and install with administrator rights LLVM, CMake, Ninja, Python, Git for
  Windows and rustup; enable long paths or check out to a short ASCII path; add a Defender exclusion
  for the checkout (real-time scanning of the JavaScriptCore archive, thousands of objects and ASan
  executables slows every build).
- Approve the decisions that override accepted documents: clang++ targeting MSVC instead of clang-cl
  (`docs/build-system.md`), Bun's JavaScriptCore instead of V8 for Windows (ADR 0005), the dynamic C
  runtime with JavaScriptCore in a DLL; accept the LGPL obligations for shipping it (a DLL, a
  notice, relink instructions); decide whether packs ship the VC++ runtime DLLs or require the
  redistributable.
- The benchmark's model accounts and keys on that machine; answer the firewall prompt the first time
  `--net-host` binds 0.0.0.0.
- For WSL2 instead: enable WSL, `wsl --install Ubuntu-26.04`, the Linux user's sudo password for
  apt.

## Recommendation

To develop on a Windows PC soon, use WSL2 (a few agent-days, no Microsoft licence). Port natively
when a Windows `.exe` for players, Windows GPU performance or Windows-only bugs matter, in the order
above, starting with a Windows-side link of Bun's JavaScriptCore through our C API before anything
else is built on it; with the reviewer's corrections, about three to eight weeks of agent work, and
periodic Windows runs afterwards, since nothing else would catch macOS-side changes breaking it.
