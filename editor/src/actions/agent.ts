// Reads the host's agent sessions and their stream into the store (server.md 3.5): on each
// connection, and again when the pushed stream has a gap (a socket that fell behind).

import { api, host } from "../host/api";
import { useAgent } from "../state/agent";

/** As many events as the host's ring keeps. */
const BACKLOG = 2000;

let loading: Promise<void> | null = null;

/** One load at a time: a gap found while one runs is settled by it. */
export function loadAgent(): Promise<void> {
  loading ??= load().finally(() => {
    loading = null;
  });
  return loading;
}

async function load() {
  // The programs the host may start: a host that has no such method starts none.
  try {
    useAgent.getState().setPrograms((await api.agent.programs()).agents);
  } catch {
    useAgent.getState().setPrograms([]);
  }
  for (let attempt = 0; attempt < 3; attempt++) {
    // Subscribed before reading, so nothing falls between the backlog and the first push; the
    // list first, so the backlog reaches back to it.
    await host.call("subscribe", { topics: ["agent"] });
    const list = await api.agent.sessions();
    const backlog = await api.agent.events({ limit: BACKLOG });
    if (useAgent.getState().load(list, backlog)) return;
  }
  throw new Error("the agent stream moved faster than it could be read");
}

/** Starts a registered agent program (server.md 3.7); its session arrives with the stream. */
export function startAgent(p: { agent: string; goal: string; seat?: string }) {
  return api.agent.start(p);
}

/** Ends a started agent program. */
export function stopAgent(session: string) {
  return api.agent.stop(session);
}
