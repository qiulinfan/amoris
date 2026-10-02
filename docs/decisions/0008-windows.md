# ADR 0008: Native Windows with Direct3D 12

- Status: Proposed (2026-10-02). The direction is the owner's: on 2026-10-02 they asked for
  development to continue on Windows with DirectX 12. The technical choices below wait for the owner
  to accept them.
- Deciders: repository owner (human), Claude (AI collaborator)

## Context

Until 2026-10-02 the engine was developed on macOS (Apple silicon), with Linux in containers, the
iOS Simulator and browsers as further targets. `docs/research/2026-10-02-windows-port-estimate.md`
costed a native Windows port and recommended WSL2 first; the owner has since moved development to a
Windows PC (`tests/evidence/windows/README.md`, Machine) and asked for DirectX 12. Three earlier
documents assumed otherwise: ADR 0001 names clang-cl for Windows, ADR 0005 names V8 as the Windows
script engine, and the estimate planned Vulkan first.

## Decision

1. **Direct3D 12 through wgpu-native.** The renderer stays on the WebGPU C API (one renderer for
   macOS, Linux, iOS, browsers and Windows, WGSL shaders compiled by naga); on Windows the instance
   is created for wgpu's D3D12 backend only, with an HWND surface. A hand-written D3D12 renderer is
   not planned: it would split the renderer and drop the web build's parity. D3D12-only features
   (DXR, mesh shaders, DirectStorage) wait until wgpu exposes them or until an ADR adds an escape
   hatch through `wgpuDeviceGetNative*`, as MetalFX is planned on Apple.
2. **Compiler: LLVM's clang++ (the GNU-style driver) for `x86_64-pc-windows-msvc`, with lld and
   llvm-lib**, against the Visual Studio C++ library and the Windows SDK, which clang finds itself.
   One set of flags serves every host; clang-cl would need its own. LLVM comes as the release
   archive unpacked under `~/.pocket-tools/llvm-<version>` (no installer, no administrator rights),
   or from PATH, or `POCKET_CXX`/`POCKET_LLVM`.
3. **The dynamic release C runtime (`/MD`) for the engine and every dependency, in every
   configuration**, because wgpu-native's Rust library is built for it. Debug builds differ from
   release only in flags (`-O0`, AddressSanitizer and UBSan, which work with it), never in the C
   runtime's debug variant.
4. **JavaScriptCore from Bun's WebKit build, sealed in `pocket_jsc.dll`.** Bun publishes static
   JavaScriptCore libraries for Windows (oven-sh/WebKit release lanes, LGPL-2.1+), compiled with
   clang-cl for the static C runtime and refusing to link with anything else (`/FAILIFMISMATCH` on
   the runtime and on the standard library's sanitizer annotations). `pocket setup` links them once
   into a DLL that exports the C API (`third_party/javascriptcore/pocket_jsc.def`) with what they
   need besides: mimalloc at the commit Bun pairs with that WebKit, a hook that decompresses the ICU
   data items Bun's build stores as zstd frames, and wrappers that take the API lock in the 22 C API
   functions Bun's build leaves it out of. The engine's `jsc_host.cpp` is unchanged; macOS, Linux
   and Windows all run scripts on JavaScriptCore through the same C API. The DLL is also the form
   the LGPL asks for: a game ships it beside the executable, replaceable. The pin is the WebKit
   commit a released Bun ships (v1.4.2: `2e2aa229`), not the newest autobuild.

## Consequences

- One script engine family on every native platform, so
  `docs/decisions/0005-script-engine-backend.md`'s plan of V8 for Windows is not needed; V8 stays
  the fallback if Bun's builds stop working through the C API (the evidence's checks are what would
  show it).
- Bun's Windows lanes are not tested by Bun through the C API; `tests/evidence/windows/README.md`
  records what was checked, and `runtime_tests` exercises it on every suite run.
- No inspector for scripts on Windows (Safari's does not exist there); errors keep mapping to
  TypeScript lines.
- Packs for Windows ship `pocket_jsc.dll` (about 46 MB) and need the Visual C++ runtime; whether a
  pack carries its DLLs or asks for the redistributable is still the owner's to decide.
- The estimate's further Windows work stands: one process API instead of the two `posix_spawn`
  copies, the parent watch for `--exit-with-parent`, a UTF-8 code page manifest, Windows packs, and
  the benchmark harness.

## Rejected alternatives

- Vulkan on Windows: works through the same wgpu build, but the owner asked for DirectX 12, which is
  also what Windows drivers and tools are best at; Vulkan stays reachable for comparison.
- clang-cl: the same compiler with MSVC-style flags; the build tool would need a second flag
  dialect.
- Linking JavaScriptCore statically into every executable (as bun.exe does): forces the static C
  runtime on the engine and on wgpu-native (rebuilt from source), rules out AddressSanitizer's
  dynamic runtime, links 700 MB of objects into each of ten executables, and is the form the LGPL
  makes awkward to ship.
- V8: no current prebuilt for the MSVC C++ library with the sandbox off except single-maintainer
  builds; a second script backend to maintain for good.
- WSL2: gives development on this PC but no Windows executable, no D3D12 and no Windows-only
  coverage.
