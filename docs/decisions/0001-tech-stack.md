# ADR 0001: Technology stack

- Status: Proposed (2026-09-17). Becomes Accepted when the human collaborator confirms; the open
  questions at the end get their own records.
- Deciders: repository owner (human), Claude (AI collaborator)

## Context

Pocket3D restarts PocketEngine from zero. The previous engine (C++17, Lua 5.4 with LuaBridge, SDL2
and SDL_Renderer, Box2D, Dear ImGui, CMake) and the autonomous reference branch archived as
`aipocket` (OpenGL 4.1 and Metal backends, a self-written 3D rigid body engine, an editor 3D mode)
showed what works and where the debt is: an untyped scripting language with hand-written bindings, a
global engine singleton, a deprecated graphics API, tooling bolted around CMake, and editor
automation hooks added late for tests.

The engine is built in 2026 with AI coding agents as first-class users. The stack is chosen to avoid
debt that is already foreseeable and to make the engine verifiable by agents without a human at the
screen.

## Decision

| Layer | Choice | Pinned version (verified 2026-09-17) | Pin policy |
|---|---|---|---|
| Hosting language | C++26 | Apple clang 21 (macOS), LLVM clang 21 (Linux), clang-cl 21 (Windows) | One compiler family. Feature allowlist below; a probe target in CI fails when an allowed feature stops compiling. |
| Scripting language | TypeScript | 7.0.x (the native compiler generation, `tsgo`) | Follow TypeScript minor releases once the SDK's own type tests pass. |
| Script engine | V8 with JIT | 13.6 line, the V8 shipped in Node 24 LTS (13.6.233 on the owner's machine) | Move to the V8 of the newest Node LTS once a year. Built as a monolithic static library with the official GN toolchain; prebuilt artifacts fetched by `pocket`. |
| TypeScript transform and bundling | oxc transformer and rolldown bundler, in-process in `pocket` | oxc 1.83, rolldown 1.2 | Rust crates; swc is the fallback if oxc blocks. |
| Type checking and IDE | `tsgo` for `pocket check` and the language server | 7.0.x | Fetched by `pocket setup`. Node is optional for developers and never required by the engine or the build. |
| Build tool | `pocket`, self-written in Rust | Rust 1.98.1 stable via `rust-toolchain.toml` | Executor: n2 (Ninja-compatible, embedded as a library). See `docs/build-system.md`. |
| Offline tooling | Python | 3.14 via uv, ruff for lint | Optional dependency, never on the build path. |
| Platform layer | SDL3 | 3.4.16 | Window, input, audio, main callbacks. SDL2 is maintenance-only and is not used. |
| Graphics API | Explicit API, no OpenGL | Open (ADR 0002): SDL3 GPU vs wgpu-native 29; shaders in Slang 2026.x or WGSL | Decided at milestone 2. |
| World model | Data-oriented ECS | Candidate: flecs 4.1.6 | Decided at milestone 1 (ADR 0003). |
| Editor and runtime UI | "Pocket UI": an own retained-mode UI system with the web model, driven by TypeScript and rendered by the engine (ADR 0004) | Yoga (layout), FreeType + HarfBuzz (text) | One UI system for the editor and for games. No Dear ImGui, no Qt, no webview. |
| C++ tests | Catch2 v3 | 3.16 | Driven by `pocket test`, which emits JSON. |
| TypeScript tests | In-engine runner executed by `pocket test` | | Scripts are tested inside V8 in the engine, not in Node. |
| CI | GitHub Actions | macos-15 (arm64), ubuntu-24.04, windows-2025 (clang-cl) | Every push builds all three; V8 artifacts are cached, never rebuilt in CI. |
| Formatting and lint | clang-format and clang-tidy, rustfmt and clippy, Biome (TypeScript), ruff (Python) | | Enforced by `pocket fmt` and `pocket check`, and by CI. |
| License | Open | | MIT or Apache-2.0 recommended. |

### C++ feature policy

The standard flag is C++26. Code uses C++20 as its base, the well-supported parts of C++23 listed
here, and a short list of C++26 sugar. Everything else needs a decision record.

Allowed C++23: `std::expected` (the error-handling convention of the engine), `std::print` and
`std::format`, deducing this, `std::to_underlying`, `std::unreachable`, `[[assume]]`, `auto(x)`,
`if consteval`, `std::string::contains`, `std::ranges::to`, `std::move_only_function`,
`std::mdspan`, multidimensional `operator[]`.

Allowed C++26 (verified on Apple clang 21 on 2026-09-17): pack indexing, placeholder `_`,
`= delete("reason")`, `static_assert` with a message expression, C++26 constexpr extensions,
`#embed` (with a fallback path for compilers that lack it).

Not used until a later decision: modules and `import std` (Apple ships no libc++ std module),
coroutine-based `std::generator` (absent in libc++), reflection (P2996), contracts (P2900),
`std::execution` (use stdexec or a job system meanwhile), `std::simd`, `std::linalg`,
`std::inplace_vector`, `std::function_ref`. Missing library pieces are polyfilled behind a
`pocket::` alias so the switch to `std::` is one line.

Compiler flags from day one: `-Wall -Wextra -Werror`; ASan and UBSan jobs in CI.

### TypeScript configuration

`target: ES2024`, `module: ESNext` (ES modules loaded by the engine's own resolver), `strict: true`,
`isolatedModules: true`, `verbatimModuleSyntax: true`, no decorators (components are declared with
explicit `defineComponent` calls, which keeps code plain for agents and tooling). Engine API
declarations (`.d.ts`) and their documentation are generated from the engine's component metadata;
hand-written declarations are forbidden.

### V8 embedding constraints

`v8_enable_sandbox=false` so that typed-array views can be backed by engine-owned memory (zero-copy
component access); ICU disabled (`Intl` is not needed by a game runtime); the platform libc++ is
used instead of V8's bundled one so the engine and V8 share a standard library; the startup snapshot
is embedded, with a custom snapshot that pre-binds the engine API.

## Consequences

- Four languages, each with one job. The boundaries in `AGENTS.md` are enforced in review.
- Building V8 from source once per platform is the accepted upfront cost; day-to-day builds fetch
  prebuilt artifacts.
- The build tool is the first deliverable and the only bootstrap dependency; milestone 0 keeps it
  deliberately small.
- Component metadata declared once in C++ drives serialization, inspector UI, script bindings and
  `.d.ts`. When compilers ship reflection, only that module changes.

## Rejected alternatives

- Lua 5.4 and Luau: smallest embed and the incumbent, but untyped (Luau's gradual types aside),
  weaker agent proficiency, and a rewrite was wanted.
- Python as the gameplay language: best agent proficiency, but interpreter speed, the GIL, packaging
  CPython per platform and awkward hot reload make it wrong for per-frame code. Kept for tooling.
- QuickJS-NG: excellent embed, no JIT. The owner wants JIT performance from day one.
- JavaScriptCore, SpiderMonkey, Hermes: non-uniform across platforms, or no JIT.
- CMake for our own code: an imperative macro language, poor fit for a graph that mixes codegen,
  TypeScript bundling, shaders and assets; agents edit it badly.
- Bazel and Buck2: hermetic and remote-cache ready, but heavyweight for a small team and a poor fit
  for game-asset pipelines.
- OpenGL: deprecated on Apple platforms and not a 2026 starting point.
- Dear ImGui, Qt and webview-based editor UI: see ADR 0004.

## Open questions

1. Graphics API and shader language (ADR 0002, milestone 2). Input from `docs/positioning.md`: web
   export makes a WebGPU-shaped RHI (wgpu-native or Dawn on desktop) the leading option, and the
   core must stay portable to wasm32.
2. World model: flecs or an own ECS (ADR 0003, milestone 1).
3. License.
4. UI system details: vector renderer and accessibility (tracked in ADR 0004).
