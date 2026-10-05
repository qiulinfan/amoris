// The live feed of what agents do over MCP (`agent` events), newest last.

import { create } from "zustand";
import type { AgentEvent } from "../host/protocol";

export interface AgentLine extends AgentEvent {
  id: number;
  ts: number;
}

let nextId = 1;
const MAX = 2000;

interface AgentState {
  feed: AgentLine[];
  push(e: AgentEvent): void;
  clear(): void;
}

export const useAgent = create<AgentState>((set) => ({
  feed: [],
  push: (e) =>
    set((s) => {
      const line: AgentLine = { ...e, id: nextId++, ts: e.ts ?? Date.now() };
      const feed = s.feed.length >= MAX ? [...s.feed.slice(1), line] : [...s.feed, line];
      return { feed };
    }),
  clear: () => set({ feed: [] }),
}));
