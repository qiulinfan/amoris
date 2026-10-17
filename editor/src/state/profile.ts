// `profile` samples (4 Hz while subscribed): the host's CPU time per tick by system, with the
// viewport's frame and GPU timings of the same moment added (state/render.ts); the last minute
// for the profiler's graphs.

import { create } from "zustand";
import type { HostProfile } from "../host/protocol";
import { useRender } from "./render";

const KEEP = 240;

/** A host sample with the editor's viewport timings; those are null while it has none. */
export interface ProfileSample extends HostProfile {
  frame_ms: number | null;
  fps: number | null;
  /** The page's time in the renderer's draw call per frame. */
  render_ms: number | null;
  gpu_ms: number | null;
  gpu: { pass: string; ms: number }[];
}

/** `p` with the viewport's latest timings. */
export function withViewport(p: HostProfile): ProfileSample {
  const r = useRender.getState().stats;
  return {
    ...p,
    systems: p.systems ?? [],
    phases: p.phases ?? [],
    frame_ms: r?.frame_ms ?? null,
    fps: r?.fps ?? null,
    render_ms: r?.render_ms ?? null,
    gpu_ms: r?.gpu_ms ?? null,
    gpu: r?.passes ?? [],
  };
}

interface ProfileState {
  samples: ProfileSample[];
  frozen: boolean;
  push(p: HostProfile): void;
  setFrozen(f: boolean): void;
  clear(): void;
}

export const useProfile = create<ProfileState>((set) => ({
  samples: [],
  frozen: false,
  push: (p) =>
    set((s) => {
      if (s.frozen) return {};
      const sample = withViewport(p);
      return { samples: s.samples.length >= KEEP ? [...s.samples.slice(1), sample] : [...s.samples, sample] };
    }),
  setFrozen: (frozen) => set({ frozen }),
  clear: () => set({ samples: [] }),
}));
