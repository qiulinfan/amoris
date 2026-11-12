# The host's server, CLI and MCP: the host protocol as built

Status: Draft 2, implemented (2026-10-04; 2026-10-09, Pioneer: a host the debugger holds answers
at once, restores keep the applied scripts, the debugger in the catalog, field-level reads, from
[the debug evaluation](../bench/debug-eval.md) and measured in
[agent-debug.md](../bench/agent-debug.md)). Wire forms: [host-protocol.md](host-protocol.md).
Charter: 3.1 (one interface), 4.3 (debugging), 4.5 (editor). Crates: `amoris-runtime` (commands,
history, the game thread's Play and kept snapshots), `amoris-link` (the event and log streams),
`amoris-server`, `amoris-mcp`, `amoris-app` (`amoris serve`, `amoris mcp`, the CLI client).

This file says what the host serves today: how to start it, every method with its parameters and
result, what the server answers itself, the pushes, the CLI, the MCP tools, and the three places the
integrator plugs in (the render feed, the debugger, captures). host-protocol.md stays the contract;
where they differ, this file is what runs and host-protocol.md has been updated.

## 1. Starting it

```sh
amoris serve samples/sailing [--port 7878] [--editor DIR] [--seed S] [--grants FILE]
amoris status                  # any client command, from the project's directory
amoris mcp samples/sailing     # MCP over stdio (uses the running host if there is one)
amoris mcp --attach <url|port> [--grant-token T]   # MCP over stdio as a session of that host
```

- `amoris serve` loads the project, builds its game on the game thread (threads.md 3) in real time
  at speed 1, **paused** (the edit world), and serves 127.0.0.1:`port` (0: any free port). It prints
  one JSON line to stdout, `{url, port, pid, project, mcp, editor}`, and writes the same to
  `<project>/.amoris/host.json`, which it removes on Ctrl-C or SIGTERM. `project` (and
  `project.info`'s `root`) is the canonical directory in its plain form: on Windows without the
  verbatim `\\?\` prefix `canonicalize` adds, unless only the verbatim form can name the path
  (`amoris_server::hostfile::project_root`), so it equals what other programs resolve the directory
  to (the desktop compares it with Node's `fs.realpath`). The editor's files come from
  `--editor`, `AMORIS_EDITOR_DIST`, or the first `editor/dist` with an `index.html` above the
  working directory or the executable; without one, `GET /` is a page listing the endpoints.
- `amoris mcp <project>` serves one MCP session over stdio. When `.amoris/host.json` names a host
  that answers, the session calls it over HTTP (agents and the editor then share one world);
  otherwise, or with `--own`, it runs the game in its own process. Nothing but MCP goes to stdout.
  Attached to a host it is an agent session of that host (3.5; 2026-11-12): it registers with
  `agent.session.open` when its MCP client says who it is (with `--seat`, as that seat's player),
  makes its calls with the handle it was given, registers again when the host no longer knows the
  handle (the host started again), and closes its session when the client closes stdin, on Ctrl-C
  and on SIGTERM.
- Client commands (section 8) find the host through `--host <url|port>`, `AMORIS_HOST`, or the
  nearest `.amoris/host.json` at or above `--project <dir>` or the working directory.
- 2026-11-12, grants (3.6). `amoris serve --grants FILE` locks the host: every request then needs
  the token of one of the file's grants, the readiness line and `.amoris/host.json` say
  `"locked": true` (and never a token; an unlocked host writes neither key), and the debugger's CDP
  endpoint is not started. `amoris mcp --attach <url|port>` names the host to be a session of (the
  project argument is then not needed, and no game of its own is ever started in the host's place);
  with `--grant-token T`, or `AMORIS_TOKEN`, the session is that grant's: it asks the host what the
  token is (`GET /api/grant`) before anything goes to stdout, makes every call with the token as
  its bearer, and registers, holds and closes nothing. `--own` with `--attach` or `--grant-token`,
  and `--seat` with a token (a grant fixes its seat), are usage errors (exit 2). `amoris mcp
  <project>` without a token, when the project's host is locked, exits 1 with `host.unauthorized`
  instead of running a private game.

## 2. Architecture

```text
 editor (/ws)   CLI, tests (/api/call)   MCP clients (/mcp, stdio)
        \              |                    /
         amoris-server: Host::call(via, method, params)
           |  answered here: catalog, events.*, log.since, profile.frame, assets.*,
           |  docs.search, scripts.guide, the type check stage, debug.* and capture (plug-ins)
           v
   amoris-link: GameClient per source (Editor; one Developer for the API; one per MCP session)
           |                         ^ SnapshotReader: snapshots, event ring, log ring,
           v                         | status, world mode and epoch
   game thread (amoris-runtime thread::Loop): Game::apply, time model, Play, kept snapshots
```

- The server holds no game. It reaches the game thread only through `amoris-link`
  ([architecture.md](architecture.md) 5): commands go through a `GameClient` with a reply closure
  that completes a tokio oneshot, so no server thread blocks on the game; everything it pushes is
  read from the `SnapshotReader`.
- Sources (threads.md 5.1): every editor socket shares `Source::Editor`; `/api/call` (the CLI,
  tests) shares one `Developer(n)`; each MCP session opens its own `Developer(n)`. A boundary
  applies the editor's commands before developers' (threads.md 5.2). 2026-11-12: one developer
  client per agent session (3.5), a bridge that registered itself included, so `amoris mcp`
  attached to a host no longer shares the API's client; `agent.session.*` is answered here. A
  checker (3.6) is a developer source too: the game does not know the role, and the server keeps
  its calls to the catalog's reads before they reach a client (`Via::Checker`).
- Threads: the game thread; tokio's worker threads (`amoris-server`) for HTTP, WebSocket, MCP and
  the pushers. A game stuck in a tick (a breakpoint) leaves the server answering and pushing the
  last snapshot, and no call waits on it (3.4): reads answer from the last publication, others are
  refused at once with `debug.paused`. A game busy in a long tick or a long step is waited for.

## 3. Methods

Every method is a catalog entry (`GET /api/catalog`, `catalog.list`):
`{name, kind, doc, aliases, thread, params}` with `params` the JSON Schema generated by `schemars`
from the type the runtime decodes (unknown fields are refused with suggestions,
`shared/contract/errors.md`). Entries with `"server": true` are answered by the server. Kinds: read,
write (recorded, undoable for edits), control (time), request (stages off the world: compiles,
files).

| Method | Kind | Params | Result |
|---|---|---|---|
| `catalog.list` | read | `{}` | the catalog |
| `status` | read | `{}` | `{tick, t_s, pacing, paused, halted, behind_ms, mode, epoch, world_hash, entities, bundle, writes, poisoned, steps_due, kept, tainted}`; `poisoned` is the tick that stopped halfway, else null, and while it is set `halted` is true and `world_hash` is null (a world between boundaries has no hash); while the debugger holds the game, from the last publication with `state: "breakpoint"` and `paused_at` (3.4) |
| `project.info` | read | `{}` | `{name, root, rate, scenes, scripts, assets}` |
| `project.save` | request | `{}` | `{files: ["scene.json"], entities}`: the edit world, one entity per line, every component whole, entity ids as places in the file (simulation.md 7.4) |
| `world.tree` | read | `{root?, depth?, filter?, with?, limit?}` | `[{id, name, components: [name], children: [...], more?}]`: the roots (entities without a live parent) with their children nested by `Parent`, roots and children in ascending id order; `root` lists one entity with what is under it; `depth` is the levels below the roots, by default and at most 48, and an entity whose children it left out carries their number in `more` (read on with `root`); `filter` and `with` list what matches with the entities it is under; `limit` counts listed entities, a parent before its children |
| `world.get` (`world_get`) | read | `{entity, components?, fields?}` | `{id, name, components: {Name: value}, fields?: {"C.f.path": value}}`; `fields` (or a dotted entry of `components`) reads `Component.field` or a path into it alone, `null` where the entity lacks the component; a path the component lacks is refused with its fields and a suggestion |
| `world.query` | read | `{with: [C], fields?: ["C.f.path"], name?, limit? = 100}` | `[{id, name, "C.f": value}]` |
| `world.schema` | read | `{component?}` | `[{name, origin, version, doc, schema}]`, or one |
| `world.edit` | write | `{ops: [op], label?, group?}` | `{tick, applied, spawned: [id], results, label}`; all or nothing; consecutive edits of one `group` from one caller are one undo entry (3.1) |
| `world_edit` | write | `{edits: [{op: ...}]}` | the canonical recorded form (below) |
| `scene.load` | write | `{path, mode? = "replace"}` | `{path, mode, spawned, first, despawned}`: another scene file of the project spawned at this boundary, replacing the world's entities or beside them; not undoable, clears the history; recorded as `scene_load {path, mode, scene}` with the scene's bytes as the recording's data (simulation.md 7.6) |
| `history.list` | read | `{}` | `{undo: [label], redo: [label], entries: {undo: [entry], redo: [entry]}}`: each stack most recent last (the next to undo or redo last), `entries` the same entries in the same order as `{label, source, group?, session?}` (3.1) |
| `history.undo` / `history.redo` | write | `{group?}` / `{}` | as `world.edit`, with the entry's `label`; an undo with `group` only when the next entry is the whole of that group, else `history.group` (3.1) |
| `time.control` (`time_control`) | control | `{pause?, speed?, pacing?}` | `status`; `speed` is real time at that speed, `pacing` the pacing itself (it wins when both are given); both are checked before either is applied, so a refused call changes nothing |
| `time.step` (`step`) | control | `{ticks?, until?, watch?, sample?, trace?}` | `{tick, world_hash, errors, stopped_by?, samples?, writes?}` (`writes`: the write trace, debugger.md 13); at a debugger stop `{tick, world_hash: null, errors: [], paused: true, stopped_by}` (3.2, 3.4) |
| `play.start` | control | `{speed? = 1, paused?}` | `status` (mode `play`) |
| `play.stop` | control | `{}` | `status` (mode `edit`) |
| `snapshots.list` | read | `{}` | `{every, keep, snapshots: [{tick, world_hash}]}` |
| `snapshots.restore` | control | `{tick, bundle? = "applied"}` | `status` with `restored` (the kept tick at or before `tick`) and `scripts: {bundle, kept: "applied"\|"snapshot", snapshot_bundle, swapped?, swap_refused?}` (3.3) |
| `snapshot` | read | `{}` | `{tick, writes, world_hash}` |
| `saves.list` | read | `{}` | `{dir, saves: [{name, bytes, label, tick, world_hash}]}` (a save whose header does not read: `{name, unreadable: code}`; 3.3) |
| `saves.write` | request | `{name, label?}` | `{name, path, bytes, tick, world_hash}`: the world shown written as a save (3.3) |
| `saves.load` | control | `{name}` | `status` with `loaded: {name, tick, world_hash, migrated, renamed, dropped, caches_rebuilt}` (3.3) |
| `scripts.list` | read | `{}` | `[{path, bytes, diagnostics}]` |
| `scripts.read` | read | `{path, lines?, numbered?}` | `{path, text}`; with `lines` (`50-80`, `50-`, `-20`, `67`; 1-based, inclusive) or `numbered`, the text of those lines (each `67\| ` prefixed when numbered) and `{first, last, total}` |
| `scripts.write` | request | `{path, text}` | `{path, bytes, diagnostics}` (compiled, not swapped) |
| `scripts.apply` | request | `{files?, force?, dry_run?}` (the server adds `types`) | `{outcome, bundle, previous, systems, components, applied_at, diagnostics, typecheck, tsc_ms, tsc_version, types}` |
| `scripts.check` | read (server) | `{}` | `{outcome, bundle, diagnostics, typecheck, tsc_ms, tsc_version, reason?, types}`; `refused` when the scripts compile but would not load |
| `scripts.types` | read | `{text?, tsconfig? = true}` | `{dir, files, tsconfig, components: {engine, project, unavailable}, project_from, diagnostics, text?}` (docs/sdk.md); `tsconfig: false` writes under `.amoris/types/` only |
| `scripts.guide` | read (server) | `{}` | `{path: "docs/sdk.md", text}`: the agent's guide to scripts, embedded in the server |
| `scripts.status` | read | `{}` | `{bundle, ran_last_tick}` |
| `scripts.profile` | read | `{ticks? = 60, interval?, top? = 10, folded? = false}` (`ticks` at most 3,600; `interval` 1 to 1,000,000, by default the run's steps over 20,000) | `{from_tick, to_tick, ticks, interval, steps, samples, estimated, systems: [{system, calls, steps, samples, functions, lines, stacks, more?}], stopped_by?, folded?}`: the step profiler over the next ticks, run on a fork; the world does not move (debugger.md 14) |
| `events.since` | read (server) | `{seq?, limit? = 100, name?}` | `{events: [event], last, missed}` |
| `events.why` | read (server) | `{seq}` | `{event, causes: [event], complete}` |
| `log.since` | read (server) | `{seq?, limit? = 100}` | `{lines: [log line], last}` |
| `profile.frame` | read (server) | `{ticks? = 60}` (at most 600) | the `profile` summary (4) of the last `ticks` ticks profiled |
| `assets.list` | read (server) | `{dir?}` | `[{path, kind, bytes}]`, served at `/assets/<path>` |
| `assets.upload` | request (server) | `{path, data, offset? = 0, done? = true, overwrite? = false}` | `{path, received}` for a part before the last, `{path, kind, bytes}` for the last: a file into the project, its bytes in base64 parts (RFC 4648's standard alphabet, padded) of at most 1 MiB (`UPLOAD_PART`), so a part's call fits the HTTP API's 2 MB body. Offset 0 begins (again) a staging file under `.amoris/uploads/`, each later part must start where the received ones end (else `assets.upload_offset {path, expected, got}`), and the last part moves it to `path`, creating its directories. Refused, changing nothing: a `path` not inside the project (`..`, a root, a dot file or directory, a trailing `/`, a directory that a link leads out of the project) or `data` that is not base64 or holds over 1 MiB (`request.invalid_value`), and a file already at `path` without `overwrite` (`assets.exists`) |
| `docs.search` | read (server) | `{query, limit? = 10}` | `[{command, doc}`, `{component, doc, fields}` or `{guide, read}]` (a matching section of `scripts.guide`) |
| `debug.*` | (server) | per amoris-debug (debugger.md 7), one catalog entry each with its schema | `debug.not_available` until a `DebugHub` is installed; `debug.rewind {tick, bundle?}` and `debug.writes {entity, component, field?, ticks?}` (debugger.md 13) are the server's own |
| `capture` | read (server) | `{position?, look_at? \| rotation?, fov_deg? = 60, width? = 1280, height? = 720, frames? = 0}` (no `position`: the scene's camera; `look_at` defaults to the origin; `rotation` a quaternion `[x, y, z, w]`; `frames` at most 1,200) | `{path, width, height, tick, instances, gpu_ms, pending_assets, spray}`: a PNG under `.amoris/captures/` (amoris-app's `present.rs`); `capture.not_available` until a `CaptureHub` is installed |
| `subscribe` / `unsubscribe` | `/ws` only | `{topics: [topic]}` | `{topics}` |
| `agent.session.list` | read (server) | `{}` | `{sessions: [row], last}`: the host's agent sessions in opening order and the newest stream number (3.5) |
| `agent.session.events` | read (server) | `{since?, limit? = 100, session?}` (`limit` at most 2,000) | `{events: [event], last, missed}`: with `since` the first `limit` after it, without it the newest; `last` is the cursor the answer reaches (3.5) |
| `agent.session.open` | control (server) | `{label?, seat?}` | the session's row and `handle`: a bridge session, a developer's or with `seat` that seat's player (3.5) |
| `agent.session.close` | control (server) | `{handle}` or `{session}` | the row, no longer live (3.5) |
| `agent.session.agents` | read (server) | `{}` | `{agents: [{name, label}], running, limit}`: the agent programs the host may start (3.7) |
| `agent.session.start` | control (server) | `{agent, goal, seat?}` | the new session's row (transport `launch`), its `agent` and `pid`: the program runs as that session (3.7) |
| `agent.session.stop` | control (server) | `{session}` | the started session's row once its program ended (3.7) |

Names follow `noun.verb`; the slice 1 names stay as aliases (`world_get`, `step`, `time_control`).
`thread: true` marks commands that need the game thread: a `Game` driven directly (checks, replays)
refuses them with `command.thread_only`.

### 3.1 `world.edit` and the history

Ops are externally tagged: `{"spawn": {name?, prefab?, components?}}`,
`{"set": {entity, component, value, replace?}}` (fields merge; keys may be paths, `position.1`;
`replace` writes the value whole; for `Name` the value is the name, a string: Names, below),
`{"remove": {entity, component}}`, `{"destroy": {entity}}`. An entity is an id, a name or `Name#id`.
The call is checked whole against the world as the earlier ops leave it, then applied at one
boundary; a refusal names the op as written (`/ops/1/set/entity`).

The hierarchy (2026-11-12; simulation.md 7.1 and 7.3): an entity is placed under another by a `set`
of `Parent {parent}` (the parent's id), taken out by a `remove` of it, and spawned under one by
naming `Parent` among the spawn's components.

- A `destroy` takes the entity and every entity under it and answers how many went
  (`{op: "destroyed", count}`). A later op of the same call naming one of them is refused like one
  naming the destroyed entity (`sim.entity_not_found` with `destroyed_by`), a second destroy
  included: destroy children before their parent, as the editor does, or the parent alone.
- A `Parent`'s parent must be alive as the earlier ops leave the world (`sim.entity_not_found`,
  `sim.entity_id_invalid`; `path` is the parent in the request, `/ops/0/set/value/parent`) and must
  not be the entity itself.
- Whether a `Parent` closes a loop is judged once, on the hierarchy the whole call leaves
  (`sim.parent_cycle {entity, parent, chain, path}`). A call may pass through a loop: swapping a
  parent with its child takes two sets, and the hierarchy between them holds one.
- A `Parent` names its parent by id: `world.edit` does not resolve a name inside an engine
  component's value.

Every edit is recorded as `world_edit {edits}` with entity names resolved to ids (threads.md 5.3),
the same write for `world.edit`, undo and redo, so a recording replays without the history.

The history (`amoris-runtime` `history.rs`) holds, for each edit from the editor or an agent, the
edits that undo it, computed at the boundary from the call's **net effect** on each entity it
touched: entities it spawned (or revived) are destroyed; entities it destroyed are **revived under
their ids** with their name and every component (`{op: "revive"}` in `world_edit`, which calls the
new `amoris_sim::entity::revive`; `sim.entity_alive` when the id is live); components it set or
removed on the other entities get their values from before the call back (`set` with `replace`, or
`remove`). The list is in the order that applies as one call (2026-11-12): first one `revive` for
everything the call destroyed, the entities a destroy took from under its entity as the edit's
`others: [{entity, name?, components?}]` (so the undo of any subtree is one edit, within the 64 of a
call; the entities of a `revive` are checked together, and an entity field or a `Parent` of one may
name another, or an entity an earlier edit of the call revives); then the sets and removes; then the
destroys, of the topmost entities alone, since a destroy takes what is under it. A restore may thus
put an entity back under one the revive brings back, and takes an entity out from under one a
destroy removes before that destroy. Undo applies that list as an edit and keeps the list's own
inverse for redo; a new edit empties redo. 500 entries per stack. An entry whose edits no longer
apply (a tick destroyed what it names) is dropped with `history.stale`; an empty stack answers
`history.empty`. A restore (and Play's fork) starts with an empty history. The history is never
persisted or hashed.

Groups (2026-10-15, editor.md 8 item 1): `world.edit` takes `group`, a string the caller picks. An
edit of a group right after an edit of the same group by the same source merges into that entry
(`history.rs` `merged`): the later edit's inverse goes first, without the sets and removes of
components the entry's inverse already restores (it writes them whole, or removes them, after), and
the entry takes the later label. A gizmo drag streams one edit per pointer move and its later
inverses all drop out, so its entry undoes to the value from before the drag in one call. An edit
of another group or of none, an edit by another source (an agent's edit during a drag splits it in
two), an undo or a redo in between (an entry undone or redone loses its group), an edit that spawns,
destroys or revives (its inverse could not come before the entry's in one call) and a merge past
the 64 edits of one `world_edit` each start a new entry. Only the history merges: every edit of a
group is applied, published and recorded on its own (`group` is not in the canonical form), so a
recording replays the same with or without groups.

Sources (2026-10-15, editor.md 8 item 5): every entry keeps the source of the edit that made it (an
undo or redo keeps it), as amoris-link's `source_json` writes it: `"editor"`, `{"developer": n}`,
`{"player": n}`. The server adds `session` to an entry whose source is a developer client it handed
out: `mcp-<n>` for an MCP session (as the `agent` events name it) and `api` for the HTTP API's
client (the CLI's), in `history.list`'s answer and the `history` push alike. The editor marks every
entry whose source is not the editor's. Both stacks list the next entry to take last, as before
sources; the editor shows the redo stack reversed (editor.md 8 item 5).

Cancelling a drag (2026-10-15): `history.undo {group}` undoes only when the next entry to undo
carries `group` and no other undo entry does, so an editor's Escape takes back its own whole drag
and nothing else, in one call: when another source's edit came after the drag or split it, the
undo is refused with `history.group` (`detail.entries` counts the group's undo entries) and
nothing changes; the editor then writes the dragged fields back instead (editor.md 6).

The server pushes `history` only when it differs from the last one it pushed (amoris-server
`push_history`): the steps of a streamed drag merge into its entry and leave the list as it was,
so a drag pushes it once, when its entry appears.

Names (2026-11-11, editor.md 8 item 2): `Name` is the entity's name and the one component whose
`value` is a string, of at most 64 bytes of UTF-8 (`Name::new`); the catalog's schema of a `set`'s
`value` is the object of fields or that string (`SetValue`). A `set` of `Name` renames the entity,
or names one that had none; a name is written whole, so `replace` changes nothing. An object given
for `Name`, or a string given for another component an edit writes, is `request.wrong_type` at the
op's `value` (`/ops/0/set/value`; a value that is neither is refused there by the shape,
`object or string`), and a longer name is `sim.name_too_long` with `path` the same place; nothing is
applied. A `remove` of `Name` leaves the entity unnamed, as one spawned without `name`: `world.tree`
and `world.get` answer `name: null`, and only its id finds it. Either is one history entry whose
undo and redo put the names back (the undo of a first naming removes the name, so the world hashes
as before it), and is recorded as `world_edit` with the entity's id and the same value, so
recordings made before, which hold objects only, decode as they did.

The later ops of a call find entities by the names its earlier ops leave, as their `entity` and
through an entity field of a project component: after a rename the new name finds the entity and
the old one is `sim.entity_not_found`, unless another entity carries it by then (two entities can
swap names in one call); a name two entities share by then is `request.ambiguous_ref` (`Name#id` or
the id says which), and a removed name finds nothing. An entity an earlier op destroys keeps its
name until the call is applied: naming it is refused with `destroyed_by`, and a name it shares
stays ambiguous. An entity the call spawns still cannot be named by a later op. In every case an
op's `entity`, or an entity field of a scene or an edit, that finds no entity says where it was
written (`path`: `/ops/1/set/entity`, `/ops/1/set/value/take`). The recorded forms hold ids, so a
replay looks no name up.

What an edit cannot write (2026-11-11): a spawn takes its name as `name`, and `Name` among its
`components` is refused (`request.invalid_value` at `ops[0].spawn.components.Name`). The registry's
other components that are neither engine components with a JSON form nor project components are
state the engine keeps itself (a seat's `Controls`, an observer's `ObserverMemory` and
`ObserverEvents`): a `set`, a `remove` or a spawn naming one is `request.invalid_value` at the op's
`component` (a spawn's `components.<Name>`), in a scene too. Until then each of these, `Name`
included, was answered `sim.component_unknown` with the registry suggesting the very name ("There
is no component 'Name'; did you mean [\"Name\"]?"); an edit or a scene now answers that code only
for a name the registry does not hold.

### 3.2 `time.step` stop conditions

`ticks` is the most ticks to run (default 1; 3600 with a condition; at most 1,000,000). Conditions
are checked after each tick on the world it left:

- `until: {event, subject?}`: an event of that name (or a prefix written `crate.*`) emitted by the
  tick, about `subject` if given; `until: {tick}`: the world reached the tick.
- `watch: {entity, component, field, op?, value?}`: `field` is a path into the component's JSON
  (`speed`, `position.y`, `position.1`); `op` is `changes` (default without `value`), `crosses`
  (default with `value`: the field passed `value` in either direction), `>`, `>=`, `<`, `<=`, `==`,
  `!=`.

`stopped_by` is `{reason: "event", event}`, `{reason: "watch", entity, field, value, previous}` (or
`gone: true`), `{reason: "tick"}`, or `{reason: "limit"}` when a condition never held. On the game
thread other sources' commands still land between the ticks of a long step (threads.md 5.4).

The script debugger ends a step too: when it stops the game inside a tick of the step (a
breakpoint, a data watch, a `debugger;` statement, an exception it pauses on, a debugger step), the
step ends with that tick and the rest is dropped, so `debug.continue` finishes the tick and the
world stands at its end. The game thread answers that step at the stop, not at the resume: before
each tick of a step the loop hands the step's reply to the loop state, and the stop's record
(amoris-link `StateHandle::stopped`) answers it `{tick, world_hash: null, errors: [], paused: true,
stopped_by}`. `stopped_by` is then the stop's summary, `{reason: "breakpoint" | "data_breakpoint" |
"debugger_statement" | "exception" | "step" | "pause", tick, system, location: {file, line,
column}, breakpoint?, watch?: {id, entity, component, field, before, after, written_at},
exception?}`. Only the step the stopped tick belongs to is answered and ended; a step queued
behind it (another client's, or a second call) has run none of its ticks, runs all of them when the
game goes on, and its caller hears `debug.paused {queued: true}` (3.4).

`sample: {fields: ["Sloop.Boat.rudder", "3.Transform.position.y"], every? = 1}` reads each
`Entity.Component.field` (entity by name, id or `Name#id`; a path after the field) after every
`every`-th tick of the step and after its last, and answers `samples: {columns: ["tick", ...],
rows: [[tick, value, ...]], every}`; at most 10,000 rows (`ticks / every`), a field whose entity is
gone `null`. A step stopped by the debugger and answered at the stop has none.

### 3.3 Play, Stop, kept snapshots

- `play.start` forks the edit world (`Game::fork`, an on-demand fork, charter 3.4) and runs the fork
  in real time at `speed`; the edit world, its time model and its kept snapshots are parked. Every
  command then goes to the fork; `project.save` still saves the edit world. `play.stop` drops the
  fork and returns to the parked edit world exactly as it was. A `time.step` waiting when the world
  switches is answered `play.started` / `play.stopped`.
- The loop keeps a snapshot of the world shown every 60 ticks, the last 120, plus its first state
  (`ThreadOptions::keep_every`, `keep`). `snapshots.restore {tick}` restores the latest kept one at
  or before `tick`, drops the later ones and clears the history; it does not replay forward (that
  is `debug.rewind`, the host's).
- A restore keeps the scripts applied since the snapshot (2026-10-09, Pioneer; the debug
  evaluation's second finding: the natural loop, fix, `scripts apply`, restore, replay, silently
  replayed the old code, and three agents reported a fix live on a host running the bug).
  Programs hold no state (charter 3.2), so the snapshot's world runs under the bundle applied now:
  it is restored under the bundle it was kept with and swapped to the applied one by the hot
  update's Host write, `scripts.swap` (`Game::swap_bundle`), which migrates components a newer
  bundle changed; a recording holds the restore's rebase and then the swap, as they ran, and
  replays identically (amoris-runtime `tests/agent_reads.rs`). `{bundle: "snapshot"}` keeps the
  snapshot's own bundle (the old behaviour). The answer's `scripts` names the bundle that runs,
  `kept`, the snapshot's bundle and `swapped`; when the applied scripts do not load on the restored
  world it runs the snapshot's and says so (`swap_refused`). The status line prints
  `restored tick 0; scripts 4381c2… (applied)`, and `status` always has the running `bundle`.
- **Saves** (2026-11-12; versions.md 6). `saves.write {name, label?}` writes the world shown
  (Play's while playing, unlike `project.save`) with amoris-persist's `write_save` to
  `<project>/.amoris/saves/<name>.save`, through a temporary file so a failed write keeps the save
  it would replace; the header's meta holds the tick and world hash that `saves.list` shows. A
  name is 1 to 64 letters, digits, `_` and `-` (`request.invalid_value`); a game without a project
  directory answers `project.no_root`. `saves.load {name}` reads the save into the current versions
  (`load_save`: renames, the engine's steps, retired types dropped, a cache of another identity
  listed and rebuilt from the components after the restore) and restores it as
  `snapshots.restore` restores a kept snapshot: the world shown is replaced under the scripts
  applied now, a waiting `time.step` is answered `saves.loaded`, the history is cleared, kept
  snapshots after the save's tick are dropped and the epoch is bumped. A recording sees a rebase
  onto the loaded world (replay.md 2.5), and a second one after a cache rebuild. TypeScript steps
  of project components are not built (versions.md 8.3), so a save holding an older version of
  one is refused with `migrate.missing_step` and the world is left as it was. A game driven
  directly (`Game::apply`, the tests, `amoris run`) loads too, without the loop's bookkeeping.
  Scripts neither save nor load yet (11).
- Each replacement of the world shown (Play, Stop, restore, a loaded save) bumps the reader's
  **epoch** (`SnapshotReader::world()`: `{mode, epoch}`); presenters reset what they derived (the
  render feed's `reset: true`, `world.changed {reset}`).

### 3.4 A host the debugger holds

Pioneer, 2026-10-09: the debug evaluation's first finding. A breakpoint holds the game thread
inside a tick (threads.md 3.5), and every call that went to the game thread waited for the resume:
`amoris step 1` with a breakpoint set did not return, nor did `status`, `world get` or `scripts
read`, and the CLI had no read timeout, so 28 calls in 11 runs hung until the agent's tool gave up,
21 % of all agent time. Now no call waits on a held game:

- The debugger records each stop in the loop state with its summary (amoris-link
  `StateHandle::stopped`; `SnapshotReader::debug_stop`, `debug_stops`), so the server knows where
  the game stands without asking the hub.
- **A step answers at the stop.** The game thread answers the `time.step` whose tick stopped,
  from inside the stop (3.2), and the loop ends that step with the stopped tick. Measured
  (provisional, a loaded Windows machine; agent-debug.md): 0.4 ms from the call to the answer over a
  kept-alive HTTP connection on the sailing sample (`tools/host/held_host_review.py`), tens of
  milliseconds through the CLI, most of it the process's start. (Until the review of 2026-10-09 the
  server looked for a stop every 5 ms and answered every waiting `time.step` at it; a second step
  queued behind the stopped one was told it had stopped and then ran all its ticks after the
  resume.)
- **Reads answer from the last publication**, the world at the boundary before the tick the game
  stands in: `status` (with `state: "breakpoint"` and `paused_at`) and `time.control {}` (which
  changes nothing and is the status), `world.get`, `world.tree`, `world.query`, `world.schema`,
  `scripts.status` (the bundle in the snapshot's header) and `snapshot`, each marked `paused_at`
  (`world.get`, `scripts.status`, `snapshot`: the stop's summary with `snapshot_tick`; each row of
  `world.tree` and `world.query`: the stopped tick), and `scripts.list` and `scripts.read` from the
  project's files (`diagnostics` empty). The snapshot's JSON is the registry's format of each
  section, which equals what the game's own reads answer (`paused_host.rs` compares them call by
  call). `catalog.list` (fetched before serving), `docs.search` (component schemas from the
  publication's registry), `events.*`, `log.since`, `profile.frame`, `assets.list`,
  `assets.upload`, `scripts.guide`, `debug.*` and `agent.session.*` (3.5) never need the game.
- **`play.stop` in Play ends Play wherever the debugger holds it.** Stop drops the fork, held tick
  and all, so the server has the debugger pass over every pause (amoris-debug `DebugHub::pass`,
  the server's `DebugHub::pass`) from the moment it sees one until the Stop lands: the held tick
  runs to its end with no further stop (a breakpoint in a loop, a data watch, a `debugger;`), the
  boundary applies `play.stop`, and the pass ends, also when the caller goes away first.
  Breakpoints and watches stay set and stop the next Play. This is the editor's Stop and an agent's
  `amoris play stop` at a breakpoint; measured (provisional) 2 to 12 ms after a one-second pause.
  In the edit world (a held `time.step`) there is no Play to end and `play.stop` is refused like
  any other call.
- **`time.control` is queued, not refused.** Sent while held, it goes to the game thread and is
  answered at once `debug.paused {queued: true}`: it runs at the boundary after the held tick,
  before any other tick (a boundary applies every command waiting). `time.control {pause: true}`
  then `debug.continue` leaves the game resting at that boundary, where every call works again and
  a `time.step` stops at the breakpoint again: the way out of a breakpoint the next tick hits again,
  short of clearing it. Its own answer is not delivered (nor a refusal of bad parameters).
- **Everything else is refused at once** with `debug.paused {method, location, reason, tick,
  system, queued: false, stop, answers}`: "The game stands at scripts/rules.ts:27 (breakpoint, tick
  58); scripts.apply needs the game thread, which waits for debug.continue (or debug.step). [...] A
  breakpoint the next tick hits holds the game again right after a continue: to act between ticks,
  clear it (debug.breakpoints.clear) first; play.stop ends Play wherever it stands." A call sent
  before a stop and still waiting when it comes answers `debug.paused {queued: true}`: it stays
  queued and runs at the boundary after the held tick.
- **"Continue, then act" is a race.** `debug.continue` answers once the game has left the pause; in
  real-time Play a breakpoint that every tick hits holds it again within milliseconds (the time
  model catches up after a pause). A call sent after the continue lands at the boundary only if it
  reaches the game thread before the next tick starts: measured on sailing (provisional), a
  `world.edit` over a kept-alive connection did in 60 of 60 rounds, over a new connection per call
  in 15 of 60 (39 refused at once, 6 answered `queued: true` and applied after the next continue);
  the CLI starts a process per call and almost never does. Before 2026-10-09 such calls waited for
  the next resume instead, which is what hung 28 agent calls. To act between ticks: clear or
  condition the breakpoint, or queue `time.control {pause: true}` and continue, or `play.stop`.
- The CLI waits 60 s at most (`--timeout <s>`, `AMORIS_TIMEOUT`; 10 s past a debugger call's
  `timeout_ms`) and answers `host.timeout` after it; the MCP stdio bridge to a running host uses
  the same client.

Not done: the edit world's kept snapshots (`snapshots.list`), the history and `project.info` stay
on the game thread and are refused while held.

### 3.5 Agent sessions

2026-11-12. Until then the host kept no sessions. An MCP client was only numbered (`mcp-<n>`), and
rmcp asks for a handler for more than sessions, so three requests of a client that keeps none were
`mcp-2`, `mcp-3` and `mcp-4`; `agent` events were pushes, dropped when no socket listened, so an
editor that connected later showed nothing and guessed sessions from the events it had received;
and `amoris mcp` attached to a host called as the shared `api` client, raised no `agent` event, and
its edits were `api`'s in the history. Now the host lists who calls it as an agent and keeps what
they called (amoris-server `sessions.rs`).

**Host state, never world state.** The rows, the handles, the counters, the timestamps and the
stream live in the server, which holds no game: nothing of them is in the world, its hash, a
snapshot or a recording, and all of it is lost with the host (a host started again begins at
`mcp-1` with an empty stream). The game sees only which `Developer(n)` or `Player(i)` source sends
a command.

A row, as `agent.session.list` answers it, every key always present:

```json
{"id": "mcp-3", "transport": "bridge", "role": "developer", "seat": null,
 "label": "claude-code 2.1.0", "calls": 12, "failed_calls": 1, "last_method": "world.edit",
 "live": true, "opened_ts": 1794468357818, "last_ts": 1794468362921}
```

`id` is `mcp-<n>` (one counter for every session, never given again) or `player:<seat>`; `role` is
`developer`, `player` or, for a checker's grant alone (3.6), `checker`, with the `seat` a player's
session plays; `label` is what the client calls
itself (an MCP client's `clientInfo` as `name version`, a bridge's `label`; at most 80 characters);
`failed_calls` counts the calls answered with an error; times are ms since the Unix epoch. `live`
says that a connection or a registration holds the session.

| `transport` | Who | Opened | Closed | `live` |
|---|---|---|---|---|
| `http` | an MCP client on `POST /mcp` that sent `initialize` | inside `initialize`, before its answer leaves | when rmcp drops the session's handler: the client's `DELETE /mcp`, 300 s without a request, the host's end | while open |
| `http` | an MCP client of the 2026-07-28 revision, which keeps no session: every request comes alone and names its client in `_meta` | at its first `tools/call`; one row for every request of one client label, with one developer client (`tools/list` opens nothing) | never | never |
| `stdio` | `amoris mcp` running the game in its own process | at `initialize` (or the first call of a client that sent none) | when the client closes stdin | while open |
| `bridge` | a process that registered with `agent.session.open {label?, seat?}`: `amoris mcp` attached to this host (1) | by the call, which answers the row and a `handle` | `agent.session.close` | until closed |
| `api` | calls made as a seat with no session (`POST /api/call {seat}`), under the id `player:<seat>` | at the first such call | never | never |
| `grant` | a grant of a locked host (3.6): every client that presents the grant's token, on `/api/call` and on MCP, under the grant's label | when the host locks, in the grants file's order, before it serves | never | for the host's life |

The editor's socket and the HTTP API's shared `api` client (the CLI, plain `/api/call`) are not
sessions: their calls are not in the stream, as before.

**The stream.** Every opening, call, result and closing is an event with a stream number:

```json
{"seq": 41, "ts": 1794468362921, "session": "mcp-3", "kind": "call", "method": "world.edit", "summary": "{\"ops\":[...]}"}
{"seq": 42, "ts": 1794468362925, "session": "mcp-3", "kind": "result", "method": "world.edit", "ok": true, "summary": "{\"applied\":1,...}"}
{"seq": 43, "ts": 1794468369000, "session": "mcp-3", "kind": "closed", "row": {"id": "mcp-3", "live": false, "calls": 13, ...}}
```

`opened` and `closed` carry the session's row as the event leaves it; `summary` is the parameters
or the result as JSON cut to 160 bytes, or a refusal's `code: message`. The host keeps the newest
2,000 events in a ring. Each event is put in the ring and then pushed on the `agent` topic (4), in
the order of the numbers, whether or not a socket listens, so a client that connects later reads
what it missed.

- `agent.session.list {}` answers `{sessions: [row], last}`: the rows in opening order, and the
  newest stream number.
- `agent.session.events {since?, limit? = 100, session?}` answers `{events, last, missed}`. With
  `since`, the first `limit` events after it, oldest first; without it, the newest `limit` (at most
  2,000). `session` keeps one session's (an id that is not listed is refused with
  `request.invalid_value` at `/session` and the listed ids). `missed` counts the events after
  `since` that left the ring, whatever their session. `last` is the cursor the answer reaches: the
  last event returned when `limit` cut the answer, else the newest stream number, so a reader that
  asks again with `since: last` loses nothing. (`events.since` answers the ring's newest as `last`
  also when `limit` cut the answer; this method does not.)
- `agent.session.open {label?, seat?}` registers the caller as a `bridge` session and answers its
  row with `handle`. Without `seat` it is a developer's, with a developer client of its own
  (`Developer(n)`, named after the session in the history, 3.1); with `seat` it is that seat's
  player, whose seat is not looked up until its first call, so opening never asks the game. The
  session's calls are `POST /api/call {handle, method, params}` (5).
- `agent.session.close {handle}` (the opener's own) or `{session}` (a `bridge` row by its id, for a
  row whose process is gone) answers the row, no longer live; a closed row answers again. Both
  keys: `request.conflict`; neither: `request.missing_field`; an `http`, `stdio` or `api` row named
  by `session`: `request.not_applicable` at `/session`; no such handle or id:
  `host.session_unknown`.

The handle is `<the host run, hex>-<n>`: unique to one run of the host, so a bridge that outlives
its host is refused with `host.session_unknown` by the next one (a session id would have named
another bridge's row there) and registers again. It is an address on a loopback host, not a
credential: admission is the tokens' (11).

A row that is not live stays listed while the ring holds one of its events, and goes with the last
one; a live row never goes. So every session the stream names is in the list.

A player's session passes `seat_permits` (player.md 7) before its seat is looked up: a developer's
method is refused with `permission.denied` and recorded as a call that failed, as is a first call
for a seat the game does not declare (`seat.unknown`). A call refused with `host.session_unknown`
belongs to no session and is not recorded, and neither is a tool call the MCP layer refuses before
it routes it (an unknown tool or action, a player session naming another tool).

The four methods never reach the game thread, so they answer while the debugger holds the game
(3.4). They are a developer's: a call made as a seat is refused them.

Not built: a bridge killed without closing (SIGKILL) stays live until `agent.session.close
{session}` or the host's end, and an MCP client that vanishes stays live for rmcp's 300 s; an
agent that idles longer than that and calls again is a new `mcp-<n>`. The requests of two
instances of one sessionless client program share a row.

### 3.6 Grants: a locked host

2026-11-12. Until then a client picked its own role: `/api/call` took `seat` from the body, or none
and was a developer; `/mcp` was always a developer session; `/ws`, `/render` and `/assets` were open
to every local process. That is right for one person and the agents they trust, and stays what a
host without grants does. A benchmark's agent must not be able to become the harness that checks
it, so a host can be started locked (amoris-server `grants.rs` and `http/locked.rs`;
shared/contract/mcp.md 3.1 and 14).

**The grants file.** `amoris serve <project> --grants <file>` reads, before the game starts:

```json
{"grants": [
  {"token": "0123456789abcdef0123456789abcdef", "role": "player", "seat": "skipper", "label": "agent"},
  {"token": "fedcba9876543210fedcba9876543210", "role": "checker", "label": "harness"},
  {"token": "00112233445566778899aabbccddeeff", "role": "developer", "label": "setup"}
]}
```

`token` is 32 hex digits in either case: 128 bits the process owner draws (the editor, a benchmark
harness; `openssl rand -hex 16`). The engine draws none. `role` is `player`, `checker` or
`developer`; `seat` is required for a player and refused for the others; `label` is the session's
label in the list (default: the role's name; at most 80 characters). A player grant may have
`"policy": {"fork": true}`, which lets it look ahead with `player.fork` (player.md 7.1; default
false, refused `session.policy_denies` before it reaches the game); a policy on a checker or a
developer is refused, since a developer always forks and a checker never does. The file is strict:
a field it does not take, mcp.md 3.1's `profile`, `clock`, `decision`, `max_wall_ms` and the
policy's fields but `fork` included (nothing enforces them yet; the bounds of a branch are the
runtime's), is refused, never accepted and ignored. Each of these ends the process
with exit 1 and `host.grants_invalid {file, grant?, label?, field?, reason, problem?}`, no readiness
line and no host file: a file that cannot be read or is not JSON; a field the file does not take
(`problem` is the decoder's `request.unknown_field` with its suggestions); an empty list; a token
that is not 32 hex digits or is given twice; a player without `seat`; a `seat` on a checker or a
developer; two player grants on one seat (`problem` is `session.seat_taken`); a seat the game gives
no player (`problem` is the game's refusal, `seat.unknown` for a seat it does not declare), found
when the host locks; and a file that lies inside the project, whose files the host serves at
`/assets/<path>`. No refusal, line, file, row, event or answer of the host says a token: the type
that holds one cannot be serialized and prints nothing.

**Admission.** On a locked host every route sits behind an admission, inside the loopback guard
(5). A request presents a token as `Authorization: Bearer <token>` (the scheme in any case), or,
for MCP, as the path `/mcp/<token>` (a client that cannot set a header); one that gives both must
give the same. A token in the query or in the body is never read. The token is compared with every
grant's, every byte of each, with no early exit and the match picked without a branch, so the time
taken says nothing of a registered token. A missing token, a malformed one and one no grant has get
the same answer, byte for byte, on every route: `401` with `WWW-Authenticate: Bearer` and
`{"error": host.unauthorized}`. A route the grant's role may not open is `403 permission.denied
{request, role, needs}`, where `request` is the method and the route (`GET /assets`), never the
path.

| Route | player | checker | developer |
|---|---|---|---|
| `POST /api/call` | its seat's `player.*`; a body `seat` other than its own is `seat.not_yours`, a body `handle` is `permission.denied` | the catalog's reads (below); a body `seat` or `handle` is `permission.denied` | everything, as its own session; a body `seat` or `handle` as on a host without grants (5) |
| `/mcp`, `/mcp/<token>` | the `player` tool alone, with the player instructions | the developer's tools; the host refuses what is no read | every tool |
| `GET /api/grant` | `{role, seat, label, session}`: what the token is, answered without the game | the same | the same |
| `GET /api/catalog`, `GET /assets/<path>` | 403 | yes | yes |
| `/ws`, `/render`, `/wasm/<path>`, the editor's files | 403 | 403 | yes |

**One session per grant.** When the host locks (`Host::lock`, once, before it serves) it asks the
game for each player grant's seat and then opens one row per grant, in the file's order: `mcp-1` to
`mcp-k`, transport `grant`, live for the host's life, labelled as the file says (3.5). Every call
made with a grant's token, over `/api/call` or over any number of MCP clients, is that row's call,
recorded and pushed like every session's; what an MCP client calls itself changes nothing. A
developer's and a checker's row has a developer client of its own, so the file's order fixes each
one's `Developer(n)`: the same grants file gives the same sources, and so the same order of
commands within a boundary, in every run. A grant's row is not closed by its id
(`request.not_applicable`), and sessions opened later are numbered after the grants. Grants are
host state like the sessions: nothing of them is in the world, its hash, a snapshot or a recording
(amoris-app's `tests/locked_host.rs` compares the hash of a locked and an unlocked host).

**The player.** Its calls are its seat's player's whatever the request says: sent from the seat's
player source, the player tools alone (`seat_permits`, player.md 7), no omniscient view. The body's
`seat` may repeat its own; another seat is refused with `seat.not_yours {path: "/seat", seat,
yours}` before any seat is looked up, and a `seat` that is not a string with `request.wrong_type`.
Other keys of the body (`role`, `token`) are not read.

**The checker.** Its calls are the catalog entries of kind `read`, by name or alias; every other
entry is `permission.denied {request, role: "checker", needs: "developer"}`, and a name the catalog
does not have is `request.unknown_method` with the reads as its candidates. The refusal is made in
`Host::call`, before the call reaches the game or a plug-in, and is recorded as the session's failed
call. A checker reads everything a developer does, the marked omniscient view included
(`player.observe {seat, omniscient: true}`); `player.session` answers `role: "developer"`, because
the game sees a developer source. Two entries the catalog calls reads are withheld, because they do
more than read: `scripts.check` compiles through `scripts.apply {dry_run}` (a request) and starts
`tsc`, and `scripts.types` writes `.amoris/types/` and, in a project that has none, `tsconfig.json`
in the project's root. By section 3's kinds both are requests; when the catalog says so the list
(`CHECKER_WITHHELD`) goes. `capture` stays a checker's: it writes a PNG under `.amoris/captures/`
and nothing of the world. `debug.eval` is kind `write` since this date (debugger.md 7), so a
checker cannot evaluate in a paused frame. Of the saves, the scene loader and the uploads (3, 3.3),
`saves.list` is a read and so a checker's (it reads the saves' headers); `saves.write` (a request),
`saves.load` (control), `scene.load` (write) and `assets.upload` (a request that writes a project
file) are a developer's alone. `/ws` and `/render` are a developer's only: a socket's requests go
through the editor's client, which no gate of a checker's stands before, so a checker polls
`events.since` and `agent.session.events`.

**The debugger's CDP endpoint is off.** That endpoint (port 9229, debugger.md) has no admission,
and any local process that reached it could evaluate in a paused frame, past every grant. A locked
host does not start it; `debug.state` says `cdp: null`, and the debugger stays behind `debug.*`,
which a developer's token reaches.

**What the lock does not protect.** It guards the port, not the disk: a process of the same user can
read the project's files and the grants file itself, and a token given as `--token` or
`--grant-token` is on a command line, where `ps` shows it (`AMORIS_TOKEN` keeps it off the command
line). Keeping an agent from those is its sandbox's job. The host still serves loopback only; a
token is a role among local clients, not a remote identity. A token cannot be added, revoked or
expired while the host runs.

Not built: mcp.md 3.1's `profile`, `decision`, `clock`, `max_wall_ms` and of `policy` all but
`fork`; `--grant
<json>` for a stdio session that runs its own game; `session.seat_taken` between sessions opened
at run time; a tool list of the checker's own (it lists the developer's eleven tools, so mcp.md
3.3's "a tool the session may not call is not listed" holds for players only); the browser editor
on a locked host (a page load and a WebSocket set no `Authorization`); a push stream for checkers.

### 3.7 Agent programs the host starts

2026-11-12. Until then the editor's New session form was a preview: the host listed the sessions
agents opened themselves and started none, because which program the editor may launch, and with
whose credentials, was not decided. It is decided this way: **the process owner registers the
programs, the editor picks one by name.**

`amoris serve <project> --agents <file>` reads the agents file before the game starts, like a grants
file and for the same reason outside the project (whose files the host serves and agents write: an
agent must never choose what the host runs):

```json
{"agents": [
  {"name": "claude", "label": "Claude Code",
   "command": ["claude", "-p", "--mcp-config", "/Users/me/amoris-agents/mcp.json"]},
  {"name": "scripted", "command": ["/bin/sh", "/Users/me/amoris-agents/scripted.sh"]}
]}
```

`name` is `^[a-z0-9][a-z0-9_.-]*$`, unique; `command` is the program and its arguments, run without
a shell; `label` names the sessions (default: the name); `env` adds variables. The file is strict,
and a refused file ends `amoris serve` with exit 1 and
`host.agents_invalid {file, reason, problem?}`. The engine holds no model, key or credential: the
program inherits the environment `amoris serve` was started with, and what model it runs and with
whose key is the program's and its owner's.

- `agent.session.agents {}` lists the programs by `name` and `label` (never their commands), with
  how many run and the limit (4).
- `agent.session.start {agent, goal, seat?}` opens a session of transport `launch` (live, labelled
  with the program's label; with `seat`, that seat's player, the seat looked up first) and then
  starts the program in the project's directory, stdin closed, with `AMORIS_HOST` (the host's URL),
  `AMORIS_PROJECT`, `AMORIS_AGENT` (its name), `AMORIS_AGENT_SESSION` and `AMORIS_AGENT_HANDLE`
  (the session's id and handle), `AMORIS_AGENT_GOAL` and, with a seat, `AMORIS_AGENT_SEAT`. The
  answer is the row with the program's `agent` and `pid`, never the handle. Each line the program
  writes (stdout, stderr; cut to 1,000 bytes) is a `message` event of the session with its
  `stream`, beside the host's own words on it (`stream: "host"`: started, stopping, the exit
  status). The session closes when the program exits.
- The program reaches the host as that session: `POST /api/call {handle}` with the handle, or
  `amoris mcp --attach "$AMORIS_HOST"` (1), which, given `AMORIS_AGENT_SESSION` and
  `AMORIS_AGENT_HANDLE` and no `--seat`, calls as that session (with `AMORIS_AGENT_SEAT`'s player
  tool alone for a seat) and neither registers nor closes one. So an MCP client such as Claude
  Code, started with an MCP configuration that runs `amoris mcp --attach "$AMORIS_HOST"`, is a
  session the editor started, and its calls and edits are that session's.
- `agent.session.stop {session}` kills the program (SIGKILL) and answers the row once the session
  is closed (at most 5 s). `agent.session.close {session}` does not take a `launch` row: only a
  stop or the program's exit ends it.
- Refusals: `host.no_agents` (the host has no agents file), `request.invalid_value` for a name the
  file does not register (with suggestions), `request.out_of_range` for a goal that is empty or
  longer than 4,000 characters, `session.limit_reached` past 4 programs, the game's `seat.unknown`,
  `host.agent_failed` for a program that did not start (its session opened and closed, saying
  why) or did not end, `host.session_unknown` to stop what does not run, and `host.not_supported`
  on a locked host (3.6), where a started program would need a grant of its own.
- Programs, like sessions, are host state: nothing of them reaches the world. A program still
  running when the host stops is killed with it.

Not built: a grant for a started program on a locked host; a program's restart; input to a running
program (its goal is all it is told); limits on its CPU, memory or time beyond Stop; the charter's
real LLM run through it (charter 2, 6), which needs the owner's go-ahead and a model key.

## 4. Pushes on `/ws`

The server pushes `{"event": topic, "data": ...}` to the sockets subscribed to the topic (`status`
always). A socket that falls more than 1,024 pushes behind gets
`{"event": "lagged", "data": {missed}}`.

| Topic | Data | When |
|---|---|---|
| `status` | `{tick, t_s, paused, pacing, halted, behind_ms, mode, epoch, world_hash, entities, tps, state, version}`: the last publication's, which a `time.control` that changes the pacing or the pause brings up to date even when no tick follows (threads.md 4.3). For a poisoned world `halted` is true and `world_hash` is still the last publication's, where the `status` method's is null | on connect; every 100 ms if it changed, else every second |
| `world.changed` | `{tick, spawned, despawned, changed: [[id, component]], truncated?}`, or `{tick, reset: true, epoch}` | every 100 ms when publications came, coalesced; a reset when the world was replaced (it covers changes in the same 100 ms) |
| `events` | `[{seq, id, tick, name, subject, cause, data}]` | every 50 ms, at most 500 per push |
| `log` | `{seq, level, source, message, file?, line?, tick?}` | one per line: script `console.*`, failed system runs, and the error of a tick that poisoned the world (`<code>: <message>` at level `error`, with that tick; threads.md 8) |
| `history` | `{undo, redo, entries}` (`history.list`'s answer, with sessions) | after `world.edit`, undo, redo, restore, Play, Stop, when it changed (3.1) |
| `agent` | `{seq, ts, session, kind: "opened"/"call"/"result"/"closed", method?, ok?, summary?, row?}` (3.5) | each agent session's opening, each of its calls and its result, its closing; recorded in the host's ring whether or not a socket listens |
| `debug` | debugger.md 7's state on a pause, `{state: "running"}` on a resume | pushed by `amoris serve`'s debugger (amoris-app `forward_debug_events`); script console lines and failed calls the debugger saw go to `log` |
| `profile` | `{tick, epoch, ticks, held, missed, tick_ms, tick_max_ms, busy_ms, systems: [{name, ms, max_ms}], phases: [{name, ms, max_ms}]}` | every 250 ms: the ticks profiled since the last push, summarized (below) |

`seq` in events is the stream number (`amoris-link` ring, threads.md 4.4), `id` and `cause` the
world's event sequence numbers. `world.changed` compares section digests, then rows of the changed
component sections (at most 2,000 pairs). Subscribing is all or nothing: a list naming a topic not
listed here is refused and changes nothing.

The profile (2026-10-14; charter 4.1, the built-in profiler). A host that serves the profile stream
turns it on: amoris-server's `Host::new` calls amoris-link's `SnapshotReader::serve_profile`, so
under `amoris serve` every tick is profiled from start-up, editor or not, and a game no host serves
(`amoris run`, the native `amoris play` window) is never profiled. The game thread then runs each
tick with the step hooks of amoris-runtime's `profile::TickTimer`, which time every system on the
loop's injected clock outside the simulation (two readings per system; a phase's time is its
systems'); a system that runs others inside it (`script.update`) keeps only its own time. After the
tick the loop adds the whole step (`tick_ms`), the render feed's extraction (`loop.present`), the
snapshot and streams it publishes (`loop.publish`) and its work after the tick before
(`loop.after`: the players' decision points, `time.step`'s stop conditions and samples; carried
into the next tick's profile, so a reply that work sends never overtakes the profile of the tick it
answers), and pushes the tick into amoris-link's profile ring (600 ticks) beside the event and log
rings. The push never waits: while a reader holds the ring the profile waits for the next tick's
push; the ring hands back the oldest tick's buffers, so a full ring allocates nothing. The ring
keeps one world's ticks: a tick of another epoch (Play, Stop, a restore) empties it, and a read for
the world shown before it has ticked finds nothing. A tick during which the script debugger stopped
the thread (its stop count changed) is marked held: its times measure the hold, so summaries count
it in `held` and leave it out. amoris-link's `profile::summary` gives the push and `profile.frame`
one shape: per tick counted, `ms` the average and `max_ms` the most in one tick; `systems` in the
order they finish in a tick (a script system before `script.update`, which runs it), then
`tick.other` (the step outside its systems), `loop.present`, `loop.publish` and `loop.after`, which
add up to `busy_ms`, the game thread's time per tick apart from the commands it applies between
ticks; `epoch` the world's. The push reads the ticks since its last push (`missed`: ticks of this
world that left the ring first) and goes out every 250 ms, with `ticks: 0` when none ran;
`profile.frame` reads the last `ticks`. The CLI's `amoris profile [--ticks n]` prints it as a table
and MCP's `time` tool has a `profile` action, so the editor, the CLI and agents read the same
numbers. The host draws no frame: the editor's profiler adds its own viewport's frame and GPU passes
(editor.md 5). docs/bench/profiler.md measures the profiler's cost.

## 5. HTTP endpoints

| Path | Answer |
|---|---|
| `GET /api/catalog` | the catalog |
| `POST /api/call` | `{id?, method, params?, seat?, handle?}` -> `{id, result}` or `{id, error}`; HTTP 400 only for a body that is not JSON. With `seat` the call is that seat's player's (player.md 7); with `handle` it is the call of the agent session `agent.session.open` gave the handle to (3.5); both: `request.conflict` |
| `GET /ws` | the WebSocket of host-protocol.md 2 and 3 |
| `GET /render` | the render feed (`RenderFeed`); `501 render.not_available` until installed |
| `GET /assets/<path>` | a project file; no `..`, no dot files, contained after resolving links |
| `POST /mcp` | MCP Streamable HTTP (rmcp, sessions per client) |
| `GET /*` | the editor's files; paths without an extension get `index.html` |
| `GET /api/grant` | a locked host only (3.6): `{role, seat, label, session}`, what the presented token is |
| `/mcp/<token>` | a locked host only: `/mcp` for a client that cannot set a header, the grant's token as the path's one segment |
| any route of a locked host | `401 host.unauthorized` with `WWW-Authenticate: Bearer` without a registered token; `403 permission.denied` for a route the grant's role may not open (3.6) |

Every request whose `Host` is not `127.0.0.1`, `localhost` or `::1` (any port), or whose `Origin`
(when present) is not one of them, gets `403 host.forbidden` (DNS rebinding). The CDP endpoint is
amoris-debug's own port (9229), not this one. On a locked host the guard stands before the
admission, so a registered token from another `Host` is refused like any request from there, and
the CDP endpoint is not started (3.6).

## 6. The integrator's plug-ins

```rust
// amoris-server
pub trait RenderFeed: Send + Sync {
    fn serve(&self, socket: axum::extract::ws::WebSocket) -> BoxFuture<()>;
}
pub trait DebugHub: Send + Sync {   // methods by name: "debug.state", "debug.breakpoints.set", ...
    fn call(&self, method: &str, params: Value) -> BoxFuture<Result<Value, Problem>>;
    fn methods(&self) -> Vec<Value>;   // catalog entries {name, kind, doc, aliases, params}
}
pub trait CaptureHub: Send + Sync {
    fn capture(&self, params: Value) -> BoxFuture<Result<Value, Problem>>;
}
impl Host {
    pub fn set_render(&self, feed: Arc<dyn RenderFeed>);
    pub fn set_debug(&self, hub: Arc<dyn DebugHub>);
    pub fn set_capture(&self, hub: Arc<dyn CaptureHub>);
    pub fn push(&self, topic: &'static str, data: Value);   // e.g. "debug", "profile"
}
```

`amoris-app`'s `serve.rs` builds the `Host` (`start`), so the integrator installs these there. The
render feed (`amoris-app`'s `present.rs`) subscribes each socket to the game's `Feed` and sends a
frame when the game thread's `on_publish` says it published, at most one every 8 ms
(host-protocol.md 5). The installed hub's
`methods()` (amoris-debug's `methods()`, each with its kind, aliases and JSON Schema) are catalog
entries like every other command, so `amoris help debug.watch`, `docs.search` and the MCP `debug`
tool describe them (the debug evaluation's fourth finding: 34 refusals of guessed parameters); the
server adds `debug.rewind {tick, bundle?}`, which it serves itself (restore, then step to `tick`),
and `debug.writes {entity, component, field?, ticks?}` (a `time.step {ticks, trace}` answered as its
write trace, debugger.md 13), and resolves `debug.watch`'s entity names (`Sloop`, `Sloop#3`) in the
last publication before the hub sees an id. Without a hub, one `debug.state` entry says the debugger
is not installed.

## 7. MCP

`amoris-mcp` projects the catalog into eleven tools, each an `action` over methods; schemas name the
parameters loosely and the runtime's decoder refuses unknown ones with suggestions:

| Tool | Actions -> methods |
|---|---|
| `world` | tree, get, query, schema, edit -> `world.*`; load -> `scene.load` |
| `scripts` | guide, list, read, write, apply, check, status, types -> `scripts.*` |
| `time` | status -> `status`; pause, resume, speed -> `time.control`; step -> `time.step`; snapshots -> `snapshots.list`; rewind -> `snapshots.restore`; saves -> `saves.list`; save -> `saves.write`; load -> `saves.load`; profile -> `profile.frame` |
| `play` | start, stop |
| `history` | undo, redo, list |
| `assets` | list |
| `events` | since, why -> `events.*`; log -> `log.since` |
| `debug` | attach, detach, breakpoints.set, breakpoints.clear, breakpoints.list, pause, continue, step, state, eval, set, watch, unwatch, exceptions, wait, rewind, writes -> `debug.*` |
| `capture` | -> `capture` |
| `docs` | -> `docs.search` |
| `player` | session, describe, observe, nearby, events, affordances, intents, act, wait, continue, pacing -> `player.*` (player.md; a developer names the seat) |

Results are the method's JSON as text; refusals are tool results with `isError` and the text
`{"error": {code, message, detail}}`. The `debug` tool's input schema lists every parameter its
methods take (`file, line, condition, log, id, brief, expr, frame, name, value, kind, timeout_ms,
entity, component, field, mode, tick, bundle`; amoris-app's `paused_host.rs` checks it against
`amoris_debug::methods()`); `world` takes `fields`, `scripts` `lines` and `numbered`, `time`
`sample` and `bundle`. `tools/list` is 8.6 KB (8,613 bytes of `tools`, about 2,150 tokens at four
bytes a token, as `tools/host/smoke_server.py` counts it; 2026-10-14, Windows; the `time` tool's
`profile` action is 87 of them); it was 6.6 KB (6,645 bytes, ten tools) on 2026-10-09, before the
`player` tool, and 4.4 KB, 1,236 cl100k tokens, on 2026-10-04.
Served on `POST /mcp` (one developer client per session, every call pushed as `agent`) and by
`amoris mcp` over stdio.

Sessions (2026-11-12; 3.5). rmcp 3.7 asks the host for a handler for every `initialize`, for every
request of a client of the 2026-07-28 revision (which has no handshake and no session: each request
names its client in `_meta`), for `server/discover` and for its own look-ups of a tool's schema, so
making a handler opens nothing (`Backend::open`). A session opens when `amoris-mcp` tells the
handler's caller who its client is (`Caller::hello`, once): inside `initialize`, so a client that
has its answer is listed, or with the first `tools/call` of a client that sent none (over HTTP that
handler serves one request, and the hello says so: such requests share a row). It closes when rmcp
drops the handler and the calls in flight have answered: the client's `DELETE /mcp`, an HTTP
session 300 s without a request (rmcp's default, kept), the closed stream of a stdio session, or
the host's end. No tool or action was added for the session methods; `tools/list` keeps its size.

Grants (2026-11-12; 3.6). A locked host keeps one MCP service per grant (`amoris_mcp::HttpService`,
each with rmcp sessions of its own, held by the router, never by the host) and hands a request to
the service of the grant whose token it presented, on `/mcp` or `/mcp/<token>`, without the
token's header or path. So an `Mcp-Session-Id` of one grant is no session under another's token.
Every handler of a grant's service calls as the grant's one row: a hello opens nothing and the end
of an MCP session closes nothing. A player grant's clients get the player instructions and list the
`player` tool alone; a checker's list the developer's eleven tools, and the host refuses each call
that is no read with `permission.denied` as a tool result. `amoris mcp --attach <url>
--grant-token <token>` serves the same over stdio (1).

## 8. The CLI

`amoris <command>` talks to the running host over `/api/call`; `amoris help` lists the commands and
the catalog, `amoris help <command|method>` prints the method's doc and parameters from its JSON
Schema, so help never drifts from what the host takes.

| Command | Method |
|---|---|
| `amoris call <method> ['<json>'|-]` | any |
| `amoris status`, `info`, `save`, `catalog`, `docs <words..>` | `status`, `project.info`, `project.save`, `catalog.list`, `docs.search` |
| `amoris world tree [filter] [--with C,..] [--root e] [--depth n] [--limit n]` | `world.tree` (each entity's children two spaces deeper; an entity the depth cut ends with `(+N below: --root <id>)`) |
| `amoris world get <entity> [C.. \| C.f,C.g..]`, `query <C,..> [--fields C.f,..]`, `schema [C]` | `world.get` (dotted names are `fields`), `world.query`, `world.schema` |
| `amoris world edit '<ops>'`, `set <entity> <C> f=v..`, `spawn [name] [--prefab j]`, `remove <e> <C>`, `destroy <e>..` | `world.edit` (`set <e> Parent parent=<id>` places an entity under another, `remove <e> Parent` makes it a root) |
| `amoris step [ticks] [--until event:<name>|tick:<n>|<e>.<C>.<f><op><v>] [--watch <e>.<C>.<f>[~v]] [--sample <e>.<C>.<f>,.. [--every k]] [--trace <e>.<C>[.<f>]]` | `time.step` (`--trace`: the write trace's table after the status line) |
| `amoris time pause|resume|speed <x>|stepped` | `time.control` |
| `amoris play start [--speed x] [--paused]|stop` | `play.*` |
| `amoris undo`, `redo`, `history` | `history.*` |
| `amoris scripts list|read <p> [--lines a-b] [--numbers]|write <p> [file|-]|apply [--force]|check|types|guide|status` | `scripts.*` (`--lines` numbers the lines) |
| `amoris events [--since n] [--name n.*] [--limit n]`, `--why <seq>` | `events.*` |
| `amoris logs [--since n]`, `snapshots [list]|restore <tick> [--bundle snapshot]`, `assets [dir]` | `log.since`, `snapshots.*`, `assets.list` |
| `amoris saves [list]`, `saves write <name> [--label text]`, `saves load <name>` | `saves.*` |
| `amoris profile [--ticks n]` | `profile.frame` (a table: ms per tick, max, share of the game thread) |
| `amoris scripts profile [--ticks n] [--interval steps] [--top n] [--folded <file>]` | `scripts.profile` (by system: its steps, then its functions, lines and stacks with self and total steps; `--folded` writes the folded stacks to the file) |
| `amoris debug break <file>:<line> [--if <expr>] [--log <text>]`, `clear [id]`, `list` | `debug.breakpoints.*` (`file` may be the module's end, `helm.ts`) |
| `amoris debug state [--full]`, `eval <expr> [--frame n]`, `set <name> <expr> [--frame n]` | `debug.state`, `debug.eval`, `debug.set` |
| `amoris debug watch <e>.<C>[.<f>]`, `unwatch [id]` | `debug.watch` (entity by name or id), `debug.unwatch` |
| `amoris debug writes <e>.<C>[.<f>] [--ticks n]` | `debug.writes` (a table: tick, writer, phase, before, after, the statement or the boundary's write) |
| `amoris debug continue`, `step [over|into|out]`, `pause [ms]`, `wait [ms]`, `exceptions <mode>`, `rewind <tick> [--bundle snapshot]`, `attach`, `detach` | `debug.*` |
| `amoris debug <action> ['<json>']` | `debug.<action>` with exact parameters |

Output is compact text, one line per entity, row, event, edit or diagnostic, with defaults left out
(`#3 Sloop  Boat Collider Crew ...`;
`tick 17 hash 5eb7f9e28e7c | stopped: Boat.speed 0.96 -> 1.03`); arrays longer than 8 items show
their first 3 and a count; `--json` prints the exact result. The status line names the running
scripts (`| scripts 3ae1a99021d4`) and, while the debugger holds the game, where (`| HELD at
scripts/rules.ts:27 (breakpoint bp1 in log, tick 58)`); a step the debugger stopped prints
`tick 58 hash - | stopped at scripts/rules.ts:27 (breakpoint bp1 in log, tick 58): amoris debug
state | amoris debug continue`; a sampled step prints its table; reads made while held start with
`(paused at …: the world as of tick 57's end)`. `debug state` prints the place, the innermost
frame's locals one per line (`bearing = 1.344`; `ctx` left out), one line per other frame, the
breakpoints and watches (`--full`: every frame's locals and closures). A refusal prints
`code: message` (with its suggestions and diagnostics) to stderr and exits 1, as does a `scripts
check` with an error diagnostic (compile, load or `tsc`) or a `tsc` past its time limit; a usage
error exits 2. Every call waits 60 s at most (`--timeout <s>`, `AMORIS_TIMEOUT`), then exits 1 with
`host.timeout`.

A locked host (3.6; 2026-11-12): every command takes `--token <32 hex>`, else `AMORIS_TOKEN`, and
sends it as `Authorization: Bearer`; the call is then the grant's (`amoris player observe --token
<player's>` plays the grant's seat, with no `--seat`). Without one a command exits 1 with
`host.unauthorized`. A token that is not 32 hex digits is a usage error (exit 2) whose message does
not repeat it. `amoris help` needs no token: when the host refuses the catalog it prints the one
this binary would serve. A host without grants reads no token, so the flag changes nothing there.

## 9. Error codes added

| Code | When |
|---|---|
| `command.thread_only` | a thread command sent to a `Game` driven directly |
| `history.empty`, `history.stale` | nothing to undo or redo; an entry that no longer applies (dropped) |
| `history.group` | `history.undo {group}` when the next entry is not the whole of the group (3.1) |
| `play.running`, `play.not_running` | `play.start` while playing; `play.stop` while editing |
| `play.started`, `play.stopped`, `snapshots.restored`, `saves.loaded` | a waiting `time.step` whose world was replaced |
| `snapshots.none` | no kept snapshot at or before the tick |
| `scene.place_invalid` | an entity number in a loaded scene that is no place in its file (`path`, `place`, `entities`; simulation.md 7.6) |
| `saves.not_found` | `saves.load` of a name with no save (`saves`, `suggestion?`) |
| `saves.unwritable` | a save's folder or file could not be written or read (`path`, `error`) |
| `sim.entity_alive` | a revive names a live id (simulation.md 11) |
| `sim.parent_cycle` | a `world.edit` (or a scene) that would leave an entity under itself or under an entity that is under it (3.1; simulation.md 11) |
| `project.no_root` | a file command on a game built from data |
| `events.not_found` | `events.why` for a stream number the ring no longer holds |
| `host.forbidden`, `host.not_found`, `host.unreachable`, `host.bind_failed` | the guard; no file; the CLI finds no host; the port is taken |
| `host.timeout` | the CLI waited its timeout (60 s by default) for an answer |
| `host.no_players`, `seat.not_playable` | a call made as a seat to a host that opens no player clients; as a seat the game declares for no player (shared/contract/errors.md) |
| `debug.paused` | the call needs the game thread, which the debugger holds (3.4); `queued: true` when it was sent before the stop (or is a `time.control` sent while held) and runs at the boundary after the held tick |
| `debug.not_available`, `capture.not_available`, `render.not_available` | plug-in not installed |
| `host.session_unknown` | a `handle` or a session id that names no open agent session: it was closed, or the host started again since it opened (3.5) |
| `host.unauthorized` | a request to a locked host with no token, a malformed one or one no grant has: HTTP 401, the same answer on every route (3.6) |
| `host.grants_invalid` | `amoris serve --grants`: the grants file, or one of its grants, is refused, `{file, grant?, label?, field?, reason, problem?}`; also `Host::lock` on a host that is locked or serves already (3.6) |
| `host.not_locked` | raised by `amoris mcp --grant-token`, not by the host: the host it was pointed at was started without grants and takes no token (1) |

## 10. Verification

`tools/host/smoke_server.py` (standard library only) starts `amoris serve` on a copy of
`samples/sailing` and checks the CLI (status, tree, set, undo, a crate parented to the Sloop with
the tree indented by it, `--root` and `--depth`, a refused loop and the undo, step with an event and
a field condition, a save written, listed, loaded and refused, a refused typo with its suggestion,
Play and Stop, help), `/api/call` and `/api/catalog` (a destroy undone revives the entity under its
id), `/ws` (status on connect, subscribe, an edit with its `history` and `world.changed` pushes,
undo, steps, Play's reset), `events.since`/`why`, and MCP over HTTP (initialize, `tools/list` with
its size, calls and refusals), a host the debugger holds (3.4: the step that returns at the
breakpoint, reads while held, `scripts apply` refused at once, the debugger's short forms, restores
with and without the applied scripts), then that `.amoris/host.json` is gone after SIGTERM
(Ctrl-Break on Windows, where it now runs:
[smoke-server.txt](https://github.com/qiulinfan/amoris-benchmarks-results/blob/main/sources/pioneer-20261010/docs/evidence/agentdebug/smoke-server.txt),
0 failures, 2026-10-09). Its WebSocket client keeps the pushes that arrive before a request's answer
for the next wait (Play's reset can come first), which `tools/tests/test_smoke_server.py` checks
against a scripted server. The Python `mcp` client package (2.3.0, `uv run --with mcp`) drove both
transports (`amoris mcp --own` over stdio and `/mcp`). `crates/amoris-runtime/tests/host.rs` tests
undo and redo of a spawn, a set and a destroy, the readers, the stop conditions, Play and Stop, and
the kept snapshots; `tests/hierarchy.rs` the hierarchy (set and remove of `Parent`, every refusal
with its path, a call through a loop, destroys of subtrees with their undo and redo, one of 100
entities in one call each way, scenes, `project.save`, scripts, a recording, a fork, a restore, the
storage shuffle, `world.tree`'s parameters); `tests/saves.rs` the saves (a loaded save continues as
the saved world did, also in a fresh game; refused names, a missing and a damaged save; an older
project component; a recording that loads one replays; the thread's load drops later kept snapshots
and bumps the epoch); `tests/agent_reads.rs` the field reads, line ranges, samples and restores
under the applied scripts (with a recording that replays); amoris-app's `tests/paused_host.rs` a
held host through `Host::call` (also that `profile.frame` answers while held and leaves the held
tick out, and that the held host answers every nested `world.tree` as the game does) and
`tests/history_host.rs` a parent destroyed by an MCP session and undone whole by the editor;
`tests/thread.rs` that ticks are profiled by system and phase once a host serves the profile (a
reader alone turns nothing on) and that Play starts the ring again; amoris-link's `profile` tests
the summaries, the rows' order, the world's ticks only, and that a reader holding the ring never
stops the game thread.

Names (3.1; 2026-11-11, macOS): `crates/amoris-runtime/tests/edits.rs` tests the rename in both
forms with its recorded form, the refusals with their paths, the removal of a name, the names the
later ops of a call see and what an edit cannot write, and `tests/history.rs` that a rename is one
entry whose undo and redo put the names back, also for an entity that had none and for renames of
one group; each records and replays. `edit.rs`'s unit test decodes a canonical `world_edit` as
recordings held it before. amoris-app's `tests/history_host.rs` renames through `Host::call` (the
catalog's schema, the publication's `Name` rows, an MCP session, undo and redo), and the smoke test
renames over `/ws` as the editor does (the `world.changed` push naming `[id, "Name"]`, the tree by
the new name, a name of 66 bytes refused at `/ops/0/set/value`, the undo) and reads the string
form in `/api/catalog`.

Agent sessions (3.5; 2026-11-12, macOS): amoris-server's `sessions` unit tests cover the registry
alone (numbering, counters, the ring and `missed`, paging with `last`, rows kept while the ring
speaks of them, the shared row of a sessionless client, handles per host run); amoris-mcp's that a
caller is told who its client is once, at `initialize` or at the first call of a client that sent
none, and that a handler nobody called through says nothing. amoris-app's `tests/agent_sessions.rs`
serves the sailing course: an MCP client on `/mcp` with no editor listening is one row from its
`initialize`, its calls are read back with `agent.session.events`, an editor's socket that
subscribes afterwards is pushed only what follows, `DELETE /mcp` closes the row, and three requests
of a 2026-07-28 client are one more row; a bridge registers, calls with its handle, plays a seat
through the player tools alone and is closed, and none of it moves the world's tick, hash or
writes; and the built `amoris mcp`, attached to a served copy, is listed after its `initialize`,
its edit is its own in the history and its session closes when stdin does. `serve.rs` tests that a
bridge holding a handle of another host run registers once and retries; `tests/paused_host.rs` that
the four methods answer while the debugger holds the game. `editor/tools/agent-sessions.ts` drives
the editor's Agent panel against a real host in headless Chrome (editor.md 9).

Grants (3.6; 2026-11-12, macOS; Direct3D 12 and Windows not run): amoris-server's `grants` unit
tests read the file strictly (each refusal, and that none says a token), find a token whatever
differs (each of the 128 bits of a registered one flipped) and derive the checker's reads from a
catalog; `http::locked`'s the tokens a request presents and the route table by role; `sessions`' a
grant's row; `hostfile`'s that only a locked host writes `locked`. amoris-app's
`tests/locked_host.rs` serves the sailing course locked with a player, a checker and a developer
grant: more than 200 requests with no token, an unknown, cut, lengthened, non-hex or one-bit-off
one, another scheme, a token in the query, the body, a cookie or the path, on every route, get one
byte-identical 401, and none is anybody's call; a registered token from another `Host` is 403; the
player's token is its seat whatever the body says (`seat`, `handle`, `role`, `token`), is refused
every developer method and route, and over MCP by header and by path lists the `player` tool alone;
the checker's reads answer and every catalog entry and alias of another kind, asked of the catalog
itself, is refused, as are the two withheld reads, a body `seat` or `handle`, and the non-reads over
MCP, with the world's tick, hash and writes unchanged; the developer's does everything and the three
grants are the list's first rows; a host never locked answers with and without a token alike; a
locked and an unlocked host hash the same world after the same edit and 60 ticks; and the built
`amoris serve --grants`, its CLI with `--token` and `AMORIS_TOKEN`, and
`amoris mcp --attach --grant-token` are driven end to end, the refused grants files included.
`serve.rs` tests that a host started without CDP serves none.

## 11. Open

1. The hierarchy is built (2026-11-12: `Parent`, `world.tree` nested by it, a destroy that takes
   the subtree; 3 and 3.1). Still open: children do not follow their parent's `Transform`
   (simulation.md 14, choice 17); `world.get` does not answer an entity's `parent`, `children` or
   `path`, and `world.edit`'s spawn takes no nested `children` (shared/contract/mcp.md 6 and 6.1);
   the MCP `world` tool's schema does not name `root` and `depth` yet.
2. `debug.rewind`'s replay forward from a kept snapshot needs the recorded inputs between them (a
   recorder on the edit world). It replays under the applied scripts by default (3.3).
3. The type check runs `tsc` (TypeScript 7) only when installed: `AMORIS_TSC`, else the first
   TypeScript 7 (`tsc --version`, asked once per path) in a `node_modules` or `sdk/node_modules` at
   or above the project (`cd sdk && bun install`), the repository the binary was built from, or
   `PATH` (`amoris check`'s `types` step finds it the same way); an older one is passed over and
   named in `reason`. Its findings never block or delay a swap: the game compiles, swaps (or, for
   `scripts.check`, loads in a throwaway host) and writes the declarations in one
   `scripts.apply {types}`, and `tsc` starts after it answers. `scripts.check` with no TypeScript 7
   answers `typecheck: "unavailable"` and exits 0 on the CLI; `amoris check` calls that
   inconclusive.
4. The web worker's loop (`amoris-web`) runs plain step counts; stop conditions, Play and kept
   snapshots are native only, and it refuses a write trace (`time.trace_unavailable`, debugger.md
   13).
5. Saves (3.3) are the host's: scripts neither write nor load one, the browser has no store for
   them (its worker answers `project.no_root`), and a save holding an older version of a project
   component waits for the TypeScript migration steps (versions.md 8.3).
6. Agent sessions (3.5): the host lists the sessions agents open themselves and starts none.
   `agent.session.start` (the editor launching an agent program) waits for the owner's decision on
   which program, with whose credentials. A dead bridge's row has no expiry (a lease belongs with
   the tokens and grants of shared/contract/mcp.md, which are not built: a handle admits nobody).
   An agent driven through the `amoris` CLI still calls as the shared `api` client and is no
   session; the stream is not kept across host restarts. 2026-11-12: tokens now admit (3.6), on a
   host started with `--grants`, and there an agent driven through the CLI with its token is its
   grant's session. Still not built: leases for dead bridge rows, tokens minted, revoked or expired
   at run time, the browser editor on a locked host, and serving beyond loopback.
