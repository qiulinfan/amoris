// The debugger's controls (host-protocol.md section 6, JSON form): breakpoints, stepping, watch
// expressions evaluated on each pause, data breakpoints, and the console's evaluation.

import { api } from "../host/api";
import type { DebugState, EntityId } from "../host/protocol";
import { openPanel } from "../layout/dock";
import { useDebug, type WatchExpr } from "../state/debug";
import { logLocal } from "../state/logs";
import { useScripts } from "../state/scripts";
import { entityName } from "../state/world";
import { attempt } from "./report";

export function isScriptFile(file: string | undefined): file is string {
  return !!file && /\.(ts|tsx|js)$/.test(file) && !file.includes(":");
}

/** On a new pause: show the line and the debugger. */
export function onPaused(state: DebugState) {
  const loc = state.location;
  openPanel("debug", true);
  if (isScriptFile(loc?.file)) {
    useScripts.getState().openFile(loc.file, loc.line);
    openPanel("scripts", true);
  }
}

export async function toggleBreakpoint(file: string, line: number) {
  const d = useDebug.getState();
  const existing = d.breakpoints.find((b) => b.file === file && b.line === line);
  if (existing) {
    const r = await attempt(api.debug.clearBreakpoint({ id: existing.id }), "Clear breakpoint");
    if (r) d.setBreakpoints(useDebug.getState().breakpoints.filter((b) => b.id !== existing.id));
    return;
  }
  const bp = await attempt(api.debug.setBreakpoint(file, line), "Set breakpoint");
  if (bp) d.setBreakpoints([...useDebug.getState().breakpoints.filter((b) => b.id !== bp.id), bp]);
}

export async function setBreakpointCondition(id: number, condition: string) {
  const d = useDebug.getState();
  const bp = d.breakpoints.find((b) => b.id === id);
  if (!bp) return;
  const r = await attempt(api.debug.setBreakpoint(bp.file, bp.line, condition || undefined), "Breakpoint condition");
  if (r) d.setBreakpoints(useDebug.getState().breakpoints.map((b) => (b.id === id ? { ...r, condition: condition || undefined } : b)));
}

export async function removeBreakpoint(id: number) {
  const r = await attempt(api.debug.clearBreakpoint({ id }), "Clear breakpoint");
  if (r) useDebug.getState().setBreakpoints(useDebug.getState().breakpoints.filter((b) => b.id !== id));
}

export async function clearAllBreakpoints() {
  const r = await attempt(api.debug.clearBreakpoint({}), "Clear breakpoints");
  if (r) useDebug.getState().setBreakpoints([]);
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

export async function refreshWatches() {
  const d = useDebug.getState();
  if (!d.watches.length) return;
  const frame = d.state.state === "paused" ? d.frame : undefined;
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

/** The console's input line: evaluated on the selected frame when paused. */
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

export async function removeDataWatch(id: number) {
  const r = await attempt(api.debug.unwatch(id), "Remove data breakpoint");
  if (r !== undefined) useDebug.getState().setDataWatches(useDebug.getState().dataWatches.filter((w) => w.id !== id));
}
