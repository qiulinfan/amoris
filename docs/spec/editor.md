# The editor

Status: Draft 1 (2026-10-04). Charter: 2 (proof 4, the editor's modernity and completeness), 3.1
(one interface), 4.3 (debugging), 4.5 (the editor). Wire protocol: [host-protocol.md](host-protocol.md).

The editor is a web application in `editor/` (React 19, TypeScript 7, Vite 8, bun). It is a client of
the host protocol and nothing else: every panel reads and writes through the same command catalog
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

# Against the real host: `pocket editor <project>` serves editor/dist at http://127.0.0.1:7878/.
# For editor development against it, Vite proxies to POCKET_HOST (default http://127.0.0.1:7878).
bun run dev
POCKET_HOST=http://127.0.0.1:7900 bun run dev

bun run build                     # type check (tsc 7) + vite build -> editor/dist
bun run typecheck                 # the editor and the mock/tools
bun run mock -- --port 7879 --no-agent --dist   # the mock alone; --dist serves editor/dist at /
bun run evidence -- --url http://127.0.0.1:5173/ # screenshots into docs/evidence/editor (needs Chrome)
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
floating crates, a helmsman and the sample's three script systems (`muster`, `log`,
`take_aboard`) emitting events with causes, a snapshot ring with restore and rewind, the sample's
scripts (read from `samples/sailing/scripts`, kept in memory) with fake diagnostics, a debugger
(line breakpoints with conditions, stepping, frames with scopes, evaluation, data breakpoints),
profile samples, assets with thumbnails, and a simulated MCP agent whose edits land in the same undo
history. It binds 127.0.0.1 and refuses non-loopback `Origin`/`Host`, as the host does.

## 2. Layout

One window: a top bar, the dock, a status bar. The dock is dockview: every panel can be dragged to
another group or edge, floated, maximized, closed and reopened (View > Panels, the palette, or
Cmd/Ctrl+Alt+1–9); the layout is saved in `localStorage` and View > Reset Layout restores the
default, which follows Godot's: hierarchy over assets on the left; viewport and scripts in the middle
over console, events, timeline, profiler and debug; inspector over history and agent on the right.

| Area | Contents |
|---|---|
| Top bar | Logo, project, menus File / Edit / View / Game / Debug / Help (built from the command registry, so every item shows its shortcut and state), the transport (Play/Stop, Pause, Step, tick, time, fps, tick cost, speed), the mode badge (EDIT, PLAY, PAUSED; Play also tints the chrome green), the palette button and the connection pill |
| Status bar | Host and connection (with retry countdown; red when offline, amber while the debugger holds the game), project, error and warning counts (click: console), the pause location, agent activity, selection, entity count, world hash, the viewport's renderer |
| Command palette | Cmd/Ctrl+K or Ctrl/Cmd+Shift+P. Fuzzy search over every command, every panel, every entity (`@` to narrow) and every method of `GET /api/catalog` (`call` to narrow). A host method opens a runner: its doc and params from the catalog schema, a JSON params line prefilled from the required fields (and the selection), Enter calls it and shows the result or the structured error |

## 3. Panels

