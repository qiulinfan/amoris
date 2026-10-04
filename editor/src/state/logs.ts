// The console's log: host `log` events plus the editor's own lines (evaluation results, errors).

import { create } from "zustand";
import type { LogEntry, LogLevel, Variable } from "../host/protocol";

export type ConsoleLevel = LogLevel | "result" | "input";

export interface ConsoleLine extends Omit<LogEntry, "level"> {
  id: number;
  ts: number;
  level: ConsoleLevel;
  /** An evaluation result, shown as an expandable tree. */
  value?: Variable;
}

const MAX = 5000;
let nextId = 1;

interface LogsState {
  lines: ConsoleLine[];
  push(entry: Omit<ConsoleLine, "id" | "ts"> & { ts?: number }): void;
  clear(): void;
}

export const useLogs = create<LogsState>((set) => ({
  lines: [],
  push: (entry) =>
    set((s) => {
      const line = { ...entry, id: nextId++, ts: entry.ts ?? Date.now() } as ConsoleLine;
      const lines = s.lines.length >= MAX ? [...s.lines.slice(s.lines.length - MAX + 1), line] : [...s.lines, line];
      return { lines };
    }),
  clear: () => set({ lines: [] }),
}));

export function logLocal(level: ConsoleLevel, message: string, extra: Partial<ConsoleLine> = {}) {
  useLogs.getState().push({ level, source: "editor", message, ...extra });
}
