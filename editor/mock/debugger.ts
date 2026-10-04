// The mock's debugger, answering in the real host's shapes (pocket-debug's agents' API,
// docs/spec/debugger.md 7): breakpoints on TypeScript lines (ids `bp<n>`, moved to the next line
// with code) with conditions and logpoints, pausing between and "inside" script systems, stepping,
// the frames the host shows (the `each` callback with its locals and closure, and `run` that called
// it; `let`/`const` locals below the line read undefined), evaluation on a paused frame (an
// assignment to a variable stays in the evaluation, as on the host; one through an object, such as a
// column's element, lasts for the pause), `debug.set` (const variables refuse; the value lasts for
// the pause), pause on exceptions (the mock's scripts throw none) and data breakpoints on
// component fields written by script systems, which stop on the statement after the write, or on a
// returned frame when the write was the system's last.

import type { World } from "./world";
import { MockError, isObject } from "./util";
import { systemRanges } from "./scripts";

const RULES = "scripts/rules.ts";

interface Breakpoint {
  id: string;
  file: string;
  /** The line asked for, and the line it binds to (the next with code). */
  requested: number;
  line: number;
  condition?: string;
  log?: string;
}

export interface DataWatch {
  id: string;
  entity: number;
  component: string;
  field?: string;
}

interface Location {
  file: string;
  line: number;
  column: number;
}

/** A frame of a pause: its values, the names declared `const`, and where `let`/`const` locals are declared. */
interface Frame {
  function: string;
  location: Location;
  locals: Record<string, unknown>;
  closure: Record<string, unknown>;
  consts: Set<string>;
  /** A local's declaration line: above it (or on it) the local is not initialized yet. */
  declared: Record<string, number>;
}

interface Paused {
  reason: "breakpoint" | "step" | "pause" | "data_breakpoint";
  system: string;
  location: Location;
  hit: string[];
  data?: Record<string, unknown>;
  /** The system already ran (a data breakpoint): resuming does not run it again. */
  ran: boolean;
  /** Seen as `run` returned: a position and no scopes. */
  returned: boolean;
  /** Innermost first; evaluations (and `debug.set`) see and change these for the pause. */
  frames: Frame[];
}

/** A variable as the host sends it. */
export interface HostVariable {
  name: string;
  type: string;
  value: unknown;
  description?: string;
}

export class Debugger {
  breakpoints: Breakpoint[] = [];
  watches: DataWatch[] = [];
  paused: Paused | null = null;
  exceptions: "none" | "uncaught" | "all" = "none";
  attached = false;
  private nextBp = 0;
  private nextWatch = 0;
  /** The system to run without checking once (resuming from a pause on it). */
  skipOnce: string | null = null;
  /** Pause at the next script system's first line, whatever the breakpoints. */
  stopAtNextSystem = false;
  pauseRequested = false;

  constructor(
    private readonly source: () => Map<string, string>,
    private readonly world: () => World,
    private readonly tick: () => number,
    private readonly log: (line: { level: "info"; source: string; message: string; file?: string; line?: number }) => void,
  ) {}

  setBreakpoint(params: Record<string, unknown>) {
    const file = String(params.file ?? "");
    const line = Number(params.line);
    const text = this.source().get(file);
    if (text === undefined) {
      throw new MockError("debug.unknown_file", `'${file}' is not a module of the running scripts.`, { file, allowed: [...this.source().keys()] });
    }
    if (!Number.isInteger(line) || line < 1) throw new MockError("request.invalid_value", "line is a whole number from 1.", { line: params.line });
    const lines = text.split("\n");
    // Move to the next line with code, as the host does.
    let actual = line;
    while (actual <= lines.length && /^\s*(\/\/.*|\/\*\*?.*|\*.*)?$/.test(lines[actual - 1]!)) actual++;
    if (actual > lines.length) throw new MockError("debug.no_code", `${file} has no code at or after line ${line}.`, { file, line });
    this.attached = true;
    const bp: Breakpoint = {
      id: `bp${++this.nextBp}`,
      file,
      requested: line,
      line: actual,
      condition: typeof params.condition === "string" && params.condition.trim() ? params.condition : undefined,
      log: typeof params.log === "string" ? params.log : undefined,
    };
    this.breakpoints.push(bp);
    return { id: bp.id, file, line: actual, verified: true, locations: [{ file, line: actual, column: 1 }] };
  }

