// The wire forms of docs/spec/host-protocol.md: requests, responses, pushed events and the typed
// params and results of the section 4 methods. Component values stay `unknown` here; their shapes
// come from `world.schema` at run time (see schema.ts), never from compiled-in types.

export type RequestId = number | string;
export type EntityId = number;
export type Vec3 = [number, number, number];
export type Quat = [number, number, number, number];

export interface WireError {
  code: string;
  message: string;
  detail?: Record<string, unknown>;
}

export interface WireRequest {
  id: RequestId;
  method: string;
  params?: unknown;
}

export interface WireResponse {
  id: RequestId | null;
  result?: unknown;
  error?: WireError;
}

export interface WireEvent {
  event: string;
  data: unknown;
}

// ---- Pushed events (section 3) ------------------------------------------------------------------

export interface Status {
  tick: number;
  t_s: number;
  paused: boolean;
  pacing: string;
  mode: "edit" | "play";
  world_hash: string;
  entities: number;
  fps?: number;
  tick_ms?: number;
  speed?: number;
}

export interface WorldChanged {
  tick: number;
  spawned: EntityId[];
  despawned: EntityId[];
  changed: [EntityId, string][];
}

export interface GameEvent {
  seq: number;
  tick: number;
  name: string;
  subject: EntityId | null;
  data: unknown;
  cause: number | null;
}

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface LogEntry {
  level: LogLevel;
  source: string;
  message: string;
  file?: string;
  line?: number;
  tick?: number;
}

export interface HistoryState {
  undo: string[];
  redo: string[];
}

// ---- The debugger ---------------------------------------------------------------------------------
// The host's `debug.*` methods are pocket-debug's agents' API (docs/spec/debugger.md 7), and its
// `debug` events carry the same state. `Host*` types are those wire shapes; `api.ts` normalizes them
// into the editor's view below (frames with scopes, variables as expandable trees).

/** A TypeScript position, 1-based; `generated` when no TypeScript maps there (the JavaScript's). */
export interface SourceLocation {
  file: string;
  line: number;
  column?: number;
  generated?: boolean;
}

/** A variable as the host sends it: its type and a JSON preview (objects to three levels). */
export interface HostVariable {
  name: string;
  type: string;
  value: unknown;
  /** Objects only: `Float64Array(1)`, `Array(3)`, `Object`. */
  description?: string;
}

/** A stopped frame, innermost first; `frame` is the engine's level, not the frame's index. */
export interface HostFrame {
  frame: number;
  function: string;
  location: SourceLocation;
  locals: HostVariable[];
  closure: HostVariable[];
  /** A frame that has returned (a data breakpoint seen when `run` returned): position only. */
  returned: boolean;
}

/** A data breakpoint's hit. */
export interface HostWatchHit {
  watch: string;
  entity: EntityId;
  component: string;
  field: string | null;
  names: string[];
  before: unknown;
  after: unknown;
  written_at: SourceLocation | null;
  after_return: boolean;
}

export interface HostBreakpoint {
  id: string;
  /** "agent" (the host's `debug.*` callers: the editor, MCP agents) or "cdp" (DevTools, VS Code). */
  owner: string;
  target: { file?: string; line?: number; url?: string; url_regex?: string; script_id?: string; js_line?: number };
  condition: string | null;
  log: string | null;
  /** Where it binds in the running scripts (TypeScript); empty when nothing runs there yet. */
  locations: SourceLocation[];
}

export interface HostDataWatch {
  id: string;
  entity: EntityId;
  component: string;
  field: string | null;
}

export type ExceptionMode = "none" | "uncaught" | "all";

/** `debug.state`; a `debug` event carries the first group (and `{state: "running"}` on a resume). */
export interface HostDebugState {
  state: "paused" | "running";
  reason?: string;
  tick?: number;
  system?: string | null;
  location?: SourceLocation | null;
  frames?: HostFrame[];
  hit_breakpoints?: string[];
  exception?: { text: string | null; caught: boolean };
  data?: HostWatchHit;
  // debug.state only:
  attached?: boolean;
  instrumented?: boolean;
  exceptions?: ExceptionMode;
  breakpoints?: HostBreakpoint[];
  watches?: HostDataWatch[];
  waiting_for_debugger?: boolean;
  /** The Chrome DevTools Protocol endpoint, while the host serves one. */
  cdp?: { ws: string; devtools: string } | null;
}

