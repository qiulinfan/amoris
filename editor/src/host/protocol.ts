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

export interface SourceLocation {
  file: string;
  line: number;
  column?: number;
}

export interface Variable {
  name: string;
  value: string;
  type: string;
  children?: Variable[];
}

export interface Scope {
  name: string;
  variables: Variable[];
}

export interface StackFrame {
  id: number;
  name: string;
  file: string;
  line: number;
  column?: number;
  system?: string;
  scopes?: Scope[];
}

export interface DebugState {
  state: "paused" | "running";
  reason?: string;
  detail?: string;
  location?: SourceLocation;
  frames?: StackFrame[];
  tick?: number;
  system?: string;
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
  debug: DebugState;
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
  /** Who found it: `"tsc"` for the type check, else the engine's compiler, lint or loader. */
  source?: string;
}

/** `scripts.types`: the SDK's declarations for the project (the files tsc checks against). */
export interface ScriptTypes {
  dir: string | null;
  components: { engine: string[]; project: string[]; unavailable: string[] };
  project_from: "scripts" | "registry";
  diagnostics: Diagnostic[];
  /** With `text: true`: `pocket.d.ts` and `components.d.ts`. */
  text?: Record<string, string>;
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
  id: number;
  file: string;
  line: number;
  condition?: string;
  verified?: boolean;
  hits?: number;
}

export interface DataWatch {
  id: number;
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
  "scripts.types": [{ text?: boolean; tsconfig?: boolean }, ScriptTypes];
  "assets.list": [{ dir?: string }, AssetInfo[]];
  "assets.import": [{ path: string }, { asset: string; meshes: number; materials: number }];
  "events.since": [{ seq: number; limit?: number }, GameEvent[]];
  "events.why": [{ seq: number }, GameEvent[]];
  "snapshots.list": [Record<string, never>, SnapshotInfo[]];
  "snapshots.restore": [{ tick: number }, Status];
  "debug.breakpoints.set": [{ file: string; line: number; condition?: string }, Breakpoint];
  "debug.breakpoints.clear": [{ id?: number; file?: string; line?: number }, { cleared: number }];
  "debug.breakpoints.list": [Record<string, never>, { breakpoints: Breakpoint[]; watches?: DataWatch[] }];
  "debug.pause": [Record<string, never>, unknown];
  "debug.continue": [Record<string, never>, unknown];
  "debug.step": [{ kind: "over" | "into" | "out" }, unknown];
  "debug.state": [Record<string, never>, DebugState];
  "debug.eval": [{ expr: string; frame?: number }, Variable];
  "debug.watch": [{ entity: EntityRef; component: string; field?: string }, DataWatch];
  "debug.unwatch": [{ id: number }, unknown];
  "debug.rewind": [{ tick: number }, Status];
  "profile.frame": [Record<string, never>, ProfileSample];
  subscribe: [{ topics: string[] }, { topics: string[] }];
}

export type MethodName = keyof Methods;
export type Params<M extends MethodName> = Methods[M][0];
export type Result<M extends MethodName> = Methods[M][1];