  clearBreakpoints(params: Record<string, unknown>) {
    if (typeof params.id === "string") {
      if (!this.breakpoints.some((b) => b.id === params.id)) {
        throw new MockError("debug.unknown_breakpoint", `There is no breakpoint '${params.id}'.`, { id: params.id, allowed: this.breakpoints.map((b) => b.id) });
      }
      this.breakpoints = this.breakpoints.filter((b) => b.id !== params.id);
      return { cleared: 1 };
    }
    const n = this.breakpoints.length;
    this.breakpoints = [];
    return { cleared: n };
  }

  listBreakpoints() {
    return this.breakpoints.map((b) => ({
      id: b.id,
      owner: "agent",
      target: { file: b.file, line: b.requested },
      condition: b.condition ?? null,
      log: b.log ?? null,
      locations: [{ file: b.file, line: b.line, column: 1 }],
    }));
  }

  addWatch(params: Record<string, unknown>) {
    const entity = Number(params.entity);
    const e = this.world().entities.get(entity);
    if (!e) throw new MockError("sim.entity_not_found", `There is no entity ${params.entity}.`, { entity: params.entity });
    const component = String(params.component);
    if (!(component in e.components)) {
      throw new MockError("world.component_missing", `${e.name} has no ${component}.`, { entity, component });
    }
    this.attached = true;
    const w: DataWatch = { id: `w${++this.nextWatch}`, entity, component, field: typeof params.field === "string" ? params.field : undefined };
    this.watches.push(w);
    return { id: w.id, entity, component, field: w.field ?? null };
  }

  removeWatches(params: Record<string, unknown>) {
    const before = this.watches.length;
    this.watches = typeof params.id === "string" ? this.watches.filter((w) => w.id !== params.id) : [];
    const cleared = before - this.watches.length;
    if (cleared === 0 && typeof params.id === "string") throw new MockError("debug.unknown_watch", `There is no watch '${params.id}'.`, { id: params.id });
    return { cleared };
  }

  private ranges(file = RULES) {
    return systemRanges(this.source().get(file) ?? "");
  }

  private pause(reason: Paused["reason"], system: string, line: number, hit: string[] = [], data?: Record<string, unknown>, ran = false, returned = false) {
    const location = { file: RULES, line, column: 9 };
    this.paused = { reason, system, location, hit, data, ran, returned, frames: returned ? [] : this.frames(system, line) };
  }

