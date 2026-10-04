# The host protocol: editor, agents and tools on one command catalog

Status: Draft 1 (2026-10-04). Charter: 3.1 (one interface), 4.3 (debugging), 4.5 (editor).

The native host (`pocket editor <project>` / `pocket serve <project>`) owns the one authoritative
world. Everything else is a client: the React editor, MCP agents, the CLI, tests. They all call the
same commands; the editor has no private channel. This file fixes the wire forms. The command
semantics (validation, `{code, message, detail}` errors, unknown-field refusal with suggestions)
are the runtime's (`pocket-runtime` catalog, `shared/contract/errors.md`).

## 1. Endpoints

All bind `127.0.0.1` on one port (default 7878, `--port`). A request whose `Origin` or `Host` is not
loopback is refused (DNS rebinding).

| Path | Form | Who |
|---|---|---|
| `GET /` | the editor's static files (`editor/dist`) | browser |
| `GET /api/catalog` | JSON: every command with kind, doc, params JSON Schema, result schema | all |
| `POST /api/call` | JSON request, JSON response (section 2) | CLI, tests, agents without MCP |
| `GET /ws` | WebSocket, JSON text frames: requests, responses, pushed events (sections 2, 3) | editor |
| `GET /render` | WebSocket, binary frames: the render feed (section 5) | editor viewport |
| `GET /assets/<path>` | the project's asset files (glTF, textures, splats, neural assets) | editor viewport |
| `POST /mcp` | MCP Streamable HTTP (rmcp) | agents |
| `GET /json/list`, `GET /json/version`, `WS /devtools/game` | CDP discovery and endpoint (section 6), on `pocket-debug`'s own port (default 9229, where VS Code's `node` attach looks); the host may proxy them | Chrome DevTools, VS Code, the editor |

`pocket mcp <project>` serves the same MCP tools over stdio instead.

## 2. Requests and responses

Request (text frame on `/ws`, or the body of `POST /api/call`):

```json
{"id": 7, "method": "world.edit", "params": {"ops": [{"set": {"entity": 12, "component": "Model", "value": {"color": [1, 0, 0, 1]}}}]}}
```

Response:

```json
{"id": 7, "result": {"tick": 1204, "applied": 1}}
{"id": 7, "error": {"code": "request.unknown_field", "message": "...", "detail": {"field": "colour", "did_you_mean": ["color"]}}}
```

`id` is chosen by the client (number or string) and echoed. Requests on one socket are answered in
order of completion, not of sending.

## 3. Pushed events (`/ws`)

The host pushes `{"event": <topic>, "data": ...}`. A client subscribes with
`{"id": n, "method": "subscribe", "params": {"topics": ["status", "log"]}}`; `status` is always on.

| Topic | Data | When |
|---|---|---|
| `status` | `{tick, t_s, paused, pacing, mode: "edit"|"play", world_hash, entities, fps, tick_ms}` | at most 10 Hz |
| `world.changed` | `{tick, spawned: [id], despawned: [id], changed: [[id, component]]}` | after a tick or write that changed something, coalesced to 10 Hz |
| `events` | `[{seq, tick, name, subject, data, cause}]` | game events as they are emitted |
| `log` | `{level, source, message, file?, line?, tick?}` | script `console.*`, errors, host messages |
| `history` | `{undo: [label], redo: [label]}` | after any edit |
| `debug` | `{state: "paused"|"running", reason?, location?: {file, line, column}, frames?}` | debugger state changes |
| `profile` | `{tick, systems: [{name, ms}], frame_ms, gpu: [{pass, ms}]}` | 4 Hz while subscribed |
| `agent` | `{session, kind: "call"|"result"|"message", method?, summary}` | each MCP call by an agent |

## 4. Methods

Each method is a command of the catalog (`GET /api/catalog` is authoritative). Kinds: Read, Write
(recorded, undoable), Control (time). Names are `noun.verb`.

| Method | Params | Result |
|---|---|---|
| `catalog.list` | `{}` | the catalog |
| `project.info` | `{}` | `{name, root, scenes, scripts, assets}` |
| `project.save` | `{}` | `{files}` |
| `world.tree` | `{root?, depth?, filter?}` | `[{id, name, components: [name], children: [...]}]` |
| `world.get` | `{entity, components?}` | `{id, name, components: {Name: value}}` |
| `world.query` | `{with: [component], fields?: ["C.f"], limit?}` | rows |
| `world.schema` | `{component?}` | component JSON Schemas with docs, from the registry |
| `world.edit` | `{ops: [op], label?}` | `{tick, applied, spawned: [id]}`; all or nothing |
| `history.undo` / `history.redo` | `{}` | `{label}` |
| `history.list` | `{}` | `{undo, redo}` |
| `time.control` | `{pause?, speed?, pacing?}` | status |
| `time.step` | `{ticks?, until?, watch?}` | `{tick, stopped_by?}` |
| `play.start` / `play.stop` | `{}` | status (Play runs a fork of the edit world; Stop discards it) |
| `scripts.list` | `{}` | `[{path, bytes, diagnostics}]` |
| `scripts.read` / `scripts.write` | `{path}` / `{path, text}` | text / `{diagnostics}` |
| `scripts.apply` | `{paths?}` | `{bundle, diagnostics}` (type check, compile, hot swap at a boundary) |
| `assets.list` | `{dir?}` | `[{path, kind, bytes, thumbnail?}]` |
| `assets.import` | `{path}` | `{asset, meshes, materials}` |
| `events.since` | `{seq, limit?}` | events |
| `events.why` | `{seq}` | the cause chain |
| `snapshots.list` / `snapshots.restore` | `{}` / `{tick}` | ring of kept snapshots / status |
| `debug.*` | section 6 | |
| `profile.frame` | `{}` | the last frame's CPU and GPU timings |
| `agent.*` | MCP tool calls mirrored (section 7) | |

