// The host's agent sessions and the stream of what they called (server.md 3.5): the rows of
// `agent.session.list` and the events of `agent.session.events`, read on each connection and kept
// up to date by the `agent` pushes, in the order of their stream numbers. The host is the only
// authority: nothing here is guessed from the events an editor happened to receive, and a gap in
// the numbers means reading again.

import { create } from "zustand";
import type { AgentEvent, AgentSession } from "../host/protocol";

export type AgentLine = AgentEvent;

/** As many lines as the host's ring keeps. */
export const FEED_MAX = 2000;

export interface AgentView {
  /** The host's rows, in opening order. */
  sessions: AgentSession[];
  /** The stream, oldest first. */
  feed: AgentEvent[];
  /** The stream number the rows and the feed stand at. */
  last: number;
}

/** A row after one of its events, as the host counts it. */
function advance(row: AgentSession, e: AgentEvent): AgentSession {
  const next = { ...row, last_ts: e.ts };
  if (e.kind === "call") {
    next.calls++;
    next.last_method = e.method ?? null;
  } else if (e.kind === "result" && e.ok === false) {
    next.failed_calls++;
  }
  return next;
}

/** The rows after `e`: `opened` and `closed` bring the row itself. */
function rowsAfter(sessions: AgentSession[], e: AgentEvent): AgentSession[] {
  const i = sessions.findIndex((s) => s.id === e.session);
  if ((e.kind === "opened" || e.kind === "closed") && e.row) {
    return i < 0 ? [...sessions, e.row] : sessions.map((s, j) => (j === i ? e.row! : s));
  }
  return i < 0 ? sessions : sessions.map((s, j) => (j === i ? advance(s, e) : s));
}

/** The rows the host still lists: a row nothing holds goes with its last line of the stream. */
function kept(sessions: AgentSession[], feed: AgentEvent[]): AgentSession[] {
  if (sessions.every((s) => s.live)) return sessions;
  const spoken = new Set(feed.map((l) => l.session));
  return sessions.filter((s) => s.live || spoken.has(s.id));
}

/** The view after the next event of the stream, as the host's registry applies it. */
export function applyEvent(view: AgentView, e: AgentEvent): AgentView {
  let sessions = rowsAfter(view.sessions, e);
  let feed = [...view.feed, e];
  if (feed.length > FEED_MAX) {
    feed = feed.slice(feed.length - FEED_MAX);
    sessions = kept(sessions, feed);
  }
  return { sessions, feed, last: e.seq };
}

interface AgentState extends AgentView {
  /** Lines up to this stream number are not shown (Clear). */
  hidden: number;
  /** The list and the backlog of this connection are in. */
  loaded: boolean;
  /** Pushes that came before they were, or after a gap. */
  pending: AgentEvent[];
  /** A new connection: nothing of the last one is kept. */
  reset(): void;
  /** Takes the host's list and backlog; false when they do not join (read them again). */
  load(list: { sessions: AgentSession[]; last: number }, backlog: { events: AgentEvent[]; last: number }): boolean;
  /** Takes a pushed event; false when the stream has a gap before it (read the host again). */
  push(e: AgentEvent): boolean;
  clear(): void;
}

export const useAgent = create<AgentState>((set, get) => ({
  sessions: [],
  feed: [],
  last: 0,
  hidden: 0,
  loaded: false,
  pending: [],
  reset: () => set({ sessions: [], feed: [], last: 0, loaded: false, pending: [] }),
  load: (list, backlog) => {
    // The list was read at `list.last` and the backlog after it: the rows take the events between.
    let sessions = list.sessions;
    let next = list.last + 1;
    for (const e of backlog.events) {
      if (e.seq < next) continue;
      if (e.seq > next) return false;
      sessions = rowsAfter(sessions, e);
      next++;
    }
    if (next <= backlog.last) return false;
    const feed = backlog.events.slice(-FEED_MAX);
    let view: AgentView = { sessions: kept(sessions, feed), feed, last: backlog.last };
    // Then what was pushed meanwhile.
    for (const e of [...get().pending].sort((a, b) => a.seq - b.seq)) {
      if (e.seq <= view.last) continue;
      if (e.seq > view.last + 1) return false;
      view = applyEvent(view, e);
    }
    // A host that started again counts from 1: what was cleared is another stream's.
    set((s) => ({ ...view, loaded: true, pending: [], hidden: backlog.last < s.hidden ? 0 : s.hidden }));
    return true;
  },
  push: (e) => {
    const s = get();
    if (!s.loaded) {
      set({ pending: [...s.pending, e].slice(-FEED_MAX) });
      return true;
    }
    if (e.seq <= s.last) return true;
    if (e.seq > s.last + 1) {
      set({ loaded: false, pending: [e] });
      return false;
    }
    set(applyEvent(s, e));
    return true;
  },
  clear: () => set((s) => ({ hidden: s.last })),
}));
