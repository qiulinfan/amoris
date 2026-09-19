# The Pocket editor

`pocket editor <project>` opens a project in a window with the scene, its hierarchy, an inspector, and a console. The editor is not a separate application: it is a TypeScript program (`editor/`) built on Pocket UI that runs beside the project in the same runtime, in its own script context. It sees the world through the commands every script and agent has, so nothing it shows is privileged, and an agent can operate it with `ui_snapshot` and `ui_click` the way a person operates it with the mouse.

```bash
./.pocket/pocket editor physics                       # window
./.pocket/pocket editor physics -- --serve 4711       # plus JSON-RPC for an agent
./.pocket/pocket editor ui -- --size 1600x1000
```

## What it does

| Pane | Backed by |
|---|---|
| Toolbar: Play / Pause / Step / Stop, Save scene, Spawn, Delete, tick, entity count, state hash | `script.start`, `pause`/`resume`/`step`, `script.reload`, `world.load`, `project.save_scene`, `world.spawn`/`destroy`, `state`, `world.summary` |
| Hierarchy: every entity by path, click to select | `world.query`, `world.describe` |
| Scene pane: the renderer confined to the pane; click selects by the id buffer; drag orbits the camera; wheel zooms | `render.viewport`, `render.pick`, `world.set` on the Camera's Transform |
| Inspector: name, path, every component the entity has with one input per field (vector fields as x/y/z), remove, add component | `world.schema` (the fields), `world.describe`, `world.set`, `world.remove`, `world.rename` |
| Console / Events / Transcript tabs | `log.tail`, `events.recent`, `transcript` |

Keyboard: Space plays or pauses, Delete removes the selected entity, Escape clears the selection (only when no text input has focus).

`pocket editor <project> --watch` (and `pocket run <project> --watch` without the editor) keeps the window open and hot reloads the project's scripts whenever a source file under the project changes: the tool rebundles, then calls `project.reload` over the control server. Under the editor the scene on disk is left alone and a dormant project stays dormant; without the editor the scene is reloaded too and the project starts again. A bundle error is printed and the previous scripts keep running.

## Play and Stop

When the editor opens a project, the runtime evaluates the project's bundle (so its handlers are registered) but does not start it: no ticks run, `onStart` has not fired, and the scene is exactly what `scene.json` says. Play snapshots the scene (`world.save`), starts the project context (`script.start`) and resumes the simulation. Stop pauses, unloads the project's handlers and interface, evaluates its bundle again, and loads the snapshot back (`world.load`), so edits made before Play survive and everything the game spawned is gone. The project's own interface (mounted with `mount()`) appears inside the scene pane while it runs.

## Agents

Everything in the editor is reachable without a window:

```bash
./.pocket/pocket ts editor && ./.pocket/pocket ts samples/physics
./build/debug/bin/pocket_runtime --project samples/physics --bundle build/ts/physics.js --editor build/ts/editor.js --headless --serve 4711 --paused --json
```

Then `ui.snapshot` lists the panes and their elements by name (`play`, `entity:Ramp`, `Transform.position.x`, `tab:transcript`), `ui.click {id}` presses buttons and selects rows, `ui.type` / `ui.key` edit inspector fields, and `capture` shows the result. `pocket mcp` exposes the same through `runtime_start {project, editor: true}` and the `ui_*` tools. `tests/evidence/editor/session.md` is one such session, and `tests/runtime_tests/editor_tests.cpp` does the same in-process.

## Not yet

Undo/redo, multi-selection, gizmos for moving entities in the scene pane, asset browsing (assets arrive with M5), docking and layout persistence, a script editor. Each is an editor-side feature on the existing commands.