`world.edit` ops (the runtime's `world_edit` ops, one transaction):

```json
{"spawn": {"name": "Crate", "components": {"Transform": {...}, "Model": {...}}}}
{"set": {"entity": 12, "component": "Transform", "value": {...}}}       // fields merge
{"remove": {"entity": 12, "component": "Model"}}
{"destroy": {"entity": 12}}
```

## 5. The render feed (`/render`)

The viewport renders the host's world with the same renderer compiled to wasm (WebGPU). The host
sends the render feed the native renderer consumes: after each tick, what changed in the visual
components (`pocket-assets`' `RenderFrame`), encoded with `bincode` 2 (standard config) in one binary
frame. The first frame after connecting, and after a restore or Play/Stop, has `reset: true` and
carries everything. The client sends text frames for its camera and picking requests:
`{"camera": {...}}`, `{"pick": {"x": px, "y": px}}` (answered on `/ws` as event `pick`).

## 6. Debugging

`pocket-debug` serves the Chrome DevTools Protocol for the game's scripts ([debugger.md](debugger.md)
is the specification of what is built):

- `GET /json/list` and `/json/version` list one target, `pocket-game`, with
  `webSocketDebuggerUrl: ws://127.0.0.1:<port>/devtools/game` (port 9229 by default, its own; a
  request whose `Host` or `Origin` is not loopback is refused).
- Domains: `Runtime` (`enable`, `evaluate`, `getProperties`, `callFunctionOn`,
  `runIfWaitingForDebugger`), `Debugger` (`enable`, `setBreakpointByUrl`, `removeBreakpoint`,
  `setBreakpointsActive`, `pause`, `resume`, `stepOver`, `stepInto`, `stepOut`,
  `evaluateOnCallFrame`, `setPauseOnExceptions`, `getScriptSource`, `getPossibleBreakpoints`,
  `setBreakpoint`, `setVariableValue`, `setSkipAllPauses`), events `Debugger.scriptParsed` (url
  `pocket:///scripts/<module>.js`, with `sourceMapURL` as a data URL whose source is the TypeScript,
  embedded, so clients map to it), `Debugger.paused`, `Debugger.resumed`,
  `Debugger.breakpointResolved`, `Runtime.consoleAPICalled`, `Runtime.exceptionThrown`. Positions on
  the wire are the JavaScript's.
- Chrome DevTools attaches with `devtools://devtools/bundled/inspector.html?ws=127.0.0.1:<port>/devtools/game`;
  VS Code with a `node`-type `attach` configuration on the port (`editors/vscode/launch.json`).
- The editor speaks the same CDP over `/devtools/game`.
- The same core is exposed as JSON methods for agents (and MCP tools of the same names), with
  TypeScript positions, 1-based (debugger.md 7 has the parameters and results):
  `debug.attach`, `debug.detach`, `debug.breakpoints.set {file, line, condition?, log?}`,
  `debug.breakpoints.clear {id?}`, `debug.breakpoints.list`, `debug.pause {timeout_ms?}`,
  `debug.continue`, `debug.step {kind: over|into|out, timeout_ms?}`, `debug.state` (frames with
  TypeScript locations, scopes with locals, the tick and the system), `debug.eval {expr, frame?}`,
  `debug.watch {entity, component, field?}` (a data breakpoint: pause when a system's staged write
  changes it, naming the statement that wrote), `debug.unwatch {id?}`,
  `debug.exceptions {mode: none|uncaught|all}`, `debug.wait {timeout_ms?}`, and `debug.rewind {tick}`
  (restore the kept snapshot at or before `tick` and replay to it; not built: it needs the snapshot
  ring).

## 7. MCP tools

The MCP server (`pocket-mcp`) is a projection of the catalog, grouped so an agent's tool list stays
small (each tool takes a `method`-like `action` where a group is natural):

- Developer: `world` (tree/get/query/schema/edit), `scripts` (list/read/write/apply), `time`
  (control/step), `play`, `history`, `assets`, `events` (since/why), `debug` (section 6), `capture`
  (a rendered image or the id buffer's summary of a camera view), `docs` (search).
- Player (per seat, restricted perception): `observe`, `act`, `wait`, `intents`, `affordances`
  (shared/contract/mcp.md).

Every MCP call is also pushed to editors as an `agent` event, so a human sees what agents do.
