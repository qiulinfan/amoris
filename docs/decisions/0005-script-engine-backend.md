# ADR 0005: Script engine backend behind `ScriptHost`

- Status: Accepted for the reference build (2026-09-18). The V8 target in ADR 0001 stands; this
  record explains why the first runnable engine uses JavaScriptCore and what the boundary
  guarantees.
- Deciders: repository owner (human), Claude (AI collaborator)

## Context

ADR 0001 selects V8 with JIT as the script engine, built as a monolithic static library by our own
GN recipe and fetched prebuilt by `pocket`. On the machine where milestone 0 was built, that recipe
cannot run:

- The Xcode license has not been accepted, which blocks Homebrew and `xcodebuild`; only the Command
  Line Tools work (`DEVELOPER_DIR=/Library/Developer/CommandLineTools`).
- The prebuilt V8 published by the Deno project (`rusty_v8` 152.2.0, V8 15.2) is compiled against
  Chromium's bundled libc++ (`std::__Cr` namespace) and cannot be linked from a normal C++ program.
- Building V8 from source needs depot_tools and hours of compile time, and would produce an artifact
  nobody has verified yet.

Milestone 0 has to prove the whole pipeline (TypeScript to bundle to engine to GPU to report) end to
end, not a V8 build recipe.

## Decision

1. Gameplay code talks to the engine only through `pocket::script::ScriptHost`
   (engine/script/include/pocket/script/script_host.hpp): evaluate a bundle, bind native functions
   under `__pocket`, share engine memory as typed arrays, call global dispatch functions with JSON
   arguments, drain microtasks. Nothing outside `engine/script` includes an engine-specific header.
2. The reference backend on macOS is JavaScriptCore (the system framework), which has a JIT, a
   stable C API, zero-copy typed arrays over engine memory
   (`JSObjectMakeTypedArrayWithBytesNoCopy`), promise support and Web Inspector attachment. A spike
   measured a 20 million iteration object-allocating loop in 1.4 s, which is JIT-class performance.
3. V8 remains the target for Linux and Windows and the long-term default. It arrives as a second
   backend implementing the same interface, selected by `pocket.toml`, when the prebuilt monolith
   recipe is verified. The SDK, the bundle format and the dispatch contract do not change.
4. The bundle produced by `pocket ts` is plain ES2023 JavaScript with a tiny CommonJS-style
   registry; it targets nothing engine-specific.

## Consequences

- macOS builds have no script-engine build step and no multi-hundred-megabyte artifact;
  `pocket setup` finishes in under a minute on a fresh checkout.
- Behavior differences between engines (number formatting, error message text, microtask timing) are
  covered by the SDK's own tests so that switching backends is a test run, not a migration.
- `JSGlobalContextSetInspectable` gives Safari's Web Inspector for free during development
  (`pocket_runtime --inspectable`).
- The JavaScriptCore C API has no module loader and no direct access to the JIT tiers; both are
  acceptable because bundling happens in `pocket` and performance-critical data crosses the boundary
  as shared typed arrays.
