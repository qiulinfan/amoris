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
