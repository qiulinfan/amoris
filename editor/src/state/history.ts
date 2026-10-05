// The host's undo history (`history` events): every edit by the editor or by an agent.

import { create } from "zustand";

interface HistoryState {
  undo: string[];
  redo: string[];
  set(h: { undo: string[]; redo: string[] }): void;
}

export const useHistory = create<HistoryState>((set) => ({
  undo: [],
  redo: [],
  set: (h) => set({ undo: h.undo, redo: h.redo }),
}));

/** Labels of edits an agent made through MCP start with "agent:" (editor.md). */
export function isAgentLabel(label: string): boolean {
  return /^agent\b/i.test(label);
}
