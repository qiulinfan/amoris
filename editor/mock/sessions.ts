// The mock's agent sessions, in the host's shapes (docs/spec/server.md 3.5; amoris-server's
// sessions.rs): the rows of `agent.session.list`, the ring of the newest 2,000 events behind
// `agent.session.events`, `agent.session.open` and `close` with their handles and refusals, and
// every event broadcast on the `agent` topic. A row nothing holds stays listed while the ring
// holds one of its events. `message` events are a started program's lines (`stream` stdout,
// stderr, or the host's own word, `host`; launch.rs), and the simulated agent's words.

import { MockError } from "./util";

export const RING = 2000;
const LABEL_MAX = 80;

export interface AgentRow {
  id: string;
  transport: "http" | "stdio" | "bridge" | "api" | "grant" | "launch";
  role: "developer" | "player";
  seat: string | null;
  label: string;
  calls: number;
  failed_calls: number;
  last_method: string | null;
  live: boolean;
  opened_ts: number;
  last_ts: number;
}

export interface AgentStreamEvent {
  seq: number;
  ts: number;
  session: string;
  kind: "opened" | "call" | "result" | "closed" | "message";
  method?: string;
  summary?: string;
  ok?: boolean;
  stream?: "stdout" | "stderr" | "host";
  row?: AgentRow;
}

interface Entry {
  row: AgentRow;
  handle: string | null;
  inRing: number;
  closed: boolean;
}

function unknown(key: "handle" | "session", value: string): MockError {
  return new MockError(
    "host.session_unknown",
    `This host has no open agent session with the ${key} '${value}': it was closed, or the host started again since it opened. agent.session.open registers one and agent.session.list lists them.`,
    { [key]: value },
  );
}

export class AgentSessions {
  private entries: Entry[] = [];
  private ring: AgentStreamEvent[] = [];
  private seq = 0;
  private numbered = 0;

  constructor(
    private readonly broadcast: (topic: string, data: unknown) => void,
    /** This host run, in handles. */
    private readonly run = Date.now().toString(16),
    private readonly now: () => number = Date.now,
  ) {}

  private entry(id: string): Entry | undefined {
    return this.entries.find((e) => e.row.id === id);
  }

  /** Opens a session: its row, and a bridge's handle. */
  open(spec: { transport: AgentRow["transport"]; seat?: string; label?: string; live?: boolean }): { row: AgentRow; handle: string | null } {
    const ts = this.now();
    const seat = spec.seat ?? null;
    const id = spec.transport === "api" && seat !== null ? `player:${seat}` : `mcp-${++this.numbered}`;
    const row: AgentRow = {
      id,
      transport: spec.transport,
      role: seat === null ? "developer" : "player",
      seat,
      label: [...(spec.label ?? "").trim()].slice(0, LABEL_MAX).join(""),
      calls: 0,
      failed_calls: 0,
      last_method: null,
      live: spec.live ?? true,
      opened_ts: ts,
      last_ts: ts,
    };
    const handle = spec.transport === "bridge" || spec.transport === "launch" ? `${this.run}-${this.numbered}` : null;
    this.entries.push({ row, handle, inRing: 0, closed: false });
    this.record(id, { kind: "opened" });
    return { row: { ...row }, handle };
  }

  /** A session's call. */
  call(session: string, method: string, summary: string) {
    const e = this.entry(session);
    if (e) {
      e.row.calls++;
      e.row.last_method = method;
    }
    this.record(session, { kind: "call", method, summary });
  }

  /** A call's result. */
  result(session: string, method: string, ok: boolean, summary: string) {
    const e = this.entry(session);
    if (e && !ok) e.row.failed_calls++;
    this.record(session, { kind: "result", method, ok, summary });
  }

  /** An agent's own words: a started program's line, with its stream, or the simulated agent's. */
  message(session: string, summary: string, stream?: AgentStreamEvent["stream"]) {
    this.record(session, stream ? { kind: "message", summary, stream } : { kind: "message", summary });
  }

