// Typed wrappers over the host-protocol methods, and the editor's single connection to the host.

import { HostClient, endpointsFromLocation } from "./client";
import type {
  Breakpoint,
  DataWatch,
  DebugState,
  EditOp,
  EntityId,
  EntityRef,
  ExceptionMode,
  HostBreakpoint,
  HostDataWatch,
  HostDebugState,
  HostVariable,
  StackFrame,
  Variable,
  WorldEditParams,
} from "./protocol";

export const host = new HostClient(endpointsFromLocation());

const call = host.call.bind(host);

// The Rust host's as-built result shapes (docs/spec/server.md) wrap some lists in objects; the
// editor's types are the plain lists. Normalize here, in one place.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const listOf = (r: any, key: string): any => (Array.isArray(r) ? r : (r?.[key] ?? []));

export const api = {
  project: {
    info: () => call("project.info", {}),
    save: () => call("project.save", {}),
  },
  world: {
    tree: (p: { root?: EntityRef; depth?: number; filter?: string } = {}) => call("world.tree", p),
    get: (entity: EntityRef, components?: string[]) => call("world.get", components ? { entity, components } : { entity }),
    query: (p: { with: string[]; fields?: string[]; limit?: number }) => call("world.query", p),
    schema: (component?: string) => call("world.schema", component ? { component } : {}),
    edit: (ops: EditOp[], label?: string, group?: string) => {
      const p: WorldEditParams = { ops };
      if (label) p.label = label;
      if (group) p.group = group;
      return call("world.edit", p);
    },
  },
  history: {
    undo: () => call("history.undo", {}),
    redo: () => call("history.redo", {}),
    list: () => call("history.list", {}),
  },
  time: {
    control: (p: { pause?: boolean; speed?: number; pacing?: string }) => call("time.control", p),
    step: (p: { ticks?: number; until?: string } = {}) => {
      // `until: "event:<name>"` or `"tick:<n>"` becomes the host's {event} / {tick} object.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const q: any = { ...p };
      if (typeof p.until === "string") {
        const [kind, rest] = p.until.split(/:(.*)/s);
        q.until = kind === "tick" ? { tick: Number(rest) } : { event: rest ?? p.until };
      }
      return call("time.step", q, 120000);
    },
  },
  play: {
    start: () => call("play.start", {}),
    stop: () => call("play.stop", {}),
  },
  scripts: {
    list: () => call("scripts.list", {}),
    read: async (path: string): Promise<string> => {
      const r = await call("scripts.read", { path });
      return typeof r === "string" ? r : r.text;
    },
    write: (path: string, text: string) => call("scripts.write", { path, text }),
    apply: (paths?: string[]) => call("scripts.apply", paths ? { paths } : {}, 120000),
  },
  assets: {
    list: (dir?: string) => call("assets.list", dir ? { dir } : {}),
    import: (path: string) => call("assets.import", { path }, 120000),
  },
  events: {
    since: async (seq: number, limit?: number) =>
      listOf(await call("events.since", limit ? { seq, limit } : { seq }), "events"),
    why: async (seq: number) => listOf(await call("events.why", { seq }), "chain"),
  },
  snapshots: {
    list: async () => listOf(await call("snapshots.list", {}), "snapshots"),
    restore: (tick: number) => call("snapshots.restore", { tick }),
  },
  debug: {
    setBreakpoint: async (file: string, line: number, condition?: string): Promise<Breakpoint> => {
      const r = await call("debug.breakpoints.set", condition ? { file, line, condition } : { file, line });
      return { id: r.id, file: r.file, line: r.line, verified: r.verified, condition: condition || undefined, owner: "agent" };
    },
    /** One breakpoint, or every breakpoint set through `debug.*` (not those of CDP clients). */
    clearBreakpoint: (id?: string) => call("debug.breakpoints.clear", id === undefined ? {} : { id }),
    listBreakpoints: async () => (await call("debug.breakpoints.list", {})).breakpoints.map(breakpointOf),
    pause: () => call("debug.pause", {}),
    resume: () => call("debug.continue", {}),
    step: async (kind: "over" | "into" | "out") => debugState(await call("debug.step", { kind }, 15000)),
    state: async () => debugState(await call("debug.state", {})),
    /** The state with the breakpoints and data breakpoints every frontend set. */
    session: async () => {
      const r = await call("debug.state", {});
      return { state: debugState(r), breakpoints: r.breakpoints?.map(breakpointOf), watches: r.watches?.map(dataWatchOf) };
    },
    /** `path`: the expression's value can be assigned through it (a variable's name). */
    eval: async (expr: string, frame?: number, path?: string): Promise<Variable> => {
      const r = await call("debug.eval", frame === undefined ? { expr } : { expr, frame });
      return toVariable("result", r.value, r.type, r.description, path);
    },
    watch: async (entity: EntityId, component: string, field?: string) =>
      dataWatchOf(await call("debug.watch", field ? { entity, component, field } : { entity, component })),
    unwatch: (id: string) => call("debug.unwatch", { id }),
    exceptions: async (mode: ExceptionMode) => (await call("debug.exceptions", { mode })).mode,
    rewind: (tick: number) => call("debug.rewind", { tick }, 120000),
  },
  profile: {
    frame: () => call("profile.frame", {}),
  },
};

// ---- The debugger's as-built shapes (docs/spec/debugger.md 7) -------------------------------------

