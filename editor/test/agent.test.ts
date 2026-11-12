// The host's agent sessions in the editor (editor.md 3, server.md 3.5): the mock answers
// `agent.session.*` in the host's shapes, and the agent store shows the host's rows and stream,
// read on connect and kept by pushes in the order of their stream numbers, with a gap settled by
// reading again. The store is driven by the mock's own registry through `Session.call`, the path
// the WebSocket takes.
import { beforeEach, expect, test } from "bun:test";
import { MockPrograms } from "../mock/programs";
import { Session } from "../mock/session";
import { RING } from "../mock/sessions";
import { MockError } from "../mock/util";
import type { AgentEvent, AgentSession } from "../src/host/protocol";
import { FEED_MAX, useAgent } from "../src/state/agent";

const ROW_KEYS = ["calls", "failed_calls", "id", "label", "last_method", "last_ts", "live", "opened_ts", "role", "seat", "transport"];

type List = { sessions: AgentSession[]; last: number };
type Backlog = { events: AgentEvent[]; last: number; missed: number };

/** A mock host and every `agent` event it pushed. */
function mock() {
  const pushed: AgentEvent[] = [];
  const session = new Session((topic, data) => {
    if (topic === "agent") pushed.push(data as AgentEvent);
  });
  const list = () => session.call("agent.session.list", {}) as List;
  const events = (p: Record<string, unknown> = {}) => session.call("agent.session.events", p) as Backlog;
  return { session, pushed, list, events };
}

/** What a refused call says. */
function refusal(run: () => unknown): { code: string; detail: Record<string, unknown> } {
  try {
    run();
  } catch (e) {
    if (e instanceof MockError) return e.toWire();
    throw e;
  }
  throw new Error("the call was not refused");
}

beforeEach(() => {
  useAgent.setState({ sessions: [], feed: [], last: 0, hidden: 0, loaded: false, pending: [] });
});

test("the mock lists and records sessions in the host's shapes", () => {
  const { session, pushed, list, events } = mock();
  expect(list()).toEqual({ sessions: [], last: 0 });
  expect(events()).toEqual({ events: [], last: 0, missed: 0 });

  const opened = session.call("agent.session.open", { label: "  bridge-test 1  " }) as AgentSession & { handle: string };
  expect(Object.keys(opened).sort()).toEqual([...ROW_KEYS, "handle"].sort());
  expect(opened).toMatchObject({ id: "mcp-1", transport: "bridge", role: "developer", seat: null, label: "bridge-test 1", calls: 0, live: true });
  const agents = session.agents;
  agents.call("mcp-1", "world.tree", "{}");
  agents.result("mcp-1", "world.tree", true, "12 entities");
  agents.call("mcp-1", "world.get", '{"entity":"Nobody"}');
  agents.result("mcp-1", "world.get", false, "sim.entity_not_found: no entity 'Nobody'");

  // A row: exactly the host's keys, none of them the handle.
  const row = list().sessions[0]!;
  expect(Object.keys(row).sort()).toEqual(ROW_KEYS);
  expect(row).toMatchObject({ calls: 2, failed_calls: 1, last_method: "world.get", live: true });
  expect(list().last).toBe(5);
  // Each kind of event and its keys.
  const all = events().events;
  expect(all.map((e) => e.kind)).toEqual(["opened", "call", "result", "call", "result"]);
  expect(all.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5]);
  expect(Object.keys(all[0]!).sort()).toEqual(["kind", "row", "seq", "session", "ts"]);
  expect(Object.keys(all[1]!).sort()).toEqual(["kind", "method", "seq", "session", "summary", "ts"]);
  expect(Object.keys(all[2]!).sort()).toEqual(["kind", "method", "ok", "seq", "session", "summary", "ts"]);
  expect(all[0]!.row).toMatchObject({ id: "mcp-1", label: "bridge-test 1" });
  expect(pushed).toEqual(all);

  // A player's session, and closing: by the handle (twice), by a bridge's id, and what is refused.
  const seat = session.call("agent.session.open", { seat: "skipper" }) as AgentSession & { handle: string };
  expect(seat).toMatchObject({ id: "mcp-2", role: "player", seat: "skipper" });
  const closed = session.call("agent.session.close", { handle: opened.handle }) as AgentSession;
  expect(closed).toMatchObject({ id: "mcp-1", live: false, calls: 2 });
  expect(session.call("agent.session.close", { handle: opened.handle })).toEqual(closed);
  expect(events({ limit: 1 }).events[0]).toMatchObject({ kind: "closed", session: "mcp-1", row: { live: false } });
  expect(session.call("agent.session.close", { session: "mcp-2" })).toMatchObject({ id: "mcp-2", live: false });
  expect(refusal(() => session.call("agent.session.close", { handle: "0-9" })).code).toBe("host.session_unknown");
  expect(refusal(() => session.call("agent.session.close", { session: "mcp-9" })).code).toBe("host.session_unknown");
  expect(refusal(() => session.call("agent.session.close", { handle: opened.handle, session: "mcp-1" })).code).toBe("request.conflict");
  expect(refusal(() => session.call("agent.session.close", {})).code).toBe("request.missing_field");
  const simulated = agents.open({ transport: "http", label: "claude-opus (simulated)" }).row.id;
  expect(refusal(() => session.call("agent.session.close", { session: simulated }))).toMatchObject({ code: "request.not_applicable", detail: { path: "/session" } });
  const unlisted = refusal(() => events({ session: "nope" }));
  expect(unlisted).toMatchObject({ code: "request.invalid_value", detail: { path: "/session", allowed: ["mcp-1", "mcp-2", "mcp-3"] } });
  expect(refusal(() => events({ sinse: 3 })).code).toBe("request.unknown_field");
  expect(refusal(() => session.call("agent.session.start", {})).code).toBe("request.missing_field");
});