  /** A row by its id. */
  row(id: string): AgentRow | undefined {
    const e = this.entry(id);
    return e && { ...e.row };
  }

  private record(session: string, fields: Pick<AgentStreamEvent, "kind" | "method" | "summary" | "ok" | "stream">) {
    const ts = this.now();
    const event: AgentStreamEvent = { seq: ++this.seq, ts, session, ...fields };
    const e = this.entry(session);
    if (e) {
      e.row.last_ts = ts;
      e.inRing++;
      if (fields.kind === "opened" || fields.kind === "closed") event.row = { ...e.row };
    }
    this.ring.push(event);
    while (this.ring.length > RING) {
      const old = this.ring.shift()!;
      const of = this.entry(old.session);
      if (!of) continue;
      of.inRing--;
      // Nothing holds it and the stream no longer speaks of it.
      if (!of.row.live && of.inRing <= 0) this.entries = this.entries.filter((x) => x !== of);
    }
    this.broadcast("agent", event);
  }

  /** Closes a session; its row as it is then. */
  close(id: string): AgentRow | undefined {
    const e = this.entry(id);
    if (!e) return undefined;
    if (!e.closed) {
      e.closed = true;
      e.row.live = false;
      this.record(id, { kind: "closed" });
    }
    return { ...e.row };
  }

  /** `agent.session.list`. */
  list() {
    return { sessions: this.entries.map((e) => ({ ...e.row })), last: this.seq };
  }

  /** `agent.session.events`: as the host's, `last` the cursor the answer reaches. */
  events(p: { since?: number; limit?: number; session?: string }) {
    if (p.session !== undefined && !this.entry(p.session)) {
      const allowed = this.entries.map((e) => e.row.id);
      throw new MockError("request.invalid_value", `'session' must be one of ${allowed.join(", ")}; got '${p.session}'.`, {
        path: "/session",
        got: p.session,
        allowed,
      });
    }
    const limit = Math.min(RING, Math.max(1, p.limit ?? 100));
    const wanted = this.ring.filter((e) => p.session === undefined || e.session === p.session);
    if (p.since === undefined) return { events: wanted.slice(-limit), last: this.seq, missed: 0 };
    const since = p.since;
    const after = wanted.filter((e) => e.seq > since);
    const events = after.slice(0, limit);
    const oldest = this.seq + 1 - this.ring.length;
    return {
      events,
      last: after.length > limit ? events.at(-1)!.seq : this.seq,
      missed: Math.max(0, oldest - 1 - since),
    };
  }

  /** `agent.session.open {label?, seat?}`: a bridge's row and its handle. */
  register(p: { label?: string; seat?: string }) {
    const { row, handle } = this.open({ transport: "bridge", label: p.label, seat: p.seat });
    return { ...row, handle };
  }

  /** `agent.session.close {handle}` or `{session}`. */
  unregister(p: { handle?: string; session?: string }): AgentRow {
    if (p.handle !== undefined && p.session !== undefined) {
      throw new MockError("request.conflict", "/handle, /session cannot be given together.", { paths: ["/handle", "/session"] });
    }
    if (p.handle === undefined && p.session === undefined) {
      throw new MockError("request.missing_field", "agent.session.close needs 'handle' (the handle agent.session.open answered, or 'session': a bridge session's id).", {
        path: "/handle",
        field: "handle",
      });
    }
    if (p.handle !== undefined) {
      const e = this.entries.find((x) => x.handle === p.handle);
      if (!e) throw unknown("handle", p.handle);
      return this.close(e.row.id)!;
    }
    const e = this.entry(p.session!);
    if (!e) throw unknown("session", p.session!);
    if (e.row.transport !== "bridge") {
      throw new MockError("request.not_applicable", "'session' does not apply: only a bridge session is closed by its id; an MCP session ends with its connection, and a seat's calls with no session hold nothing open.", {
        path: "/session",
      });
    }
    return this.close(e.row.id)!;
  }
}
