# Windows evidence

The native Windows port (docs/decisions/0008-windows.md). Each entry says how it was produced.

## Machine

AMD Ryzen 9 270 (8 cores, 16 threads), 15.3 GB, NVIDIA GeForce RTX 5060 Laptop GPU (driver 617.14)
and AMD Radeon 780M, Windows 11 Pro 10.0.26200 (en-US, code page 1252). LLVM 23.1.2
(`clang+llvm-23.1.2-x86_64-pc-windows-msvc`, unpacked under `~/.pocket-tools/llvm-23.1.2`), Visual
Studio Community 2026 18.4 (MSVC 14.50.35717, Windows SDK 10.0.26100), Rust 1.98.1 (MSVC target),
CMake 3.29.2 and Ninja 1.12.0 (Strawberry Perl's), Python 3.14.3.

## 2026-10-02: the first build

- `./.pocket/pocket setup --json`: 11 dependencies ready, among them SDL3, FreeType, HarfBuzz,
  libwebp and Draco built by CMake with clang for the MSVC ABI and the dynamic C runtime, and
  `pocket_jsc.dll` (46 MB) linked from Bun's WebKit build `autobuild-2e2aa229` (the one Bun v1.4.2
  ships) with mimalloc `6a64e1ba` and `third_party/javascriptcore`. About 5 minutes.
- `./.pocket/pocket build --config release`: every module and the ten executables. The first full
  compile took about 3 minutes on 16 threads.