test("the mock's stream is bounded and read with since, limit and session as the host's", () => {
  const { session, list, events } = mock();
  const agents = session.agents;
  const a = agents.open({ transport: "http", label: "a" }).row.id;
  const b = agents.open({ transport: "bridge", label: "b" }).row.id;
  for (let i = 0; i < 5; i++) {
    agents.call(a, "world.get", "{}");
    agents.result(a, "world.get", true, "ok");
    agents.call(b, "world.tree", "{}");
    agents.result(b, "world.tree", true, "ok");
  }
  // 22 events. Without `since` the newest; with it the first after it, `last` where it stopped.
  expect(events({ limit: 3 })).toMatchObject({ last: 22, missed: 0 });
  expect(events({ limit: 3 }).events.map((e) => e.seq)).toEqual([20, 21, 22]);
  const read: number[] = [];
  for (let since = 0; ; ) {
    const page = events({ since, limit: 5 });
    read.push(...page.events.map((e) => e.seq));
    if (page.last === since) break;
    since = page.last;
  }
  expect(read).toEqual(Array.from({ length: 22 }, (_, i) => i + 1));
  expect(events({ since: 0, limit: 2 })).toMatchObject({ last: 2 });
  const one = events({ since: 0, session: b, limit: 4 });
  expect(one.events.map((e) => e.seq)).toEqual([2, 5, 6, 9]);
  expect(one.last).toBe(9);
  expect(events({ since: 9, session: b }).events.map((e) => e.seq)).toEqual([10, 13, 14, 17, 18, 21, 22]);
  expect(events({ since: 9, session: b }).last).toBe(22);

  // The ring keeps the newest 2,000; a closed row goes with its last event, a live one stays.
  agents.close(a);
  for (let i = 0; i < RING - 1; i++) agents.call(b, "status", "{}");
  expect(list().sessions.map((s) => s.id)).toEqual([a, b]);
  expect(events({ since: 0, limit: RING }).events[0]).toMatchObject({ kind: "closed", session: a });
  expect(events({ since: 0 }).missed).toBe(22);
  agents.call(b, "status", "{}");
  expect(list().sessions.map((s) => s.id)).toEqual([b]);
  expect(events({ since: 0, limit: RING }).events.length).toBe(RING);
  expect(events({ since: 0 }).missed).toBe(23);
});

test("an edit made as a mock session is the session's in the history", () => {
  const { session } = mock();
  const id = session.agents.open({ transport: "http", label: "claude-opus (simulated)" }).row.id;
  const crate = [...session.world.entities.values()].find((e) => e.name === "Crate4")!;
  session.call("world.edit", { ops: [{ set: { entity: crate.id, component: "Cargo", value: { value: 9 } } }], label: "agent edit" }, { source: { developer: 1 }, session: id });
  const h = session.call("history.list", {}) as { entries: { undo: { label: string; session?: string }[] } };
  expect(h.entries.undo.at(-1)).toMatchObject({ label: "agent edit", session: "mcp-1" });
  expect(id).toBe("mcp-1");
});

