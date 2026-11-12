// The mock's command catalog (`GET /api/catalog`): every method of host-protocol.md section 4 with
// its kind, doc and params JSON Schema. Params are refused with suggestions when they name a field
// the schema lacks, as the runtime does.

export interface CatalogCommand {
  name: string;
  kind: "read" | "write" | "control" | "request";
  doc: string;
  params: Record<string, unknown>;
  result?: Record<string, unknown>;
}

const obj = (properties: Record<string, unknown> = {}, required: string[] = []) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
});
const entityRef = { description: "An entity id, or its name when unique.", type: ["integer", "string"] };
const str = (description: string) => ({ type: "string", description });
const int = (description: string) => ({ type: "integer", description });

export const CATALOG: CatalogCommand[] = [
  { name: "catalog.list", kind: "read", doc: "The catalog: every command with its kind, doc and schemas.", params: obj() },
  { name: "project.info", kind: "read", doc: "The project's name, root, scenes, scripts and assets.", params: obj() },
  { name: "project.save", kind: "write", doc: "Writes the edit world back to the project's scene files.", params: obj() },
  {
    name: "world.tree",
    kind: "read",
    doc: "The entity hierarchy with names and component names.",
    params: obj({ root: entityRef, depth: int("Levels below the roots."), filter: str("A name substring or a component name.") }),
  },
  {
    name: "world.get",
    kind: "read",
    doc: "An entity's components as JSON.",
    params: obj({ entity: entityRef, components: { type: "array", items: { type: "string" }, description: "Components to read; all when empty." } }, ["entity"]),
  },
  {
    name: "world.query",
    kind: "read",
    doc: "Rows of the entities that have every listed component, with the listed fields.",
    params: obj({ with: { type: "array", items: { type: "string" } }, fields: { type: "array", items: { type: "string" }, description: "\"Component.field\" columns." }, limit: int("At most this many rows.") }, ["with"]),
  },
  {
    name: "world.schema",
    kind: "read",
    doc: "Component JSON Schemas with docs, from the registry.",
    params: obj({ component: str("One component; all when absent.") }),
  },
  {
    name: "world.edit",
    kind: "write",
    doc: "Spawn, set, remove and destroy, checked whole and applied at one boundary; one undo entry. Edits that share a `group` merge into one entry.",
    params: obj(
      {
        ops: { type: "array", minItems: 1, maxItems: 64, items: { type: "object" }, description: "{spawn}, {set}, {remove} or {destroy}." },
        label: str("The undo entry's label."),
        group: str("Edits with the same group, one after another, merge into one undo entry (a drag)."),
      },
      ["ops"],
    ),
  },
  {
    name: "history.undo",
    kind: "write",
    doc: "Undoes the last edit; with `group`, only when that edit is the whole of the group.",
    params: obj({ group: str("Undo only when the next entry is the whole of this group (a drag), else history.group.") }),
  },
  { name: "history.redo", kind: "write", doc: "Redoes the last undone edit.", params: obj() },
  { name: "history.list", kind: "read", doc: "The undo labels and the redo labels, the next to undo or redo last, with each entry's source and group under `entries`.", params: obj() },
  {
    name: "time.control",
    kind: "control",
    doc: "Pause, resume, set the real-time speed or the pacing.",
    params: obj({
      pause: { type: "boolean", description: "Pause (true) or resume (false)." },
      speed: { type: "number", description: "Real time at this multiple of real time (0 < speed <= 1000)." },
      pacing: {
        description: "`\"stepped\"` or `{\"real_time\": {\"speed\": x}}`.",
        oneOf: [
          { type: "string", const: "stepped", description: "Ticks run only when a `step` asks for them." },
          {
            type: "object",
            description: "Ticks follow the loop's clock at `speed` times real time.",
            properties: { real_time: { type: "object", properties: { speed: { type: "number" } }, required: ["speed"] } },
            required: ["real_time"],
            additionalProperties: false,
          },
        ],
      },
    }),
  },
  {
    name: "time.step",
    kind: "control",
    doc: "Run ticks; answered after the last, or when an event named `until` is emitted.",
    params: obj({ ticks: int("Ticks to run (1 by default)."), until: str("Stop when an event of this name is emitted."), watch: { type: "object" } }),
  },
  {
    name: "play.start",
    kind: "control",
    doc: "Play: forks the edit world and runs the fork in real time; edits go to the fork.",
    params: obj({ speed: { type: "number", description: "Real-time speed, default 1." }, paused: { type: "boolean", description: "Start paused." } }),
  },
  { name: "play.stop", kind: "control", doc: "Stop: discard the play world and return to the edit world.", params: obj() },
  { name: "scripts.list", kind: "read", doc: "The project's scripts with their diagnostics.", params: obj() },
  { name: "scripts.read", kind: "read", doc: "A script's text.", params: obj({ path: str("Relative to the project root.") }, ["path"]) },
  { name: "scripts.write", kind: "write", doc: "Writes a script and returns its diagnostics.", params: obj({ path: str("Relative to the project root."), text: str("The whole file.") }, ["path", "text"]) },
  { name: "scripts.apply", kind: "write", doc: "Type check, compile and hot swap the scripts at a tick boundary.", params: obj({ paths: { type: "array", items: { type: "string" } } }) },
  { name: "assets.list", kind: "read", doc: "The project's asset files with kinds and thumbnails.", params: obj({ dir: str("A directory under assets/.") }) },
  {
    name: "assets.upload",
    kind: "request",
    doc: "Writes a file into the project (an asset from the editor's browser or an agent): its bytes in base64 parts {path, data, offset, done}, offset 0 first, each part at most 1 MiB; the last part (done, the default) moves the file into place and answers {path, kind, bytes}. A file already at path is replaced only with overwrite.",
    params: obj(
      {
        path: str("Where the file goes: a path inside the project, such as `models/lighthouse.glb`."),
        data: str("This part's bytes, base64 (the standard alphabet, padded)."),
        offset: { type: "integer", minimum: 0, default: 0, description: "Where this part starts in the file: 0 begins the upload (again)." },
        done: { type: "boolean", default: true, description: "Whether this is the last part (default true): the file is then moved into place." },
        overwrite: { type: "boolean", default: false, description: "Whether a file already at `path` is replaced (default false: refused with `assets.exists`)." },
      },
      ["path", "data"],
    ),
  },
  {
    name: "events.since",
    kind: "read",
    doc: "Game events after a stream number (the newest without one), as {events, last, missed}.",
    params: obj({ seq: int("Events after this stream number; default: the newest."), limit: int("At most this many, default 100.") }),
  },
  { name: "events.why", kind: "read", doc: "An event and the chain of events that caused it, nearest first, as {event, causes, complete}.", params: obj({ seq: int("The event's stream number.") }, ["seq"]) },
  { name: "snapshots.list", kind: "read", doc: "The ring of kept snapshots.", params: obj() },
  { name: "snapshots.restore", kind: "control", doc: "Restores the kept snapshot at a tick.", params: obj({ tick: int("A kept snapshot's tick.") }, ["tick"]) },
  // The debugger: amoris-debug's agents' API as the real host serves it (docs/spec/debugger.md 7).
  { name: "debug.attach", kind: "control", doc: "Starts debugging: the scripts are instrumented from the next tick (breakpoints, pauses and watches attach implicitly).", params: obj() },
  { name: "debug.detach", kind: "control", doc: "Stops debugging: the agents' breakpoints and watches go, a pause resumes, scripts run uninstrumented from the next tick.", params: obj() },
  {
    name: "debug.breakpoints.set",
    kind: "control",
    doc: "Sets a breakpoint on a TypeScript line (1-based), with an optional condition, or a logpoint.",
    params: obj({ file: str("The module path, as scripts.list names it: scripts/rules.ts."), line: int("The TypeScript line, 1-based; a line without code moves to the next line with code."), condition: str("Stop only when this expression, evaluated in the stopped frame, is truthy."), log: str("A logpoint: log this template literal's text instead of stopping.") }, ["file", "line"]),
  },
  { name: "debug.breakpoints.clear", kind: "control", doc: "Removes a breakpoint, or every breakpoint agents set.", params: obj({ id: str("The breakpoint; every breakpoint agents set when absent.") }) },
  { name: "debug.breakpoints.list", kind: "read", doc: "Lists every breakpoint (the agents' and the CDP clients') with where it resolves.", params: obj() },
  { name: "debug.pause", kind: "control", doc: "Stops the game at the next script statement; waits up to timeout_ms for it.", params: obj({ timeout_ms: int("Wait up to this long for the game to stop (0: return at once).") }) },
  { name: "debug.continue", kind: "control", doc: "Resumes a paused game (and releases a game waiting for a debugger).", params: obj() },
  { name: "debug.step", kind: "control", doc: "Steps over, into or out of the current statement and returns where it stopped.", params: obj({ kind: { type: "string", enum: ["over", "into", "out"] }, timeout_ms: int("Wait up to this long for the step to stop (default 5000).") }, ["kind"]) },
  { name: "debug.state", kind: "read", doc: "Running or paused; when paused, why, the tick and system, and the frames with TypeScript locations and locals.", params: obj() },
  { name: "debug.eval", kind: "write", doc: "Evaluates an expression in a frame of the paused game.", params: obj({ expr: str("A JavaScript expression; it sees the frame's arguments, locals and closure variables."), frame: int("The frame, as debug.state numbers them (0: where the game stopped).") }, ["expr"]) },
  {
    name: "debug.set",
    kind: "control",
    doc: "Sets a variable (an argument, a local or a closure variable) of a frame of the paused game to an expression's value; a const refuses.",
    params: obj(
      {
        name: str("The variable: an argument, a local or a closure variable of the frame, by name. A value inside an object (l.distance[0]) is assigned through debug.eval."),
        value: str("A JavaScript expression, evaluated in the frame; its value is assigned."),
        frame: int("The frame, as debug.state numbers them (0: where the game stopped)."),
      },
      ["name", "value"],
    ),
  },
  {
    name: "debug.watch",
    kind: "control",
    doc: "A data breakpoint: pause when a script system's staged write changes the field.",
    params: obj({ entity: int("The entity's id."), component: str("A component scripts can write, such as Log."), field: str("One field (every field when absent).") }, ["entity", "component"]),
  },
  { name: "debug.unwatch", kind: "control", doc: "Removes a data breakpoint, or every one agents set.", params: obj({ id: str("The watch; every watch agents set when absent.") }) },
  { name: "debug.exceptions", kind: "control", doc: "Pauses on exceptions: none, uncaught (no try catches them) or all.", params: obj({ mode: { type: "string", enum: ["none", "uncaught", "all"] } }, ["mode"]) },
  { name: "debug.wait", kind: "read", doc: "Waits up to timeout_ms for the game to stop and returns the state.", params: obj({ timeout_ms: int("Wait up to this long for the game to stop (default 5000).") }) },
  { name: "debug.rewind", kind: "control", doc: "Restores the kept snapshot at or before a tick and steps the world forward to it.", params: obj({ tick: int("The tick to stand at.") }, ["tick"]) },
  {
    name: "profile.frame",
    kind: "read",
    doc: "What the last ticks cost the game thread (default 60), in ms per tick: each system's own time, tick.other, loop.present and loop.publish, and each phase; ticks the debugger held are counted in held and left out.",
    params: obj({ ticks: int("The last this many ticks, default 60 (the ring keeps 600).") }),
  },
  // The host's agent sessions (docs/spec/server.md 3.5).
  {
    name: "agent.session.list",
    kind: "read",
    doc: "The host's agent sessions in opening order, {sessions: [{id, transport, role, seat, label, calls, failed_calls, last_method, live, opened_ts, last_ts}], last}.",
    params: obj(),
  },
  {
    name: "agent.session.events",
    kind: "read",
    doc: "What the agent sessions called, {seq, ts, session, kind: opened|call|result|closed, ...}: the newest (the host keeps 2,000), or with since the first limit after it; last is the cursor to ask again with, missed counts what left the ring.",
    params: obj({
      since: int("Events after this stream number, oldest first; without it, the newest."),
      limit: int("At most this many, default 100 (the host keeps the newest 2,000)."),
      session: str("One session's events, by its id (mcp-3)."),
    }),
  },
  {
    name: "agent.session.open",
    kind: "control",
    doc: "Registers the caller as an agent session (with seat: that seat's player) and answers its row and a handle.",
    params: obj({ label: str("What the session is called in the list."), seat: str("The seat the session plays; without it the session is a developer's.") }),
  },
  {
    name: "agent.session.close",
    kind: "control",
    doc: "Closes a registered session: by the handle its opener holds, or a bridge session by its id (session) when its process is gone.",
    params: obj({ handle: str("The handle agent.session.open answered."), session: str("A bridge session's id (mcp-3).") }),
  },
  {
    name: "agent.session.agents",
    kind: "read",
    doc: "The agent programs this host may start, {agents: [{name, label}], running, limit}: registered by the process owner with amoris serve --agents <file>.",
    params: obj(),
  },
  {
    name: "agent.session.start",
    kind: "control",
    doc: "Starts a registered agent program as an agent session of this host (transport launch) with a goal, and with seat as that seat's player; its output lines are the session's message events and the session closes when it exits.",
    params: obj({ agent: str("A program the host registered."), goal: str("What the agent is asked to do."), seat: str("The seat it plays; without it a developer.") }, ["agent", "goal"]),
  },
  {
    name: "agent.session.stop",
    kind: "control",
    doc: "Ends a started agent program; answers its session's row once closed.",
    params: obj({ session: str("The started session's id (mcp-3).") }, ["session"]),
  },
];
