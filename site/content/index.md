# Start with a running sample

Amoris is a Rust 3D host with TypeScript game systems, a web editor, and a shared command API.
The executable is currently named `pocket`.

## Build and open the editor

Install Rust through [rustup](https://rustup.rs/) and [Bun](https://bun.sh/).
The repository pins Rust 1.98.1, rustfmt, Clippy, and the wasm targets in `rust-toolchain.toml`.

From the repository root:

```sh
cd editor
bun install --frozen-lockfile
bun run build
cd ../sdk
bun install --frozen-lockfile
cd ..
cargo build --release
./target/release/pocket serve samples/sailing --editor editor/dist
```

Open **http://127.0.0.1:7878/**. The host starts paused. Press **Play** in the editor
to run a fork of the edit world; **Stop** returns to the original edit world.

For the detailed sailing workflow shown on the homepage, serve `site/demos/harbor` instead.
`site/demos/detail` contains the CC0 Flight Helmet material study.
`site/demos/bistro` contains the full licensed Bistro exterior.
Run `python3 tools/showcase_assets.py --projects` to fetch the pinned CC0 models
and prepare the detailed sailing and helmet projects. The large Bistro source needs
its separately audited FBX conversion; see [asset credits](credits.md).

The host binds to loopback. The editor, CLI, and MCP all reach the same command catalog.

## Open a native window

```sh
./target/release/pocket play samples/sailing
./target/release/pocket play samples/anim
```

The native renderer uses Metal on macOS and Vulkan on supported native platforms.
The browser renderer uses WebGPU; this version has no WebGL fallback.

## The small technology map

| Responsibility | Implementation |
| --- | --- |
| Host and entities | Rust · Bevy ECS |
| Game systems | TypeScript 7 · oxc · QuickJS-ng |
| Physics | Rapier 3D with enhanced determinism |
| Rendering | wgpu · WGSL · Metal / Vulkan / WebGPU |
| Editor | React · dockview · Monaco |
| Interfaces | CLI · HTTP / WebSocket · MCP |

All game state lives in components. TypeScript systems hold no mutable module state.
Snapshots, replay, and explicit forks operate on the simulation; rendering reads its output.

## Where to go next

- [Editor](editor.md): select an entity, edit a property, play, and debug a script.
- [Script API](sdk.md): components, queries, systems, events, and generated types.
- [CLI and MCP](api.md): discover commands and automate a running host.
- [Data and validation](data.md): what the published numbers actually measured.

For browser builds, QuickJS-ng's C code also needs a wasm-capable clang, LLVM tools,
and wasi-libc. On macOS the build scripts find Homebrew LLVM; `POCKET_LLVM` can point
to another installed LLVM. Node is used by the wasm validation probes.
