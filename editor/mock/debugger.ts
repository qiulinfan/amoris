// The mock's debugger (host-protocol.md section 6, JSON form): breakpoints on script lines with
// conditions, pausing between and "inside" script systems, stepping, frames with scopes built from
// the world, evaluation on a frame and data breakpoints on component fields.

import type { World } from "./world";
import { MockError, isObject } from "./util";
import { systemRanges } from "./scripts";

export interface Breakpoint {
  id: number;
  file: string;
  line: number;
  condition?: string;
  verified: boolean;
  hits: number;
}

export interface DataWatch {
  id: number;
  entity: number;
  component: string;
  field?: string;
}

export interface Variable {
  name: string;
  value: string;
  type: string;
  children?: Variable[];
}

interface Paused {
  reason: "breakpoint" | "step" | "pause" | "data" | "exception";
  system: string;
  file: string;
  line: number;
  detail?: string;
}

export interface Location {
  file: string;
  line: number;
  column: number;
}

export class Debugger {
  breakpoints: Breakpoint[] = [];
  watches: DataWatch[] = [];
  paused: Paused | null = null;
  private nextId = 1;
  /** The system to run without checking once (resuming from a pause on it). */
  skipOnce: string | null = null;
  /** Pause at the next script system's first line, whatever the breakpoints. */
  stopAtNextSystem = false;
  pauseRequested = false;

  constructor(
    private readonly source: () => Map<string, string>,
    private readonly world: () => World,
    private readonly tick: () => number,
  ) {}

  setBreakpoint(params: Record<string, unknown>): Breakpoint {
    const file = String(params.file ?? "");
    const line = Number(params.line);
    const text = this.source().get(file);
    if (text === undefined) throw new MockError("debug.unknown_file", `There is no script '${file}'.`, { file });
    if (!Number.isInteger(line) || line < 1) throw new MockError("request.invalid_value", "line is a whole number from 1.", { line: params.line });
    const lines = text.split("\n");
    // Move to the next line with code, as a real debugger does.
    let actual = Math.min(line, lines.length);
    while (actual < lines.length && /^\s*(\/\/.*)?$/.test(lines[actual - 1]!)) actual++;
    const existing = this.breakpoints.find((b) => b.file === file && b.line === actual);
    if (existing) {
      existing.condition = typeof params.condition === "string" && params.condition ? params.condition : undefined;
      return existing;
    }
    const inSystem = systemRanges(text).some((r) => actual >= r.body && actual <= r.end);
    const bp: Breakpoint = {
      id: this.nextId++,
      file,
      line: actual,
      condition: typeof params.condition === "string" && params.condition ? params.condition : undefined,
      verified: inSystem,
      hits: 0,
    };
    this.breakpoints.push(bp);
    return bp;
  }

  clearBreakpoints(params: Record<string, unknown>): number {
    const before = this.breakpoints.length;
    if (typeof params.id === "number") this.breakpoints = this.breakpoints.filter((b) => b.id !== params.id);
    else if (typeof params.file === "string" && typeof params.line === "number") {
      this.breakpoints = this.breakpoints.filter((b) => !(b.file === params.file && b.line === params.line));
    } else if (typeof params.file === "string") this.breakpoints = this.breakpoints.filter((b) => b.file !== params.file);
    else this.breakpoints = [];
    return before - this.breakpoints.length;
  }

  addWatch(params: Record<string, unknown>): DataWatch {
    const entity = Number(params.entity);
    const e = this.world().entities.get(entity);
    if (!e) throw new MockError("sim.entity_not_found", `There is no entity ${params.entity}.`, { entity: params.entity });
    const component = String(params.component);
    if (!(component in e.components)) {
      throw new MockError("world.component_missing", `${e.name} has no ${component}.`, { entity, component });
    }
    const w: DataWatch = { id: this.nextId++, entity, component, field: typeof params.field === "string" ? params.field : undefined };
    this.watches.push(w);
    return w;
  }