- `POCKET_ROOT=$PWD ./build/release/bin/<module>_tests.exe`: core (10 cases, 845,734 assertions,
  `[repro]`'s pinned hash included), world (13), physics (44), nav (13) and assets (35) pass.
- `./build/release/bin/pocket_runtime.exe --project samples/hello --bundle build/ts/hello.js --headless --frames 120 --json`:
  `state_hash` 28c5a84fae20c60d, the value in `tests/evidence/hello-golden.json` written on macOS
  arm64. The engine's simulation computes the same bits on x86-64 with the MSVC standard library as
  on Apple silicon with libc++, the first run of the engine on x86-64 hardware. JavaScriptCore
  reports `jit: true`.
- `cargo test --release` in `tools/pocket`: 11 tests pass, two of them new (diagnostics with a drive
  letter, linker errors as diagnostics).

## 2026-10-02: JavaScriptCore through its C API, before the engine was built on it

Small programs against `pocket_jsc.dll` (built from scratch-directory copies of the files now in
`third_party/javascriptcore`, compiled with `clang++ -fms-runtime-lib=dll`, the engine's runtime)
showed, in the release and the debug (assertion-enabled) builds of Bun's libraries:

- native functions through `JSObjectMakeFunctionWithCallback`, a no-copy `Float32Array` written by
  script and read by C++, a promise's reaction drained when `JSEvaluateScript` returns, a thrown
  `TypeError` with its message, a JSON round trip with CJK text;
- a 20-million-iteration object-allocating loop in 24 to 35 ms (the JIT's tiers on);
- `Intl.NumberFormat` (en-US, de-DE, zh-CN), `Intl.DisplayNames` in French and Chinese
  ("États-Unis", "美国") and dates with their zone names, once the ICU hook decompresses the items
  Bun's build stores as zstd frames (without it, number formatting throws "Failed to format a
  number" and display names fall back to codes);
- creating and releasing a context group with a typed array in it: the debug build asserted
  `container.vm().currentThreadIsHoldingAPILock()` inside `JSObjectMakeTypedArray`, and the release
  build crashed (0xC0000409) while releasing the group. Bun's fork removes the API lock from 22 C
  API functions that upstream WebKit locks (found by comparing each function of
  `Source/JavaScriptCore/API/*.cpp` at `a0ec3b71` with WebKit's `main`); with `jsc_locks.cpp`'s
  wrappers both builds pass.

## 2026-10-02: Direct3D 12, the process API, the test modules

- `pocket_runtime.exe --project samples/hello --bundle build/ts/hello.js --headless --frames 120 --json --capture hello-d3d12.png`:
  `gpu.backend` d3d12, `gpu.shader_compiler` dxc (the Windows SDK's dxcompiler.dll 1.8.2502 beside
  the executable), state hash 28c5a84fae20c60d (the golden); the picture shows the ball, its shadow
  and the ground as on macOS.
- Startup, `--frames 1`, release: Vulkan 0.77 s, Direct3D 12 with FXC about 21 s, with DXC 3.4 s,
  and 2.6 s once the renderer makes its scene and pass pipelines at once (`renderer.cpp`, init).
  Timing every pipeline (a temporary wrapper, not committed) put 57 pipelines at 2.8 s one after
  another on Direct3D 12 against 0.12 s on Vulkan, where NVIDIA's driver keeps compiled shaders
  between runs; the largest are the lit fragment shader's variants (mesh 376 ms, mesh.cut 350,
  probe.mesh 364, oit 319) and water (229). Made at once, those few grow about threefold each: wgpu
  compiles them through DXC one at a time.
- Release test modules on Direct3D 12: audio (15 cases), ui (30) and renderer (50, pixel probes
  included) pass; runtime (195 cases) passed 186 with 2 skipped (no Blender) and 7 failed, fixed
  since: three named bundle lines wrongly (JavaScriptCore writes a Windows path's drive letter in
  lower case, and Rust's canonicalize gave `//?/C:` paths), one wrote its fixture as CRLF, one
  expected Apple's "Can't find variable" where Bun's JavaScriptCore says "is not defined", one
  expected a 0.5 ms GPU target to be missed (the RTX 5060 draws that window in 0.05 ms), and two
  wanted scenario bundles `pocket test` makes and a bare run does not.
- Debug (AddressSanitizer and UBSan with the dynamic C runtime, the C++ library's container
  annotations off): core, world, physics and nav pass; hello's state hash is the golden with no
  sanitizer report.
- `pocket run hello --config release -- --headless --serve 4711 --paused --json`, then
  `pocket rpc project.apply '{"ticks": 10}'`: bundled in 78 ms, type-checked in 208 ms (tsc.exe),
  reloaded and stepped, through `pocket::process::run` (CreateProcessW).

## 2026-10-02: windows, the editor and packs

- The display here is at 200% (`AppliedDPI` 192). SDL counts pixels on Windows (its pixel density is
  1 there), so the platform layer now treats a point as the display's scale in pixels, as macOS does
  on a Retina display: `pocket_runtime --hidden --frames 30` opens 960x540 points as 1920x1080
  pixels and presents all 30 frames through the HWND's swap chain.
- `editor-d3d12.png`:
  `pocket editor physics --config release -- --hidden --frames 90 --capture ...`, the editor drawn
  on Direct3D 12 at the display's scale.
- `pocket pack hello --config release`: `dist/hello/` with `bin/pocket_runtime.exe`,
  `pocket_jsc.dll`, `dxcompiler.dll` and `hello.cmd`; `hello.cmd --headless --frames 120 --json`
  (from PowerShell) reports d3d12 with dxc and the golden state hash.
- `pocket mcp` answers `initialize` and lists its 32 tools; `pocket check` finds no type errors;
  `pocket scenario sprites --seeds 3` passes its scenarios.

## 2026-10-02: every scenario, the arena fix, the web build from Windows

- `pocket scenario <sample>` for the 18 samples with scenarios (5 seeds, release, Direct3D 12): 405
  of 405 runs pass, arena's two among them after the fix below; `runtime_tests "[net]"`: 6 cases
  pass.
- arena: `tools/scripts/dev/arena_probe.py` printed the ball, Red and Blue every 15 ticks. Before
  the fix Blue was walked back 0.3 units every 15 ticks (to z -10.5 at tick 330); with the shove
  commit undone, the ball barged Blue into the goal (a goal resets the players, which is how the old
  scenario passed); with the skin given way to by the mass share, Blue moves 0.18 in 105 ticks while
  the ball is pressed into it, and the scenario
  `a ball Red drives into Blue does not walk Blue back` checks it.
- Emscripten 6.0.9 (`emsdk install 6.0.9`, with `EMSDK_PYTHON` or `python emsdk.py`, since the
  `emsdk` script calls the Store's python3 alias): `pocket setup --target wasm` builds libwebp,
  Draco, FreeType and HarfBuzz; `pocket build --config wasm` makes a 14 MB `pocket_runtime.wasm`;
  `pocket pack hello --web`, served by `tools/scripts/web_evidence.py`, runs in the Claude app's
  Chrome 152 pane on WebGPU: after `step {ticks: 120}` its state is hello's golden state (hue 0.1,
  ball.y 0.3913, two bounces), the ball and its shadow are drawn, and the console has no WGSL or GPU
  error.
