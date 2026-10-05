// The debugger's controls (host-protocol.md section 6, the JSON `debug.*` form; the host's as-built
// shapes are docs/spec/debugger.md 7): breakpoints, stepping, pause on exceptions, watch expressions
// evaluated on each pause, editing a paused frame's values, data breakpoints, and the console's
// evaluation.

import { api } from "../host/api";
import { HostError } from "../host/client";
import type { DebugState, EntityId, ExceptionMode, Variable } from "../host/protocol";
import { openPanel } from "../layout/dock";
import { useDebug, type WatchExpr } from "../state/debug";
import { logLocal } from "../state/logs";
import { useScripts } from "../state/scripts";
import { entityName } from "../state/world";
import { attempt, reportError } from "./report";

export function isScriptFile(file: string | undefined): file is string {
  return !!file && /\.(ts|tsx|js)$/.test(file) && !file.includes(":");
}

/** On a new pause: show the line and the debugger, and the host's breakpoints as they are now. */
export function onPaused(state: DebugState) {
  const loc = state.location;
  openPanel("debug", true);
  if (isScriptFile(loc?.file) && !loc.generated) {
    useScripts.getState().openFile(loc.file, loc.line);
    openPanel("scripts", true);
  }
  // MCP agents and CDP clients set and clear breakpoints too; the host pushes no event for that.
  void refreshBreakpoints(true);
}

/** Set while Stop waits for the game thread (time.ts): every pause is continued, not shown. */
let continuing = false;
export function continueEveryPause(on: boolean) {
  continuing = on;
}

/** Continues a pause that comes while Stop waits; true when it did. */
export function continueIfStopping(state: DebugState): boolean {
  if (!continuing || state.state !== "paused") return false;
  void api.debug.resume().catch(() => undefined);
  return true;
}

/** The breakpoints as the host has them (every frontend's). `quiet`: a failure is not reported. */
export async function refreshBreakpoints(quiet = false) {
  try {
    useDebug.getState().setBreakpoints(await api.debug.listBreakpoints());
  } catch (e) {
    if (!quiet) reportError(e, "Breakpoints");
  }
}

/** The host no longer has the breakpoint: another frontend cleared it. */
const goneBreakpoint = (e: unknown) => e instanceof HostError && e.code === "debug.unknown_breakpoint";

/** Clears a breakpoint on the host and here; one the host no longer has is dropped here too. */
async function clearBreakpoint(id: string, context: string): Promise<boolean> {
  try {
    await api.debug.clearBreakpoint(id);
  } catch (e) {
    if (!goneBreakpoint(e)) {
      reportError(e, context);
      return false;
    }
    void refreshBreakpoints(true);
  }
  useDebug.getState().setBreakpoints(useDebug.getState().breakpoints.filter((b) => b.id !== id));
  return true;
}

export async function toggleBreakpoint(file: string, line: number) {
  const d = useDebug.getState();
  const existing = d.breakpoints.find((b) => b.file === file && b.line === line);
  if (existing) {
    await clearBreakpoint(existing.id, "Clear breakpoint");
    return;
  }
  const bp = await attempt(api.debug.setBreakpoint(file, line), "Set breakpoint");
  if (!bp) return;
  // The host moves a breakpoint on a line without code to the next line with code.
  const others = useDebug.getState().breakpoints.filter((b) => b.id !== bp.id);
  const same = others.find((b) => b.file === bp.file && b.line === bp.line);
  if (same) {
    // One already binds there: keep that one.
    await api.debug.clearBreakpoint(bp.id).catch(() => undefined);
    return;
  }
  d.setBreakpoints([...others, bp]);
}

/**
 * The host cannot change a breakpoint's condition in place: set the new one (a logpoint stays a
 * logpoint with its message), then clear the old.
 */
export async function setBreakpointCondition(id: string, condition: string) {
  const d = useDebug.getState();
  const bp = d.breakpoints.find((b) => b.id === id);
  if (!bp) return;
  const r = await attempt(api.debug.setBreakpoint(bp.file, bp.line, condition.trim() || undefined, bp.log), "Breakpoint condition");
  if (!r) return;
  try {
    await api.debug.clearBreakpoint(id);
  } catch (e) {
    if (!goneBreakpoint(e)) reportError(e, "Breakpoint condition");
  }
  useDebug.getState().setBreakpoints([...useDebug.getState().breakpoints.filter((b) => b.id !== id && b.id !== r.id), r]);
}

export async function removeBreakpoint(id: string) {
  await clearBreakpoint(id, "Clear breakpoint");
}

/** Clears the breakpoints set through `debug.*`; those of CDP clients (DevTools, VS Code) stay. */
export async function clearAllBreakpoints() {
  const r = await attempt(api.debug.clearBreakpoint(), "Clear breakpoints");
  if (r) await refreshBreakpoints();
}