  private ranges(file = "scripts/rules.ts") {
    return systemRanges(this.source().get(file) ?? "");
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
      this.paused = { reason, system, file: "scripts/rules.ts", line: range.body };
      return true;
    }
    const scope = this.flatScope(system);
    for (const bp of this.breakpoints
      .filter((b) => b.file === "scripts/rules.ts" && b.line >= range.body && b.line <= range.end)
      .sort((a, b) => a.line - b.line)) {
      if (bp.condition) {
        try {
          if (!evaluate(bp.condition, scope)) continue;
        } catch {
          continue;
        }
      }
      bp.hits++;
      this.paused = { reason: "breakpoint", system, file: bp.file, line: bp.line };
      return true;
    }
    return false;
  }

  /** Called after a system (script or engine) ran with the values watched before it. */
  afterSystem(system: string, before: Map<number, unknown>): boolean {
    for (const w of this.watches) {
      const now = this.watchedValue(w);
      if (JSON.stringify(now) !== JSON.stringify(before.get(w.id))) {
        const e = this.world().entities.get(w.entity);
        const range = this.ranges().find((r) => r.name === system);
        const text = this.source().get("scripts/rules.ts") ?? "";
        let line = range?.body ?? 1;
        if (range && w.field) {
          const idx = text.split("\n").findIndex((l, i) => i + 1 >= range.body && i + 1 <= range.end && l.includes(`${w.field}`) && l.includes("="));
          if (idx >= 0) line = idx + 1;
        }
        this.paused = {
          reason: "data",
          system,
          file: range ? "scripts/rules.ts" : `engine:${system}`,
          line: range ? line : 0,
          detail: `${e?.name ?? `#${w.entity}`}.${w.component}${w.field ? `.${w.field}` : ""} written by ${system}`,
        };
        return true;
      }
    }
    return false;
  }

  snapshotWatches(): Map<number, unknown> {
    return new Map(this.watches.map((w) => [w.id, this.watchedValue(w)]));
  }

  private watchedValue(w: DataWatch): unknown {
    const c = this.world().entities.get(w.entity)?.components[w.component];
    if (c === undefined) return undefined;
    return w.field && isObject(c) ? c[w.field] : c;
  }

  step(kind: string) {
    if (!this.paused) throw new MockError("debug.not_paused", "The game is running; pause it first.");
    if (!["over", "into", "out"].includes(kind)) {
      throw new MockError("request.invalid_value", "debug.step takes kind 'over', 'into' or 'out'.", { kind });
    }
    const range = this.ranges().find((r) => r.name === this.paused!.system);
    const lines = (this.source().get("scripts/rules.ts") ?? "").split("\n");
    if (kind !== "out" && range) {
      let next = this.paused.line + 1;
      while (next <= range.end && /^\s*(\/\/.*|\}\)?;?|\},?)?\s*$/.test(lines[next - 1]!)) next++;
      if (next < range.end) {
        this.paused = { ...this.paused, reason: "step", line: next };
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
    if (this.paused) this.skipOnce = this.paused.reason === "data" ? null : this.paused.system;
    this.paused = null;
  }

  location(): Location | undefined {
    return this.paused ? { file: this.paused.file, line: this.paused.line, column: 1 } : undefined;
  }

  /** The values a system's code sees at its pause, as plain JavaScript. */
  flatScope(system: string): Record<string, unknown> {
    const w = this.world();
    const boats = [...w.entities.values()].filter((e) => e.components.Boat);
    const crates = [...w.entities.values()].filter((e) => e.components.Cargo);
    const boat = boats[0];
    const scope: Record<string, unknown> = {
      ctx: { tick: this.tick(), dt: 1 / 60, time: this.tick() / 60, system: `script:${system}` },
      REACH: 3,
      REACH_UP: 3,
    };
    if (system === "muster") {
      Object.assign(scope, { boats: { len: boats.length, ids: boats.map((b) => b.id) }, crates: { len: crates.length, ids: crates.map((c) => c.id) } });
    }
    if (boat) {
      const b = boat.components.Boat as Record<string, number>;
      const l = boat.components.Log as Record<string, number> | undefined;
      const t = boat.components.Transform as { position: number[] };
      if (system === "log") {
        Object.assign(scope, {
          boats: { len: 1, ids: [boat.id] },
          r: 0,
          e: boat.id,
          speed: Math.abs(b.speed ?? 0),
          set: (b.hoist_now ?? 0) >= 0.5,
          b: { speed: [b.speed], hoist_now: [b.hoist_now] },
          l: l ? { distance: [l.distance], top_speed: [l.top_speed], sail_set: [l.sail_set ? 1 : 0] } : undefined,
        });
      }
      if (system === "take_aboard") {
        const crew = boat.components.Crew as { take: number | null } | undefined;
        const tally = boat.components.Tally as Record<string, number> | undefined;
        Object.assign(scope, {
          boats: { len: 1, ids: [boat.id] },
          r: 0,
          boat: boat.id,
          target: crew?.take ?? 0,
          at: { x: [t.position[0]], y: [t.position[1]], z: [t.position[2]] },
          tally: tally ? { taken: [tally.taken], worth: [tally.worth], total: [tally.total] } : undefined,
          crew: { take: [crew?.take ?? 0] },
        });
      }
    }
    return scope;
  }

  state(system: string | null) {
    const loc = this.location();
    if (!this.paused || !loc) return { state: "running", tick: this.tick() };
    const sys = this.paused.system;
    const scope = this.flatScope(sys);
    const locals: Variable[] = [];
    const closure: Variable[] = [];
    for (const [k, v] of Object.entries(scope)) {
      if (k === "REACH" || k === "REACH_UP") closure.push(toVar(k, v));
      else if (v !== undefined) locals.push(toVar(k, v));
    }
    const w = this.world();
    const boat = [...w.entities.values()].find((e) => e.components.Boat);
    const entityVars = boat
      ? Object.entries(boat.components).filter(([c]) => ["Boat", "Log", "Tally", "Crew", "Transform"].includes(c)).map(([c, v]) => toVar(c, v))
      : [];
    const scopes = [
      { name: "Local", variables: locals },
      { name: "Closure (rules.ts)", variables: closure },
      ...(boat ? [{ name: `Entity ${boat.name} #${boat.id}`, variables: entityVars }] : []),
    ];
    const frames = [
      { id: 0, name: loc.file.startsWith("engine:") ? sys : `run (${sys})`, file: loc.file, line: loc.line, column: 1, system: `script:${sys}`, scopes },
      { id: 1, name: `system ${sys}`, file: "pocket:runtime/schedule", line: 0, column: 0, system: `script:${sys}`, scopes: [] },
      { id: 2, name: `tick ${this.tick()} · update`, file: "pocket:runtime/tick", line: 0, column: 0, scopes: [] },
    ];
    return {
      state: "paused",
      reason: this.paused.reason,
      detail: this.paused.detail,
      location: loc,
      frames,
      tick: this.tick(),
      system: system ?? `script:${sys}`,
    };
  }

  eval(expr: string, frame: number | undefined, globals: Record<string, unknown>) {
    const scope = this.paused && (frame ?? 0) === 0 ? { ...globals, ...this.flatScope(this.paused.system) } : globals;
    try {
      const v = evaluate(expr, scope);
      return toVar("result", v);
    } catch (e) {
      throw new MockError("debug.eval_failed", e instanceof Error ? e.message : String(e), { expr });
    }
  }
}

