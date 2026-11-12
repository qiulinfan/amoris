// A poisoned world (editor.md 8, item 14): the game thread's own status then carries
// `world_hash: null`, which once took the whole window down. The status is read as halted, the
// shell draws it, Pause, Resume and Step ask for nothing, and a part of the window that cannot draw
// fails alone. Also here: the wire forms of two calls the editor makes on connect and from the
// inspector (`events.since` for the newest events, a component without fields as `{}`), with the
// editor's client answered by the mock over the wire.
import { beforeEach, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Session } from "../mock/session";
import { MockError } from "../mock/util";
import type { ComponentInfo, Status } from "../src/host/protocol";
import { componentFields, defaultComponent } from "../src/host/schema";
import { ownsKey } from "../src/commands/keys";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { api, haltNotice, haltedOf, host, shortHash } = await import("../src/host/api");
const { stepTick, togglePause } = await import("../src/actions/time");
const { StatusBar } = await import("../src/shell/StatusBar");
const { Transport } = await import("../src/shell/Transport");
const { useConnection } = await import("../src/state/connection");
const { useSession } = await import("../src/state/session");
const { useUi } = await import("../src/state/ui");
const { Boundary, failed } = await import("../src/ui/Boundary");

/** `time.control {}` as `amoris serve` answered it for a Play world whose first tick failed
 * halfway (a Vessel whose collider spans no volume): the game thread's own status. */
const THREAD: Status = JSON.parse(
  `{"behind_ms":null,"bundle":"0777f5f6f6a0d432e345e34abf101761e01ebd0a5d62f698c7c8026a1e353ea3","entities":26,"epoch":5,"halted":true,"kept":1,"mode":"play","pacing":{"real_time":{"speed":1}},"paused":true,"poisoned":1,"steps_due":0,"t_s":0.016666666666666666,"tainted":null,"tick":1,"world_hash":null,"writes":39}`,
);
/** The status the same host pushed for that world: the last publication's world, with its hash. */
const PUSHED: Status = JSON.parse(
  `{"behind_ms":null,"entities":26,"epoch":5,"fps":0,"halted":true,"mode":"play","pacing":{"real_time":{"speed":1}},"paused":true,"state":"waiting","t_s":0.016666666666666666,"tick":1,"tps":0,"version":105,"world_hash":"f21c7d27634e235ffbbd44838b057de8"}`,
);
const RUNNING: Status = { ...PUSHED, halted: false, paused: false, tick: 40 };

/** The editor's client answered by a mock session over the wire; `sent` holds each request. */
function wire(session = new Session(() => {})) {
  const sent: { method: string; params: Record<string, unknown> }[] = [];
  const client = host as unknown as { ws: unknown; receive(raw: string): void };
  client.ws = {
    readyState: WebSocket.OPEN,
    send(raw: string) {
      const m = JSON.parse(raw) as { id: number; method: string; params: Record<string, unknown> };
      sent.push({ method: m.method, params: m.params });
      queueMicrotask(() => {
        try {
          client.receive(JSON.stringify({ id: m.id, result: session.call(m.method, m.params) }));
        } catch (e) {
          if (!(e instanceof MockError)) throw e;
          client.receive(JSON.stringify({ id: m.id, error: e.toWire() }));
        }
      });
    },
  };
  return { session, sent };
}

/** Sets a status for the actions and for markup: rendered to a string, a component reads a
 * store's initial state (React's server snapshot), so the status is put there as well. */
function show(status: Status) {
  useSession.getState().setStatus(status);
  Object.assign(useSession.getInitialState(), { status });
}

beforeEach(() => {
  useUi.setState({ toasts: [] });
  const info = { state: "open" as const, retryAt: null, attempts: 0, lastError: null };
  useConnection.getState().set({ info });
  Object.assign(useConnection.getInitialState(), { info });
});

test("a poisoned world's status is read as halted, with or without a hash", () => {
  expect(haltedOf(THREAD)).toBe(true);
  expect(haltedOf(PUSHED)).toBe(true);
  expect(haltedOf(RUNNING)).toBe(false);
  expect(haltedOf({ ...RUNNING, halted: undefined })).toBe(false);
  expect(haltedOf(null)).toBe(false);
  expect(shortHash(THREAD)).toBe("—");
  expect(shortHash(PUSHED)).toBe("f21c7d27");
  expect(shortHash(null)).toBe("—");
  // The failed tick: the one the game thread names; the pushed status has the clock, which stands
  // in it.
  expect(haltNotice(THREAD).title).toBe("Play halted at tick 1");
  expect(haltNotice(PUSHED).title).toBe("Play halted at tick 1");
  expect(haltNotice({ ...THREAD, poisoned: 10, tick: 10 }).title).toBe("Play halted at tick 10");
  expect(haltNotice(THREAD).body).toContain("Stop returns to the edit world.");
  const edit = haltNotice({ ...THREAD, mode: "edit" });
  expect(edit.title).toBe("The edit world halted at tick 1");
  expect(edit.body).toContain("Restore a snapshot");
});

/** The buttons of some markup by their aria-label's first words, and whether each is disabled. */
function buttons(html: string): Record<string, boolean> {
  const out: Record<string, boolean> = {};
  for (const m of html.matchAll(/<button[^>]*>/g)) {
    const label = /aria-label="([^"(]*)/.exec(m[0])?.[1]?.trim();
    if (label) out[label] = /\sdisabled(=|\s|>)/.test(m[0]);
  }
  return out;
}