export async function resume() {
  await attempt(api.debug.resume(), "Continue");
}

export async function pauseScripts() {
  await attempt(api.debug.pause(), "Pause scripts");
}

export async function step(kind: "over" | "into" | "out") {
  await attempt(api.debug.step(kind), `Step ${kind}`);
}

export async function setExceptionMode(mode: ExceptionMode) {
  const m = await attempt(api.debug.exceptions(mode), "Pause on exceptions");
  if (m) useDebug.getState().setExceptions(m);
}

let watchId = 1;

export function addWatch(expr: string) {
  const e = expr.trim();
  if (!e) return;
  const d = useDebug.getState();
  d.setWatches([...d.watches, { id: watchId++, expr: e }]);
  void refreshWatches();
}

export function removeWatch(id: number) {
  const d = useDebug.getState();
  d.setWatches(d.watches.filter((w) => w.id !== id));
}

/** Evaluates the watch expressions on the selected frame; the host evaluates only while paused. */
export async function refreshWatches() {
  const d = useDebug.getState();
  if (!d.watches.length) return;
  if (d.state.state !== "paused") {
    d.setWatches(d.watches.map((w) => ({ id: w.id, expr: w.expr })));
    return;
  }
  const frame = d.frame;
  const next: WatchExpr[] = await Promise.all(
    d.watches.map(async (w) => {
      try {
        return { id: w.id, expr: w.expr, value: await api.debug.eval(w.expr, frame) };
      } catch (e) {
        return { id: w.id, expr: w.expr, error: e instanceof Error ? e.message : String(e) };
      }
    }),
  );
  useDebug.getState().setWatches(next);
}

const IDENT = /^[A-Za-z_$][\w$]*$/;

/** The same value as shown (type and preview). */
const sameValue = (a: Variable, b: Variable) => a.type === b.type && a.value === b.value;

/**
 * Assigns `text` (a JavaScript expression) to a paused frame's variable or to a value inside one
 * (`l.distance[0]`), reads it back and shows the variable again. A variable is set with `debug.set`
 * (an assignment evaluated in the frame would not reach it: QuickJS evaluates on a copy of each
 * local no closure captured); a value inside an object is assigned with `debug.eval`, which writes
 * the object itself. Assigning to a query column's element writes the component, which the system's
 * call commits. Either taints the run from that tick (docs/spec/debugger.md 5).
 */
export async function setValue(frame: number, path: string, text: string): Promise<boolean> {
  const expr = text.trim();
  if (!expr) return false;
  let assigned: Variable;
  try {
    assigned = IDENT.test(path) ? await api.debug.set(path, expr, frame) : await api.debug.eval(`${path} = (${expr})`, frame, path);
  } catch (e) {
    reportError(e, `Set ${path}`);
    return false;
  }
  // What the frame holds now: a column of another type converts (an Int32Array keeps 1 of 1.5).
  const now = await api.debug.eval(path, frame, path).catch(() => undefined);
  const root = /^[A-Za-z_$][\w$]*/.exec(path)?.[0];
  if (root) {
    try {
      const v = await api.debug.eval(root, frame, root);
      useDebug.getState().patchVariable(frame, { ...v, name: root });
    } catch {
      // Showing it again is a nicety.
    }
  }
  void refreshWatches();
  if (now && !sameValue(now, assigned)) {
    reportError(new Error(`${path} reads ${now.value} after the assignment of ${assigned.value}.`), `Set ${path}`);
    return false;
  }
  logLocal("info", `Set ${path} = ${expr}${now ? ` (reads ${now.value})` : ""}`, { source: "debugger" });
  return true;
}

/** The console's input line: evaluated on the selected frame (the host evaluates only while paused). */
export async function evaluate(expr: string) {
  const d = useDebug.getState();
  logLocal("input", expr);
  try {
    const v = await api.debug.eval(expr, d.state.state === "paused" ? d.frame : undefined);
    logLocal("result", v.value, { source: v.type, value: v });
  } catch (e) {
    logLocal("error", e instanceof Error ? e.message : String(e), { source: "eval" });
  }
}

export async function addDataWatch(entity: EntityId, component: string, field?: string) {
  const w = await attempt(api.debug.watch(entity, component, field), "Data breakpoint");
  if (w) {
    useDebug.getState().setDataWatches([...useDebug.getState().dataWatches, w]);
    logLocal("info", `Data breakpoint: pause when a system writes ${entityName(entity)}.${component}${field ? `.${field}` : ""}`);
    openPanel("debug", false);
  }
}

export async function removeDataWatch(id: string) {
  const r = await attempt(api.debug.unwatch(id), "Remove data breakpoint");
  if (r !== undefined) useDebug.getState().setDataWatches(useDebug.getState().dataWatches.filter((w) => w.id !== id));
}
