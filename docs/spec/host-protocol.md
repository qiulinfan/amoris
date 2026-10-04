# The host protocol: editor, agents and tools on one command catalog

Status: Draft 2 (2026-10-04): implemented by `pocket-server`, `pocket-mcp` and `pocket serve`; the
method table, parameters and results as built are [server.md](server.md). Charter: 3.1 (one
interface), 4.3 (debugging), 4.5 (editor).

The native host (`pocket editor <project>` / `pocket serve <project>`) owns the one authoritative
world. Everything else is a client: the React editor, MCP agents, the CLI, tests. They all call the
same commands; the editor has no private channel. This file fixes the wire forms. The command
semantics (validation, `{code, message, detail}` errors, unknown-field refusal with suggestions) are
the runtime's (`pocket-runtime` catalog, `shared/contract/errors.md`).

## 1. Endpoints

All bind `127.0.0.1` on one port (default 7878, `--port`; 0 for any free port), except the CDP
endpoint, which `pocket-debug` serves on its own port (9229). A request whose `Host` is not
`127.0.0.1`, `localhost` or `::1`, or whose `Origin` (when present) is not, is refused with
`403 host.forbidden` (DNS rebinding). `pocket serve` writes `{url, port, pid, project}` to
`<project>/.pocket/host.json`, where the CLI and `pocket mcp` find it.

| Path | Form | Who |
|---|---|---|
| `GET /` | the editor's static files (`editor/dist`) | browser |
| `GET /api/catalog` | JSON: every command with kind, doc, params JSON Schema, result schema | all |
| `POST /api/call` | JSON request, JSON response (section 2) | CLI, tests, agents without MCP |
| `GET /ws` | WebSocket, JSON text frames: requests, responses, pushed events (sections 2, 3) | editor |
| `GET /render` | WebSocket, binary frames: the render feed (section 5); `501 render.not_available` until the integrator installs it | editor viewport |
| `GET /assets/<path>` | the project's asset files (glTF, textures, splats, neural assets) | editor viewport |
| `GET /wasm/<path>` | pocket-web's wasm-bindgen output, `pocket_web.js` exporting `createViewport` (editor.md 5) | editor viewport |
| `POST /mcp` | MCP Streamable HTTP (rmcp) | agents |
| `GET /json/list`, `GET /json/version`, `WS /devtools/<id>` | CDP discovery and endpoint (section 6), on pocket-debug's port | Chrome DevTools, VS Code |

| `GET /json/list`, `GET /json/version`, `WS /devtools/game` | CDP discovery and endpoint (section 6), on `pocket-debug`'s own port (default 9229, where VS Code's `node` attach looks); the host may proxy them | Chrome DevTools, VS Code, the editor |

`pocket mcp <project>` serves the same MCP tools over stdio instead (through the running host when
there is one). The `pocket` CLI is a client of the same `/api/call` (server.md 8).

## 2. Requests and responses

Request (text frame on `/ws`, or the body of `POST /api/call`):

```json
{"id": 7, "method": "world.edit", "params": {"ops": [{"set": {"entity": 12, "component": "Model", "value": {"color": [1, 0, 0, 1]}}}]}}
```

Response:

```json
{"id": 7, "result": {"tick": 1204, "applied": 1, "spawned": [], "results": [...], "label": "set Model on #12"}}
{"id": 7, "error": {"code": "request.unknown_field", "message": "...", "detail": {"field": "colour", "suggestions": ["color"], "path": "/ops/0/set/value/colour"}}}
```

`id` is chosen by the client (number or string) and echoed. Requests on one socket are answered in
order of completion, not of sending. `POST /api/call` answers HTTP 200 with either form (400 only
for a body that is not JSON).

## 3. Pushed events (`/ws`)

The host pushes `{"event": <topic>, "data": ...}`. A client subscribes with
`{"id": n, "method": "subscribe", "params": {"topics": ["status", "log"]}}` (and `unsubscribe`);
`status` is always on and is pushed once on connect. A socket more than 1,024 pushes behind gets
`{"event": "lagged", "data": {"missed": n}}`.

