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
| Toolbar: Play / Pause / Step / Stop, Undo / Redo, Save scene, Spawn, Duplicate, Delete, Overlays (colliders and joints as lines in the scene pane, through `render.debug`), tick, entity count, state hash | `script.start`, `pause`/`resume`/`step`, `script.reload`, `world.load`, `project.save_scene`, `world.spawn`/`instantiate`/`destroy`, `state`, `world.summary` |
| Hierarchy: every entity by path; click selects, shift-click (or cmd/ctrl-click) extends the selection | `world.query`, `world.describe` |
| Scene pane: the renderer confined to the pane; click selects by the id buffer (shift extends); drag orbits the camera; wheel zooms; a gizmo on the selection (X, Y, Z handles move along a world axis, the center handle moves in the camera plane, R turns around world Y, S scales uniformly; every selected entity moves together) | `render.viewport`, `render.pick`, `render.project`, `world.set` on Transforms |
| Splitters between the panes; widths, the bottom height and the chosen tab persist in the project's `.pocket/editor.json` | `project.read`, `project.write` |
| Inspector: name, path, every component the entity has with one input per field (vector fields as x/y/z), remove, add component | `world.schema` (the fields), `world.describe`, `world.set`, `world.remove`, `world.rename` |
| Console / Events / Transcript tabs | `log.tail`, `events.recent`, `transcript` |

Keyboard (when no text input has focus): Space plays or pauses, Delete removes the selection, Escape clears it, Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z or Ctrl+Y redoes, Cmd/Ctrl+D duplicates, Cmd/Ctrl+A selects everything, Cmd/Ctrl+S saves the scene.

## Undo

Every edit the editor makes (a field, a rename, adding or removing a component, spawning, deleting, duplicating, a gizmo drag) is one entry in `editor/history.ts` with its inverse; deleted entities come back from a `world.save {entity}` fragment with their children, and entities that return get new ids, which later entries follow. Play clears the history because Stop restores the whole scene anyway.

`pocket editor <project> --watch` (and `pocket run <project> --watch` without the editor) keeps the window open and hot reloads the project's scripts whenever a source file under the project changes: the tool rebundles, then calls `project.reload` over the control server. Under the editor the scene on disk is left alone and a dormant project stays dormant; without the editor the scene is reloaded too and the project starts again. A bundle error is printed and the previous scripts keep running.

## Play and Stop

When the editor opens a project, the runtime evaluates the project's bundle (so its handlers are registered) but does not start it: no ticks run, `onStart` has not fired, and the scene is exactly what `scene.json` says. Play snapshots the scene (`world.save`), starts the project context (`script.start`) and resumes the simulation. Stop pauses, unloads the project's handlers and interface, evaluates its bundle again, and loads the snapshot back (`world.load`), so edits made before Play survive and everything the game spawned is gone. The project's own interface (mounted with `mount()`) appears inside the scene pane while it runs.

## Agents

Everything in the editor is reachable without a window:

```bash
./.pocket/pocket ts editor && ./.pocket/pocket ts samples/physics
./build/debug/bin/pocket_runtime --project samples/physics --bundle build/ts/physics.js --editor build/ts/editor.js --headless --serve 4711 --paused --json
```

Then `ui.snapshot` lists the panes and their elements by name (`play`, `undo`, `entity:Ramp`, `Transform.position.x`, `gizmo:x`, `gizmo:rotate`, `split:hierarchy`, `tab:transcript`), `ui.click {id, mods}` presses buttons and selects rows (`mods: ["shift"]` extends), `ui.type` / `ui.key` edit inspector fields (`ui.key {key: "Z", mods: ["meta"]}` undoes), `ui.drag {id, dx, dy}` pulls a gizmo handle or a splitter, and `capture` shows the result. `pocket mcp` exposes the same through `runtime_start {project, editor: true}` and the `ui_*` tools. `tests/evidence/editor/session.md` is one such session, and `tests/runtime_tests/editor_tests.cpp` does the same in-process.

## Not yet

Rotation about other axes and per-axis scale handles, snapping, an asset browser, drag-to-reparent in the hierarchy, tabbed docking beyond resizable panes, a script editor. Each is an editor-side feature on the existing commands. Gizmo drags apply the world-space delta to each entity's local position, which is exact for entities whose parents are not rotated or scaled.
