# Build system and toolchain

This document is the overview the project works from. It explains the vocabulary, surveys the tools we could have used, dissects UnrealBuildTool, and then specifies `pocket`, the Rust build tool that replaces all of them for this repository.

## 1. Vocabulary

**Toolchain.** Everything that turns source into a runnable program for one target platform: the compiler driver (`clang++`), preprocessor, assembler, linker (`ld64`, `lld`, `link.exe`), the C++ standard library (libc++ everywhere in our setup), the platform SDK or sysroot (macOS SDK, Windows SDK, glibc headers), plus debuggers, sanitizers and profilers. A toolchain is identified by its versions; a build is reproducible only when they are pinned.

**Build system.** A program that (1) reads a description of what to build, (2) turns it into a dependency graph of actions (compile this file with these flags, link these objects, run this generator), (3) executes the graph incrementally and in parallel, re-running only actions whose inputs, flags or tools changed.

**Generator versus executor.** Most modern systems split the two. CMake, GN and Meson are generators: they read their own description language and write Ninja files. Ninja is an executor: it knows nothing about C++ and only runs commands in dependency order, fast. Bazel, Buck2, Cargo and UnrealBuildTool are integrated: one program does both.

**Incremental correctness.** An executor must know every input of an action. For C++ that includes headers, which the compiler discovers while compiling; the compiler therefore writes a depfile (`-MD -MF`) listing them and the executor reads it on the next run. Flags and tool versions are hashed into the action so a changed flag rebuilds. Timestamps are cheap but fragile; content hashes are exact.

