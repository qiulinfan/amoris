# The editor

Status: Draft 1, built (2026-10-04); the debugger and the edit loop verified against the real host
(section 10).
Charter: 2 (proof 4, the editor's modernity and completeness), 3.1 (one interface), 4.3 (debugging),
4.5 (the editor). Wire protocol: [host-protocol.md](host-protocol.md).

The editor is a web application in `editor/` (React 19, TypeScript 7, Vite 8, bun). It is a client
of the host protocol and nothing else: every panel reads and writes through the same command catalog
agents use over MCP, so the editor has no private channel (charter 3.1). The host serves the built
editor (`editor/dist`) at `/`; during development Vite serves it and proxies the host's endpoints.

Dependencies, each for one job: `dockview-react` (docking), `monaco-editor` (the script editor and
its TypeScript worker, bundled, no CDN), `lucide-react` (icons), `zustand` (stores), and the
`@fontsource-variable` Inter and JetBrains Mono fonts (bundled so the typography is the same
everywhere and offline).

## 1. Running

```bash
cd editor
bun install

# Against the mock host (no Rust needed): mock on 7879, Vite on 5173 proxying to it.
bun run dev:mock                  # open http://127.0.0.1:5173/

# Against the real host: `amoris serve <project>` serves editor/dist at http://127.0.0.1:7878/.
# For editor development against it, Vite proxies to AMORIS_HOST (default http://127.0.0.1:7878).
bun run dev
AMORIS_HOST=http://127.0.0.1:7900 bun run dev

bun run build                     # type check (tsc 7) + vite build -> editor/dist
VITE_AMORIS_EXPOSE=1 bun run build   # the same with window.amoris, for tools/gizmo-drag.ts
bun run typecheck                 # the editor and the mock/tools
bun run mock -- --port 7879 --no-agent --dist   # the mock alone; --dist serves editor/dist at /
bun run evidence -- --url http://127.0.0.1:5173/ # screenshots into out/bench-runs/editor (needs Chrome)
bun tools/viewport-loads.ts --url http://127.0.0.1:5173/ \
  --touch src/panels/viewport/ViewportPanel.tsx   # the viewport: loads, a remount, hot updates (5)
# The edit loop against a real host serving a scratch copy of samples/anim (10):
bun tools/edit-host.ts --url http://127.0.0.1:7878/
```

URL parameters: `?host=127.0.0.1:7879` connects to a host directly instead of the serving origin;
`?viewport=fallback` forces the fallback view, `?viewport=wasm` refuses it (to test the
integration). `EDITOR_PORT` and `MOCK_PORT` change the ports of `dev`/`dev:mock`.

The build puts its own files under `/_app/`, because `/assets/<path>` belongs to the host (the
project's assets, host-protocol.md 1).

### 1.1 The mock host

`editor/mock/` implements enough of the protocol for the whole editor to work before the Rust host
does: the catalog with params schemas (unknown fields refused with suggestions), the sailing sample
as a world (Sea, Breeze, Sloop, four crates, a sun, a camera and a buoy course under a parent) with
schemars-shaped engine schemas and registry-shaped project schemas, `world.edit` checked whole with
an undo history and `group` merging, Play as a fork of the edit world, a 60 Hz tick with a boat,
floating crates, a helmsman and the sample's three script systems (`muster`, `log`, `take_aboard`)
emitting events with causes, a snapshot ring with restore and rewind, the sample's scripts (read
from `samples/sailing/scripts`, kept in memory) with fake diagnostics, a debugger answering in the
real host's shapes (section 8.1: `bp<n>` breakpoints moved to the next line with code, conditions
and logpoints, stepping, the frames the host shows (inside an `each` callback, the callback with its
`locals` and the run's columns and `ctx` as its `closure`, and the `run` that called it) as JSON
previews, evaluation only while paused, `debug.set` lasting for the pause (a `const` refuses), data
breakpoints on fields script systems write, which stop on the statement after the write and on a
returned frame only when the write was the call's last, `debug.exceptions`), profile samples in the
host's shape, assets with thumbnails, and a simulated MCP agent whose edits land in the same undo
history. It binds 127.0.0.1 and refuses non-loopback `Origin`/`Host`, as the host does.

The mock is not the host: a form counts as working once it ran against `amoris serve` (section 10),
and what the editor sends the mock must answer as the host does. Where the editor's basic edits
read or write a form, the mock answers the host's as-built one (2026-11-12; section 8, items 8 and
12): the status's `pacing` (`"stepped"` or `{real_time: {speed}}`, one per world, and no `speed`),
`time.control {pause?, speed?, pacing?}` with the host's refusal of a speed outside (0, 1000],
`play.start {speed?, paused?}` (the fork runs at `speed`, 1 when absent; Stop shows the edit
world's pacing again), events with the world's `id` beside the stream's `seq`, `events.since` as
`{events, last, missed}` and `events.why` as `{event, causes, complete}`, `world.get` listing
`Name`, and the visual components Model, Light, Camera and Splat with the host's fields and
defaults. An enum value a schema lacks (`Light.kind: "Point"`) is refused with the host's code,
message and detail. The visual components' schemas are hand copies of the host's `world.schema`
answer; `tools/edit-host.ts` compares them with a real host's when it runs against one.

## 2. Layout

One window: a top bar, the dock, a status bar. The dock is dockview: every panel can be dragged to
another group or edge, floated, maximized, closed and reopened (View > Panels, the palette, or
Cmd/Ctrl+Alt+1–9); the layout is saved in `localStorage` and View > Reset Layout restores the
default, which follows Godot's: hierarchy over assets on the left; viewport and scripts in the
middle over console, events, timeline, profiler and debug; inspector over history and agent on the
right. A part of the window that fails to draw (the top bar, a panel, the status bar, an overlay)
shows the error in its own place with Try again, and the rest goes on working (`ui/Boundary.tsx`):
one value a panel cannot draw no longer leaves a blank page.

| Area | Contents |
|---|---|
| Top bar | Logo, project, menus File / Edit / View / Game / Debug / Help (built from the command registry, so every item shows its shortcut and state), the transport (Play/Stop, Pause, Step, tick, time, ticks per second, tick cost, speed), the mode badge (EDIT, PLAY, PAUSED, or HALTED for a poisoned world, whose Pause and Step are off: section 6; Play also tints the chrome green), the palette button and the connection pill |
| Status bar | Host and connection (with retry countdown; red when offline, amber while the debugger holds the game), project, error and warning counts (click: console), the pause location, agent activity, selection, entity count, world hash (a dash for a status without one), the viewport's renderer |
| Command palette | Cmd/Ctrl+K or Ctrl/Cmd+Shift+P. Fuzzy search over every command, every panel, every entity (`@` to narrow) and every method of `GET /api/catalog` (`call` to narrow). A host method opens a runner: its doc and params from the catalog schema, a JSON params line prefilled from the required fields (and the selection), Enter calls it and shows the result or the structured error |

## 3. Panels

| Panel | What it does | Protocol |
|---|---|---|
| Hierarchy | Virtualized tree (children by `Parent`), icons by kind (camera, light, boat, sea, wind, mesh, empty), search by name, `t:Component` or `#id`; click, Cmd/Ctrl-click toggles, Shift-click ranges; arrows move and fold; F2 or Enter renames inline; drag rows onto a row to reparent, onto empty space to unparent; drop an asset to place it (under the row); context menu (rename, duplicate, delete, frame, copy id, create child, add component, unparent, break when written); + creates an empty, primitives, lights or a camera | `world.tree`, `world.edit` (spawn, set `Name`, set/remove `Parent`, destroy) |
| Viewport | The renderer's canvas (section 5) with: orbit (right-drag or Alt+drag), pan (middle-drag or Shift+right-drag), zoom (wheel, pinch), click to pick (Shift/Cmd toggles), drag on empty space for rectangle selection, double-click to frame; tools Select/Move/Rotate/Scale (Q/W/E/R) with 3D gizmos (arrows with cones, plane handles, a screen handle; rotation rings with a view ring; scale cubes), World/Local (X), snapping with steps (hold Cmd/Ctrl to invert while dragging), grid (G), the axis gizmo (click an axis to look along it, the center for perspective/orthographic), camera menu (views, frame, reset), stats, the wind, selection labels; drop assets on the ground plane | `world.edit` with `group` while dragging, one undo entry on release; Escape takes the drag back (section 6) |
| Inspector | The selection's name and components, each a collapsible card generated from its JSON Schema with the doc as tooltip: numbers (drag the strip to scrub, Shift ×10, Alt ×0.1; arithmetic accepted; arrows step), vectors per axis, rotations as Euler degrees (YXZ) over the stored quaternion, colours with a picker (stored linear, shown sRGB) and alpha, enums as selects with per-variant docs, booleans as switches, entity references as pickers (searchable, or drop a hierarchy row), nullable fields, nested objects, lists, tagged unions (`{Cuboid: {...}}`), the rest as JSON. Engine-written fields are marked. Card menu: reset to defaults, copy as JSON, break when written, remove. Add Component lists the registry with docs; its search puts names before docs that only mention the text, so Enter on a typed name adds that component; a component without fields is added as `{}` and its card says so. A number field writes what was typed into it: left untouched, it writes nothing, whatever the value has become meanwhile. Several selected: each card edits every selected entity that has the component, and a field whose values differ among them shows as mixed (section 6.2) | `world.schema`, `world.get`, `world.edit` |
| Scripts | File list with problem counts, Monaco with tabs (one model per script so imports type-check), the `amoris` module's types (section 7), Cmd/Ctrl+S saves (`scripts.write` then `scripts.apply`), host diagnostics as markers beside the TypeScript worker's own, a breakpoint gutter (click or F9; conditional and unbound breakpoints drawn differently), the paused line, sticky scroll, minimap | `scripts.list/read/write/apply`, `debug.breakpoints.*` |
| Debug | Continue/Pause, Step Over/Into/Out (F5, F6, F10, F11, Shift+F11), pause on exceptions (off, uncaught, all); the pause reason, tick, system and TypeScript location, and for a data breakpoint the field, its value before and after and the statement that wrote it; call stack (click a frame to show its source and its scopes; a frame seen after `run` returned is marked), the frame's Local and Closure scopes as expandable trees whose leaf values can be set (double-click, type an expression, Enter: section 8.1), watch expressions re-evaluated on each pause and frame change, breakpoints with editable conditions (the host's list, read again on each pause: logpoints, MCP agents' and CDP clients' breakpoints shown as such), data breakpoints. Three columns when wide | `debug` events, `debug.state/continue/pause/step/eval/set/exceptions/breakpoints.*/watch/unwatch` |
| Console | The host's log with level filters and counts, search, follow, timestamps and ticks, a link to the source line; an input line evaluating in the game (on the selected frame while paused), with history; results are expandable trees | `log` events, `debug.eval` |
| Events | The event stream as a table (seq, tick, name, subject, data) with a filter; selecting one asks the host why: the cause chain, oldest first (built from `events.why`'s `{event, causes, complete}`; a line says when earlier causes have left the host's ring), then the data; subjects select their entity | `events` events, `events.since`, `events.why` |
| Timeline | Kept snapshots and event lanes (the most frequent kinds, and the selection's) on a tick ruler with the playhead; drag to scrub and release to rewind there; click a snapshot to restore it; back to the previous snapshot, pause, step | `snapshots.list/restore`, `debug.rewind`, `time.*` |
| Profiler | Per sample (4 a second), the host's CPU time per tick stacked by system and the viewport's frame time against the 60 Hz budget (hover for one sample), then over the last 3 s: the game thread's systems (each system's own time, `tick.other`, `loop.present`, `loop.publish`, `loop.after`) and the tick's phases with their largest tick, and the viewport's GPU passes; the tick and game-thread time, the frame interval, fps and GPU time; held ticks left out; freeze and clear | `profile` events, the viewport's `stats()` |
| Assets | Grid or list with thumbnails or kind icons, kind filters with counts, search; drag into the viewport or the hierarchy (or double-click) to place a model or a splat (the host lists the project's other files too, which are not placed); Import uploads files picked in the browser's file chooser or dropped on the panel (section 6.3); context menu (place, copy path) | `assets.list/upload`, `world.edit` spawn with `Model.mesh`, or `Splat.asset` for a splat |
| Agent | The live feed of agent activity grouped by session (call, result with success, message), sessions with their call counts and liveness, and a New session form (model, goal, tools) that waits for host support; copies the MCP endpoint command | `agent` events |
| History | The undo history as a timeline from "Opened scene", each edit another client made marked with its session (`mcp-1`, `api`); click an entry to undo or redo up to it; undo and redo buttons | `history` events, `history.undo/redo/list` |

## 4. Shortcuts

Mod is Cmd on macOS and Ctrl elsewhere. Keys marked global also work inside text fields and the
code editor. A select (an enum in the inspector) keeps its plain keys and leaves every key with Mod
to the editor, so Mod+Z right after choosing a value undoes the choice (`ownsKey` in
`src/commands/keys.ts`). Help > Keyboard Shortcuts (Mod+/) lists them all from the registry.

| Keys | Command |
|---|---|
| Mod+K, Mod+Shift+P (global) | Command palette |
| Mod+S (global) | Save the open script (in the Scripts panel) or the scene |
| Mod+Z, Mod+Shift+Z / Mod+Y | Undo, redo (the host's history) |
| Mod+D, Delete / Backspace, F2, Mod+A, Escape | Duplicate, delete, rename, select all, deselect (Escape first cancels a drag or closes a menu) |
| Q, W, E, R, X, G, F | Select, move, rotate, scale tools; world/local; grid; frame selection |
| Alt+7 / 1 / 3 / 5 | Top, front, right view; perspective/orthographic |
| Mod+P, Mod+Alt+P, Mod+Alt+., Shift+F5 (global) | Play/Stop, Pause/Resume, Step one tick, Stop |
| Mod+Shift+B (global) | Apply scripts |
| F5, F6, F9, F10, F11, Shift+F11 (global) | Continue (or Play), pause scripts, toggle breakpoint, step over, into, out |
| Mod+Alt+1…9 | Show a panel |
| Mod+/ | Keyboard shortcuts |

## 5. The viewport interface

The engine's renderer, compiled to wasm with wasm-bindgen (`crates/amoris-viewport`, built into
`web/viewport/pkg` by `tools/build_viewport.sh`), implements this behind
`web/viewport/amoris_web.js`; `editor/src/viewport/engine.ts` is the source of truth.

```ts
// Served by the host at /wasm/amoris_web.js (web/viewport; the wasm-bindgen output in pkg/ beside it).
export default function init(input?: unknown): Promise<unknown>;          // wasm-bindgen's init, run first
export function createViewport(canvas: HTMLCanvasElement, options: ViewportOptions): Viewport | Promise<Viewport>;

interface ViewportOptions {
  renderUrl: string;   // ws://127.0.0.1:7878/render: the render feed (host-protocol.md 5)
  assetsUrl: string;   // http://127.0.0.1:7878/assets/: project assets
}

interface Viewport {
  setCamera(camera: Camera): void;
  pick(x: number, y: number): Promise<PickHit | null>;   // CSS pixels from the canvas's top left
  resize(width: number, height: number, devicePixelRatio: number): void;  // CSS pixels + DPR
  setGizmo(gizmo: Gizmo | null): void;
  setSelection(entities: readonly number[], hovered: number | null): void;
  setOverlays(overlays: Overlays): void;
  stats?(): RenderStats | null;   // the frames drawn since the last call (read 4 times a second)
  dispose(): void;
}

interface RenderStats {          // every field past `frames` may be absent
  frames: number;                // animation frames since the last call
  fps?: number | null;           // from their average interval
  frame_ms?: number | null;      // average and largest interval between frames
  frame_max_ms?: number | null;
  render_ms?: number | null;     // the page's time in the draw call per frame (encode, submit)
  gpu_ms?: number | null;        // the wasm renderer's last frame on the GPU (timestamp queries)
  passes?: { pass: string; ms: number }[];
  renderer?: Record<string, unknown>;   // the rest of the wasm renderer's stats (backend, draws)
}

interface Camera {
  position: [number, number, number];
  target: [number, number, number];
  up: [number, number, number];                  // [0, 1, 0]
  fov_y_deg: number;
  near: number;
  far: number;
  projection: "perspective" | "orthographic";
  ortho_height: number;                         // world units, bottom to top, when orthographic
}

interface PickHit { entity: number; position?: [number, number, number] }   // world point if known

interface Gizmo {
  mode: "translate" | "rotate" | "scale";
  space: "world" | "local";
  position: [number, number, number];
  rotation: [number, number, number, number];
  size_px: number;
  hovered: GizmoHandle | null;
  active: GizmoHandle | null;
  shapes: GizmoShape[];                         // ready to draw, already sized for the camera
}
type GizmoHandle = "x" | "y" | "z" | "xy" | "yz" | "xz" | "xyz";
interface GizmoShape {
  kind: "line" | "polygon";                     // polyline, or filled convex polygon
  points: [number, number, number][];           // world space
  color: [number, number, number, number];      // sRGB 0..1 with alpha
  width_px?: number;                            // line width (and polygon outline width)
  outline?: [number, number, number, number];   // polygon outline colour
  handle?: GizmoHandle;
}

interface Overlays { grid: boolean; axes: boolean }
```

Conventions: right-handed, +y up, metres, quaternions `[x, y, z, w]`; the projection is the ordinary
perspective (or orthographic) of `Camera` with the canvas's aspect, which
`editor/src/viewport/ camera.ts` (`View`) reproduces for hit tests and drops, so the two must agree.
The renderer draws the world from the render feed; the editor owns the camera (orbit), the
selection, the gizmo geometry and its hit testing and drag math (`viewport/gizmo.ts`), so a renderer
only draws `shapes` on top of the scene, in order (they are sorted far to near), and outlines the
selection. `pick` should come from the entity id buffer (charter 4.4).

Loading (`loadViewport`): `HEAD /wasm/amoris_web.js` (the host serves `web/viewport`, found beside
the working directory or the executable, or `AMORIS_WEB_VIEWPORT`); if it is JavaScript, import it,
run `default`, call `createViewport`. When it is missing (the mock serves none), or on any error (no
WebGPU), the editor uses its fallback (`viewport/fallback/renderer.ts`): a Canvas 2D view of the
world data the editor already holds (flat-shaded stand-ins chosen from `Model.mesh`, `Boat`, the
`Collider` shape, light and camera icons, the animated sea from `Sea.waves`, a fading grid), which
implements the same interface. The status bar and the viewport badge say which renderer runs.

One wasm instance per page (2026-10-15): `amoris_web.js` runs wasm-bindgen's init once and hands
every caller the same promise. Before, under `bun run dev` React's StrictMode mounted the viewport
panel twice, both mounts called `default` before the first instantiation had finished, and the page
made two instances of the module (two fetches of `amoris_viewport_bg.wasm`). The generated glue
keeps one instance, so the first viewport's closures and frees ran on the second's memory: almost
every development load threw "closure invoked recursively or after being dropped", then often
panicked in dlmalloc (`psize >= size + min_overhead`, then `RuntimeError: unreachable`), and some
drew nothing. A production build mounts once and was not affected.

The canvas's one WebGPU context belongs to the viewport that configured it last, and every
viewport configures it when it is created. StrictMode's two mounts both create one, in either
order (the editor awaits a request and an import before `createViewport`), so the mount the editor
disposes could configure last. The kept viewport's frames then used that device's texture, the GPU
rejected every one (`[TextureView] ... cannot be used with [Device "pocket"]`, which the renderer
logs through `console.log` as `[amoris ERROR]`) and the canvas stayed black, while the stats still
counted frames. Disposing a viewport now has the newest viewport left on its canvas configure the
context again (`amoris_web.js`). `dispose` frees the renderer's wasm memory at once; its GPU
objects still go with the browser's collector (wgpu's WebGPU backend destroys nothing on drop).

`bun tools/viewport-loads.ts` loads the editor in headless Chrome several times, resets the layout
(the viewport panel unmounts and mounts: a new viewport on the same instance) and hot-updates a
source file. A phase fails on an exception, a console error or a renderer `[amoris ERROR]` line (of
its own document: what the replaced page logs while a load navigates does not count), and unless the
renderer's stats count frames and entities and a screenshot of the viewport's canvas shows the world
(at most 80% of its pixels one color: a canvas that presents nothing is black, 95 to 99% one color
under the editor's overlays, the sailing sample 16%). Against `bun run dev` and `amoris serve`
(quiet re-measurement, 2026-10-16 04:27 to 04:38: every other track finished, the bench lock held,
CPU 6 to 8% busy, the RTX 5060 idle): master's module passed 0 of 6 phases (two wasm fetches per
load, the closure error 0.4 to 2.2 s after each load, the allocator's panic 5 to 15 s after it in 3
of 5 loads, no viewport attached in one load and after the layout reset); the single instance alone
passed 7 of 11, its kept viewport black with 2,800 to 2,900 validation errors in 4 of 10 loads (the
first version of this check, blind to both, had passed it 6 of 6); with the handoff, 13 of 13 (ten
loads, a layout reset, two hot updates), 11 of 11 with a build of the module that also logged each
creation and disposal (the disposed mount was created second in 5 of its 10 loads), and 6 of 6
against the production build the host serves. Which mount configures last is a race, and how often
the single instance lost it changed from run to run: 3 of 10 loads in the first runs (2026-10-15,
other tracks running, master 0 of 6), 7 of 10 in a run while a game ran on the machine (2026-10-16
03:33, master 1 of 6: one load threw nothing in its 12 s, the handoff 13 of 13 again), 4 of 10 here.
A hot update of the panel's module or the dock leaves the viewport panel mounted (Fast Refresh does
not run its effects again here), so only a load or a layout reset makes a new viewport; the
hot-update phases check only that the page keeps drawing.