export interface HostEvalResult {
  type: string;
  value: unknown;
  description?: string | null;
}

/** A value shown as a tree (debugger scopes, watches, console results). */
export interface Variable {
  name: string;
  value: string;
  type: string;
  children?: Variable[];
  /** The expression that reaches it in its frame (`l.distance[0]`), when it can be assigned. */
  path?: string;
}

export interface Scope {
  name: string;
  variables: Variable[];
}

/** A frame as the editor shows it; `id` is its index, which `debug.eval`'s `frame` takes. */
export interface StackFrame {
  id: number;
  name: string;
  file: string;
  line: number;
  column?: number;
  generated?: boolean;
  returned?: boolean;
  scopes: Scope[];
}

export interface DebugState {
  state: "paused" | "running";
  reason?: string;
  detail?: string;
  location?: SourceLocation;
  frames?: StackFrame[];
  tick?: number;
  system?: string;
  hitBreakpoints?: string[];
  data?: HostWatchHit;
  exception?: { text: string | null; caught: boolean };
  // From debug.state only (undefined in events):
  exceptions?: ExceptionMode;
  attached?: boolean;
  instrumented?: boolean;
  cdp?: { ws: string; devtools: string } | null;
}

export interface ProfileSample {
  tick: number;
  systems: { name: string; ms: number }[];
  frame_ms: number;
  gpu: { pass: string; ms: number }[];
}

export interface AgentEvent {
  session: string;
  kind: "call" | "result" | "message";
  method?: string;
  summary: string;
  ok?: boolean;
  ts?: number;
}

export interface PickEvent {
  x: number;
  y: number;
  entity: EntityId | null;
}

export interface TopicData {
  status: Status;
  "world.changed": WorldChanged;
  events: GameEvent[];
  log: LogEntry;
  history: HistoryState;
  debug: HostDebugState;
  profile: ProfileSample;
  agent: AgentEvent;
  pick: PickEvent;
}

export type Topic = keyof TopicData;
export const ALL_TOPICS: Topic[] = ["status", "world.changed", "events", "log", "history", "debug", "profile", "agent", "pick"];

// ---- Method params and results (section 4) ------------------------------------------------------

export interface CatalogCommand {
  name: string;
  kind: string;
  doc: string;
  params?: JsonSchemaLike;
  result?: JsonSchemaLike;
}

export type JsonSchemaLike = Record<string, unknown>;

export interface ProjectInfo {
  name: string;
  root: string;
  scenes: string[];
  scripts: string[] | number;
  assets: unknown;
  host?: string;
}

export interface TreeNode {
  id: EntityId;
  name: string;
  components: string[];
  children: TreeNode[];
}

export interface EntityData {
  id: EntityId;
  name: string | null;
  components: Record<string, unknown>;
}

export interface ComponentInfo {
  name: string;
  origin: "Engine" | "Project" | string;
  version: number;
  doc: string;
  schema: JsonSchemaLike;
}

export type EntityRef = EntityId | string;

export type EditOp =
  | { spawn: { name?: string; components?: Record<string, unknown> } }
  | { set: { entity: EntityRef; component: string; value: unknown } }
  | { remove: { entity: EntityRef; component: string } }
  | { destroy: { entity: EntityRef } };

export interface WorldEditParams {
  ops: EditOp[];
  label?: string;
  /** Consecutive edits with the same group merge into one undo entry (editor.md, Protocol needs). */
  group?: string;
}

export interface WorldEditResult {
  tick: number;
  applied: number;
  spawned: EntityId[];
}

export interface Diagnostic {
  file?: string;
  line: number;
  column: number;
  end_line?: number;
  end_column?: number;
  severity: "error" | "warning" | "info" | "hint";
  message: string;
  code?: string | number;
}

export interface ScriptInfo {
  path: string;
  bytes: number;
  diagnostics: Diagnostic[];
}

