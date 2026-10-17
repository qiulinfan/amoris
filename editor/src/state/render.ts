// The viewport's own frame and GPU timings (engine.ts `RenderStats`), read from the renderer four
// times a second by the viewport panel: the stats overlay shows them, and the profiler adds them
// to the host's CPU samples (the host has no frame; the editor's viewport draws it).

import { create } from "zustand";
import type { RenderStats } from "../viewport/engine";

interface RenderState {
  stats: RenderStats | null;
  set(stats: RenderStats | null): void;
}

export const useRender = create<RenderState>((set) => ({
  stats: null,
  set: (stats) => set({ stats }),
}));
