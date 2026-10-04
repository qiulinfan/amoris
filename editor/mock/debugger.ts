// The mock's debugger, answering in the real host's shapes (pocket-debug's agents' API,
// docs/spec/debugger.md 7): breakpoints on TypeScript lines (ids `bp<n>`, moved to the next line
// with code) with conditions and logpoints, pausing between and "inside" script systems, stepping,
// one frame per pause with `locals` and `closure` as JSON previews, evaluation on a paused frame
// (assignments last for the pause; they do not write the world), pause on exceptions (the mock's
// scripts throw none) and data breakpoints on component fields written by script systems.

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

interface Paused {
  reason: "breakpoint" | "step" | "pause" | "data_breakpoint";
  system: string;
  location: Location;
  hit: string[];
  data?: Record<string, unknown>;
  /** The frame's values for the pause: evaluations (and assignments) see and change these. */
  scope: Record<string, unknown>;
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

  private pause(reason: Paused["reason"], system: string, line: number, hit: string[] = [], data?: Record<string, unknown>) {
    this.paused = { reason, system, location: { file: RULES, line, column: 9 }, hit, data, scope: this.flatScope(system) };
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
    const scope = this.flatScope(system);
    for (const bp of this.breakpoints.filter((b) => b.file === RULES && b.line >= range.body && b.line <= range.end).sort((a, b) => a.line - b.line)) {
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
      const text = this.source().get(RULES) ?? "";
      let line = range.body;
      if (w.field) {
        const idx = text.split("\n").findIndex((l, i) => i + 1 >= range.body && i + 1 <= range.end && l.includes(`${w.field}`) && l.includes("="));
        if (idx >= 0) line = idx + 1;
      }
      const at = { file: RULES, line, column: 13 };
      this.pause("data_breakpoint", system, line, [], {
        watch: w.id,
        entity: w.entity,
        component: w.component,
        field: w.field ?? null,
        names: [w.field ?? w.component],
        before: was ?? null,
        after: now ?? null,
        written_at: at,
        after_return: true,
      });
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
    const range = this.ranges().find((r) => r.name === this.paused!.system);
    const lines = (this.source().get(RULES) ?? "").split("\n");
    if (kind !== "out" && range && this.paused.reason !== "data_breakpoint") {
      let next = this.paused.location.line + 1;
      while (next <= range.end && /^\s*(\/\/.*|\}\)?;?|\},?)?\s*$/.test(lines[next - 1]!)) next++;
      if (next < range.end) {
        this.paused = { ...this.paused, reason: "step", hit: [], data: undefined, location: { ...this.paused.location, line: next } };
        return "stay" as const;
      }
    }
    // Leave the system: run it, then stop at the next one.
    this.skipOnce = this.paused.system;
    this.stopAtNextSystem = true;
    this.paused = null;
    return "resume" as const;
  }

  resume() {
    if (this.paused) this.skipOnce = this.paused.reason === "data_breakpoint" ? null : this.paused.system;
    this.paused = null;
  }

  location(): Location | undefined {
    return this.paused?.location;
  }

  /** The values a system's code sees at its pause, as plain JavaScript. */
  flatScope(system: string): Record<string, unknown> {
    const w = this.world();
    const boats = [...w.entities.values()].filter((e) => e.components.Boat);
    const crates = [...w.entities.values()].filter((e) => e.components.Cargo);
    const boat = boats[0];
    const scope: Record<string, unknown> = {
      ctx: { tick: this.tick(), dt: 1 / 60, time: this.tick() / 60, system },
      REACH: 3,
      REACH_UP: 3,
    };
    if (system === "muster") {
      Object.assign(scope, { boats: { len: boats.length }, crates: { len: crates.length } });
    }
    if (boat) {
      const b = boat.components.Boat as Record<string, number>;
      const l = boat.components.Log as Record<string, number> | undefined;
      const t = boat.components.Transform as { position: number[] };
      if (system === "log") {
        Object.assign(scope, {
          r: 0,
          e: boat.id,
          speed: Math.abs(b.speed ?? 0),
          set: (b.hoist_now ?? 0) >= 0.5,
          b: { speed: new Float64Array([b.speed ?? 0]), hoist_now: new Float64Array([b.hoist_now ?? 0]) },
          l: l ? { distance: new Float64Array([l.distance ?? 0]), top_speed: new Float64Array([l.top_speed ?? 0]), sail_set: new Uint8Array([l.sail_set ? 1 : 0]) } : undefined,
        });
      }
      if (system === "take_aboard") {
        const crew = boat.components.Crew as { take: number | null } | undefined;
        const tally = boat.components.Tally as Record<string, number> | undefined;
        Object.assign(scope, {
          r: 0,
          boat: boat.id,
          target: crew?.take ?? 0,
          at: { x: new Float64Array([t.position[0]!]), y: new Float64Array([t.position[1]!]), z: new Float64Array([t.position[2]!]) },
          tally: tally ? { taken: new Float64Array([tally.taken ?? 0]), worth: new Float64Array([tally.worth ?? 0]), total: new Float64Array([tally.total ?? 0]) } : undefined,
          crew: { take: new Float64Array([crew?.take ?? 0]) },
        });
      }
    }
    return scope;
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
    const locals: HostVariable[] = [];
    const closure: HostVariable[] = [];
    for (const [k, v] of Object.entries(p.scope)) {
      if (k === "REACH" || k === "REACH_UP") closure.push(hostVar(k, v));
      else if (k !== "ctx") locals.push(hostVar(k, v));
    }
    closure.push(hostVar("ctx", p.scope.ctx));
    const returned = p.reason === "data_breakpoint";
    const frames = [
      {
        frame: 0,
        function: returned ? "run" : "(anonymous)",
        location: p.location,
        locals: returned ? [] : locals,
        closure: returned ? [] : closure,
        returned,
      },
    ];
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

  eval(expr: string, frame: number | undefined) {
    const p = this.paused;
    if (!p) throw new MockError("debug.not_paused", "The game is running; pause it (debug.pause) or wait for a breakpoint (debug.wait) first.");
    if ((frame ?? 0) !== 0) throw new MockError("debug.no_frame", `There is no frame ${frame}; the game stopped with 1 frames.`, { frame, frames: 1 });
    try {
      const v = evaluate(expr, p.scope, true);
      const h = hostVar("result", v);
      return { type: h.type, value: h.value, description: h.description ?? describe(v) };
    } catch (e) {
      const text = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
      throw new MockError("debug.eval_failed", `The expression threw: ${text}`, { error: text });
    }
  }
}

/**
 * Evaluates an expression over named values. With `assign`, a plain assignment to a name changes
 * the scope (the mock is a loopback-only development tool).
 */
function evaluate(expr: string, scope: Record<string, unknown>, assign = false): unknown {
  const names = Object.keys(scope).filter((n) => /^[A-Za-z_$][\w$]*$/.test(n));
  const m = assign ? /^\s*([A-Za-z_$][\w$]*)\s*=(?!=)([\s\S]*)$/.exec(expr) : null;
  if (m && names.includes(m[1]!)) {
    const v = evaluate(m[2]!, scope);
    scope[m[1]!] = v;
    return v;
  }
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