Timings (2026-10-14): the viewport panel reads `stats()` four times a second into
`state/render.ts`. The wasm module (`web/viewport/amoris_web.js`) keeps the renderer's per-frame
stats JSON (parsed only when read, so fields the renderer adds pass through as `renderer`) and the
page's frame counters (`viewport/meter.ts` in the fallback); `gpu_ms` and `passes` come from the
renderer's timestamp queries and are absent without them. The stats overlay shows the viewport's fps
and GPU time beside the host's ticks per second, and the profiler adds them to each host `profile`
sample (`state/profile.ts`): the host has no frame, so the frame and GPU columns are this editor's
own viewport.

## 6. Editing

Every change is a `world.edit`: one transaction, one labelled entry in the host's undo history,
shared with agents (each entry names its source and the server its session, server.md 3.1). The
editor never changes its copy of the world itself; it refetches what `world.changed` names
(coalesced, a few requests at a time), and the tree and every entity on a `{reset: true}` push
(Play, Stop or a restore replaced the world), with one exception: while a drag (gizmo, scrubbed
number) is in flight it shows the dragged values as a local preview. (Until 2026-10-09 resets were
ignored, and the old world's values stayed shown until a later diff named them: after a Stop that
came 0.1 s after the click, the inspector kept the Play world's Log.)

Drags stream: the changes of each input event go as one `world.edit` with the drag's `group`, at
most one in flight: an event's changes are sent at once when no edit of the drag waits for its
answer, and what comes while one does is sent, merged, the moment it is answered (2026-10-15,
Pioneer; before, at most one edit every 60 ms, each after the last one's answer). So a drag sends
about once per pointer frame on a host that answers within a frame, and never faster than the host
answers. (Measured quietly, the host answered a client without a page in 0.7 ms, and the editor page
saw the answer 5.2 ms after sending: docs/bench/gizmo.md 6.) The host merges consecutive edits
of one group into one undo entry, and release sends the final value after the last edit's answer.
The viewport draws the object where the host has it, so during a drag it follows the gizmo by the
round trip and the render feed's next frame (docs/bench/gizmo.md). Escape takes the drag back with
`history.undo {group}`, which the host performs only when the next entry to undo is the whole drag
(server.md 3.1): the drag then moves to the redo stack, so Redo brings it back. When another
client's edit came after the drag or in the middle of it (which splits the drag into two entries),
the host refuses and changes nothing, and an edit (`Cancel <label>`) writes the dragged fields back
to their values from before the drag; the other edit stays. Asking the host in one call means an
edit another client makes while Escape is on its way is never the one undone. When the catalog's
`world.edit` params schema has no `group` (the real host before 2026-10-15), the editor previews
locally and sends one edit on release instead.

Mapping: rename is `set` of `Name` with a string value; reparent is `set` `Parent {parent}` (or
`remove` it) and needs a `Parent` component in the registry; duplicate is `world.get` then `spawn`
with the components (without `Name`) and the next free name (`Crate4` → `Crate5`, `Sloop` →
`Sloop 2`); create spawns `Transform` plus, over the schemas' defaults, `Model {mesh}` with the
engine's own primitive name (`cube`, `sphere`, `cylinder` or `plane`: names of
`amoris_assets::PRIMITIVES`, which a test pins), `Light {kind}` with the kind in the registry's own
spelling (read from `world.schema`, never a literal: `enumValue` in `src/host/schema.ts`), or
`Camera`; an entry is disabled when the registry lacks the component or the kind, so the menu
offers nothing the host would refuse. Placing an asset spawns `Transform` at the ground point under
the drop and `Model {mesh: <asset path>}`, or `Splat {asset: <asset path>}` for an asset whose kind
is `splat` (as `assets.list` says, else by its extension), under the next free name (`Hero`, then
`Hero 2`). An asset the host lists with another kind (`assets.list` holds the project's every
file: `project.toml`, scripts, textures) is not placed, by a double-click, the menu or a drop: as
a `Model` it would be an entity that draws nothing, which the host takes without a word. Add
Component sets the component's schema defaults (`{}` for a component without fields); scale
handles edit
`Transform.scale` when it exists, else `Model.scale`. Enum values are compared in any spelling
when read (`sameVariant`: a light's icon) and sent in the registry's.

The speed is the pacing of the world shown, and each world has its own (server.md 3.3): the
transport reads it from the status's `pacing` (`speedOf` in `src/host/api.ts`; `stepped` when the
world is stepped) and sets it with `time.control {speed}`. A fork starts at 1 whatever the edit
world's speed, so Play sends `play.start {speed}` with the speed shown; after Stop the control
shows the edit world's speed again, and a speed chosen during Play stays the fork's.

A poisoned world (a tick that stopped halfway, threads.md 8) runs no further tick: the host pauses
it, refuses a step and halts again on a resume. The editor reads it from the status (`haltedOf` in
`src/host/api.ts`: `halted`, or the game thread's `poisoned`): the badge reads HALTED, Pause,
Resume and Step are off (their keys say why instead of asking), a toast and a Console line say
where it halted beside the host's own `log` line for the tick's error, and Stop (in Play) or a
restore is the way out. The game thread's status of such a world has no hash (`world_hash: null`).

### 6.1 Water (Pioneer, 2026-10-23)

Terrain and water are authored as their components declare them (water.md 65); the editor's part is
placing them. The create menu's Terrain spawns `Transform` and `Terrain` (128 x 128 cells of half
a metre, base 2 m, a little noise) centred on the ground under the click; a terrain's own menu and
the create menu's Water over Terrain spawn a resident `WaterRegion` over the selected terrain (or
the first one), as large as fits it in whole tiles of 16 columns, its bed `{Terrain: {terrain}}`,
dry. Two viewport tools join select, move, rotate and scale (`state/viewport.ts`), each enabled
while one entity holding its component is selected:

- **River** (a terrain): each click adds the ground point under it (the renderer's pick: its
  terrain hit marches the heights) to the course the viewport shows as a dashed line; Enter sets
  `Terrain.features` with a `Channel` appended, its path in the terrain's frame 1.5 m under the
  points, 4 m wide with 3 m banks (`actions/water.ts` `RIVER`); Escape drops the course.
- **Pour** (a water region): a click sets `WaterRegion.fill` with a pour appended at the point,
  its level a metre over the ground there.

Each is one `world.edit` with its label (Create Terrain, Draw River, Create Water, Pour Water).
`bun tools/water-authoring.ts --url <host>` drives all of it in headless Chrome with real input
against `amoris serve` (the build with `VITE_AMORIS_EXPOSE=1`), checks every edit on the host and
saves the edit world and Play.

### 6.2 Several entities selected

With several entities selected the inspector shows the primary one's components (the last clicked;
the head says `editing N selected`), and each card works on every selected entity that has its
component: a card on only some of them says `2 of 3`. A field shows one value when all of them
hold the same and a dash when they differ (`src/panels/inspector/multi.ts`): a number or text
field reads `—`, a select `—`, a switch stands half-way, an entity reference reads `—`. Vectors
say it per axis, a colour for its red, green and blue together and for its alpha, a rotation per
Euler angle, and a nested object per field; a list whose items differ reads as a note and is
edited one entity at a time.

