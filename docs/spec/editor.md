# The editor

Status: Draft 1, built (2026-10-04); the debugger verified against the real host (section 10).
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

# Against the real host: `amoris editor <project>` serves editor/dist at http://127.0.0.1:7878/.
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

## 2. Layout

One window: a top bar, the dock, a status bar. The dock is dockview: every panel can be dragged to
another group or edge, floated, maximized, closed and reopened (View > Panels, the palette, or
Cmd/Ctrl+Alt+1–9); the layout is saved in `localStorage` and View > Reset Layout restores the
default, which follows Godot's: hierarchy over assets on the left; viewport and scripts in the
middle over console, events, timeline, profiler and debug; inspector over history and agent on the
right.

| Area | Contents |
|---|---|
| Top bar | Logo, project, menus File / Edit / View / Game / Debug / Help (built from the command registry, so every item shows its shortcut and state), the transport (Play/Stop, Pause, Step, tick, time, ticks per second, tick cost, speed), the mode badge (EDIT, PLAY, PAUSED; Play also tints the chrome green), the palette button and the connection pill |
| Status bar | Host and connection (with retry countdown; red when offline, amber while the debugger holds the game), project, error and warning counts (click: console), the pause location, agent activity, selection, entity count, world hash, the viewport's renderer |
| Command palette | Cmd/Ctrl+K or Ctrl/Cmd+Shift+P. Fuzzy search over every command, every panel, every entity (`@` to narrow) and every method of `GET /api/catalog` (`call` to narrow). A host method opens a runner: its doc and params from the catalog schema, a JSON params line prefilled from the required fields (and the selection), Enter calls it and shows the result or the structured error |

## 3. Panels

| Panel | What it does | Protocol |
|---|---|---|
| Hierarchy | Virtualized tree (children by `Parent`), icons by kind (camera, light, boat, sea, wind, mesh, empty), search by name, `t:Component` or `#id`; click, Cmd/Ctrl-click toggles, Shift-click ranges; arrows move and fold; F2 or Enter renames inline; drag rows onto a row to reparent, onto empty space to unparent; drop an asset to place it (under the row); context menu (rename, duplicate, delete, frame, copy id, create child, add component, unparent, break when written); + creates an empty, primitives, lights or a camera | `world.tree`, `world.edit` (spawn, set `Name`, set/remove `Parent`, destroy) |
| Viewport | The renderer's canvas (section 5) with: orbit (right-drag or Alt+drag), pan (middle-drag or Shift+right-drag), zoom (wheel, pinch), click to pick (Shift/Cmd toggles), drag on empty space for rectangle selection, double-click to frame; tools Select/Move/Rotate/Scale (Q/W/E/R) with 3D gizmos (arrows with cones, plane handles, a screen handle; rotation rings with a view ring; scale cubes), World/Local (X), snapping with steps (hold Cmd/Ctrl to invert while dragging), grid (G), the axis gizmo (click an axis to look along it, the center for perspective/orthographic), camera menu (views, frame, reset), stats, the wind, selection labels; drop assets on the ground plane | `world.edit` with `group` while dragging, one undo entry on release; Escape takes the drag back (section 6) |
| Inspector | The selection's name and components, each a collapsible card generated from its JSON Schema with the doc as tooltip: numbers (drag the strip to scrub, Shift ×10, Alt ×0.1; arithmetic accepted; arrows step), vectors per axis, rotations as Euler degrees (YXZ) over the stored quaternion, colours with a picker (stored linear, shown sRGB) and alpha, enums as selects with per-variant docs, booleans as switches, entity references as pickers (searchable, or drop a hierarchy row), nullable fields, nested objects, lists, tagged unions (`{Cuboid: {...}}`), the rest as JSON. Engine-written fields are marked. Card menu: reset to defaults, copy as JSON, break when written, remove. Add Component lists the registry with docs. Several selected: edits apply to each that has the component | `world.schema`, `world.get`, `world.edit` |
| Scripts | File list with problem counts, Monaco with tabs (one model per script so imports type-check), the `amoris` module's types (section 7), Cmd/Ctrl+S saves (`scripts.write` then `scripts.apply`), host diagnostics as markers beside the TypeScript worker's own, a breakpoint gutter (click or F9; conditional and unbound breakpoints drawn differently), the paused line, sticky scroll, minimap | `scripts.list/read/write/apply`, `debug.breakpoints.*` |
| Debug | Continue/Pause, Step Over/Into/Out (F5, F6, F10, F11, Shift+F11), pause on exceptions (off, uncaught, all); the pause reason, tick, system and TypeScript location, and for a data breakpoint the field, its value before and after and the statement that wrote it; call stack (click a frame to show its source and its scopes; a frame seen after `run` returned is marked), the frame's Local and Closure scopes as expandable trees whose leaf values can be set (double-click, type an expression, Enter: section 8.1), watch expressions re-evaluated on each pause and frame change, breakpoints with editable conditions (the host's list, read again on each pause: logpoints, MCP agents' and CDP clients' breakpoints shown as such), data breakpoints. Three columns when wide | `debug` events, `debug.state/continue/pause/step/eval/set/exceptions/breakpoints.*/watch/unwatch` |
| Console | The host's log with level filters and counts, search, follow, timestamps and ticks, a link to the source line; an input line evaluating in the game (on the selected frame while paused), with history; results are expandable trees | `log` events, `debug.eval` |
| Events | The event stream as a table (seq, tick, name, subject, data) with a filter; selecting one asks the host why: the cause chain, oldest first, then the data; subjects select their entity | `events` events, `events.since`, `events.why` |
| Timeline | Kept snapshots and event lanes (the most frequent kinds, and the selection's) on a tick ruler with the playhead; drag to scrub and release to rewind there; click a snapshot to restore it; back to the previous snapshot, pause, step | `snapshots.list/restore`, `debug.rewind`, `time.*` |
| Profiler | Per sample (4 a second), the host's CPU time per tick stacked by system and the viewport's frame time against the 60 Hz budget (hover for one sample), then over the last 3 s: the game thread's systems (each system's own time, `tick.other`, `loop.present`, `loop.publish`, `loop.after`) and the tick's phases with their largest tick, and the viewport's GPU passes; the tick and game-thread time, the frame interval, fps and GPU time; held ticks left out; freeze and clear | `profile` events, the viewport's `stats()` |
| Assets | Grid or list with thumbnails or kind icons, kind filters with counts, search; drag into the viewport or the hierarchy (or double-click) to place a model; import by path; context menu (place, copy path, reimport) | `assets.list/import`, `world.edit` spawn with `Model.mesh` |
| Agent | The live feed of agent activity grouped by session (call, result with success, message), sessions with their call counts and liveness, and a New session form (model, goal, tools) that waits for host support; copies the MCP endpoint command | `agent` events |
| History | The undo history as a timeline from "Opened scene", each edit another client made marked with its session (`mcp-1`, `api`); click an entry to undo or redo up to it; undo and redo buttons | `history` events, `history.undo/redo/list` |

