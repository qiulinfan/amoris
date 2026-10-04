// The debugger as the host reports it (`debug` events, `debug.state`), the breakpoints and data
// breakpoints the editor set, and the watch expressions it evaluates on each pause.

import { create } from "zustand";
import type { Breakpoint, DataWatch, DebugState, Variable } from "../host/protocol";

export interface WatchExpr {
  id: number;
  expr: string;
  value?: Variable;
  error?: string;
}

interface DebugStore {
  state: DebugState;
  frame: number;
  breakpoints: Breakpoint[];
  dataWatches: DataWatch[];
  watches: WatchExpr[];
  setState(s: DebugState): void;
  setFrame(f: number): void;
  setBreakpoints(b: Breakpoint[]): void;
  setDataWatches(w: DataWatch[]): void;
  setWatches(w: WatchExpr[]): void;
}

export const useDebug = create<DebugStore>((set) => ({
  state: { state: "running" },
  frame: 0,
  breakpoints: [],
  dataWatches: [],
  watches: [],
  setState: (state) => set({ state, frame: 0 }),
  setFrame: (frame) => set({ frame }),
  setBreakpoints: (breakpoints) => set({ breakpoints }),
  setDataWatches: (dataWatches) => set({ dataWatches }),
  setWatches: (watches) => set({ watches }),
}));

export function isPaused(): boolean {
  return useDebug.getState().state.state === "paused";
}