| Topic | Data | When |
|---|---|---|
| `status` | `{tick, t_s, paused, pacing, halted, behind_ms, mode: "edit"|"play", epoch, world_hash, entities, tps, state, version}` | at most 10 Hz when it changed, else every second |
| `world.changed` | `{tick, spawned: [id], despawned: [id], changed: [[id, component]], truncated?}`, or `{tick, reset: true, epoch}` when the world was replaced (restore, Play, Stop) | after a tick or write that changed something, coalesced to 10 Hz |
| `events` | `[{seq, id, tick, name, subject, data, cause}]` (`seq` the stream number, `id` and `cause` the world's event numbers) | game events as they are emitted, every 50 ms |
| `log` | `{seq, level, source, message, file?, line?, tick?}` | one per line: script `console.*`, failed system runs |
| `history` | `{undo: [label], redo: [label]}` | after any edit, undo, redo, restore, Play or Stop |
| `debug` | `{state: "paused"|"running", reason?, location?: {file, line, column}, frames?}` | debugger state changes |
| `profile` | `{tick, systems: [{name, ms}], frame_ms, gpu: [{pass, ms}]}` | 4 Hz while subscribed |
| `agent` | `{session, kind: "call"|"result", method, ok?, summary}` | each MCP call by an agent and its result |

## 4. Methods

Each method is a command of the catalog (`GET /api/catalog` is authoritative). Kinds: Read, Write
(recorded, undoable), Control (time). Names are `noun.verb`.

| Method | Params | Result |
|---|---|---|
| `catalog.list` | `{}` | the catalog |
| `status` | `{}` | status (section 3) with `bundle`, `writes`, `poisoned` |
| `project.info` | `{}` | `{name, root, rate, scenes, scripts, assets}` |
| `project.save` | `{}` | `{files, entities}` (the edit world, even during Play) |
| `world.tree` | `{root?, depth?, filter?, with?, limit?}` | `[{id, name, components: [name], children: [...]}]` |
| `world.get` | `{entity, components?}` | `{id, name, components: {Name: value}}` |
| `world.query` | `{with: [component], fields?: ["C.f"], name?, limit?}` | rows `{id, name, "C.f": value}` |
| `world.schema` | `{component?}` | component JSON Schemas with docs, from the registry |
| `world.edit` | `{ops: [op], label?}` | `{tick, applied, spawned: [id], results, label}`; all or nothing |
| `history.undo` / `history.redo` | `{}` | as `world.edit`, with the entry's `label` |
| `history.list` | `{}` | `{undo, redo}` |
| `time.control` | `{pause?, speed?, pacing?}` | status |
| `time.step` | `{ticks?, until?: {event?, subject?, tick?}, watch?: {entity, component, field, op?, value?}}` | `{tick, world_hash, errors, stopped_by?}` |
| `play.start` / `play.stop` | `{speed?, paused?}` / `{}` | status (Play runs a fork of the edit world; Stop discards it) |
| `scripts.list` | `{}` | `[{path, bytes, diagnostics}]` |
| `scripts.read` / `scripts.write` | `{path}` / `{path, text}` | `{path, text}` / `{path, bytes, diagnostics}` |
| `scripts.apply` | `{files?, force?, dry_run?}` | `{outcome, bundle, diagnostics, typecheck, ...}` (type check if `tsc` is installed, compile, hot swap at a boundary) |
| `scripts.check` | `{}` | `{outcome, diagnostics, typecheck}` (no swap) |
| `assets.list` | `{dir?}` | `[{path, kind, bytes}]` |
| `assets.import` | `{path}` | `{asset, meshes, materials}` (not built yet) |
| `events.since` | `{seq?, limit?, name?}` | `{events, last, missed}` |
| `events.why` | `{seq}` | `{event, causes, complete}`: the cause chain |
| `log.since` | `{seq?, limit?}` | `{lines, last}` |
| `snapshots.list` / `snapshots.restore` | `{}` / `{tick}` | `{every, keep, snapshots: [{tick, world_hash}]}` / status with `restored` |
| `docs.search` | `{query, limit?}` | matching commands and components |
| `debug.*` | section 6 | `debug.not_available` until pocket-debug's hub is installed |
| `profile.frame` | `{}` | the last frame's CPU and GPU timings (not built yet) |
| `agent.*` | MCP tool calls mirrored (section 7) | |

`world.edit` ops (the runtime's `world_edit` ops, one transaction):

```json
{"spawn": {"name": "Crate", "prefab": {...}, "components": {"Transform": {...}, "Model": {...}}}}
{"set": {"entity": 12, "component": "Transform", "value": {...}, "replace": false}}  // fields merge
{"remove": {"entity": 12, "component": "Model"}}
{"destroy": {"entity": 12}}
```

Each is recorded as the runtime's `world_edit` with names resolved to ids; undo restores a destroyed
entity under its id with every component (server.md 3.1).

## 5. The render feed (`/render`)

The viewport renders the host's world with the same renderer compiled to wasm (WebGPU). On each
socket the host subscribes a mailbox to the game's render feed (`pocket-assets`' `Feed`; the game
thread extracts after each tick with change detection, `pocket-runtime/src/present.rs`) and sends
the merged frame at up to 60 per second as one binary message:

- the bytes are `RenderFrame::encode`: 8 bytes of `FORMAT` (little endian; a fingerprint of the
  sources of the feed's types), then the frame in `bincode` 2's standard configuration. A viewport
  built from another revision refuses the frame and logs that it must be rebuilt, instead of
  misreading it;
- the first frame after connecting, and after a restore, a reload or Play/Stop (a new world or
  generation), has `reset: true` and carries everything; later frames carry what changed;
- while the world is paused the game thread still wakes every 50 ms when a subscriber waits for a
  full frame.

The viewport owns its camera, picking (a CPU ray against the drawn instances' oriented boxes) and
overlays; the client sends nothing on this socket. The same frames reach a browser game: the
worker (`web/game.js`) extracts them when the page subscribes (`pocket.onRender`) and transfers the
bytes to the page's viewport.

## 6. Debugging

`pocket-debug` serves the Chrome DevTools Protocol for the game's scripts ([debugger.md](debugger.md)
is the specification of what is built):

- `GET /json/list` and `/json/version` list one target, `pocket-game`, with
  `webSocketDebuggerUrl: ws://127.0.0.1:<port>/devtools/game` (port 9229 by default, its own, not the
  host's; a request whose `Host` or `Origin` is not loopback is refused).
- Domains: `Runtime` (`enable`, `evaluate`, `getProperties`, `callFunctionOn`,
  `runIfWaitingForDebugger`), `Debugger` (`enable`, `setBreakpointByUrl`, `removeBreakpoint`,
  `setBreakpointsActive`, `pause`, `resume`, `stepOver`, `stepInto`, `stepOut`,
  `evaluateOnCallFrame`, `setPauseOnExceptions`, `getScriptSource`, `getPossibleBreakpoints`,
  `setBreakpoint`, `setVariableValue`, `setSkipAllPauses`), events `Debugger.scriptParsed` (url
  `pocket:///scripts/<module>.js`, with `sourceMapURL` as a data URL whose source is the TypeScript,
  embedded, so clients map to it), `Debugger.paused`, `Debugger.resumed`,
  `Debugger.breakpointResolved`, `Runtime.consoleAPICalled`, `Runtime.exceptionThrown`. Positions on
  the wire are the JavaScript's.
- Chrome DevTools attaches with
  `devtools://devtools/bundled/js_app.html?experiments=true&v8only=true&ws=127.0.0.1:<port>/devtools/game`
  (the page `chrome://inspect` opens for a Node target); VS Code with a `node`-type `attach`
  configuration on the port (`editors/vscode/launch.json`). `debug.state` names the endpoint while it
  runs (`cdp: {ws, devtools}`, `devtools` being that URL); it is the authority on the port.
- The same core is exposed as JSON methods for agents, with TypeScript positions, 1-based
  (debugger.md 7 has the parameters and results; MCP's `debug` tool takes some of them as actions,
  section 7):
  `debug.attach`, `debug.detach`, `debug.breakpoints.set {file, line, condition?, log?}`,
  `debug.breakpoints.clear {id?}`, `debug.breakpoints.list`, `debug.pause {timeout_ms?}`,
  `debug.continue`, `debug.step {kind: over|into|out, timeout_ms?}`, `debug.state` (frames with
  TypeScript locations, scopes with locals, the tick and the system), `debug.eval {expr, frame?}`,
  `debug.set {name, value, frame?}` (a frame's variable to an expression's value),
  `debug.watch {entity, component, field?}` (a data breakpoint: pause when a system's staged write
  changes it, naming the statement that wrote), `debug.unwatch {id?}`,
  `debug.exceptions {mode: none|uncaught|all}`, `debug.wait {timeout_ms?}`, and `debug.rewind {tick}`
  (restore the kept snapshot at or before `tick` and step to it; served by `pocket serve`'s bridge,
  server.md 3.3). Pauses and resumes are pushed as `debug` events, console lines as `log`.
- The editor uses these JSON methods over `/ws` (editor.md 8.1), not CDP.

## 7. MCP tools

The MCP server (`pocket-mcp`) is a projection of the catalog, grouped so an agent's tool list stays
small (each tool takes a `method`-like `action` where a group is natural):

- Developer (built, server.md 7): `world` (tree/get/query/schema/edit), `scripts`
  (list/read/write/apply/check), `time` (status/pause/resume/speed/step/snapshots/rewind), `play`
  (start/stop), `history` (undo/redo/list), `assets` (list), `events` (since/why/log), `debug`
  (breakpoints.set/breakpoints.clear/pause/continue/step/state/eval/watch/rewind: section 6's
  methods of those names; `breakpoints.list`, `unwatch`, `set`, `exceptions`, `wait`, `attach` and
  `detach` are not actions yet), `capture` (a rendered image or the id
  buffer's summary of a camera view; not available yet), `docs` (search). `tools/list` is 1,236
  tokens.
- Player (per seat, restricted perception): `observe`, `act`, `wait`, `intents`, `affordances`
  (shared/contract/mcp.md); not projected yet.

Every MCP call is also pushed to editors as an `agent` event, so a human sees what agents do.

## 8. Forms the editor relies on

The editor ([editor.md](editor.md) 8) and its mock host settle forms this draft leaves open:
`world.edit` takes `group` (consecutive edits of one group are one undo entry, so a drag streams
live and undoes at once), `set` of `Name` takes a string, the hierarchy is a `Parent` component,
`debug.step` takes `{kind}`, the shapes of the `debug.*` results, `debug.breakpoints.list` and
`debug.unwatch`, the order of `redo` (next first) and the `agent:` prefix of agent edits' labels.
The host adopts them, or the editor changes with this file.