test("the store shows the host's rows and stream, loaded on connect", () => {
  const { session, list, events } = mock();
  const agents = session.agents;
  const a = agents.open({ transport: "http", label: "probe 1" }).row.id;
  agents.call(a, "world.edit", "{}");
  agents.result(a, "world.edit", true, "applied 1");
  agents.call(a, "world.get", "{}");
  agents.result(a, "world.get", false, "sim.entity_not_found");
  agents.close(a);
  const b = agents.open({ transport: "bridge", label: "bridge 2" }).row.id;

  // An editor that connects now heard none of it.
  const store = useAgent.getState;
  expect(store().load(list(), events({ limit: FEED_MAX }))).toBe(true);
  expect(store().sessions).toEqual(list().sessions);
  expect(store().feed).toEqual(events({ limit: FEED_MAX }).events);
  expect(store().last).toBe(7);
  expect(store().sessions.map((s) => [s.id, s.live, s.calls, s.failed_calls])).toEqual([
    [a, false, 2, 1],
    [b, true, 0, 0],
  ]);
});

test("pushes apply once and in order; a gap is settled by reading the host again", () => {
  const { session, pushed, list, events } = mock();
  const agents = session.agents;
  const store = useAgent.getState;
  const a = agents.open({ transport: "http", label: "probe 1" }).row.id;
  agents.call(a, "world.tree", "{}");

  // Events pushed before the list and the backlog are in are kept, and applied once.
  expect(store().push(pushed[0]!)).toBe(true);
  expect(store().push(pushed[1]!)).toBe(true);
  expect(store().feed).toEqual([]);
  const read = [list(), events({ limit: FEED_MAX })] as const;
  agents.result(a, "world.tree", true, "12 entities");
  expect(store().push(pushed[2]!)).toBe(true);
  expect(store().load(...read)).toBe(true);
  expect(store().feed.map((e) => e.seq)).toEqual([1, 2, 3]);
  expect(store().sessions).toEqual(list().sessions);

  // A pushed event applies once: opened adds a row, a call counts, a failed result counts.
  const b = agents.open({ transport: "bridge", label: "bridge 2" }).row.id;
  agents.call(b, "world.get", "{}");
  agents.result(b, "world.get", false, "sim.entity_not_found");
  for (const e of pushed.slice(3)) expect(store().push(e)).toBe(true);
  for (const e of pushed) expect(store().push(e)).toBe(true);
  expect(store().feed.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5, 6]);
  expect(store().sessions).toEqual(list().sessions);
  expect(store().sessions[1]).toMatchObject({ id: b, calls: 1, failed_calls: 1, last_method: "world.get", live: true });

  // Closed clears `live`; the rows stay the host's through more opens, calls and closes.
  agents.close(a);
  const c = agents.open({ transport: "http", label: "probe 3" }).row.id;
  agents.call(c, "status", "{}");
  agents.result(c, "status", true, "tick 0");
  agents.message(c, "Checking on the voyage.");
  agents.close(c);
  for (const e of pushed.slice(6)) expect(store().push(e)).toBe(true);
  expect(store().sessions).toEqual(list().sessions);
  expect(store().sessions.map((s) => s.live)).toEqual([false, true, false]);
  expect(store().feed).toEqual(events({ limit: FEED_MAX }).events);

  // A skipped event (a socket that fell behind): refused, then a reload equals the host again.
  const at = pushed.length;
  agents.call(b, "world.tree", "{}");
  agents.result(b, "world.tree", true, "12 entities");
  agents.close(b);
  expect(store().push(pushed[at + 1]!)).toBe(false);
  expect(store().loaded).toBe(false);
  expect(store().push(pushed[at + 2]!)).toBe(true);
  expect(store().last).toBe(at);
  expect(store().load(list(), events({ limit: FEED_MAX }))).toBe(true);
  expect(store().sessions).toEqual(list().sessions);
  expect(store().feed).toEqual(events({ limit: FEED_MAX }).events);
  expect(store().last).toBe(at + 3);

  // A list and a backlog that do not join (the backlog starts past the list) are read again.
  const stale = list();
  for (let i = 0; i < 4; i++) agents.call(b, "status", "{}");
  expect(store().load(stale, events({ limit: 2 }))).toBe(false);
  expect(store().load(stale, events({ limit: FEED_MAX }))).toBe(true);
  expect(store().sessions).toEqual(list().sessions);
});

test("the store keeps as many lines as the host's ring and drops the rows it drops", () => {
  const { session, pushed, list, events } = mock();
  const agents = session.agents;
  const store = useAgent.getState;
  expect(store().load(list(), events({ limit: FEED_MAX }))).toBe(true);
  const a = agents.open({ transport: "http", label: "short" }).row.id;
  agents.close(a);
  const b = agents.open({ transport: "bridge", label: "long" }).row.id;
  for (let i = 0; i < RING; i++) agents.call(b, "status", "{}");
  for (const e of pushed) expect(store().push(e)).toBe(true);
  expect(store().feed.length).toBe(FEED_MAX);
  expect(store().sessions).toEqual(list().sessions);
  expect(store().sessions.map((s) => s.id)).toEqual([b]);
  expect(store().feed).toEqual(events({ limit: FEED_MAX }).events);
});

