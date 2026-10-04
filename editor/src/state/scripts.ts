// The project's scripts: the list with diagnostics, which files are open in the editor's tabs,
// which have unsaved edits, and requests to reveal a line (from the console, the debugger, events).

import { create } from "zustand";
import type { Diagnostic, ScriptInfo } from "../host/protocol";

interface ScriptsState {
  files: ScriptInfo[];
  open: string[];
  active: string | null;
  dirty: Record<string, boolean>;
  diagnostics: Record<string, Diagnostic[]>;
  reveal: { path: string; line: number; nonce: number } | null;
  setFiles(files: ScriptInfo[]): void;
  openFile(path: string, line?: number): void;
  closeFile(path: string): void;
  setActive(path: string): void;
  setDirty(path: string, dirty: boolean): void;
  setDiagnostics(byFile: Record<string, Diagnostic[]>): void;
}

let nonce = 0;

export const useScripts = create<ScriptsState>((set, get) => ({
  files: [],
  open: [],
  active: null,
  dirty: {},
  diagnostics: {},
  reveal: null,
  setFiles: (files) =>
    set((s) => ({
      files,
      diagnostics: { ...s.diagnostics, ...Object.fromEntries(files.map((f) => [f.path, f.diagnostics ?? []])) },
    })),
  openFile: (path, line) =>
    set((s) => ({
      open: s.open.includes(path) ? s.open : [...s.open, path],
      active: path,
      reveal: line ? { path, line, nonce: ++nonce } : s.reveal,
    })),
  closeFile: (path) => {
    const open = get().open.filter((p) => p !== path);
    const active = get().active === path ? (open.at(-1) ?? null) : get().active;
    set({ open, active });
  },
  setActive: (active) => set({ active }),
  setDirty: (path, dirty) => set((s) => ({ dirty: { ...s.dirty, [path]: dirty } })),
  setDiagnostics: (byFile) => set((s) => ({ diagnostics: { ...s.diagnostics, ...byFile } })),
}));