  /** Called before a script system runs; true pauses before it (at a line of its body). */
  beforeSystem(system: string): boolean {
    if (this.skipOnce === system) {
      this.skipOnce = null;
      return false;
    }
    const range = this.ranges().find((r) => r.name === system);
    if (!range) return false;
    if (this.stopAtNextSystem || this.pauseRequested) {
      const reason = this.pauseRequested ? "pause" : "step";
      this.stopAtNextSystem = false;
      this.pauseRequested = false;
      this.pause(reason, system, range.body);
      return true;
    }
    for (const bp of this.breakpoints.filter((b) => b.file === RULES && b.line >= range.body && b.line <= range.end).sort((a, b) => a.line - b.line)) {
      const scope = scopeOf(this.frames(system, bp.line)[0]);
      if (bp.condition) {
        try {
          if (!evaluate(bp.condition, scope)) continue;
        } catch {
          continue;
        }
      }
      if (bp.log !== undefined) {
        let message: string;
        try {
          message = String(evaluate(`\`${bp.log.replace(/`/g, "\\`")}\``, scope));
        } catch (e) {
          message = `(logpoint failed: ${e instanceof Error ? e.message : String(e)})`;
        }
        this.log({ level: "info", source: "script", message, file: bp.file, line: bp.line });
        continue;
      }
      this.pause("breakpoint", system, bp.line, [bp.id]);
      return true;
    }
    return false;
  }

  /** Called after a system ran, with the values watched before it; only script systems' writes stop. */
  afterSystem(system: string, before: Map<string, unknown>): boolean {
    const range = this.ranges().find((r) => r.name === system);
    if (!range) return false;
    for (const w of this.watches) {
      const now = this.watchedValue(w);
      const was = before.get(w.id);
      if (JSON.stringify(now) === JSON.stringify(was)) continue;
      const lines = (this.source().get(RULES) ?? "").split("\n");
      let line = range.body;
      if (w.field) {
        const idx = lines.findIndex((l, i) => i + 1 >= range.body && i + 1 <= range.end && l.includes(`${w.field}`) && l.includes("="));
        if (idx >= 0) line = idx + 1;
      }
      // The host notices a staged write at the next statement it traces: the one after the write in
      // the same call, or, when the write was the call's last, the system's return.
      const next = nextCode(lines, line, range.end);
      const at = { file: RULES, line, column: 13 };
      this.pause(
        "data_breakpoint",
        system,
        next ?? line,
        [],
        {
          watch: w.id,
          entity: w.entity,
          component: w.component,
          field: w.field ?? null,
          names: [w.field ?? w.component],
          before: was ?? null,
          after: now ?? null,
          written_at: at,
          after_return: next === undefined,
        },
        true,
        next === undefined,
      );
      return true;
    }
    return false;
  }

  snapshotWatches(): Map<string, unknown> {
    return new Map(this.watches.map((w) => [w.id, this.watchedValue(w)]));
  }

  private watchedValue(w: DataWatch): unknown {
    const c = this.world().entities.get(w.entity)?.components[w.component];
    if (c === undefined) return undefined;
    return w.field && isObject(c) ? c[w.field] : c;
  }

  step(kind: string) {
    if (!this.paused) throw new MockError("debug.not_paused", "The game is running; pause it (debug.pause) or wait for a breakpoint (debug.wait) first.");
    if (!["over", "into", "out"].includes(kind)) {
      throw new MockError("request.invalid_value", "debug.step takes kind 'over', 'into' or 'out'.", { kind });
    }
    const p = this.paused;
    const range = this.ranges().find((r) => r.name === p.system);
    const lines = (this.source().get(RULES) ?? "").split("\n");
    if (kind !== "out" && range && !p.returned) {
      const next = nextCode(lines, p.location.line, range.end);
      if (next !== undefined) {
        const location = { ...p.location, line: next };
        this.paused = { ...p, reason: "step", hit: [], data: undefined, location, frames: this.moved(p.frames, location) };
        return "stay" as const;
      }
    }
    // Leave the system: run it (unless it ran), then stop at the next one.
    this.skipOnce = p.ran ? null : p.system;
    this.stopAtNextSystem = true;
    this.paused = null;
    return "resume" as const;
  }

  resume() {
    if (this.paused) this.skipOnce = this.paused.ran ? null : this.paused.system;
    this.paused = null;
  }

  location(): Location | undefined {
    return this.paused?.location;
  }

  /**
   * The frames a system's code is in at `line`, as the host shows them: inside the `each` callback,
   * the callback (its arguments and locals; the run's columns and `ctx` as its closure) and `run`
   * that called it; in `run` itself, `run` alone. Values are plain JavaScript; columns are typed
   * arrays shared by both frames, so a write through one shows in the other.
   */
  frames(system: string, line: number): Frame[] {
    const w = this.world();
    const lines = (this.source().get(RULES) ?? "").split("\n");
    const range = this.ranges().find((r) => r.name === system);
    const eachAt = range ? lines.findIndex((l, i) => i + 1 >= range.body && i + 1 <= range.end && /\.each\(/.test(l)) + 1 : 0;
    const declared = (names: string[]) =>
      Object.fromEntries(
        names.flatMap((n) => {
          const i = lines.findIndex((l, j) => j + 1 >= (range?.body ?? 0) && j + 1 <= (range?.end ?? 0) && new RegExp(`\\b(const|let)\\s+${n}\\b`).test(l));
          return i >= 0 ? [[n, i + 1]] : [];
        }),
      );
    const boats = [...w.entities.values()].filter((e) => e.components.Boat);
    const crates = [...w.entities.values()].filter((e) => e.components.Cargo);
    const boat = boats[0];
    const ctx = { tick: this.tick(), dt: 1 / 60, time: this.tick() / 60, system };
    const module = { REACH: 3, REACH_UP: 3 };
    const runAt = { file: RULES, line: eachAt || (range?.body ?? line), column: 9 };
    const here = { file: RULES, line, column: 9 };
    if (system === "muster" || !boat) {
      const locals = { ctx, boats: { len: boats.length }, crates: { len: crates.length } };
      return [{ function: "run", location: here, locals, closure: {}, consts: new Set(), declared: {} }];
    }
    const b = boat.components.Boat as Record<string, number>;
    const l = boat.components.Log as Record<string, number> | undefined;
    const t = boat.components.Transform as { position: number[] };
    let run: Frame;
    let callback: Frame;
    if (system === "log") {
      const cols = {
        b: { speed: new Float64Array([b.speed ?? 0]), hoist_now: new Float64Array([b.hoist_now ?? 0]) },
        l: l ? { distance: new Float64Array([l.distance ?? 0]), top_speed: new Float64Array([l.top_speed ?? 0]), sail_set: new Uint8Array([l.sail_set ? 1 : 0]) } : undefined,
      };
      run = { function: "run", location: runAt, locals: { ctx, boats: { len: boats.length }, ...cols }, closure: {}, consts: new Set(["b", "l"]), declared: declared(["b", "l"]) };
      callback = {
        function: "(anonymous)",
        location: here,
        locals: { r: 0, e: boat.id, speed: Math.abs(b.speed ?? 0), set: (b.hoist_now ?? 0) >= 0.5 },
        closure: { ...cols, ctx },
        consts: new Set(["speed", "set", "b", "l"]),
        declared: declared(["speed", "set"]),
      };
    } else {
      const crew = boat.components.Crew as { take: number | null } | undefined;
      const tally = boat.components.Tally as Record<string, number> | undefined;
      const cols = {
        crew: { take: new Float64Array([crew?.take ?? 0]) },
        at: { x: new Float64Array([t.position[0]!]), y: new Float64Array([t.position[1]!]), z: new Float64Array([t.position[2]!]) },
        tally: tally ? { taken: new Float64Array([tally.taken ?? 0]), worth: new Float64Array([tally.worth ?? 0]), total: new Float64Array([tally.total ?? 0]) } : undefined,
      };
      const named = ["crew", "at", "tally"];
      run = { function: "run", location: runAt, locals: { ctx, boats: { len: boats.length }, ...cols }, closure: { ...module }, consts: new Set([...named, "REACH", "REACH_UP"]), declared: declared(named) };
      callback = {
        function: "(anonymous)",
        location: here,
        locals: { r: 0, boat: boat.id, target: crew?.take ?? 0 },
        closure: { ...cols, ctx, ...module },
        consts: new Set(["target", ...named, "REACH", "REACH_UP"]),
        declared: declared(["target"]),
      };
    }
    return eachAt && line > eachAt ? [callback, run] : [{ ...run, location: here }];
  }

  /** The frames after a step to `location`: the same values, the innermost frame moved. */
  private moved(frames: Frame[], location: Location): Frame[] {
    return frames.map((f, i) => (i === 0 ? { ...f, location } : f));
  }

  /** `debug.state` (and, without the last group, the `debug` event). */
  state(full = true) {
    const extra = full
      ? {
          attached: this.attached,
          instrumented: this.attached,
          exceptions: this.exceptions,
          breakpoints: this.listBreakpoints(),
          watches: this.watches.map((w) => ({ id: w.id, entity: w.entity, component: w.component, field: w.field ?? null })),
          waiting_for_debugger: false,
          cdp: null,
        }
      : {};
    const p = this.paused;
    if (!p) return { state: "running", tick: this.tick(), system: null, ...extra };
    const frames = p.returned
      ? [{ frame: 0, function: "(returned)", location: p.location, locals: [], closure: [], returned: true }]
      : p.frames.map((f, i) => ({
          frame: i,
          function: f.function,
          location: f.location,
          locals: Object.keys(f.locals).map((k) => hostVar(k, visible(f, k))),
          closure: Object.entries(f.closure).map(([k, v]) => hostVar(k, v)),
          returned: false,
        }));
    return {
      state: "paused",
      reason: p.reason,
      tick: this.tick(),
      system: p.system,
      location: p.location,
      frames,
      hit_breakpoints: p.hit,
      ...(p.data ? { data: p.data } : {}),
      ...extra,
    };
  }

  /** The paused frame numbered `frame`, or the host's problem (`failed`: the code for a returned frame). */
  private frameAt(frame: number | undefined, failed: string): Frame {
    const p = this.paused;
    if (!p) throw new MockError("debug.not_paused", "The game is running; pause it (debug.pause) or wait for a breakpoint (debug.wait) first.");
    const n = p.returned ? 1 : p.frames.length;
    const i = frame ?? 0;
    if (i >= n) throw new MockError("debug.no_frame", `There is no frame ${i}; the game stopped with ${n} frames.`, { frame: i, frames: n });
    if (p.returned) throw new MockError(failed, "The system has returned; its variables are gone.");
    return p.frames[i]!;
  }

  eval(expr: string, frame: number | undefined) {
    const f = this.frameAt(frame, "debug.eval_failed");
    try {
      const v = evaluate(expr, scopeOf(f));
      const h = hostVar("result", v);
      return { type: h.type, value: h.value, description: h.description ?? describe(v) };
    } catch (e) {
      const text = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
      throw new MockError("debug.eval_failed", `The expression threw: ${text}`, { error: text });
    }
  }

  /** `debug.set`: the frame's variable takes the expression's value, for the pause. */
  set(name: string, value: string, frame: number | undefined) {
    const f = this.frameAt(frame, "debug.set_failed");
    const own = name in f.locals ? f.locals : name in f.closure ? f.closure : undefined;
    if (!own) throw new MockError("debug.set_failed", `No variable ${name} in this frame.`, { name });
    if (f.consts.has(name)) throw new MockError("debug.set_failed", `${name} is a constant.`, { name });
    let v: unknown;
    try {
      v = evaluate(value, scopeOf(f));
    } catch (e) {
      const text = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
      throw new MockError("debug.set_failed", `The expression threw: ${text}`, { name });
    }
    own[name] = v;
    delete f.declared[name];
    const h = hostVar("result", v);
    return { type: h.type, value: h.value, description: h.description ?? describe(v) };
  }
}

