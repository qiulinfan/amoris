# The Pocket editor

`pocket editor <project>` opens a project in a window with the scene, its hierarchy, an inspector,
and a console. The editor is not a separate application: it is a TypeScript program (`editor/`)
built on Pocket UI that runs beside the project in the same runtime, in its own script context. It
sees the world through the commands every script and agent has, so nothing it shows is privileged,
and an agent can operate it with `ui_snapshot` and `ui_click` the way a person operates it with the
mouse.

```bash
./.pocket/pocket editor physics                       # window
./.pocket/pocket editor physics -- --serve 4711       # plus JSON-RPC for an agent
./.pocket/pocket editor ui -- --size 1600x1000
```

## What it does

| Pane | Backed by |
|---|---|
| Toolbar: Play / Pause, Step and Stop as one group, Undo / Redo, Spawn, Clone, Delete, and Save at the right beside the scene's file; the status bar under the docks says whether the project is being edited, plays or is paused (tinted green while it plays, amber while paused), the last notice, the tick, the entity count and the state hash | `script.start`, `pause`/`resume`/`step`, `script.reload`, `world.load`, `project.save_scene`, `world.spawn`/`instantiate`/`destroy`, `state`, `world.summary` |
| Hierarchy: every entity by path; click selects, shift-click (or cmd/ctrl-click) extends the selection; a row dragged onto another becomes its child, dropped on the panel's background it becomes a root, staying where it stands in the world (`world.reparent` with `keep_world`; one undoable edit) | `world.query`, `world.describe` |
| Scene pane: the renderer confined to the pane; click selects by the id buffer (shift extends); drag orbits the camera; wheel zooms; a gizmo on the selection (X, Y, Z handles move along a world axis, the center handle moves in the camera plane, R, RX and RZ turn around world Y, X and Z, S scales uniformly and SX, SY, SZ one axis; every selected entity moves together, a child under a turned or scaled parent by exactly the handle's world move or turn; with Local on, kept with the layout, the handles sit on the entity's own axes and moves and turns follow them); a view bar over the pane's top-right corner (out of the way of a HUD at the top left) holds Overlays (colliders, joints and lights as lines, through `render.debug`), Snap, its step and World/Local (a toggle that is on is tinted in the accent, and Overlays and Snap read `✓ Overlays`, `✓ Snap`); with Snap on (kept in the project's `.pocket/editor.json` with the step) a drag lands on the grid: positions to the snap step on the dragged axes (the step button beside Snap cycles 0.1, 0.25, 0.5, 1, 2 units), turns to 15 degrees, scales to quarters | `render.viewport`, `render.pick`, `render.project`, `world.set` on Transforms |
| Tile brush: with a `TileMap` entity selected, the inspector's Tiles section lists the map's layers and tiles (and erase); picking one turns the brush on, a click or drag in the scene pane paints the cells under the mouse, every stroke is one undo step, Stop painting returns the click to selecting, Save map writes the `.tmj` back (`docs/design/tilemaps.md`, Editing) | `tilemap.info`, `render.unproject`, `tilemap.cell`, `tilemap.set`, `tilemap.save` |
| Docks: the panes (Hierarchy, Inspector, Console, Events, Transcript, Assets, Input, Audio, Script, Timeline) live in three docks, left, right and bottom, each showing one behind tabs (the hierarchy's tab counts its entities, the inspector's names the selection); a tab dragged onto another dock, or near that edge of the scene pane, moves its pane there, and a dock left empty closes and gives the scene its room. Splitters between the docks; the docks, what each shows, the widths and the bottom height persist in the project's `.pocket/editor.json` (a layout saved before the docks opens as it was) | `project.read`, `project.write` |
| Inspector: name, path, every component the entity has as a section (its header names it and holds Remove, or says `derived`) with a row per field, the field's name in a label column and its input filling the rest (a MeshRenderer's texture with a pattern stepped through by name: the engine's bricks, tiles, planks, cobble and the rest put on as its texture, normal map and world-laid tile in one undoable edit, `docs/design/assets.md`, Patterns) (vector fields as x/y/z and colours as r/g/b/a, one input a part, each led by its letter in the axis colour; a flag as a check box; a code with value names, such as `Light.kind`, `RigidBody.kind` or a project's own, stepped through by name), remove, add component | `world.schema` (the fields), `world.describe`, `world.set`, `world.remove`, `world.rename` |
| Console / Events / Transcript tabs | `log.tail`, `events.recent`, `transcript` |
| Script tab: the project's scripts (`scripts/`, `scenarios/`) in a list; one opens in a text area coloured as TypeScript (the area's `syntax` is the file's path, `docs/design/pocket-ui.md`), Save or Cmd/Ctrl+Return writes it back to the project, and with `--watch` the tool rebuilds the bundle and reloads the project and type-checks it: the errors are listed under the text area (the open file's in red, with line and column) and logged in the Console | `assets.list` (kind `script`), `project.read`, `project.write`, `script.diagnostics` |
| Input tab: every action of the input map with its positive, negative and axis bindings as text (comma-separated key names: `Space`, `Left`, `pad:a`, `pad1:a` for one pad, `pad:leftx`, `mouse:x`); a field committed rebinds at once (one undoable edit, the live state kept), Press beside a field takes the next key or pad button pressed as that part's binding (its keyboard keys are replaced, pad and mouse bindings stay; Escape cancels), Add makes an action from a name and its first keys (an action without keys is refused), Remove takes one out, Save bindings writes `input.json` beside `project.toml`, which the runtime takes over the TOML's map from then on (`docs/design/input.md`, Map) | `input.describe`, `input.map`, `project.write` |
| Terrain brush: with a `Terrain` selected, the inspector's Sculpt section (raise, lower, flatten, smooth; radius; strength) turns the scene pane into a brush over the ground where the camera's ray meets it, one undo step per stroke; Save heightmap and Reset (`docs/design/terrain.md`, In the editor) | `terrain.sculpt`, `terrain.heights`, `terrain.save`, `terrain.reset`, `physics.raycast`, `render.unproject` |
| Sky clock: with a `Sky` selected, the inspector's Time of day section shows its hour as a clock and sets it by a slider (a quarter hour a step) or the dawn, noon, dusk and night buttons, each one undoable edit; the sun stands where the hour puts it at once (`docs/design/rendering.md`, A day) | `world.get`, `world.set` |
| Weather sliders: with a `Weather` selected, the inspector's Weather section sets how hard it rains and snows, how wet things are and how much snow lies by sliders, each change one undoable edit (`docs/design/rendering.md`, Weather) | `world.get`, `world.set` |
| Audio tab: the mixer. The master volume and mute (the player's, not saved), then every bus with a volume slider (0 to 200 percent), a mute, a low-pass slider, its voices and its ducking (what it ducks under, to how much, how fast, and how far it is down right now); changes are heard at once, and Save mixer writes `audio.json` beside `project.toml`, which the runtime applies over the TOML's `[audio.buses]` (`docs/design/audio.md`, Buses) | `audio.buses`, `audio.bus`, `audio.master`, `project.write` |
| Assets tab: every file under the project's `assets/` with its kind (mesh, image, tilemap, audio), size and whether it is loaded, then the meshes the engine makes (the humanoid and the props: tree, pine, rock, house, well, ...; `docs/design/assets.md`, Props), each with its thumbnail, picked, dragged and placed as a file is, images with a thumbnail, a model with a thumbnail drawn by `assets.preview` into the project's `.pocket/thumbs` (once a session, again on Reimport), and for a model read by an importer of its own which one (OBJ, STL, via Blender for `.blend`, `.fbx` and the other formats Blender reads, `docs/design/assets.md`); a click describes it (vertices, nodes, clips, lights and cameras, and for a Blender-read file where its conversion is kept; pixel size; layers); a row dragged onto the scene pane becomes an entity under the pointer, and Place puts the picked file in the middle of the pane, one undoable edit: a mesh as a `MeshRenderer` where the ray meets the ground plane, an image as a `Sprite` (one unit on its longer side, the aspect kept) and a map as a `TileMap` where it meets the XY plane, a sound as an `AudioSource`; a model that carries lights or cameras (a Blender scene with its lamp and camera) comes in as its node tree instead, so they come along (the lights lit, the cameras inactive), and undo takes the whole tree away; Reimport reads a picked model from its file again (a Blender-read one converted anew) | `assets.list`, `assets.describe`, `assets.import`, `render.unproject`, `world.spawn`, `world.instantiate {mesh}` |
| Timeline tab: the selected entity's `Timeline` file (`docs/design/timelines.md`) as a row per track (its entity, component and field) with its keys as marks on a ruler of the file's duration and a red playhead; the playhead's slider scrubs (`timeline.seek` applies the tracks at that time without a tick, so the scene shows the moment), Play and Stop play it; a mark picked shows its key (Go to it, Delete key); Key on a track keys its field's value now at the playhead (replacing a key already there); a new track (entity, component, field) starts with the field's value now; Remove drops a track. Every change writes the file, which the Timeline reads again, and is one undo step; with no Timeline selected the tab lists the entities that have one, and a Timeline naming a missing file offers to create it | `timeline.seek`, `timeline.play`/`stop`, `project.read`, `project.write` |

Keyboard (when no text input has focus): Space plays or pauses, Delete removes the selection, Escape
clears it, Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z or Ctrl+Y redoes, Cmd/Ctrl+D duplicates, Cmd/Ctrl+A
selects everything, Cmd/Ctrl+S saves the scene.

## Look

A dark theme defined once at the top of `editor/main.tsx` (the Look section): a palette (`C`), a
spacing scale (`S`, 2 to 12 points), three font sizes (11, 12 and 13 points) and the row heights,
and the few pieces every pane is built from: the editor's own `Button` (raised, primary in the
accent, ghost until hovered, a toggle that is on tinted in the accent, disabled in faint type) and
`TextInput` (sunken, outlined in the accent while it has the focus), `Section` (a header bar and its
rows), `Prop` (a label column and the fields), `Empty` (what an empty pane says). The SDK's own
widgets (`Choice`, `Slider`, `Checkbox`) take the palette through `theme`, which the editor sets in
its own bundle only: the project's interface keeps the SDK's colours.