An edit sets the edited value on every one of them and keeps what each holds besides: typing a
y sets each entity's y and keeps its own x and z, an Euler angle keeps each rotation's other two,
a colour keeps each alpha, a nested field keeps its siblings, and a variant chosen keeps the
payload of an entity already of that variant. Each entity's new value is the edit applied to its
own (an `Update`, old value to new); all of them go in one `world.edit`, one undo entry (`Set
Transform.position on 2 entities`), and a drag streams them as one group (section 6). A mixed
field takes any value typed, the primary's own too.

### 6.3 Import by upload

Import (the panel's button, File > Import Asset…) opens the browser's file chooser, and files
dropped on the Assets panel from the system are imported the same way (`src/actions/assets.ts`).
Each file is uploaded into the project with `assets.upload` (server.md 3): its bytes in base64
parts of at most 1 MiB, in order from offset 0, the last part `done`; the host writes the parts to
a staging file under `.amoris/uploads/` and moves it into place at the last one, so a file is
listed only once whole. A file goes into the folder of its kind by its extension: `models/` (with
`.bin`, a glTF's buffers), `textures/`, `splats/`, `audio/`, `scripts/`, else `files/`. When the
project already has that path (`assets.exists`), a dialog offers it: kept, the file is uploaded
again with `overwrite`; another path, there. After the uploads the panel asks `assets.list` again,
and a toast names each file, its kind and size. Files are not the world: an import adds no undo
entry. Import took a path the host reads (`assets.import`), which the host never built; it and the
context menu's Reimport are gone (2026-11-12, section 8 item 15).

## 7. Schema-driven fields and script types

`world.schema` answers `[{name, origin, version, doc, schema}]` (amoris-sim's `ComponentInfo`).
`src/host/schema.ts` reads both producers: schemars for engine components (`$ref`/`$defs`,
fixed-size number arrays, `oneOf` of documented `const`s, externally tagged unions, `EntityId` by
`$ref`) and the registry for project components (`{x, y, z}` objects, `enum`, nullable integers
bounded by 2^53 − 1 for entities). Colours and rotations are recognised by name and doc (`color`,
"RGBA", "quaternion"), since the schema cannot say; unknown shapes fall back to a JSON editor, never
to nothing.

Monaco's TypeScript worker loads the project's declarations from the host
(`scripts.types {text: true, tsconfig: false}`: `amoris.d.ts` and the generated `components.d.ts`,
the files `tsc` checks in `scripts.check`, docs/sdk.md; connecting never writes a `tsconfig.json`
into the project) on connect and after the editor's own apply. Its compiler options are fixed in
`monaco.ts` to mirror `amoris_script::types::TSCONFIG` (strict, `es2023`, isolated modules); it does
not read the project's `tsconfig.json`, and resolves `amoris` as `node_modules/amoris`
(`moduleResolution` `NodeJs`, where the project's file says `bundler`). Until the declarations
arrive (or against the mock, which lacks `scripts.types`) it uses the SDK the engine embeds
(`crates/amoris-script/src/prelude/amoris.d.ts`, bundled at build time) with loosely typed
components. The worker is monaco-editor 0.57's TypeScript 5.9 (JavaScript), not `tsc` 7: on the same
declarations the two reported the same codes and places for the 15 mistakes of
`tools/sdk_mistakes.ts.txt` (`tools/sdk_check.ts`, section 10), but they may differ in corners. The
status bar counts the shown file's markers, the worker's and the host's; the host's `tsc` findings
are left out of the markers once the worker has the project's declarations, since the worker reports
them live.

## 8. What the editor needs from the host

Beyond host-protocol.md Draft 1, the editor (and the mock) assume the following; the host should
adopt them or the editor should change. The mock is not the host: an item is settled once the form
ran against `amoris serve` (section 10), and where the two differed the mock was wrong.

1. `world.edit` takes `group?: string`: consecutive edits with the same group merge into one undo
   entry (a drag). Detected from the catalog's params schema. Adopted 2026-10-15.
2. `set` of `Name` takes the name as a string `value`; `world.get` may list `Name` among components
   (the editor ignores it there). Adopted 2026-11-11 (server.md 3.1, Names): the host renames from
   the string, as one undo entry. A name over 64 bytes of UTF-8 is refused with
   `sim.name_too_long`, which the editor shows as an error toast: the inspector's name field stops
   at 64 characters, not bytes, so a name of 22 CJK characters reaches the host and is refused
   there. A `remove` of `Name`, which the editor never sends, leaves an entity unnamed (`name:
   null`).
3. The hierarchy is the `Parent {parent: EntityId}` component, and `world.tree` nests by it.
   Adopted 2026-11-12 (server.md 3 and 3.1). Two things the host does that the mock does not: a
   destroy takes the entities under the destroyed one (destroying children first, as the editor
   sends them, still works; destroying a parent and then its child in one call is refused), and
   `world.tree` nests 48 levels at most, an entity whose children were left out carrying their
   number in `more` (`root` reads on from it).
4. The debugger is the host's as-built `debug.*` (section 8.1), not an editor-specific form.
5. Agent edits are marked in the history. Adopted 2026-10-15 with item 1 (server.md 3.1): each
   entry names its `source` (and the server its `session`); a host without sources marks agent
   edits by an `agent:` label prefix. The editor changed for the order instead: the host lists
   `redo` most recent last, as Draft 1 has it (the editor had assumed the next to redo first), and
   the history store reverses it.
6. `agent` events may carry `ok` (a result's success) and `ts` (ms since the epoch).
7. `scripts.read` answers the text (a string) or `{text}`; diagnostics are `{file?, line, column,
   end_line?, end_column?, severity: "error" | "warning" | "info" | "hint", message, code?}`.
8. `snapshots.list` answers `[{tick, t_s?, hash?}]` on the mock; the host answers `{every, keep,
   snapshots: [{tick, world_hash}]}`, which the editor unwraps, so against it the timeline's
   tooltip has no hash to show. The editor changed for the rest (2026-11-12), which only the mock
   had answered: `events.why` is the host's `{event, causes, complete}`, the
   causes nearest first, which `causeChain` (`src/host/api.ts`) turns into the chain the panel
   shows, oldest first and ending with the event (it had read a `chain` field, so no event showed
   a cause); events carry `id`, the world's event number, which `cause` names, beside the
   stream's `seq`; the status carries `pacing`, `"stepped"` or `{real_time: {speed}}`, and no
   `speed` (the transport had read `status.speed` and always showed 1). On connect the editor asks
   `events.since {limit: 5000}` with no `seq`, which answers the newest events: with a `seq` (it
   had sent 0) the host answers the first `limit` after it, the oldest of a ring of 65,536.
9. `GET /api/catalog` answers an array of commands or `{commands}`; `kind` in any case.
10. The host serves the browser viewport (`web/viewport`, amoris-viewport's wasm-bindgen output)
    under `/wasm/` (section 5).
11. `profile` pushes the host's CPU time per tick by system and phase (host-protocol.md 3); the
    editor adds its viewport's frame and GPU passes. `subscribe` is all or nothing: the editor asks
    for the topics the host pushes (an earlier `pick` topic was never pushed; picking is the
    viewport's), and when the host refuses one (`/topics/<i>`: an editor newer than its host) the
    client drops it, warns on the console and subscribes the rest. The transport shows the host's
    `tps` (ticks per second; `fps` repeats it for older editors), the stats overlay the viewport's
    own fps.
12. The visual components are the host's (amoris-assets' `visual.rs`), and the editor changed to
    them (2026-11-12). Until then Create > Point Light was refused (`'kind' must be one of
    directional, point, spot; got 'Point'`), Create > Cube spawned `Model.mesh: "primitive:cube"`,
    which imports nothing and draws nothing (the names are `cube`, `sphere`, `plane`, `cylinder`,
    `capsule`, `cone`, `torus`), a splat was placed as a `Model`, and a directional light had a
    bulb for an icon; section 6 has the forms sent now. `play.start` takes `{speed?, paused?}`
    and starts the fork at `speed`, 1 when absent: the editor sends the speed it shows.
13. The `status` push says the pacing and the pause of the game now. Adopted 2026-11-12
    (threads.md 4.3): the host publishes the time status after a `time.control` that changes it
    when no tick follows. Until then the push repeated the last publication's, so a second after
    the speed was set to 2 in Edit the transport showed 1 again (and Play then ran at 1), and a
    Play world paused from the transport showed PLAY, whose Pause button then paused again instead
    of resuming. A refused `time.control` changes nothing (2026-11-12): the host checks `speed`
    and `pacing` before it applies either, as the mock did; before, a good `speed` beside a
    refused `pacing` stayed applied, and with this item's publication the transport would have
    shown it after the error.
14. A poisoned world's status (2026-11-12). The status the game thread answers (`time.control`,
    `play.start`, `play.stop`, a restore) carries `halted: true`, the failed tick in `poisoned`
    and `world_hash: null`; the pushed one `halted: true` and the last publication's hash. The
    editor had typed the hash as a string and cut it for the status bar, so the first such answer
    (a click on Resume, a speed, a reload) threw while drawing and left a blank page until someone
    stopped Play from outside; nothing had said the world was poisoned either. A Play world is
    poisoned from the inspector alone: a `Vessel` beside a `Collider` at its schema defaults (a
    ball of radius 0) fails the first tick with `water.vessel_invalid`. The editor changed (section
    6); the host now logs the error of the tick that poisoned the world on `log` (threads.md 8),
    which the Console shows. The mock's worlds never halt; its status says `halted: false`.
15. Files into the project (2026-11-12): `assets.upload {path, data, offset?, done?, overwrite?}`
    (server.md 3) in place of `assets.import {path}`, which only the mock answered and which needed
    the file on the host's disk. The host writes uploads itself, natively (amoris-server, no game
    thread), and refuses a path out of the project or into a dot directory, a part that does not
    start where the received ones end (`assets.upload_offset`), more than 1 MiB a part and an
    existing file without `overwrite` (`assets.exists`); the mock answers the same forms and codes
    (`mock/assets.ts`).

### 8.1 The debugger

The editor drives the script debugger through the host's `debug.*` methods over `/ws`: they are
amoris-debug's agents' API ([debugger.md](debugger.md) 7), the same core Chrome DevTools and VS Code
reach over CDP, and the `debug` topic pushes its pauses and resumes. `src/host/protocol.ts` types
the wire shapes (`HostDebugState`, `HostFrame`, `HostVariable`, `HostBreakpoint`, `HostDataWatch`,
`HostEvalResult`) and `src/host/api.ts` normalizes them, in one place, into what the panels show:

- `debug.state` and the `debug` event: `{state, reason, tick, system, location: {file, line,
  column, generated?}, frames: [{frame, function, location, locals, closure, returned}],
  hit_breakpoints, exception?, data?}`, and from `debug.state` also `{attached, instrumented,
  exceptions, breakpoints, watches, waiting_for_debugger, cdp}`. Frames become `{id, name, file,
  line, column, returned, scopes: [Local, Closure]}` where `id` is the frame's index (what
  `debug.eval`'s `frame` takes; `frame` on the wire is the engine's level). A frame that `returned`
  (a data breakpoint seen when `run` returned) has a position and no scopes.
- Variables are `{name, type, value, description?}` with `value` a JSON preview (objects to three
  levels, 48 entries; `undefined` is `null` at the top and the text `"undefined"` below it; NaN and
  the infinities as text; objects past the depth as `"[Description]"`; functions as
  `"[function name]"`). The editor turns each into a tree (`Float64Array(1) [0.42]`, `{speed: …}`)
  and gives every leaf reached by names and indices a `path` (`l.distance[0]`).
- Setting a value: a variable of the frame (an argument, a local, a closure variable: `r`, `e`) is
  set with `debug.set {name, value: "<text>", frame}`; a value inside one (`l.distance[0]`) is
  assigned with `debug.eval {expr: "<path> = (<text>)", frame}`. An assignment to a variable through
  `debug.eval` would not reach the frame (QuickJS-ng evaluates on a copy of each local no closure
  captured, debugger.md 7); one through an object writes the object, and a query column's element
  is the component the call commits, so setting `l.distance[0]` changes the boat's `Log.distance`.
  `const` variables refuse (`debug.set_failed`). The editor then reads the path back and reports a
  failure when it does not read what was assigned (a typed column converts: an `Int32Array` keeps 1
  of 1.5), and evaluates the root variable again to show it (the pause's frames are captured once,
  at the stop). Either taints the run from that tick (debugger.md 5).
- Breakpoints: `debug.breakpoints.set {file, line, condition?, log?}` answers
  `{id: "bp<n>", file, line, verified, locations}` with `line` where it binds (the next line with
  code); the host has no update, so a new condition sets a new breakpoint (a logpoint keeps its
  `log`) and clears the old one. `debug.breakpoints.list` and `debug.state.breakpoints` list every
  frontend's (`owner` `agent` or `cdp`, `target`, `condition`, `log`, `locations`);
  `debug.breakpoints.clear {id}` removes any of them and `{}` every one set through `debug.*` (the
  editor's and MCP agents'). The host pushes no event when a breakpoint is set or cleared, so the
  editor reads the list on (re)connection, on each pause, after Clear All, and when a clear answers
  `debug.unknown_breakpoint` (another frontend cleared it: the editor drops its entry). A restarted
  host has none: the editor sets its own again on reconnect, and the pause-on-exceptions mode.
- `debug.watch {entity: id, component, field?}` answers `{id: "w<n>", entity, component, field}`;
  `debug.unwatch {id}`; `debug.exceptions {mode}` answers `{mode}`.
- `debug.eval` answers `{type, value, description}` and only while paused (`debug.not_paused`
  otherwise), so watch expressions and the console evaluate on the selected frame of a pause.
- `debug.rewind {tick}` (the timeline) answers `{restored, tick}`: amoris-app's bridge restores the
  kept snapshot at or before `tick` and steps to it.
- Play and Step debug the Play world: the host's game thread hands the script debugger to Play's
  fork and back on Stop, so breakpoints set in Edit hit in Play; `time.step` in Edit runs the edit
  world's scripts under the debugger too.
- Stop while paused: the game thread answers `play.stop` only between ticks (debugger.md 8), and the
  host ends Play wherever the debugger holds it: it has the debugger pass over the held tick's pause
  and any later one until the Stop lands (server.md 3.4, 2026-10-09). The editor sends `play.stop`
  and nothing else; until then it continued each pause itself, which stopped working when the host
  began refusing calls made while held. The timeline does not poll `snapshots.list` while paused.
- While paused, the host answers reads from the last publication and refuses calls that need the
  game thread at once with `debug.paused` (server.md 3.4): an inspector edit, a snapshot restore,
  Step, a script apply or a project save shows the refusal as an error toast, which says where the
  game stands. Pause (`time.control {pause: true}`) is queued instead, shown as an information
  toast: after Continue, Play rests at the end of the held tick, where every call works.
- Stop and scripts: Stop returns to the edit world exactly as it was (server.md 3.3), with the
  bundle it ran, while scripts saved during Play are on disk. Stop reads the Play world's bundle
  (`time.control {}`, answered while paused too) before `play.stop`; when it differs from the edit
  world's, the editor runs `scripts.apply` again, so the edit world and the next Play run the text
  the editor shows and breakpoints bind to its lines. Other clients that apply scripts during Play
  apply them again after Stop themselves.
- Debug > Copy Chrome DevTools URL copies `debug.state.cdp.devtools`, the host's CDP endpoint
  (amoris-debug's own port, 9229 by default; absent when the port was taken). The host does not
  serve `/devtools` or `/json` on its own port, so the Vite dev server does not proxy them.

## 9. Code map

| Path | Role |
|---|---|
| `src/host/` | `protocol.ts` (wire types, method params and results), `client.ts` (WebSocket with ids, events, reconnect; catalog), `api.ts` (typed wrappers, the connection), `sync.ts` (stores from host events), `schema.ts` (JSON Schema to field kinds) |
| `src/state/` | One zustand store per domain: world, selection, session, history, logs, events, profile, the viewport's render timings, agent, debug, scripts, assets, viewport settings, UI |
| `src/actions/` | Edits (`world.ts`, `LiveEdit` for drags), transport, debugger, scripts, error reporting |
| `src/commands/` | The command registry, key handling, the built-in commands |
| `src/layout/` | The panel registry, the dock and its default layout, a handle for other code |
| `src/shell/` | Top bar, menus, transport, status bar, palette, dialogs |
| `src/ui/` | Primitives: buttons, menus and context menus, tree, fields, split pane, toolbar, tooltip, toasts, variable tree, icons, formatting, the error boundary |
| `src/viewport/` | The renderer interface, camera, math, gizmo, the frame meter, the fallback renderer |
| `src/panels/<panel>/` | One module per panel (and its CSS) |
| `mock/` | The mock host (section 1.1) |
| `test/` | bun unit tests (`bun test`): history, live edits, subscriptions, the profile, water authoring, the desktop guards, arithmetic, the host's forms (`host-forms`: the cause chain, the pacing, enum spellings, the component search, and the mock answering them as the host does), what Create, Place and Duplicate send (`create`), and a poisoned world's status with the shell drawn from it, the newest events on connect, a component without fields and a select's keys (`halted`, the editor's client answered by the mock over the wire), mixed values and what an edit of several entities makes of each (`multi`), what Import sends and the mock's answers to it (`upload`) |
| `tools/` | `capture.ts` (evidence of every panel against the mock), `debug-host.ts` (the debugger against the real host), `rename-host.ts` (the rename against the real host, section 8 item 2), `edit-host.ts` (the edit loop against the real host, or the mock: section 10), `sdk_check.ts` (the script editor's types against the real host), `profiler-capture.ts` (the profiler against the real host, docs/bench/profiler.md), `water-authoring.ts` (the water tools against the real host, 6.1), `viewport-loads.ts` (the viewport's loads, remounts and hot updates, 5), `gizmo-drag.ts` (live gizmo drags, docs/bench/gizmo.md), `serve-clients.ts` (clients held on a real host while its game thread is profiled, docs/bench/profiler.md); the pages are driven in headless Chrome over CDP (`cdp.ts`, `chrome.ts`): `launchChrome` starts the browser `chrome.ts` finds with a temporary profile of its own, on a CDP port Chrome picks (read from the profile's `DevToolsActivePort`) or on `--chrome-port`, which it refuses when something already listens there; it hands out a page only once the browser answering on the port is the process it started, and its `close` ends that process alone |

## 10. Evidence

`out/bench-runs/editor/` holds screenshots of an edit, play and debug session against the mock,
taken by `tools/capture.ts` (headless Chrome, 1600×1000 at 2×): the full window in Edit, Play and
paused in the debugger, and each panel (viewport with a live gizmo drag and the rotate gizmo,
inspector, Add Component, hierarchy context menu, Edit menu, palette and a host method call, assets,
events with a cause chain, timeline, profiler, console, agent, debug, scripts, history, shortcuts).

The debugger against the real host: `amoris serve` on a copy of `samples/sailing` serving the built
editor, driven by `tools/debug-host.ts` in headless Chrome with real input only (clicks, keys,
typing; the exceptions select excepted, step 7), 2026-10-04; `debug-host-*.png` and the driver's log
`debug-host.txt` (exit 0, every step). Run again on 2026-10-09 on Windows against the host whose
Stop crosses a pause (server.md 3.4), with the editor built by Vite under Node and the driver run by
Node 25 (`--experimental-transform-types`, a preload defining `Bun.sleep`, `Bun.write` and
`Bun.spawn`; no Bun on that machine): exit 0, every step, Edit mode 0.1 s after Stop and the edit
world on the edited bundle; the log and steps 9 and 10 downscaled are in
`out/bench-runs/agentdebug/`. The driver pressed Cmd+S for Mod+S everywhere; it now presses Ctrl+S
off macOS.

| Screenshot | Shows |
|---|---|
| `debug-host-1-breakpoint.png` | Edit mode; rules.ts open in the Scripts panel with a breakpoint set by clicking the gutter of line 27 (in the `log` system); the Debug panel lists `rules.ts:27` |
| `debug-host-2-paused.png` | After Play: the fork stopped at tick 1 on rules.ts:27 (the line highlighted, the banner "Paused on breakpoint, tick 1, system log, scripts/rules.ts:27:13", the status bar "Paused at rules.ts:27"); the call stack with TypeScript locations, `(anonymous)` (the `each` callback) at rules.ts:27 and `run` at rules.ts:26; Local `r = 0`, `e = 3`, `speed` and `set` undefined (not yet initialized) |
| `debug-host-3-scopes-watches.png` | The Debug panel: Closure `b` and `l` (open), `ctx` opened (functions as `ƒ emit()`); watches `b.speed[r] * 3.6 = 0`, `l.distance[r] = 0`, `ctx.tick = 1` evaluated on the paused frame |
| `debug-host-4-stepped.png` | F10: stopped at rules.ts:28, `speed = 0` in Local, the watches evaluated again |
| `debug-host-5-set-value.png` | `l.distance[0]` set to 1000 by double-clicking its value in the Variables tree and typing; `distance` shows `[1000]` and the watch `l.distance[r]` 1000 |
| `debug-host-5b-set-local.png` | The argument `r` set to 1 by double-clicking its value in Local (`debug.set`): Local shows `r = 1` and the watches, evaluated on the frame again, read `l.distance[r] = undefined` and `b.speed[r] * 3.6 = NaN`; `speed` (a `const`) refused, the toast "Set speed: debug.set_failed, speed is a constant." The driver then sets `r` back to 0 and the watch reads 1000 again |
| `debug-host-6-next-tick.png` | After F10 over line 28 and F5: tick 2 stops at rules.ts:27 again and `l.distance` is `[1000]`, the value tick 1 committed (the boat's speed was 0 then); `b.speed[r] * 3.6 = -0.9028` |
| `debug-host-7-data-breakpoint.png` | The line breakpoint removed, the Sloop's Log card in the inspector, Break When Written, `top_speed`; Exceptions set to uncaught (the host's `debug.state` answered `uncaught`); F5: "Paused on data breakpoint, tick 2, rules.ts:30:13, Sloop.Log.top_speed: 0 → 0.250767 (written at rules.ts:29)" |
| `debug-host-8-running.png` | The data breakpoint removed and F5: the game runs (tick 477), the Viewport tab drawing it with WebGPU; the inspector's Log card shows Distance 1019 and Top speed 3.32 |
| `debug-host-9-edited-script.png` | While it runs: `const knots = speed * 1.944;` typed as a new line 28 of rules.ts, Mod+S (`scripts.write`, then `scripts.apply`: the toast "Scripts applied, bundle 4685e123… hot-swapped, no errors"), a breakpoint on line 29: tick 655 stops there with `knots = 2.4262` in Local |
| `debug-host-10-stopped.png` | Stop clicked while paused there: Edit mode 0.1 s later (Stop continued the pause first); the edit world came back with its old bundle, so the editor applied the scripts on disk to it (the toast "Scripts applied, bundle 4685e123…"; `time.control` answers that bundle in mode `edit`); the breakpoint on line 29 stays |
| `debug-host-11-replay.png` | Play again: the new fork runs the edited scripts, tick 1 stops on line 29 with `knots = 0` in Local. The driver then removes the breakpoint and stops again while paused |

The rename against the real host (section 8 item 2; 2026-11-11, macOS): `amoris serve` on a copy of
`samples/sailing` serving the built editor, driven by `tools/rename-host.ts` in headless Chrome with
real input only (clicks, keys, typed text; `?viewport=fallback`, so the run needs no WebGPU).
Crate1's row clicked in the hierarchy, F2, `CrateA` typed, Enter; Mod+Z and Mod+Shift+Z with the
row focused; `CrateB` typed over the inspector's name field, Enter; then 22 CJK characters, which
fit the field's 64 characters and are 66 bytes. After each step the driver asks the host over
`/api/call` what the entity is called and reads the hierarchy row and the name field: exit 0, every
step. The host's history holds `Rename Crate1 → CrateA` (source `editor`) and `Rename CrateA →
CrateB`; the long name is refused, shown as the toast `Rename CrateB → …: sim.name_too_long`, and
the entity stays CrateB. `rename-host-1-renamed.png` and `rename-host-2-refused.png` show the editor
after the first rename and at the refusal. The same driver passes against the mock (`bun run mock
-- --port <n> --no-agent --dist`), which answers these forms as the host does.

The edit loop against the real host (section 6; section 8, items 8 and 12 to 14; 2026-11-12,
macOS): `amoris serve` (a debug build) on a copy of `samples/anim` serving the editor built with
`VITE_AMORIS_EXPOSE=1`, driven by `tools/edit-host.ts` in headless Chrome with real input (clicks,
keys, typed text; the two places where it is not are named below) and the engine's renderer in the
page (`?viewport=wasm`, Chrome's WebGPU; the host's own captures on Metal). After each step the
driver asks the host over `/api/call` (`world.tree`, `world.get`, `history.list`, `capture`,
`time.control {}`, `events.why`, `log.since`) and then reads the page: exit 0, every check.

- Create > Cube, Sphere, Cylinder and Plane from the hierarchy's + menu: the host has a new entity
  of that name whose `Model.mesh` is `cube`, `sphere`, `cylinder`, `plane`; its `capture` holds one
  instance more each time (22, the scene's, to 26) with no asset pending, and the page's viewport
  the same counts; one undo entry `Create <name>` from the source `editor`; the hierarchy row and
  the inspector's name field show it. Each is then moved by typing its position into the
  inspector (a row of four in front of the scene), read back from the host's
  `Transform.position`; `edit-host-1-host-capture.png` is the host's own picture of the four.
- Several selected (section 6.2; added 2026-11-12): the sphere's row clicked, the cylinder's
  Mod+clicked; the inspector reads `editing 2 selected`, the position `— 0.5 4` (their x differ),
  `Model.mesh` a dash and metallic its one value. 1.5 typed into y sets both entities' y on the
  host and keeps x at -1 and 1, one undo entry `Set Transform.position on 2 entities`; 0 typed
  into the mixed x sets both to 0 and the field shows 0; two Mod+Z put each x and each y back.
  `edit-host-1b-mixed.png`. The same driver passed against the mock.
- Create > Directional, Point and Spot Light: the host's `Light.kind` is the value its own schema
  spells for that kind (the driver has no literal), the directional's row has the sun for an icon
  and the others a bulb. Create > Camera: `Camera 2`, with `fov_deg` at the schema's default.
- The cube: F2 and `Block` typed renames it on the host; 0.5 typed into the Model card's metallic
  field is one undo entry and stays 0.5; Add Component with `Light` typed lists Light first, and
  Enter adds it at its schema's defaults (one entry), which Mod+Z takes off again; `Occluder`, a
  component without fields, is added as `{}` and taken off the same way; Mod+D makes
  `Block 2` with Block's components and one instance more in both renderers; Delete, Mod+Z,
  Mod+Shift+Z and Mod+Z again remove and bring back that entity, under the same id, on the host
  and among the entities the page's renderer draws.
- Fields that were only passed through: the metallic field is clicked, another client sets the
  value to 0.9 over `/api/call`, and a click elsewhere leaves the field: the host still has 0.9
  and no undo entry but the other client's. The focus is moved through the colour's swatch with
  Shift+Tab and Tab: no undo entry, the same colour. A light's kind is set in the inspector's
  select (the option by script: a native select's popup is not driven) and Mod+Z, pressed with
  the focus still in the select, puts the host's `Light.kind` back.
- Assets (Import by upload, section 6.3; 2026-11-12): Import clicked, the file chooser answered
  (over CDP) with a copy of `models/hero.glb` named `uploaded-hero.glb` and a four-splat file from
  outside the project: the host lists `models/uploaded-hero.glb` (26,232 bytes) and
  `splats/edit-host.splat`, each the file sent byte for byte, the panel shows both, and no undo
  entry; imported again, the splat's path is offered and Enter replaces it (seven files listed
  before and after). A double-click on the uploaded model places `Uploaded-hero` with
  `Model.mesh` the path (the host's capture goes from 27 to 31 instances, none pending, the
  page's viewport too, which fetched it from `/assets/`); a double-click on the splat places an
  entity with `Splat.asset` the path and no Model.
- The transport: 2× sets the edit world's pacing to `{real_time: {speed: 2}}` on the host and the
  button still reads 2× after the host's next status pushes; Play runs the fork at 2 (the editor
  sent `play.start {speed: 2}`); Pause pauses it and the badge reads PAUSED, also after the next
  pushes; the Events panel lists the host's events, and a click on `probe.echo` shows the chain
  `probe.knock` → `probe.echo`, which is `events.why`'s causes reversed and then the event; the
  same button resumes; Stop returns to Edit with the ten entities created and placed here and the
  edit world's 2×; 1× sets it back.
- A Play world that halts (section 8, item 14): a new cube gets `Vessel` and `Collider` from Add
  Component at their defaults, and Play fails its first tick. The host's status says `halted`,
  `poisoned: 1` and `world_hash: null`, and its log has the tick's error
  (`water.vessel_invalid: ... the collider's points span no volume`). The page's badge reads
  HALTED, Stop is offered and Resume and Step are not, the Console shows the host's line and the
  editor's (`Play halted at tick 1`), one toast says the same. A speed chosen from the menu is
  taken by the host and its answer, a status without a hash, by the page, which is all there
  afterwards; a reloaded page comes up again with HALTED; Stop returns to an edit world that is
  not halted, and the cube is deleted.
- The page showed no error toast but the one of the halt, threw nothing and logged no error.
- Last, not input: a status whose `tick_ms` is no number is put into the page's store, as an
  unexpected value from a host would arrive. The top bar says `The top bar failed to draw`, the
  hierarchy, the dock and the status bar stay, and a click on Try again brings the bar back.

Before the flow the driver sets the project up, over `/api/call` and the file system (it refuses a
project whose root has no `out` in its path, unless told `--scribble`): `scripts.write` and
`scripts.apply` of a `scripts/main.ts` with two systems, one emitting `probe.knock` every 30 ticks
and one answering it with a `probe.echo` that names it as its cause (no sample emits an event with
a cause), and `splats/edit-host.splat`, four splats of 32 bytes (`samples/anim` has no splat). It
also compares the mock's Model, Light, Camera and Splat schemas with this host's: the same fields,
defaults and enum values.

Not checked there: that a light lights, that a camera shows, and that the placed splat is drawn
(their components on the host are); and a removal in the host's `capture`, whose `instances` is
its renderer's slot high-water mark (it rises with what is added and does not fall), so Delete,
undo and redo are read from the entities the page's renderer draws. The count does separate a
mesh that resolved from one that did not: an entity with `Model.mesh: "primitive:cube"`, the
editor's old form, adds no instance. Windows and Direct3D 12 were not run. The screenshots
(`edit-host-1-created.png`, `-2-inspector.png`, `-3-placed.png`, `-4-cause-chain.png`,
`-5-halted.png`), the host's capture and the driver's log `edit-host.txt` are under the ignored
`out/editor/edit-host/`. The driver exits 2, with a sentence, when the served project is not such a
copy: one that lists no model, or whose scripts declare components (the probe script would drop
them, which the host refuses).

The run found two things the forms alone did not explain, both fixed. A number field selects its
text on the frame after it takes the focus; when that frame came after Enter had committed and
left the field, the late `select()` focused it again with the value from before the edit, and its
next blur committed that old value back as a second undo entry (`NumberField.tsx` now selects only
while the field still has the focus). And the host's `status` push went on saying the pacing and
the pause of its last publication (section 8, item 13): against the host as it was, the driver
failed at "after the host's next status pushes the button still reads 2×".

Its review found more, each reproduced on `amoris serve` first and then fixed. A poisoned Play
world left a blank page (section 8, item 14). A number field wrote its text back whenever it was
left, typed into or not, so the value of the moment of the focus overwrote a newer one from
another client, the gizmo or a tick, as an undo entry of its own; a colour's swatch did the same
with the colour rounded to its hex, on a Tab through it. Both now write only what was typed or
picked. Mod+Z did nothing with the focus in a select. A file of any kind dropped on the viewport
was placed as a `Model`. Add Component sent `""` for a component without fields. On connect the
Events panel was filled with the host's oldest 5000 events instead of its newest. The driver
itself could click Pause before the page knew of Play (the page then asked for Play again:
`play.already`), and took an event from before its own Play for the cause chain; it now waits for
the page's badge and reads the events after the last one from before Play. And `launchChrome`,
given a CDP port another Chrome held, handed out that browser's page: two drivers on one port
drove one browser.

The same driver passes against the mock (`bun run mock -- --port <n> --no-agent --dist`), with the
fallback view: every step and every check on the mock's world, the checks of what a renderer holds
printed as `not checked`, as are the component without fields (the mock registers none) and the
world that halts (the mock's never do). It also passes against a mock that has already played and
stopped, the state in which it once failed.

`debug.png`, `debug-paused.png`, `console.png` and `scripts.png` were taken again against the mock
(`tools/capture.ts`) after the mock moved to the real shapes and frames (the callback and `run`, the
closure's columns); the console evaluates on the paused frame.

`out/bench-runs/sdk/` shows the script editor against a real host (`amoris serve samples/sailing`),
taken by `tools/sdk_check.ts`: with the host's declarations loaded (4 engine and 4 game components),
a misspelt column (`b.sped`, "Did you mean 'speed'?") and a column the query's `fields` leaves out
(`NotInFields<"Boat.heel_deg">`) are the worker's 2 errors in the status bar, and `boats.cols.Boat.`
completes the Boat's fields.

## 11. Gaps

- The editor debugs through `debug.*`, not CDP (section 8.1); the host's CDP endpoint serves Chrome
  DevTools and VS Code beside it (Debug > Copy Chrome DevTools URL).
- Values deeper than the host's preview (three levels, 48 entries) cannot be expanded further: the
  agents' API hands out no object ids. A string that reads `"undefined"` or `"[Object]"` below the
  top level shows as that marker.
- While paused, the game thread answers no command (debugger.md 8): an inspector edit, a snapshot
  restore or a save is refused at once (server.md 3.4) and is not retried after Continue; a value
  is set in the paused frame instead, and Stop and Pause cross the pause (section 8.1). A breakpoint
  that every tick hits holds Play again right after Continue, so an edit made then is usually
  refused too: Pause, then Continue, or remove the breakpoint first.
- `debug.set` sets the first variable of that name a function declares: two block scopes declaring
  one name (`for (let r ...)` twice) cannot be told apart at run time.
- Agent sessions cannot be started from the editor yet (no `agent.session.*`); the form is a
  preview, and agents connect over MCP.
- An upload sends its parts one after another, and only a toast at the end says how it went:
  a large file shows no progress meanwhile. The CLI has no upload command (`amoris assets` lists).
- The worker's declarations are fetched on connect and after the editor's own apply: a component
  added in an unsaved file is unknown to it until the save, and one an agent adds (no push says the
  scripts changed) until the next apply or reconnect.
- `cargo xtask check`'s `editor` step (checks.md 4) runs the bun unit tests, the type check and
  three of the CDP harnesses (`edit-host`, with the fallback view, `rename-host`, `debug-host`)
  against the release `amoris serve`; `sdk_check`, `viewport-loads` and `water-authoring` (the
  Vite dev server, WebGPU) and the measuring harnesses still run by hand, and no harness drives
  the wasm viewport in the check.
- Add Component lists every registered component, and the real host refuses nine of them with the
  editor's schema defaults (2026-11-12): state the engine keeps itself (`Controls`,
  `ObserverEvents`, `ObserverMemory`) and components whose defaults are not a valid value
  (`Terrain`, `WaterRegion`, `WaterResident`, `WaterColliderBed`, `WaterDisplacement`,
  `WaterFeedback`). The refusal shows as an error toast. The registry does not say which
  components an edit may add. Others are taken at defaults that cannot run: a `Collider`'s is a
  ball of radius 0, and with a `Vessel` beside it Play halts at its first tick (section 8, item
  14).
- A mesh or an asset that does not import draws nothing and says nothing in the editor: the host
  reports it on its own stderr only (`[WARN] ... does not import`), not on the `log` topic.
- Lights, cameras and splats are created and placed as components the host holds; that a placed
  splat is drawn is checked by no harness, and the wasm viewport draws no icon for a light or a
  camera (the fallback view does).
- The real host registers no `Parent` component, so Create Child and reparenting are disabled
  against it (section 8, item 3).
- The Create menu offers four of the engine's seven primitives (no capsule, cone or torus). A
  skinned model is placed without an `Animator`.
- Step and Pause from Edit send `play.start` and then `time.control {pause: true}`, so a few ticks
  run before the pause lands; `play.start {paused: true}` would start paused.
- The mock's schemas of the visual components are hand copies of the host's; only a run of
  `tools/edit-host.ts` against a real host compares them. `rename-host.ts` and
  `water-authoring.ts` still start Chrome themselves instead of through `launchChrome`. The
  inspector's selects and drops of assets are not driven with real input by any harness (a select's
  option is set by script in `edit-host.ts`; the key after it is real).