test("clear hides what was shown, and a host that started again shows its stream", () => {
  const { session, pushed, list, events } = mock();
  const store = useAgent.getState;
  const a = session.agents.open({ transport: "http", label: "probe 1" }).row.id;
  session.agents.call(a, "status", "{}");
  session.agents.result(a, "status", true, "tick 0");
  expect(store().load(list(), events({ limit: FEED_MAX }))).toBe(true);
  store().clear();
  expect(store().hidden).toBe(3);
  expect(store().feed.filter((l) => l.seq > store().hidden)).toEqual([]);
  // The rows are the host's still, and what follows is shown.
  expect(store().sessions).toEqual(list().sessions);
  session.agents.call(a, "world.tree", "{}");
  expect(store().push(pushed.at(-1)!)).toBe(true);
  expect(store().feed.filter((l) => l.seq > store().hidden).map((l) => l.seq)).toEqual([4]);
  // The same host again (a reconnection): what was cleared stays hidden.
  store().reset();
  expect(store().load(list(), events({ limit: FEED_MAX }))).toBe(true);
  expect(store().hidden).toBe(3);
  // Another host run counts from 1: nothing of it is hidden.
  const again = mock();
  again.session.agents.open({ transport: "http", label: "probe 1" });
  store().reset();
  expect(store().load(again.list(), again.events({ limit: FEED_MAX }))).toBe(true);
  expect(store().hidden).toBe(0);
  expect(store().feed.map((l) => l.seq)).toEqual([1]);
});

test("the mock starts a registered agent program as a launch session, as the host does", async () => {
  const { session, events } = mock();
  session.programs = new MockPrograms(session, 2);
  const listed = session.call("agent.session.agents", {}) as { agents: { name: string; label: string }[]; running: number; limit: number };
  expect(listed).toEqual({ agents: [{ name: "scripted", label: "Scripted agent (mock)" }], running: 0, limit: 4 });
  const row = session.call("agent.session.start", { agent: "scripted", goal: "List the scene." }) as AgentSession & { agent: string; pid: number };
  expect(row).toMatchObject({ id: "mcp-1", transport: "launch", role: "developer", live: true, agent: "scripted" });
  expect("handle" in row).toBe(false);
  await Bun.sleep(40);
  const told = events({ session: row.id }).events;
  expect(told.map((e) => (e.kind === "message" ? `${e.stream}: ${e.summary}` : e.method ? `${e.kind} ${e.method}` : e.kind))).toEqual([
    "opened",
    "host: started scripted (pid 4001): List the scene.",
    "stdout: goal: List the scene.",
    "call world.tree",
    "result world.tree",
    "stdout: Listed the scene; nothing else to do.",
    "host: exit status: 0",
    "closed",
  ]);
  // Stop ends one that runs; refusals as the host's.
  const slow = new MockPrograms(session, 1000);
  session.programs = slow;
  const running = session.call("agent.session.start", { agent: "scripted", goal: "Wait.", seat: "skipper" }) as AgentSession;
  expect(running).toMatchObject({ role: "player", seat: "skipper" });
  expect(session.call("agent.session.stop", { session: running.id })).toMatchObject({ id: running.id, live: false });
  expect(events({ session: running.id }).events.at(-2)).toMatchObject({ kind: "message", stream: "host", summary: "signal: 9 (SIGKILL)" });
  expect(refusal(() => session.call("agent.session.start", { agent: "scriptd", goal: "x" }))).toMatchObject({
    code: "request.invalid_value",
    detail: { path: "/agent" },
  });
  expect(refusal(() => session.call("agent.session.start", { agent: "scripted", goal: " " })).code).toBe("request.out_of_range");
  expect(refusal(() => session.call("agent.session.stop", { session: "mcp-9" })).code).toBe("host.session_unknown");
  // The store takes the launch rows and the messages' streams as any other.
  const state = useAgent.getState();
  state.reset();
  expect(state.load(session.call("agent.session.list", {}) as List, events({ limit: 2000 }))).toBe(true);
  expect(useAgent.getState().sessions.map((s) => [s.transport, s.live])).toEqual([
    ["launch", false],
    ["launch", false],
  ]);
  expect(useAgent.getState().feed.filter((e) => e.stream === "stdout").length).toBe(2);
});