/** A local's value as the frame sees it: undefined above its declaration. */
function visible(f: Frame, name: string): unknown {
  const at = f.declared[name];
  return at !== undefined && f.location.line <= at ? undefined : f.locals[name];
}

/** What code in the frame sees: its locals over its closure. */
function scopeOf(f: Frame | undefined): Record<string, unknown> {
  if (!f) return {};
  const locals = Object.fromEntries(Object.keys(f.locals).map((k) => [k, visible(f, k)]));
  return { ...f.closure, ...locals };
}

/** The next line with code after `line`, before the system's end; none when the call ends first. */
function nextCode(lines: string[], line: number, end: number): number | undefined {
  for (let n = line + 1; n < end; n++) {
    const src = lines[n - 1]!;
    if (/^\s*\}\);?\s*$/.test(src)) return undefined; // the callback (or run) ends
    if (/^\s*(\/\/.*|\}\)?;?|\},?)?\s*$/.test(src)) continue;
    return n;
  }
  return undefined;
}

/**
 * Evaluates an expression over named values. The values are the function's arguments, so an
 * assignment to a name stays in the evaluation (as on the host); a write through an object lasts.
 */
function evaluate(expr: string, scope: Record<string, unknown>): unknown {
  const names = Object.keys(scope).filter((n) => /^[A-Za-z_$][\w$]*$/.test(n));
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const fn = new Function(...names, `"use strict"; return (${expr});`);
  return fn(...names.map((n) => scope[n]));
}