test("the status bar and the transport draw a status without a hash", () => {
  // The game thread's status: this is the render that threw `reading 'slice'` of null.
  show(THREAD);
  const bar = renderToStaticMarkup(createElement(StatusBar));
  expect(bar).toContain("26 entities");
  expect(bar).toContain("—");
  let transport = renderToStaticMarkup(createElement(Transport));
  expect(transport).toContain("HALTED");
  expect(transport).toContain("is-halted");
  // Stop is the way out; Resume and Step have nothing to ask a halted world.
  expect(buttons(transport)).toEqual({ Stop: false, Resume: true, "Step one tick": true });
  // The pushed status of the same world, and a world that runs.
  show(PUSHED);
  expect(renderToStaticMarkup(createElement(StatusBar))).toContain("f21c7d27");
  expect(renderToStaticMarkup(createElement(Transport))).toContain("HALTED");
  show(RUNNING);
  transport = renderToStaticMarkup(createElement(Transport));
  expect(transport).toContain("PLAY");
  expect(transport).not.toContain("HALTED");
  expect(buttons(transport)).toEqual({ Stop: false, Pause: false, "Step one tick": false });
});

test("Pause, Resume and Step ask a halted world for nothing, and say why", async () => {
  const { sent } = wire();
  useSession.getState().setStatus(THREAD);
  await togglePause();
  await stepTick();
  expect(sent).toEqual([]);
  const toasts = useUi.getState().toasts;
  expect(toasts.map((t) => [t.kind, t.title])).toEqual([["warn", "Play halted at tick 1"], ["warn", "Play halted at tick 1"]]);
  // A world that runs is paused as before.
  useUi.setState({ toasts: [] });
  const { session, sent: later } = wire();
  session.call("play.start", {});
  useSession.getState().setStatus(session.status() as Status);
  await togglePause();
  expect(later).toEqual([{ method: "time.control", params: { pause: true } }]);
  expect(useSession.getState().status?.paused).toBe(true);
  expect(useUi.getState().toasts).toEqual([]);
});

test("a part that cannot draw takes the error, and what draws passes through", () => {
  expect(failed(new TypeError("Cannot read properties of null (reading 'slice')")).error?.message).toBe("Cannot read properties of null (reading 'slice')");
  expect(failed("boom").error?.message).toBe("boom");
  expect(Boundary.getDerivedStateFromError(new Error("x")).error).toBeInstanceOf(Error);
  expect(renderToStaticMarkup(createElement(Boundary, { name: "The status bar" }, createElement("b", null, "ok")))).toBe("<b>ok</b>");
});

test("on connect the editor asks for the newest events, not the oldest", async () => {
  const { session, sent } = wire();
  session.call("play.start", {});
  for (let i = 0; i < 20000 && session.events.length < 8; i++) session.stepOnce();
  expect(session.events.length).toBeGreaterThanOrEqual(8);
  const newest = await api.events.since(undefined, 3);
  // No `seq` on the wire: with one (0 included) the host answers the first `limit` after it.
  expect(sent.at(-1)).toEqual({ method: "events.since", params: { limit: 3 } });
  expect(newest.map((e) => e.seq)).toEqual(session.events.slice(-3).map((e) => e.seq));
  const oldest = await api.events.since(0, 3);
  expect(sent.at(-1)).toEqual({ method: "events.since", params: { seq: 0, limit: 3 } });
  expect(oldest.map((e) => e.seq)).toEqual(session.events.slice(0, 3).map((e) => e.seq));
  await api.events.since(oldest.at(-1)!.seq);
  expect(sent.at(-1)).toEqual({ method: "events.since", params: { seq: oldest.at(-1)!.seq } });
});

test("a component without fields is added as {}, one that is a string as text", () => {
  // The host's schema of `Occluder` (`struct Occluder {}`): an object with no properties.
  const occluder: ComponentInfo = { name: "Occluder", origin: "Engine", doc: "Blocks sight.", version: 1, schema: { type: "object", additionalProperties: false } } as ComponentInfo;
  expect(componentFields(occluder)).toEqual([]);
  expect(defaultComponent(occluder)).toEqual({});
  const empty: ComponentInfo = { ...occluder, name: "Marker", schema: { type: "object", properties: {} } } as ComponentInfo;
  expect(defaultComponent(empty)).toEqual({});
  const name: ComponentInfo = { ...occluder, name: "Name", schema: { type: "string" } } as ComponentInfo;
  expect(componentFields(name)).toBeNull();
  expect(defaultComponent(name)).toBe("");
});

test("with the focus in a select, keys with Mod are the editor's and plain keys the select's", () => {
  const select = { tagName: "SELECT" } as unknown as EventTarget;
  const input = { tagName: "INPUT", type: "text" } as unknown as EventTarget;
  expect(ownsKey(select, "Mod+Z")).toBe(false);
  expect(ownsKey(select, "Mod+Shift+Z")).toBe(false);
  expect(ownsKey(select, "ArrowDown")).toBe(true);
  expect(ownsKey(select, "Delete")).toBe(true);
  expect(ownsKey(select, "S")).toBe(true);
  // A text field keeps its own undo.
  expect(ownsKey(input, "Mod+Z")).toBe(true);
  expect(ownsKey({ tagName: "DIV" } as unknown as EventTarget, "Mod+Z")).toBe(false);
  expect(ownsKey(null, "Mod+Z")).toBe(false);
});
