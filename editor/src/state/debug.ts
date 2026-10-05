// The debugger as the host reports it (`debug` events, `debug.state`), the breakpoints and data
// breakpoints set on the host, the pause-on-exceptions mode, and the watch expressions the editor
// evaluates on each pause.

import { create } from "zustand";
import type { Breakpoint, DataWatch, DebugState, ExceptionMode, Variable } from "../host/protocol";

export interface WatchExpr {
  id: number;
  expr: string;
  value?: Variable;
  error?: string;
}

interface DebugStore {
  state: DebugState;
  /** The selected frame's index (what `debug.eval` takes). */
  frame: number;
  breakpoints: Breakpoint[];
  dataWatches: DataWatch[];
  watches: WatchExpr[];
  exceptions: ExceptionMode;
  /** The host's Chrome DevTools Protocol endpoint, when it serves one. */
  cdp: { ws: string; devtools: string } | null;
  setState(s: DebugState): void;
  setFrame(f: number): void;
  setBreakpoints(b: Breakpoint[]): void;
  setDataWatches(w: DataWatch[]): void;
  setWatches(w: WatchExpr[]): void;
  setExceptions(m: ExceptionMode): void;
  /** Replaces a top-level variable of a frame's scopes (after the debugger assigned to it). */
  patchVariable(frame: number, v: Variable): void;
}

export const useDebug = create<DebugStore>((set) => ({
  state: { state: "running" },
  frame: 0,
  breakpoints: [],
  dataWatches: [],
  watches: [],
  exceptions: "none",
  cdp: null,
  setState: (state) =>
    set((s) => ({
      state,
      frame: 0,
      exceptions: state.exceptions ?? s.exceptions,
      cdp: state.cdp !== undefined ? state.cdp : s.cdp,
    })),
  setFrame: (frame) => set({ frame }),
  setBreakpoints: (breakpoints) => set({ breakpoints }),
  setDataWatches: (dataWatches) => set({ dataWatches }),
  setWatches: (watches) => set({ watches }),
  setExceptions: (exceptions) => set({ exceptions }),
  patchVariable: (frame, v) =>
    set((s) => ({
      state: {
        ...s.state,
        frames: s.state.frames?.map((f) =>
          f.id !== frame
            ? f
            : { ...f, scopes: f.scopes.map((sc) => ({ ...sc, variables: sc.variables.map((x) => (x.name === v.name ? v : x)) })) },
        ),
      },
    })),
}));

export function isPaused(): boolean {
  return useDebug.getState().state.state === "paused";
}