## 4. Shortcuts

Mod is Cmd on macOS and Ctrl elsewhere. Keys marked global also work inside text fields and the
code editor. Help > Keyboard Shortcuts (Mod+/) lists them all from the registry.

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
`Sloop 2`); create spawns `Transform` plus `Model {mesh: "primitive:cube"}`, `Light {kind}` or
`Camera` from the schemas' defaults (entries are disabled when the registry lacks the component);
placing an asset spawns `Transform` and `Model {mesh: <asset path>}` at the ground point under the
drop. Add Component sets the component's schema defaults; scale handles edit `Transform.scale` when
it exists, else `Model.scale`.

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
adopt them or the editor should change:

1. `world.edit` takes `group?: string`: consecutive edits with the same group merge into one undo
   entry (a drag). Detected from the catalog's params schema. Adopted 2026-10-15.
2. `set` of `Name` takes the name as a string `value`; `world.get` may list `Name` among components
   (the editor ignores it there).
3. The hierarchy is the `Parent {parent: EntityId}` component, and `world.tree` nests by it.
4. The debugger is the host's as-built `debug.*` (section 8.1), not an editor-specific form.
5. Agent edits are marked in the history. Adopted 2026-10-15 with item 1 (server.md 3.1): each
   entry names its `source` (and the server its `session`); a host without sources marks agent
   edits by an `agent:` label prefix. The editor changed for the order instead: the host lists
   `redo` most recent last, as Draft 1 has it (the editor had assumed the next to redo first), and
   the history store reverses it.
6. `agent` events may carry `ok` (a result's success) and `ts` (ms since the epoch).
7. `scripts.read` answers the text (a string) or `{text}`; diagnostics are `{file?, line, column,
   end_line?, end_column?, severity: "error" | "warning" | "info" | "hint", message, code?}`.
8. `snapshots.list` answers `[{tick, t_s?, hash?}]`; `events.why` answers the chain oldest first,
   ending with the event itself; `status` may carry `speed`.
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
| `src/ui/` | Primitives: buttons, menus and context menus, tree, fields, split pane, toolbar, tooltip, toasts, variable tree, icons, formatting |
| `src/viewport/` | The renderer interface, camera, math, gizmo, the frame meter, the fallback renderer |
| `src/panels/<panel>/` | One module per panel (and its CSS) |
| `mock/` | The mock host (section 1.1) |
| `tools/` | `capture.ts` (evidence of every panel against the mock), `debug-host.ts` (the debugger against the real host), `sdk_check.ts` (the script editor's types against the real host), `profiler-capture.ts` (the profiler against the real host, docs/bench/profiler.md), `water-authoring.ts` (the water tools against the real host, 6.1), all through headless Chrome over CDP (`cdp.ts`) |

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
- Import takes a path the host can read; there is no upload.
- The worker's declarations are fetched on connect and after the editor's own apply: a component
  added in an unsaved file is unknown to it until the save, and one an agent adds (no push says the
  scripts changed) until the next apply or reconnect.
- Multi-selection edits apply to every selected entity with the component but show the primary's
  values (no mixed-value display).
- No automated tests beyond `tools/capture.ts` (the mock), `tools/debug-host.ts` (the real host's
  debugger) and `tools/sdk_check.ts` (the script editor's types against a real host), which drive
  the main flows end to end.
