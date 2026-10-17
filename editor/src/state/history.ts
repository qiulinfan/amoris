// The host's undo history (`history` events): every edit by the editor or by an agent, with who
// made it.

import { create } from "zustand";
import type { HistoryEntry, HistoryState as HostHistory } from "../host/protocol";

interface HistoryState {
  /** Oldest first: the next to undo is the last. */
  undo: HistoryEntry[];
  /** The next to redo first. */
  redo: HistoryEntry[];
  set(h: HostHistory): void;
}

/** The host's entries, or entries made from the labels for a host that sends none; the host lists
 * the redo stack most recent last, the store the next to redo first (as the panel shows it). */
export function historyEntries(h: HostHistory): { undo: HistoryEntry[]; redo: HistoryEntry[] } {
  const pick = (labels: string[], entries: HistoryEntry[] | undefined) =>
    labels.map((label, i) => (entries?.[i]?.label === label ? entries[i]! : { label }));
  return { undo: pick(h.undo, h.entries?.undo), redo: pick(h.redo, h.entries?.redo).reverse() };
}

export const useHistory = create<HistoryState>((set) => ({
  undo: [],
  redo: [],
  set: (h) => set(historyEntries(h)),
}));

/**
 * Who made an entry, when it was not the person at this editor: the session's name (`mcp-1`,
 * `api`), else the source (`developer 2`, `player 0`). A host that sends no sources marks agent
 * edits by a label starting with "agent:" (the mock did, editor.md 8).
 */
export function agentOf(e: HistoryEntry): string | null {
  const s = e.source;
  if (s === undefined) return /^agent\b/i.test(e.label) ? "agent" : null;
  if (s === "editor" || s === "host") return null;
  if (e.session) return e.session;
  return "developer" in s ? `developer ${s.developer}` : `player ${s.player}`;
}