/** Evaluates an expression over named values. The mock is a loopback-only development tool. */
function evaluate(expr: string, scope: Record<string, unknown>): unknown {
  const names = Object.keys(scope).filter((n) => /^[A-Za-z_$][\w$]*$/.test(n));
  // eslint-disable-next-line @typescript-eslint/no-implied-eval
  const fn = new Function(...names, `"use strict"; return (${expr});`);
  return fn(...names.map((n) => scope[n]));
}

function preview(v: unknown[]): string {
  const parts = v.slice(0, 5).map((x) => (typeof x === "string" ? JSON.stringify(x) : typeof x === "object" && x !== null ? (Array.isArray(x) ? "[…]" : "{…}") : String(x)));
  return `[${parts.join(", ")}${v.length > 5 ? ", …" : ""}]`;
}

export function toVar(name: string, v: unknown, depth = 0): Variable {
  if (v === null) return { name, value: "null", type: "null" };
  if (v === undefined) return { name, value: "undefined", type: "undefined" };
  if (typeof v === "number") return { name, value: Number.isInteger(v) ? String(v) : String(Math.round(v * 1e6) / 1e6), type: "number" };
  if (typeof v === "string") return { name, value: JSON.stringify(v), type: "string" };
  if (typeof v === "boolean") return { name, value: String(v), type: "boolean" };
  if (typeof v === "function") return { name, value: "ƒ ()", type: "function" };
  if (Array.isArray(v)) {
    const isNum = v.every((x) => typeof x === "number");
    return {
      name,
      value: isNum ? `Float64Array(${v.length}) [${v.map((x) => (Number.isInteger(x) ? x : (x as number).toFixed(3))).join(", ")}]` : `Array(${v.length}) ${preview(v)}`,
      type: isNum ? "Float64Array" : "Array",
      children: depth < 4 ? v.map((x, i) => toVar(String(i), x, depth + 1)) : undefined,
    };
  }
  if (isObject(v)) {
    const keys = Object.keys(v);
    return {
      name,
      value: `{${keys.slice(0, 4).join(", ")}${keys.length > 4 ? ", …" : ""}}`,
      type: "Object",
      children: depth < 4 ? keys.map((k) => toVar(k, v[k], depth + 1)) : undefined,
    };
  }
  return { name, value: String(v), type: typeof v };
}
