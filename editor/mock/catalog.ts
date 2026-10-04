// The mock's command catalog (`GET /api/catalog`): every method of host-protocol.md section 4 with
// its kind, doc and params JSON Schema. Params are refused with suggestions when they name a field
// the schema lacks, as the runtime does.

export interface CatalogCommand {
  name: string;
  kind: "read" | "write" | "control";
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
  { name: "history.undo", kind: "write", doc: "Undoes the last edit.", params: obj() },
  { name: "history.redo", kind: "write", doc: "Redoes the last undone edit.", params: obj() },
  { name: "history.list", kind: "read", doc: "The undo and redo labels.", params: obj() },
  {
    name: "time.control",
    kind: "control",
    doc: "Pause, resume or change the speed and pacing.",
    params: obj({ pause: { type: "boolean" }, speed: { type: "number", minimum: 0.0625, maximum: 16 }, pacing: { type: "string", enum: ["realtime", "fast"] } }),
  },
  {
    name: "time.step",
    kind: "control",
    doc: "Run ticks; answered after the last, or when an event named `until` is emitted.",
    params: obj({ ticks: int("Ticks to run (1 by default)."), until: str("Stop when an event of this name is emitted."), watch: { type: "object" } }),
  },
  { name: "play.start", kind: "control", doc: "Play: run a fork of the edit world.", params: obj() },
  { name: "play.stop", kind: "control", doc: "Stop: discard the play world and return to the edit world.", params: obj() },
  { name: "scripts.list", kind: "read", doc: "The project's scripts with their diagnostics.", params: obj() },
  { name: "scripts.read", kind: "read", doc: "A script's text.", params: obj({ path: str("Relative to the project root.") }, ["path"]) },
  { name: "scripts.write", kind: "write", doc: "Writes a script and returns its diagnostics.", params: obj({ path: str("Relative to the project root."), text: str("The whole file.") }, ["path", "text"]) },
  { name: "scripts.apply", kind: "write", doc: "Type check, compile and hot swap the scripts at a tick boundary.", params: obj({ paths: { type: "array", items: { type: "string" } } }) },
  { name: "assets.list", kind: "read", doc: "The project's asset files with kinds and thumbnails.", params: obj({ dir: str("A directory under assets/.") }) },
  { name: "assets.import", kind: "write", doc: "Imports a file into the project's assets.", params: obj({ path: str("The file to import.") }, ["path"]) },
  { name: "events.since", kind: "read", doc: "Game events after a sequence number.", params: obj({ seq: int("Exclusive."), limit: int("At most this many.") }, ["seq"]) },
  { name: "events.why", kind: "read", doc: "The cause chain of an event, oldest first.", params: obj({ seq: int("The event.") }, ["seq"]) },
  { name: "snapshots.list", kind: "read", doc: "The ring of kept snapshots.", params: obj() },
  { name: "snapshots.restore", kind: "control", doc: "Restores the kept snapshot at a tick.", params: obj({ tick: int("A kept snapshot's tick.") }, ["tick"]) },
  {
    name: "debug.breakpoints.set",
    kind: "control",
    doc: "Sets a breakpoint on a script line, with an optional condition.",
    params: obj({ file: str("Script path."), line: int("1-based."), condition: str("A JavaScript expression over the frame's locals.") }, ["file", "line"]),
  },
  { name: "debug.breakpoints.clear", kind: "control", doc: "Clears one breakpoint (by id or file and line), a file's, or all.", params: obj({ id: int("Breakpoint id."), file: str("Script path."), line: int("1-based.") }) },
  { name: "debug.breakpoints.list", kind: "read", doc: "Every breakpoint.", params: obj() },
  { name: "debug.pause", kind: "control", doc: "Pauses at the next script statement.", params: obj() },
  { name: "debug.continue", kind: "control", doc: "Resumes after a pause.", params: obj() },
  { name: "debug.step", kind: "control", doc: "Steps over, into or out of the current statement.", params: obj({ kind: { type: "string", enum: ["over", "into", "out"] } }, ["kind"]) },
  { name: "debug.state", kind: "read", doc: "Frames with TypeScript locations, scopes with locals, the tick and the system.", params: obj() },
  { name: "debug.eval", kind: "read", doc: "Evaluates an expression on a paused frame, or in the console's scope.", params: obj({ expr: str("JavaScript."), frame: int("Frame id; 0 is the top.") }, ["expr"]) },
  {
    name: "debug.watch",
    kind: "control",
    doc: "A data breakpoint: pause when a system writes the component (or one field).",
    params: obj({ entity: entityRef, component: str("Component name."), field: str("Field name.") }, ["entity", "component"]),
  },
  { name: "debug.unwatch", kind: "control", doc: "Removes a data breakpoint.", params: obj({ id: int("Watch id.") }, ["id"]) },
  { name: "debug.rewind", kind: "control", doc: "Restores the kept snapshot at or before a tick and replays to it.", params: obj({ tick: int("The tick to reach.") }, ["tick"]) },
  { name: "profile.frame", kind: "read", doc: "The last frame's CPU and GPU timings.", params: obj() },
];