const IDENT = /^[A-Za-z_$][\w$]*$/;
/** What the host's JSON previews put in place of a value they do not expand. */
const ELIDED = /^\[(function\b.*|proxy|promise|getter|[A-Z]\w*(\(\d+\))?)\]$/;
const PREVIEW_ITEMS = 5;

function childPath(path: string | undefined, key: string, index: boolean): string | undefined {
  if (path === undefined || key === "\u2026") return undefined;
  if (index) return `${path}[${key}]`;
  return IDENT.test(key) ? `${path}.${key}` : `${path}[${JSON.stringify(key)}]`;
}

function short(v: unknown): string {
  if (v === null) return "null";
  if (Array.isArray(v)) return v.length ? "[\u2026]" : "[]";
  if (typeof v === "object") return "{\u2026}";
  if (typeof v === "string") return ELIDED.test(v) || v === "undefined" ? v : JSON.stringify(v);
  return String(v);
}

/**
 * A value of the host's JSON preview as a tree. `type` and `description` are known for top-level
 * values (variables, evaluations); nested ones are inferred from the JSON, where `undefined`,
 * NaN and the infinities arrive as text and objects past the preview's depth as `[Description]`.
 */
export function toVariable(name: string, value: unknown, type?: string, description?: string | null, path?: string): Variable {
  if (type === "undefined" || (type === undefined && value === "undefined")) return { name, value: "undefined", type: "undefined", path };
  if (value === null) return { name, value: type === "undefined" ? "undefined" : "null", type: "null", path };
  if (typeof value === "boolean") return { name, value: String(value), type: "boolean", path };
  if (typeof value === "number") return { name, value: String(value), type: "number", path };
  if (typeof value === "string") {
    if (type === "string") return { name, value: JSON.stringify(value), type: "string", path };
    if (type === "function" || value.startsWith("[function")) {
      return { name, value: `\u0192 ${value.replace(/^\[function ?|\]$/g, "")}()`, type: "function" };
    }
    if (type !== undefined) return { name, value, type, path };
    if (["NaN", "Infinity", "-Infinity", "-0"].includes(value)) return { name, value, type: "number", path };
    if (ELIDED.test(value)) return { name, value: value.slice(1, -1), type: "object" };
    return { name, value: JSON.stringify(value), type: "string", path };
  }
  if (Array.isArray(value)) {
    // The host describes top-level values only (`Float64Array(1)`); a nested array may be typed.
    const items = value.slice(0, PREVIEW_ITEMS).map(short).join(", ");
    const head = description ? `${description} ` : "";
    return {
      name,
      value: `${head}[${items}${value.length > PREVIEW_ITEMS ? ", \u2026" : ""}]`,
      type: type ?? "array",
      path,
      children: value.map((x, i) => toVariable(String(i), x, undefined, undefined, childPath(path, String(i), true))),
    };
  }
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    const items = entries.slice(0, PREVIEW_ITEMS).map(([k, v]) => `${k}: ${short(v)}`).join(", ");
    const head = description && description !== "Object" ? `${description} ` : "";
    return {
      name,
      value: `${head}{${items}${entries.length > PREVIEW_ITEMS ? ", \u2026" : ""}}`,
      type: type ?? "object",
      path,
      children: entries.map(([k, v]) => toVariable(k, v, undefined, undefined, childPath(path, k, false))),
    };
  }
  return { name, value: String(value), type: type ?? typeof value, path };
}

function scopeVars(vars: HostVariable[]): Variable[] {
  return vars.map((v) => toVariable(v.name, v.value, v.type, v.description, IDENT.test(v.name) ? v.name : undefined));
}

/** The host's debugger state (a `debug` event or `debug.state`) as the editor shows it. */
export function debugState(raw: HostDebugState): DebugState {
  const frames: StackFrame[] | undefined = raw.frames?.map((f, i) => ({
    id: i,
    name: f.function,
    file: f.location.file,
    line: f.location.line,
    column: f.location.column,
    generated: f.location.generated,
    returned: f.returned,
    scopes: f.returned
      ? []
      : [
          { name: "Local", variables: scopeVars(f.locals) },
          { name: "Closure", variables: scopeVars(f.closure) },
        ],
  }));
  return {
    state: raw.state === "paused" ? "paused" : "running",
    reason: raw.reason,
    location: raw.location ?? undefined,
    frames,
    tick: raw.tick,
    system: raw.system ?? undefined,
    hitBreakpoints: raw.hit_breakpoints,
    data: raw.data,
    exception: raw.exception,
    exceptions: raw.exceptions,
    attached: raw.attached,
    instrumented: raw.instrumented,
    cdp: raw.cdp,
  };
}

/** A breakpoint as the host lists it: where it binds (TypeScript), or what a CDP client asked. */
export function breakpointOf(b: HostBreakpoint): Breakpoint {
  const at = b.locations[0];
  return {
    id: b.id,
    file: at?.file ?? b.target.file ?? b.target.url ?? b.target.url_regex ?? b.target.script_id ?? "",
    line: at?.line ?? b.target.line ?? (b.target.js_line ?? 0) + 1,
    condition: b.condition ?? undefined,
    log: b.log ?? undefined,
    verified: b.locations.length > 0,
    owner: b.owner,
  };
}

export function dataWatchOf(w: HostDataWatch): DataWatch {
  return { id: w.id, entity: w.entity, component: w.component, field: w.field ?? undefined };
}
