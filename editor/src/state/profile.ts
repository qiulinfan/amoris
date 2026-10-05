// `profile` samples (4 Hz while subscribed): the last few minutes for the profiler's graphs.

import { create } from "zustand";
import type { ProfileSample } from "../host/protocol";

const KEEP = 240;

interface ProfileState {
  samples: ProfileSample[];
  frozen: boolean;
  push(p: ProfileSample): void;
  setFrozen(f: boolean): void;
  clear(): void;
}

export const useProfile = create<ProfileState>((set) => ({
  samples: [],
  frozen: false,
  push: (p) =>
    set((s) => (s.frozen ? {} : { samples: s.samples.length >= KEEP ? [...s.samples.slice(1), p] : [...s.samples, p] })),
  setFrozen: (frozen) => set({ frozen }),
  clear: () => set({ samples: [] }),
}));
