// The debugger's controls (host-protocol.md section 6, the JSON `debug.*` form; the host's as-built
// shapes are docs/spec/debugger.md 7): breakpoints, stepping, pause on exceptions, watch expressions
// evaluated on each pause, editing a paused frame's values, data breakpoints, and the console's
// evaluation.

import { api } from "../host/api";
import type { DebugState, EntityId, ExceptionMode } from "../host/protocol";
import { openPanel } from "../layout/dock";
import { useDebug, type WatchExpr } from "../state/debug";
import { logLocal } from "../state/logs";
import { useScripts } from "../state/scripts";
import { entityName } from "../state/world";
import { attempt, reportError } from "./report";

export function isScriptFile(file: string | undefined): file is string {
  return !!file && /\.(ts|tsx|js)$/.test(file) && !file.includes(":");
}

/** On a new pause: show the line and the debugger. */
export function onPaused(state: DebugState) {
  const loc = state.location;
  openPanel("debug", true);
  if (isScriptFile(loc?.file) && !loc.generated) {
    useScripts.getState().openFile(loc.file, loc.line);
    openPanel("scripts", true);
  }
}

/** The breakpoints as the host has them (every frontend's). */
export async function refreshBreakpoints() {
  const list = await attempt(api.debug.listBreakpoints(), "Breakpoints");
  if (list) useDebug.getState().setBreakpoints(list);
}

export async function toggleBreakpoint(file: string, line: number) {
  const d = useDebug.getState();
  const existing = d.breakpoints.find((b) => b.file === file && b.line === line);
  if (existing) {
    const r = await attempt(api.debug.clearBreakpoint(existing.id), "Clear breakpoint");
    if (r) d.setBreakpoints(useDebug.getState().breakpoints.filter((b) => b.id !== existing.id));
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

/** The host cannot change a breakpoint's condition in place: set the new one, then clear the old. */
export async function setBreakpointCondition(id: string, condition: string) {
  const d = useDebug.getState();
  const bp = d.breakpoints.find((b) => b.id === id);
  if (!bp) return;
  const r = await attempt(api.debug.setBreakpoint(bp.file, bp.line, condition.trim() || undefined), "Breakpoint condition");
  if (!r) return;
  await attempt(api.debug.clearBreakpoint(id), "Breakpoint condition");
  d.setBreakpoints(useDebug.getState().breakpoints.map((b) => (b.id === id ? r : b)));
}

export async function removeBreakpoint(id: string) {
  const r = await attempt(api.debug.clearBreakpoint(id), "Clear breakpoint");
  if (r) useDebug.getState().setBreakpoints(useDebug.getState().breakpoints.filter((b) => b.id !== id));
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

/**
 * Assigns `text` (a JavaScript expression) to a paused frame's variable or to a value inside one
 * (`l.distance[0]`), then shows the variable again. Assigning to a query column's element writes
 * the component, which the system's call commits; the run is tainted from that tick
 * (docs/spec/debugger.md 5).
 */
export async function setValue(frame: number, path: string, text: string): Promise<boolean> {
  const expr = text.trim();
  if (!expr) return false;
  try {
    await api.debug.eval(`${path} = (${expr})`, frame);
  } catch (e) {
    reportError(e, `Set ${path}`);
    return false;
  }
  const root = /^[A-Za-z_$][\w$]*/.exec(path)?.[0];
  if (root) {
    try {
      const v = await api.debug.eval(root, frame, root);
      useDebug.getState().patchVariable(frame, { ...v, name: root });
    } catch {
      // The value was set; showing it again is a nicety.
    }
  }
  logLocal("info", `Set ${path} = ${expr}`, { source: "debugger" });
  void refreshWatches();
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