**Hermeticity and caching.** A hermetic action depends only on declared inputs, so its output can be cached by the hash of those inputs, locally or on a shared server. Bazel made this mainstream; game engines apply the same idea to derived data (Unreal's DDC).

**Foreign builds.** Third-party libraries come with their own build system. A build tool either drives it once and caches the result (a foreign node) or fetches a prebuilt artifact by hash.

## 2. Landscape

| Tool | Kind | Used by | Strengths | Why not for us |
|---|---|---|---|---|
| Make | integrated, rule based | everything old | universal | no real dependency model for C++ |
| CMake | generator (Ninja, VS, Xcode) | most C++ projects, PocketEngine | ecosystem, IDE support | imperative macro language, no notion of codegen or assets, agents edit it poorly |
| Ninja, n2 | executor | Chromium, V8, CMake, GN, Meson | very fast, simple file format, depfiles | not a description language; we use n2 as our executor |
| GN | generator | Chromium, V8 | fast, clear config | Chromium-specific; we only touch it to build V8 |
| Meson | generator | GNOME, systemd | pleasant DSL | same shape as CMake for our needs |
| Bazel, Buck2 | integrated, hermetic, remote cache | Google, Meta (Buck2 is written in Rust) | reproducible, scales to huge repos | heavy for a small team, poor asset-pipeline fit, steep for newcomers |
| Cargo | integrated | Rust | pinned lockfile, one command, registry | Rust only; the model we imitate for `pocket` |
| UnrealBuildTool | integrated, domain aware | Unreal Engine | module graph, targets, codegen, platform layers | C# and .NET, imperative descriptions, complexity |

## 3. Anatomy of UnrealBuildTool (our teacher)

UnrealBuildTool (UBT) is a C# program under `Engine/Source/Programs/UnrealBuildTool`. Study it in the read-only reference clone; do not copy from it.

- **Modules.** Every module has a `<Name>.Build.cs`, a C# class deriving from `ModuleRules`, declaring `PublicDependencyModuleNames`, `PrivateDependencyModuleNames`, include paths, definitions and per-platform conditionals. Public dependencies propagate to dependents; private ones do not. This explicit public/private edge is the best idea in UBT and we keep it.
- **Targets.** `<Name>.Target.cs` (`TargetRules`) selects a target type (Game, Editor, Client, Server, Program), a link type (monolithic or modular DLLs), platform and configuration. Editor and game are the same modules built with different defines.
- **Platforms and toolchains.** `UEBuildPlatform` and `UEToolChain` classes per platform encapsulate SDK discovery, compiler flags and linking. This layering is also worth keeping.
- **Header tool.** UnrealHeaderTool (a C# component of UBT since 5.1) parses `UCLASS`, `USTRUCT`, `UPROPERTY` and `UFUNCTION` macros and generates reflection code (`*.generated.h`, `*.gen.cpp`). Reflection is what makes the editor, serialization, Blueprints and networking work; the macro-and-generator approach is a pre-standard workaround. We do the same job with a metadata DSL and a generator node, and plan to replace it with C++26 reflection when compilers ship it.
- **Action graph and executor.** UBT builds an action graph, caches it in a "makefile", and runs it with its own parallel executor or through distributed backends (XGE, FASTBuild, SN-DBS, Horde with the Unreal Build Accelerator). Precompiled headers and unity builds (many .cpp files concatenated, "adaptive" unity to keep edited files separate) reduce compile time but hide include hygiene and cause surprising rebuilds.
- **Project generators.** UBT writes Visual Studio, Xcode, Rider, VS Code, CLion and CMake project files so IDEs can navigate and debug.
- **Live Coding.** Patches running processes with recompiled functions, the successor of Hot Reload.
- **Around UBT.** UnrealAutomationTool (UAT) and BuildGraph (XML) script cooking, packaging and CI; the Derived Data Cache (DDC, Zen storage in 5.x) caches cooked and processed assets by content hash.

What UBT does well: explicit module graph, target types, platform abstraction, codegen as part of the build, one entry point for automation. What it does badly: it requires .NET to build anything, descriptions are imperative C# so tools cannot reason about them, startup and graph construction are slow (hence the makefile cache), unity builds and PCH are load-bearing rather than optional, the reflection macros are invasive, and the whole thing is hundreds of thousands of lines that only Epic can maintain.

## 4. `pocket`: the design

Implementation status (2026-09-18): sections 4.1, 4.2 (macOS), 4.3 (C++ and TypeScript nodes, foreign CMake builds, prebuilt fetch), 4.5 (`--json`, `compile_commands.json`) and the commands `setup`, `doctor`, `build`, `run`, `test`, `ts`, `graph`, `clean` exist in `tools/pocket`. The executor is Ninja invoked as a subprocess; n2 embedding, codegen nodes, `check`, `fmt`, `mcp` and the other platforms are future tool milestones (4.10). Since then `gen`, `mcp` and `check` have come (2026-09-29: `check` runs TypeScript 7's native compiler, the `tsgo` line below released as `tsc`, as a prebuilt dependency; `docs/sdk.md`, Types).

`pocket` is a single static Rust binary. It is the only tool a fresh checkout needs. It is a generator, an executor, a toolchain manager, a package fetcher and an agent interface at once, but each part is a separate crate with a narrow job.

### 4.1 Domain model (declarative)

- `pocket.toml` at the repository root: workspace, targets, platforms, toolchain pins, dependency manifest (name, version, URL, content hash, license).
- `module.toml` in every C++ module directory: name, kind (static library, executable, test), sources (globs), `public_deps`, `private_deps`, public and private include directories, definitions, per-platform sections written as structured conditions, optional codegen inputs.
- `[targets]` in `pocket.toml`: runtime, editor, tests, tools; each is a set of root modules plus a platform and configuration matrix.
- Everything is TOML validated by a JSON schema shipped with `pocket`. No logic in descriptions; if a need for logic appears, it becomes a first-class field or a Rust plugin in `pocket`, never inline scripting.

Example:

```toml
# engine/script/module.toml
name = "pocket_script"
kind = "static_library"
sources = ["src/**/*.cpp"]
public_include = ["include"]
public_deps = ["pocket_core"]
private_deps = ["v8"]

[codegen]
metadata = ["meta/*.toml"]   # component metadata consumed by the generator
emit = ["bindings", "dts"]
```

### 4.2 Toolchain layer

`pocket setup` discovers or fetches the pinned toolchain per platform: Apple clang from the Command Line Tools on macOS (version checked), an LLVM release tarball on Linux and Windows (clang-cl), the Windows SDK through the registry, `tsgo` for TypeScript checking, and prebuilt artifacts (the V8 monolith, optionally SDL3) by content hash into `.pocket/`. `pocket doctor` reports what is missing and why.

### 4.3 Graph builder

One graph with several node kinds:

- C++ compile and link nodes with depfiles, flags hashed into the action, `compile_commands.json` written on every build.
- Codegen nodes: component metadata to C++ bindings, `.d.ts`, serialization tables, documentation.
- TypeScript nodes: transform and bundle with oxc and rolldown in-process, source maps kept for runtime stack traces; type checking through `tsgo` as a separate `check` node so builds do not wait for it.
- Shader nodes (from M2): Slang or WGSL to the backend formats.
- Asset nodes (from M5): cook steps keyed by content hash into the derived-data cache.
- Foreign nodes: run a third-party build once (for example SDL3 with its CMake) and cache the result by the hash of its inputs.

### 4.4 Executor

The graph is emitted as Ninja-compatible files (so `ninja` itself can run them when debugging the tool) and executed by n2 embedded as a library. We do not write our own scheduler, dependency scanner or file watcher in v0. Content hashes decide staleness; timestamps are a hint.

### 4.5 Outputs for humans and agents

- `compile_commands.json` for clangd and every IDE; project generators for Xcode and Visual Studio later (`pocket ide`).
- Every command accepts `--json` and prints one JSON document with a stable schema (`docs/schemas/`), including structured diagnostics with file, line, column and the action that produced them.
- `pocket mcp` serves the same commands over the Model Context Protocol so agents inside editors or CI can build, test, capture and query without shelling out.

### 4.6 Commands

`setup`, `doctor`, `build [target] [--platform] [--config]`, `run <target> [--headless]`, `test [--filter] [--json]`, `check` (types, lint, format), `fmt`, `gen` (codegen only), `cook`, `package`, `ide`, `clean`, `mcp`.

### 4.7 What happens on `pocket build`

1. Load `pocket.toml` and every `module.toml`; validate against the schema; resolve the module graph; reject cycles and private-dependency leaks.
2. Resolve the toolchain and dependency manifest; fetch anything missing into `.pocket/` and verify hashes.
3. Run codegen nodes whose inputs changed (metadata to bindings and `.d.ts`).
4. Emit the Ninja graph for the requested target, platform and configuration.
5. Execute with n2; stream structured diagnostics; write `compile_commands.json`.
6. Exit with a code and, with `--json`, a document listing outputs, timings and diagnostics.

### 4.8 Bootstrapping a fresh machine

1. Install rustup (developers only, once) or download a released `pocket` binary.
2. `cargo install --path tools/pocket`, or use the binary.
3. `pocket setup` fetches toolchain pieces and prebuilt dependencies.
4. `pocket build && pocket test`.

Developers never need CMake, Node, Python or .NET to build and run the engine.

### 4.9 Deliberate non-goals for v0

Distributed builds, remote caches, C++ modules, unity builds, precompiled headers by default, a GUI, cross-compilation to mobile or consoles. Each returns as a milestone when a concrete need appears.

### 4.10 Tool milestones

- v0 (M0): macOS, static libraries and executables, tests, prebuilt fetch, TypeScript transform, `--json`.
- v1 (M1): codegen nodes, in-engine TypeScript test runner, Linux CI, foreign builds.
- v2 (M2 and M3): Windows with clang-cl, shader nodes, `pocket ide`, `pocket mcp`.
- v3 (M5): asset cook nodes, derived-data cache, packaging.

## 5. Third-party strategy

| Library | How it enters | Why |
|---|---|---|
| V8 | prebuilt monolith per platform, built by our GN recipe and published as release artifacts | hours to build, must be pinned exactly |
| SDL3 | foreign CMake build cached by hash, or prebuilt | small, stable, official CMake |
| Dear ImGui, flecs, glm, Catch2, stb | vendored source compiled in our graph | header-heavy, trivial to build, we may patch |
| tsgo | prebuilt binary from the TypeScript release | a tool, not linked |

### V8 build recipe (starting point, verified in M0)

```
gn gen out/pocket --args='is_debug=false v8_monolithic=true v8_use_external_startup_data=false is_component_build=false use_custom_libcxx=false v8_enable_i18n_support=false v8_enable_sandbox=false treat_warnings_as_errors=false'
ninja -C out/pocket v8_monolith
```

`use_custom_libcxx=false` links against the platform libc++ so the engine and V8 share one standard library. Disabling ICU removes tens of megabytes and the `Intl` API, which a game runtime can live without. `v8_enable_sandbox=false` is required for typed arrays backed by engine-owned memory. Pointer compression stays on (4 GB heap per isolate, faster).