Surfaces get lighter from the window behind the panes (the toolbar, the status bar, the splitters
between the docks) to a pane, a section header, a button and a hovered one; inputs sit darker than
their pane. The tab in front of a dock takes its pane's colour under a thin accent line, so it joins
its pane; the others are dim text on the darker strip. The hierarchy indents a child under its
parent with a guide line per level, marks a parent with a square and a leaf with a ring, and shows
the primary selection stronger than the rest of it. Buttons, rows, tabs, splitters and gizmo handles
show the pointer over them (`hover` events into one `hovered` signal). Glyphs come from the UI font,
Noto Sans CJK SC, which has `▶` and `✓` but no pause, stop or cross symbols, so those icons are
drawn from boxes.

Names agents and tests use did not change with the look (`play`, `entity:Ramp`, `tab:script`,
`Transform.position.x`, ...), and editor tests click hierarchy rows at 1024 by 640 points: the
toolbar is 36 points high, a tab strip 24 and a row 22, which leaves the fifteenth row of the
physics sample (`Ramp`) inside the pane.

## Undo

Every edit the editor makes (a field, a rename, adding or removing a component, spawning, deleting,
duplicating, a gizmo drag) is one entry in `editor/history.ts` with its inverse; deleted entities
come back from a `world.save {entity}` fragment with their children, and entities that return get
new ids, which later entries follow. Play clears the history because Stop restores the whole scene
anyway.