| Panel | What it does | Protocol |
|---|---|---|
| Hierarchy | Virtualized tree (children by `Parent`), icons by kind (camera, light, boat, sea, wind, mesh, empty), search by name, `t:Component` or `#id`; click, Cmd/Ctrl-click toggles, Shift-click ranges; arrows move and fold; F2 or Enter renames inline; drag rows onto a row to reparent, onto empty space to unparent; drop an asset to place it (under the row); context menu (rename, duplicate, delete, frame, copy id, create child, add component, unparent, break when written); + creates an empty, primitives, lights or a camera | `world.tree`, `world.edit` (spawn, set `Name`, set/remove `Parent`, destroy) |
| Viewport | The renderer's canvas (section 5) with: orbit (right-drag or Alt+drag), pan (middle-drag or Shift+right-drag), zoom (wheel, pinch), click to pick (Shift/Cmd toggles), drag on empty space for rectangle selection, double-click to frame; tools Select/Move/Rotate/Scale (Q/W/E/R) with 3D gizmos (arrows with cones, plane handles, a screen handle; rotation rings with a view ring; scale cubes), World/Local (X), snapping with steps (hold Cmd/Ctrl to invert while dragging), grid (G), the axis gizmo (click an axis to look along it, the center for perspective/orthographic), camera menu (views, frame, reset), stats, the wind, selection labels; drop assets on the ground plane | `world.edit` with `group` while dragging, one undo entry on release; Escape cancels the drag (undoes it) |
| Inspector | The selection's name and components, each a collapsible card generated from its JSON Schema with the doc as tooltip: numbers (drag the strip to scrub, Shift ×10, Alt ×0.1; arithmetic accepted; arrows step), vectors per axis, rotations as Euler degrees (YXZ) over the stored quaternion, colours with a picker (stored linear, shown sRGB) and alpha, enums as selects with per-variant docs, booleans as switches, entity references as pickers (searchable, or drop a hierarchy row), nullable fields, nested objects, lists, tagged unions (`{Cuboid: {...}}`), the rest as JSON. Engine-written fields are marked. Card menu: reset to defaults, copy as JSON, break when written, remove. Add Component lists the registry with docs. Several selected: edits apply to each that has the component | `world.schema`, `world.get`, `world.edit` |
| Scripts | File list with problem counts, Monaco with tabs (one model per script so imports type-check), the `pocket` module's types (section 7), Cmd/Ctrl+S saves (`scripts.write` then `scripts.apply`), host diagnostics as markers beside the TypeScript worker's own, a breakpoint gutter (click or F9; conditional and unbound breakpoints drawn differently), the paused line, sticky scroll, minimap | `scripts.list/read/write/apply`, `debug.breakpoints.*` |
| Debug | Continue/Pause, Step Over/Into/Out (F5, F6, F10, F11, Shift+F11), the pause reason, tick and system; call stack (click a frame to show its source and its scopes), variables tree per scope, watch expressions re-evaluated on each pause, breakpoints with editable conditions and hit counts, data breakpoints. Three columns when wide | `debug` events, `debug.state/continue/pause/step/eval/breakpoints.*/watch/unwatch` |
| Console | The host's log with level filters and counts, search, follow, timestamps and ticks, a link to the source line; an input line evaluating in the game (on the selected frame while paused), with history; results are expandable trees | `log` events, `debug.eval` |
| Events | The event stream as a table (seq, tick, name, subject, data) with a filter; selecting one asks the host why: the cause chain, oldest first, then the data; subjects select their entity | `events` events, `events.since`, `events.why` |
| Timeline | Kept snapshots and event lanes (the most frequent kinds, and the selection's) on a tick ruler with the playhead; drag to scrub and release to rewind there; click a snapshot to restore it; back to the previous snapshot, pause, step | `snapshots.list/restore`, `debug.rewind`, `time.*` |
| Profiler | Per sample, the tick's CPU time stacked by system and the frame time against the 60 Hz budget (hover for one sample), then per-system CPU and per-pass GPU averages with maxima; freeze and clear | `profile` events |
| Assets | Grid or list with thumbnails or kind icons, kind filters with counts, search; drag into the viewport or the hierarchy (or double-click) to place a model; import by path; context menu (place, copy path, reimport) | `assets.list/import`, `world.edit` spawn with `Model.mesh` |
| Agent | The live feed of agent activity grouped by session (call, result with success, message), sessions with their call counts and liveness, and a New session form (model, goal, tools) that waits for host support; copies the MCP endpoint command | `agent` events |
| History | The undo history as a timeline from "Opened scene", agent edits marked; click an entry to undo or redo up to it; undo and redo buttons | `history` events, `history.undo/redo/list` |

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

The engine's renderer, compiled to wasm with wasm-bindgen (`crates/pocket-web`), implements this;
`editor/src/viewport/engine.ts` is the source of truth.

```ts
// Served by the host at /wasm/pocket_web.js (wasm-bindgen output; its .wasm beside it).
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
  dispose(): void;
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
perspective (or orthographic) of `Camera` with the canvas's aspect, which `editor/src/viewport/
camera.ts` (`View`) reproduces for hit tests and drops, so the two must agree. The renderer draws the
world from the render feed; the editor owns the camera (orbit), the selection, the gizmo geometry and
its hit testing and drag math (`viewport/gizmo.ts`), so a renderer only draws `shapes` on top of the
scene, in order (they are sorted far to near), and outlines the selection. `pick` should come from
the entity id buffer (charter 4.4).

Loading (`loadViewport`): `HEAD /wasm/pocket_web.js`; if it is JavaScript, import it, run `default`,
call `createViewport`. Otherwise, or on any error, the editor uses its fallback
(`viewport/fallback/renderer.ts`): a Canvas 2D view of the world data the editor already holds
(flat-shaded stand-ins chosen from `Model.mesh`, `Boat`, the `Collider` shape, light and camera
icons, the animated sea from `Sea.waves`, a fading grid), which implements the same interface, so
camera, picking, selection and gizmo code is exercised now and unchanged later. The status bar and
the viewport badge say which renderer runs.

## 6. Editing

Every change is a `world.edit`: one transaction, one labelled entry in the host's undo history,
shared with agents (whose labels start `agent:`). The editor never changes its copy of the world
itself; it refetches what `world.changed` names (coalesced, a few requests at a time), with one
exception: while a drag (gizmo, scrubbed number) is in flight it shows the dragged values as a local
preview.

Drags stream: each pointer move sends a `world.edit` with the same `group` (at most every 60 ms),
the host merges consecutive edits of one group into one undo entry, and release sends the final
value; Escape undoes it. When the catalog's `world.edit` params schema has no `group`, the editor
previews locally and sends one edit on release instead.

Mapping: rename is `set` of `Name` with a string value; reparent is `set` `Parent {parent}` (or
`remove` it) and needs a `Parent` component in the registry; duplicate is `world.get` then `spawn`
with the components (without `Name`) and the next free name (`Crate4` → `Crate5`, `Sloop` → `Sloop
2`); create spawns `Transform` plus `Model {mesh: "primitive:cube"}`, `Light {kind}` or `Camera` from
the schemas' defaults (entries are disabled when the registry lacks the component); placing an asset
spawns `Transform` and `Model {mesh: <asset path>}` at the ground point under the drop. Add
Component sets the component's schema defaults; scale handles edit `Transform.scale` when it exists,
else `Model.scale`.

## 7. Schema-driven fields and script types

`world.schema` answers `[{name, origin, version, doc, schema}]` (pocket-sim's `ComponentInfo`).
`src/host/schema.ts` reads both producers: schemars for engine components (`$ref`/`$defs`, fixed-size
number arrays, `oneOf` of documented `const`s, externally tagged unions, `EntityId` by `$ref`) and
the registry for project components (`{x, y, z}` objects, `enum`, nullable integers bounded by
2^53 − 1 for entities). Colours and rotations are recognised by name and doc (`color`, "RGBA",
"quaternion"), since the schema cannot say; unknown shapes fall back to a JSON editor, never to
nothing.

Monaco loads the SDK's declarations when `sdk/**/*.d.ts` exists (bundled at build time), else the
stub `editor/sdk-stub/pocket.d.ts` (script-host.md 5.1, loosely typed components).

## 8. What the editor needs from the host

Beyond host-protocol.md Draft 1, the editor (and the mock) assume the following; the host should
adopt them or the editor should change:

1. `world.edit` takes `group?: string`: consecutive edits with the same group merge into one undo
   entry (a drag). Detected from the catalog's params schema.
2. `set` of `Name` takes the name as a string `value`; `world.get` may list `Name` among components
   (the editor ignores it there).
3. The hierarchy is the `Parent {parent: EntityId}` component, and `world.tree` nests by it.
4. `debug.step {kind: "over" | "into" | "out"}`; `debug.breakpoints.set` answers `{id, file, line,
   condition?, verified?, hits?}` (the line moved to the next statement); `debug.breakpoints.clear`
   takes `{id}`, `{file, line}`, `{file}` or `{}` and answers `{cleared}`; `debug.breakpoints.list`
   answers `{breakpoints, watches?}` (optional: when absent the editor keeps its own list and sets it
   again after a host restart); `debug.watch` answers `{id, entity, component, field?}` and
   `debug.unwatch {id}` removes it; `debug.state` answers `{state, reason?, detail?, location?,
   frames?: [{id, name, file, line, column?, system?, scopes?: [{name, variables}]}], tick?,
   system?}`; `debug.eval` answers a variable `{name, value, type, children?}`.
5. `history.list` and the `history` event list `redo` with the next entry to redo first; agent
   edits' labels start with `agent:`.
6. `agent` events may carry `ok` (a result's success) and `ts` (ms since the epoch).
7. `scripts.read` answers the text (a string) or `{text}`; diagnostics are `{file?, line, column,
   end_line?, end_column?, severity: "error" | "warning" | "info" | "hint", message, code?}`.
8. `snapshots.list` answers `[{tick, t_s?, hash?}]`; `events.why` answers the chain oldest first,
   ending with the event itself; `status` may carry `speed`.
9. `GET /api/catalog` answers an array of commands or `{commands}`; `kind` in any case.
10. The host serves pocket-web's wasm-bindgen output under `/wasm/` (section 5).

The charter (4.3) and host-protocol.md 6 say the editor speaks CDP over `/devtools/game`. This round
the editor uses the JSON `debug.*` methods, which drive the same debugging core and are what agents
use; a CDP client in the editor is open.

## 9. Code map

| Path | Role |
|---|---|
| `src/host/` | `protocol.ts` (wire types, method params and results), `client.ts` (WebSocket with ids, events, reconnect; catalog), `api.ts` (typed wrappers, the connection), `sync.ts` (stores from host events), `schema.ts` (JSON Schema to field kinds) |
| `src/state/` | One zustand store per domain: world, selection, session, history, logs, events, profile, agent, debug, scripts, assets, viewport settings, UI |
| `src/actions/` | Edits (`world.ts`, `LiveEdit` for drags), transport, debugger, scripts, error reporting |
| `src/commands/` | The command registry, key handling, the built-in commands |
| `src/layout/` | The panel registry, the dock and its default layout, a handle for other code |
| `src/shell/` | Top bar, menus, transport, status bar, palette, dialogs |
| `src/ui/` | Primitives: buttons, menus and context menus, tree, fields, split pane, toolbar, tooltip, toasts, variable tree, icons, formatting |
| `src/viewport/` | The renderer interface, camera, math, gizmo, the fallback renderer |
| `src/panels/<panel>/` | One module per panel (and its CSS) |
| `mock/` | The mock host (section 1.1) |
| `tools/` | `capture.ts` (evidence through headless Chrome over CDP) |
| `sdk-stub/` | The `pocket` declarations Monaco uses until `sdk/` has generated ones |

## 10. Evidence

`docs/evidence/editor/` holds screenshots of an edit, play and debug session against the mock,
taken by `tools/capture.ts` (headless Chrome, 1600×1000 at 2×): the full window in Edit, Play and
paused in the debugger, and each panel (viewport with a live gizmo drag and the rotate gizmo,
inspector, Add Component, hierarchy context menu, Edit menu, palette and a host method call, assets,
events with a cause chain, timeline, profiler, console, agent, debug, scripts, history, shortcuts).

## 11. Gaps

- The viewport draws with the fallback until pocket-web's renderer is served (section 5); the
  render feed (`/render`) is the renderer's, not the editor's.
- The editor debugs through `debug.*`, not CDP (section 8).
- Agent sessions cannot be started from the editor yet (no `agent.session.*`); the form is a
  preview, and agents connect over MCP.
- Import takes a path the host can read; there is no upload.
- Monaco checks against the stub types until `sdk/` generates declarations.
- Multi-selection edits apply to every selected entity with the component but show the primary's
  values (no mixed-value display).
- No automated tests beyond `tools/capture.ts`, which drives the main flows end to end.
