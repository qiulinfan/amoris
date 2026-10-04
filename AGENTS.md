# Amoris foundation

This checkout contains the Rust/TypeScript foundation merged from `feature/rebuild-foundation`.
Read the accepted [charter](https://github.com/qiulinfan/pocketEngine/blob/documentation/docs/charter.md)
and [roadmap](https://github.com/qiulinfan/pocketEngine/blob/documentation/docs/roadmap.md) on `documentation`
(`git show documentation:docs/charter.md` locally). Those documents are authoritative; the previous
Rust/Lua prototypes and legacy main are historical implementations.

## Current slice

F0 has pocket-contract, pocket-sim, pocket-persist, pocket-link, a headless pocket-app verifier and
cargo xtask. It does not yet execute TypeScript or provide physics, rendering or editor services.
Later slices use QuickJS-ng/oxc, Rapier, wgpu/WGSL, React/dockview/Monaco and the same versions as
the pinned aipocket2 reference. Product graphics targets include D3D12, Vulkan, Metal, OpenGL
and browser WebGPU, with shared 2D/3D backend resources and distinct draw passes.

## Rules

- Use feature/* branches. Keep the original C++/Lua files, CMake targets, sample projects and
  owner-written README untouched. New code does not depend on those targets.
- Decision/specification and evidence summaries live on documentation, not this code branch.
- Do not commit or push documentation without explicit owner permission. Past synchronization
  instructions do not grant standing permission for later commits or pushes.
- State is self-contained and deterministic. Scripts will be stateless systems; fork is an
  explicit capability, never the implementation of each tick or action's atomicity.
- Preserve restricted player projection and intentions when rebuilding gameplay; developers'
  omniscience is explicit. Renderers cannot write simulation state.
- Validate crate edges against tools/crate-graph.toml. No GPU/window/server/LLM dependency in
  game crates; add capabilities as runnable slices without speculative adapters.
- Imported modules name source repository and exact source commit in the commit message.
- Every parallel agent uses its own worktree and CARGO_TARGET_DIR. Preserve unrelated changes.
- Test touched crates; before a shared-code commit run the current cargo xtask check gate.
  Performance is measured, not a pass threshold. Never set POCKET_BLESS in validation.
- Do not claim pending TypeScript, GPU, editor or model checks have run. Code publication and
  merging main are separate from local validation; no automatic GUI/main merge.

## Run

`cargo run -p pocket-app -- foundation --ticks 600 --seed 7 --json`

`cargo xtask check` runs dependency boundaries, formatting, Clippy, native tests, the headless
scenario, wasm compilation and native/wasm reference-vector parity. It builds no GPU or GUI.
Keep CARGO_TARGET_DIR inside this worktree. Requires pinned Rust targets and Node for wasm probes.