`pocket editor <project> --watch` (and `pocket run <project> --watch` without the editor) keeps the
window open and hot reloads the project's scripts whenever a source file under the project changes:
the tool rebundles, then calls `project.reload` over the control server, which also reads
`project.toml`'s settings again (the input map, audio buses and room, render and physics settings,
sprite clips), so an edited action or bus takes effect without a restart. A changed file under
`assets/` (a picture, a map, a sound, a model, a `.blend` saved from Blender) needs no bundle: the
tool calls `assets.reload`, so the runtime forgets its decoded copy and the next frame draws the
file as it is now, and instances of a changed model are made again in place
(`docs/design/assets.md`, Live models), with the scripts and the world left running. Under the
editor the scene on disk is left alone and a dormant project stays dormant; without the editor the
scene is reloaded too and the project starts again. A bundle error is printed and the previous
scripts keep running.

## Play and Stop

When the editor opens a project, the runtime evaluates the project's bundle (so its handlers are
registered) but does not start it: no ticks run, `onStart` has not fired, and the scene is exactly
what `scene.json` says. Play snapshots the scene (`world.save`), starts the project context
(`script.start`) and resumes the simulation. Stop pauses, unloads the project's handlers and
interface, evaluates its bundle again, and loads the snapshot back (`world.load`), so edits made
before Play survive and everything the game spawned is gone. The project's own interface (mounted
with `mount()`) appears inside the scene pane while it runs.

## Agents

Everything in the editor is reachable without a window:

```bash
./.pocket/pocket ts editor && ./.pocket/pocket ts samples/physics
./build/debug/bin/pocket_runtime --project samples/physics --bundle build/ts/physics.js --editor build/ts/editor.js --headless --serve 4711 --paused --json
```

Then `ui.snapshot` lists the panes and their elements by name (`play`, `undo`, `entity:Ramp`,
`Transform.position.x`, `gizmo:x`, `gizmo:rotate`, `split:hierarchy`, `tab:transcript`,
`tab:assets`, `asset:assets/bowl.glb`, `tab:script`, `script:scripts/main.ts`, `script:text`),
`ui.click {id, mods}` presses buttons and selects rows (`mods: ["shift"]` extends), `ui.type` /
`ui.key` edit inspector fields (`ui.key {key: "Z", mods: ["meta"]}` undoes), `ui.drag {id, dx, dy}`
pulls a gizmo handle (`gizmo:x`, `gizmo:rotate_x`, `gizmo:scale_z`, ...) or a splitter, drops a
hierarchy row on another, an asset row on the scene pane or a tab (`tab:audio`) on another dock, and
`capture` shows the result. `pocket mcp` exposes the same through
`runtime_start {project, editor: true}` and the `ui_*` tools. `tests/evidence/editor/session.md` is
one such session, and `tests/runtime_tests/editor_tests.cpp` does the same in-process.

## Not yet

Tabbed docking beyond resizable panes, errors marked in the script's text itself (they are listed
under it, from a watched save; nothing checks while typing), thumbnails for tile maps in the asset
list (images and models have them). Each is an editor-side feature on the existing commands. A gizmo
drag under a parent with shear (a non-uniform scale under a rotation) is close, not exact: moves and
turns are expressed in the parent's frame as a rotation and a scale.
