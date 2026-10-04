# Working on aipocket2 (Pocket3D)

aipocket2 is an agent-native 3D game engine: Rust host, TypeScript scripts on QuickJS-ng, wgpu
rendering (Metal and Vulkan natively, WebGPU in the browser), a React web editor, MCP for agents.
Read [the charter](docs/charter.md) first: it fixes the stack, the architecture and what this round
must prove. The schedule is [docs/schedule.md](docs/schedule.md). The game-side subsystems are
specified under [docs/spec/](docs/spec/README.md); their measured spikes are under
[docs/spikes/](docs/spikes/).

## Layout

- `crates/`: the Rust workspace. Game-side crates (`pocket-sim`, `pocket-assets`, `pocket-persist`,
  `pocket-physics`, `pocket-interface`, `pocket-script`, `pocket-link`, `pocket-runtime`,
  `pocket-check`) build headless and for `wasm32`; they never depend on wgpu, winit, tokio or rmcp.
  Presenters (`pocket-render`, `pocket-debug`, `pocket-mcp`, `pocket-server`, `pocket-app`,
  `pocket-web`) reach the game only through `pocket-link`. The allowed edges are
  `tools/crate-graph.toml`; a new crate or edge changes it together with the charter's table.
- `shaders/` live next to the renderer code that uses them, as `.wgsl` files, never Rust strings.
- `sdk/`: the TypeScript `pocket` module scripts import, with generated types.
- `editor/`: the React web editor (Vite, bun); [docs/spec/editor.md](docs/spec/editor.md) says how to
  run it against the mock host (`bun run dev:mock`) or the real one, and what it needs from the host.
- `samples/`: game projects. `tools/`: Python and helper scripts (`tools/neural/` trains neural
  assets). `third_party/`: the patched `rquickjs-sys` (QuickJS-ng with PR #1421 and P1 to P8).
- `docs/`: charter, schedule, specs, spikes, evidence.

## Rules

1. A change that departs from the charter updates the charter first, with the reason.
2. Keep `main` runnable end to end: add capabilities on top of a verified baseline.
3. Code is organized by responsibility, with no file-length limit. Performance is measured and
   recorded (docs/bench/), never a pass condition.
4. Testing is light and targeted: run the tests of the crate you touch; run the workspace before a
   commit that touches shared code. Research, reach and performance come first.
5. Material brought from another repository (aipocket, PocketEngine) names its source repository
   and commit in the commit message.
6. Owner-facing explanations are Chinese; code, identifiers, commit messages and specs are English.
   The charter and schedule are Chinese.

## Building

- Rust 1.98.1 (`rust-toolchain.toml`); `source ~/.cargo/env` if cargo is not on PATH.
- `cargo build --release` builds the workspace; `cargo xtask check` runs the full local check.
- `wasm32-unknown-unknown` builds of QuickJS-ng need a clang with the wasm backend: the build script
  finds Homebrew's LLVM (`brew install llvm lld wasi-libc`) or `POCKET_LLVM`.
- Vulkan on macOS runs through MoltenVK (`brew install molten-vk vulkan-loader`); select it with
  `POCKET_BACKEND=vulkan` and launch with `DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib` so the
  loader is found. Metal is the default on macOS. `MVK_CONFIG_LOG_LEVEL=3` prints MoltenVK's shader
  compile errors; `cargo run --release -p pocket-render --example gpu_probe` checks a backend.
- Compute pipelines use `shaders::compute_options()` (no automatic workgroup zeroing: it breaks
  MoltenVK); a compute shader must initialize the workgroup memory it reads.
- Agents working in parallel use their own git worktree and their own cargo target directory.
- macOS has no `timeout`; use `perl -e 'alarm 60; exec @ARGV' cmd` as a watchdog.
