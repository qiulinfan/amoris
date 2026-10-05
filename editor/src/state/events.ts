// Game events (`events` pushes, `events.since` on connect) and the selected event's cause chain.

import { create } from "zustand";
import type { GameEvent } from "../host/protocol";

const MAX = 20000;

interface EventsState {
  events: GameEvent[];
  selected: number | null;
  why: GameEvent[] | null;
  push(batch: GameEvent[]): void;
  replace(all: GameEvent[]): void;
  clear(): void;
  select(seq: number | null, why?: GameEvent[] | null): void;
}

export const useEvents = create<EventsState>((set) => ({
  events: [],
  selected: null,
  why: null,
  push: (batch) =>
    set((s) => {
      const seen = s.events.length ? s.events[s.events.length - 1]!.seq : 0;
      const fresh = batch.filter((e) => e.seq > seen);
      if (!fresh.length) return {};
      const events = [...s.events, ...fresh];
      return { events: events.length > MAX ? events.slice(events.length - MAX) : events };
    }),
  replace: (all) => set({ events: all.slice(-MAX) }),
  clear: () => set({ events: [], selected: null, why: null }),
  select: (seq, why = null) => set({ selected: seq, why }),
}));
