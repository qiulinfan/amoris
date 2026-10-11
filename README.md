# <img src="assets/branding/amoris-icon-morandi.png" alt="Amoris icon" width="64" valign="middle"> Amoris

Amoris is an agent-native 3D game engine with a Rust host, TypeScript gameplay, and desktop and web editors.
The editor, CLI, and agents share one command API. A wgpu renderer runs natively
on Metal, Vulkan, and Direct3D 12, and in the browser on WebGPU.

[Showcase](https://qiulinfan.github.io/amoris/) ·
[Editor guide](https://qiulinfan.github.io/amoris/documentation/editor/) ·
[Script API](https://qiulinfan.github.io/amoris/documentation/sdk/) ·
[CLI and MCP](https://qiulinfan.github.io/amoris/documentation/api/) ·
[Measurements](https://qiulinfan.github.io/amoris/documentation/data/) ·
[Profiling guide](https://qiulinfan.github.io/amoris/documentation/profiling/)

[![Bistro with global illumination rendered by Amoris](site/media/scene-poster.jpg)](https://qiulinfan.github.io/amoris/#showcase)

*Bistro exterior, captured with multi-bounce Metal path tracing: 512 spp and a 12-bounce limit.*

[Amazon Lumberyard / ORCA](https://developer.nvidia.com/orca/amazon-lumberyard-bistro),
CC BY 4.0. [Asset credits](site/content/credits.md).

## What you can do

- Build games with typed components and stateless TypeScript systems.
- Inspect and edit a scene through the desktop or web editor, CLI, HTTP, or MCP.
- Run Play in a fork, pause or step the simulation, and return to the edit world.
- Debug TypeScript with breakpoints, locals, watches, and source-level stepping.
- Record snapshots, replay inputs, and check deterministic runs.
- Capture CPU spans and GPU passes with frame IDs in a Perfetto trace.

The editor combines a WebGPU viewport with a Hierarchy, Inspector, asset browser,
Monaco script editor, debugger, History, Timeline, and Profiler. Its edits and agent
commands enter the same undoable history.

## Start the desktop editor

The desktop app opens projects in its own window, with a native folder picker, recent
projects, and application menus. It starts and stops the Rust host for you. The app
includes the engine, WebGPU viewport, and TypeScript checker.

Install [Rust](https://rustup.rs/), [Bun](https://bun.sh/), and [Node.js](https://nodejs.org/),
then build from the repository:

```sh
cd sdk && bun install --frozen-lockfile
cd ../editor && bun install --frozen-lockfile && bun run build
cd ..
cargo build --release -p pocket-app
cd desktop && npm ci
npm start -- ../samples/sailing
```

To create a local macOS application bundle, run `npm run package` in `desktop/`.
Open **Amoris.app** from `out/desktop/`; choose a folder containing `project.toml`.
**Cmd/Ctrl+O** switches projects. The app asks before discarding unsaved edits.
See the [desktop build guide](docs/spec/desktop.md) for packaging options.

## Start the web editor

Install [Rust](https://rustup.rs/) and [Bun](https://bun.sh/). The repository pins its
Rust toolchain in [rust-toolchain.toml](rust-toolchain.toml).

```sh
git clone https://github.com/qiulinfan/amoris.git
cd amoris

cd sdk
bun install --frozen-lockfile
cd ../editor
bun install --frozen-lockfile
bun run build
cd ..

cargo build --release -p pocket-app
./target/release/pocket serve samples/sailing --editor editor/dist
```

Open **http://127.0.0.1:7878/** and press **Play**. The executable is currently named
`pocket`. The browser viewport requires WebGPU.

Select **Sloop** in the Hierarchy to edit its components. Open `scripts/rules.ts` to
change its gameplay; **Cmd/Ctrl+S** applies the script. Set a breakpoint in the gutter,
then use **F5**, **F10**, and **F11** to continue, step over, and step into. **Stop**
returns to the original edit world. See the [editor guide](site/content/editor.md).

For a native game window:

```sh
./target/release/pocket play samples/sailing
./target/release/pocket play samples/anim
```

## The small technology map

| Responsibility | Technology |
| --- | --- |
| Host and entities | Rust · Bevy ECS |
| Gameplay | TypeScript 7 · oxc · QuickJS-ng |
| Physics | Rapier 3D with enhanced determinism |
| Rendering | wgpu · WGSL · Metal / Vulkan / Direct3D 12 / WebGPU |
| Editor | React · dockview · Monaco · Electron (desktop) |
| Automation | CLI · HTTP / WebSocket · MCP |

Game state lives in components. Scripts run as stateless systems, and the renderer
reads the simulation's output. Snapshots, replay, and explicit forks operate on the
same self-contained world.

## API and agents

Discover commands and inspect a running game:

```sh
./target/release/pocket help
./target/release/pocket catalog --host http://127.0.0.1:7878 --json
./target/release/pocket world get Sloop Boat Tally --host http://127.0.0.1:7878 --json
```

Use `./target/release/pocket mcp samples/sailing` as a stdio MCP server, or connect to
the running host at `http://127.0.0.1:7878/mcp`. These tools expose the developer
command surface. Games that declare player seats also expose engine-enforced perception
and intention commands. For example, `pocket mcp samples/sailing-course --seat skipper`
exposes only the player tool; developer edits and debugger commands are refused.
See the [player workflow](site/content/api.md#play-through-a-restricted-seat).

- [Script API](site/content/sdk.md): components, queries, systems, events, and types.
- [CLI and MCP](site/content/api.md): discovery, world edits, time, capture, and debugging.
- [Host protocol](docs/spec/host-protocol.md): HTTP calls, WebSocket feeds, and schemas.

## Rendering and profiling

The interactive renderer includes GPU culling, an adaptive depth prepass, clustered PBR,
shadows, animation, and 3D Gaussian splats. Metal is the macOS default, Direct3D 12 the
Windows default; Vulkan remains selectable with `POCKET_BACKEND=vulkan`.
Separate path-tracing and neural-lighting research tools require supported native hardware.

The **10 October 2026** Windows study measured a dense 1.6-million-cube workload at
2560 × 1440, 4× MSAA, with occlusion culling and GTAO disabled. Forcing the depth prepass
on reduced the median of run-mean GPU pass sums as follows:

| GPU and backend | Prepass off | Prepass on |
| --- | --- | --- |
| RTX 5060 Laptop · Direct3D 12 | 51.33 ms | 16.22 ms |
| RTX 5060 Laptop · Vulkan | 42.51 ms | 16.59 ms |
| Radeon 780M · Direct3D 12 | 297.57 ms | 96.26 ms |

These provisional, three-round measurements describe heavy overdraw. The sphere layout
got slower with a forced prepass; auto mode probes both paths and retains the cheaper one.
[Workloads, default-culling results, and limitations](docs/bench/prepass.md).

On **Apple M5 / Metal**, the profiling follow-up fixes stale-frame timestamps and clock
calibration, and adds asynchronous counter readback. All **2,700 captured GPU frames**
fit their own CPU submission/completion bounds within calibration tolerance, with no missing
or dropped records.
Six prepass comparison scenes retain identical pixels and entity coverage. The timing
runs vary too much to establish a stable Metal speedup.
[Metal method and measurements](docs/bench/metal-profiling.md).

Use the [profiling guide](https://qiulinfan.github.io/amoris/documentation/profiling/)
to capture traces and distinguish CPU wall time, GPU frame spans, and pass sums.
Sanitized run data and logs live in
[amoris-benchmarks-results](https://github.com/qiulinfan/amoris-benchmarks-results).

## Samples and measurements

| Project | Demonstrates |
| --- | --- |
| [Sailing](samples/sailing) | Wind, buoyancy, boat controls, and cargo collection |
| [Animation](samples/anim) | Animation, particles, and in-game UI |
| [Sailing course](samples/sailing-course) | Restricted player perception, intentions, and decision points |

Rendering measurements report their scene, hardware, resolution, and timing method.
Recorded demonstration videos use fixed simulation steps; playback frame rate is
separate from the engine's measured performance. See the [showcase data](site/content/data.md),
[web rendering study](docs/bench/web.md), and [physics study](docs/bench/physics.md).

### High-detail showcase

The site includes native Bistro and Flight Helmet videos, a CC0 Dutch ship controlled
through MCP, and a recording of the real WebGPU editor. Fetch the pinned CC0 models
locally to open the detailed sailing sample:

```sh
python3 tools/showcase_assets.py --projects
./target/release/pocket serve site/demos/harbor --editor editor/dist
```

In the **5 October 2026** forward-renderer study at 1920 × 1080 on Apple M5 / Metal,
1,024 complete Flight Helmets contribute
96,995,330 scene triangles before culling. The static render-and-wait workload took
82.43 ms mean and 83.66 ms p95 over 300 completed frames. This excludes game simulation
and window presentation. [Method and p95](site/content/data.md).

Large third-party models stay outside Git; the website serves compact videos and
posters. Source authors and licenses are preserved in the [credits](site/content/credits.md).

### Real agent gameplay and development

The **5 October 2026** DeepSeek Flash experiment plays the detailed sailing course through
a project-level player gateway.
Its native replay matches every recorded action and final world hash. The selected clip
collects 4/4 cargo; all trials and limitations are retained in the
[agent report](site/content/agents.md).

A frozen six-task development benchmark uses native MCP tools, fresh candidate projects
and external grading. All **18/18 patches** passed behavior, regression, types and native
integrity checks; **15/18 agent sessions** ended cleanly, with three provider disconnects
after valid patches. Each golden control exercises 63 actual component/event assertions.

## License

Amoris is released under the [MIT License](LICENSE). Third-party dependencies and
showcase assets retain their own licenses and attribution.