const TYPED = [Float64Array, Float32Array, Int32Array, Uint32Array, Uint8Array, Int8Array, Uint16Array, Int16Array];

function kindOf(v: unknown): string {
  if (v === undefined) return "undefined";
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  return typeof v === "object" ? "object" : typeof v;
}

function describe(v: unknown): string | undefined {
  if (Array.isArray(v)) return `Array(${v.length})`;
  const typed = TYPED.find((t) => v instanceof t);
  if (typed) return `${typed.name}(${(v as Float64Array).length})`;
  if (typeof v === "function") return `function ${v.name}() { [code] }`;
  if (isObject(v)) return "Object";
  return undefined;
}

/** The host's JSON preview: objects to three levels, `undefined` as null at the top. */
function preview(v: unknown, depth: number): unknown {
  if (v === undefined) return depth === 0 ? null : "undefined";
  if (typeof v === "number") return Number.isFinite(v) ? v : String(v);
  if (typeof v === "function") return `[function ${v.name}]`;
  if (v === null || typeof v !== "object") return v;
  if (depth >= 3) return `[${describe(v)}]`;
  if (Array.isArray(v) || TYPED.some((t) => v instanceof t)) return Array.from(v as ArrayLike<unknown>, (x) => preview(x, depth + 1));
  return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, preview(x, depth + 1)]));
}

export function hostVar(name: string, v: unknown): HostVariable {
  const d = typeof v === "object" || typeof v === "function" ? describe(v) : undefined;
  return { name, type: kindOf(v), value: preview(v, 0), ...(d && v !== null ? { description: d } : {}) };
}
