# The editor

The editor is a client of the Rust host. Its viewport uses the engine's WebGPU renderer;
edits use the same commands exposed to CLI and MCP clients.

## Desktop application

The desktop editor opens the same panels in an independent Amoris window. It provides a native
project-folder picker, recent projects, and application menus, and manages the Rust host for you.
Choose a folder containing `project.toml`, or use **Cmd/Ctrl+O** to switch projects. Closing or
switching asks before discarding unsaved scene changes or script buffers.

Build and package the app using the [README](https://github.com/qiulinfan/amoris#start-the-desktop-editor).
The local application includes the engine, WebGPU module, and TypeScript checker. Its viewport
uses the same engine renderer as the browser editor.

## A first edit

1. Open the sailing sample using the [quick start](index.md).
2. Select **Sloop** in the Hierarchy. The Inspector shows its components and fields.
3. Edit a transform or a boat control. Changes go through `world.edit` and appear in History.
4. Use the viewport's move and rotate tools, or the Inspector for an exact value.
5. Undo the change. Edits from the editor and automation share one history.

The command palette (**Cmd/Ctrl+K**) searches panels, entities, and host commands.
Type `call world.get` to inspect the selected entity through the catalog.

## Play, pause, and return

- **Play** forks the edit world and runs the fork.
- **Pause** holds simulation time; **Step** advances a tick.
- **Stop** discards the play fork. It does not save play-mode edits into the scene.
- Save the edit world deliberately when you want to keep scene changes.

The Timeline shows retained snapshots. Events can be inspected together with their cause
chains. The Profiler shows simulation and presentation measurements reported by the host.
For bounded CPU/GPU captures with frame IDs, use the native
[profiling workflow](profiling.md) and open the exported trace in Perfetto.

## Write and check a script

Open `scripts/rules.ts` in the Scripts panel. Monaco receives the `pocket` SDK and the
project's component declarations from the live registry.

**Cmd/Ctrl+S** writes and applies a script at a tick boundary. For a checked CLI workflow,
run `scripts check` before `scripts apply`: type diagnostics do not themselves prevent a swap.

```sh
./target/release/pocket scripts check --host http://127.0.0.1:7878
./target/release/pocket scripts apply --host http://127.0.0.1:7878
```

## Debug at the TypeScript source

Set a breakpoint in the gutter or press **F9**. Start Play, then inspect the paused stack,
locals, and watches. Continue with **F5**; use **F10** to step over and **F11** to step into.
The debugger also exposes data breakpoints and `debug.*` commands through the shared API.
While stopped at a breakpoint, ordinary world reads show the last published tick boundary;
use `debug.eval` to inspect the currently paused script frame. **Stop** can leave a breakpoint
and return to the edit world directly.

The Agent panel can display sessions and calls reported by the host. Its New session form
is not a bundled LLM service: starting an autonomous model session still needs host support.

## Develop the editor

```sh
cd editor
POCKET_HOST=http://127.0.0.1:7878 bun run dev
bun run typecheck
bun run build
```

Vite proxies the host endpoints while serving the editor on its development port.
Use `?viewport=wasm` to require the WebGPU viewport. `bun run dev:mock` is useful for UI
development, but the showcase recording uses the real Rust host.