export interface AssetInfo {
  path: string;
  kind: string;
  bytes: number;
  thumbnail?: string;
}

export interface SnapshotInfo {
  tick: number;
  t_s?: number;
  hash?: string;
}

export interface Breakpoint {
  id: string;
  file: string;
  line: number;
  condition?: string;
  log?: string;
  /** It binds to code of the running scripts. */
  verified?: boolean;
  /** "agent" (set through `debug.*`) or "cdp" (DevTools, VS Code). */
  owner?: string;
  hits?: number;
}

export interface DataWatch {
  id: string;
  entity: EntityId;
  component: string;
  field?: string;
}

export interface Methods {
  "catalog.list": [Record<string, never>, CatalogCommand[]];
  "project.info": [Record<string, never>, ProjectInfo];
  "project.save": [Record<string, never>, { files: string[] }];
  "world.tree": [{ root?: EntityRef; depth?: number; filter?: string }, TreeNode[]];
  "world.get": [{ entity: EntityRef; components?: string[] }, EntityData];
  "world.query": [{ with: string[]; fields?: string[]; limit?: number }, Record<string, unknown>[]];
  "world.schema": [{ component?: string }, ComponentInfo[]];
  "world.edit": [WorldEditParams, WorldEditResult];
  "history.undo": [Record<string, never>, { label: string | null }];
  "history.redo": [Record<string, never>, { label: string | null }];
  "history.list": [Record<string, never>, HistoryState];
  "time.control": [{ pause?: boolean; speed?: number; pacing?: string }, Status];
  "time.step": [{ ticks?: number; until?: string; watch?: unknown }, { tick: number; stopped_by?: unknown }];
  "play.start": [Record<string, never>, Status];
  "play.stop": [Record<string, never>, Status];
  "scripts.list": [Record<string, never>, ScriptInfo[]];
  "scripts.read": [{ path: string }, string | { text: string }];
  "scripts.write": [{ path: string; text: string }, { diagnostics: Diagnostic[] }];
  "scripts.apply": [{ paths?: string[] }, { bundle: string | null; diagnostics: Diagnostic[] }];
  "assets.list": [{ dir?: string }, AssetInfo[]];
  "assets.import": [{ path: string }, { asset: string; meshes: number; materials: number }];
  "events.since": [{ seq: number; limit?: number }, GameEvent[]];
  "events.why": [{ seq: number }, GameEvent[]];
  "snapshots.list": [Record<string, never>, SnapshotInfo[]];
  "snapshots.restore": [{ tick: number }, Status];
  "debug.attach": [Record<string, never>, HostDebugState];
  "debug.detach": [Record<string, never>, HostDebugState];
  "debug.breakpoints.set": [{ file: string; line: number; condition?: string; log?: string }, { id: string; file: string; line: number; verified: boolean; locations: SourceLocation[] }];
  "debug.breakpoints.clear": [{ id?: string }, { cleared: number }];
  "debug.breakpoints.list": [Record<string, never>, { breakpoints: HostBreakpoint[] }];
  "debug.pause": [{ timeout_ms?: number }, HostDebugState];
  "debug.continue": [Record<string, never>, { state: "running" }];
  "debug.step": [{ kind: "over" | "into" | "out"; timeout_ms?: number }, HostDebugState];
  "debug.state": [Record<string, never>, HostDebugState];
  "debug.eval": [{ expr: string; frame?: number }, HostEvalResult];
  "debug.watch": [{ entity: EntityId; component: string; field?: string }, HostDataWatch];
  "debug.unwatch": [{ id?: string }, { cleared: number }];
  "debug.exceptions": [{ mode: ExceptionMode }, { mode: ExceptionMode }];
  "debug.wait": [{ timeout_ms?: number }, HostDebugState];
  "debug.rewind": [{ tick: number }, { restored: number; tick: number }];
  "profile.frame": [Record<string, never>, ProfileSample];
  subscribe: [{ topics: string[] }, { topics: string[] }];
}

export type MethodName = keyof Methods;
export type Params<M extends MethodName> = Methods[M][0];
export type Result<M extends MethodName> = Methods[M][1];
