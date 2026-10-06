// Saving scripts: `scripts.write` then `scripts.apply` (type check, compile, hot swap), with the
// diagnostics of both shown as markers and in the file list.

import { api } from "../host/api";
import { refreshScripts, refreshTypes } from "../host/sync";
import type { Diagnostic } from "../host/protocol";
import { useScripts } from "../state/scripts";
import { useUi } from "../state/ui";
import { attempt } from "./report";

function byFile(diags: Diagnostic[], fallback: string): Record<string, Diagnostic[]> {
  const out: Record<string, Diagnostic[]> = {};
  for (const d of diags) (out[d.file ?? fallback] ??= []).push(d);
  return out;
}

export async function saveScript(path: string, text: string) {
  const written = await attempt(api.scripts.write(path, text), `Save ${path}`);
  if (!written) return false;
  useScripts.getState().setDiagnostics({ [path]: written.diagnostics.filter((d) => !d.file || d.file === path) });
  await applyScripts();
  return true;
}

export async function applyScripts() {
  const applied = await attempt(api.scripts.apply(), "Apply scripts");
  if (!applied) return;
  const files = useScripts.getState().files.map((f) => f.path);
  const grouped = byFile(applied.diagnostics, files[0] ?? "");
  useScripts.getState().setDiagnostics(Object.fromEntries(files.map((f) => [f, grouped[f] ?? []])));
  const errors = applied.diagnostics.filter((d) => d.severity === "error").length;
  useUi.getState().toast(
    applied.bundle
      ? { kind: "success", title: "Scripts applied", body: `Bundle ${applied.bundle} hot-swapped${errors ? "" : ", no errors"}.` }
      : { kind: "error", title: "Scripts not applied", body: `${errors} error${errors === 1 ? "" : "s"}; the previous bundle keeps running.` },
  );
  void refreshScripts();
  // A save can add or change the game's components: reload the declarations the worker checks with.
  void refreshTypes().catch(() => undefined);
}

/** The open editor registers how to save its active file (Cmd+S anywhere in the Scripts panel). */
let saver: (() => void) | null = null;
export function registerScriptSaver(fn: (() => void) | null) {
  saver = fn;
}
export function saveActiveScript(): boolean {
  if (!saver) return false;
  saver();
  return true;
}

let lineToggler: (() => void) | null = null;
export function registerBreakpointToggler(fn: (() => void) | null) {
  lineToggler = fn;
}
export function toggleBreakpointAtCursor() {
  lineToggler?.();
}
