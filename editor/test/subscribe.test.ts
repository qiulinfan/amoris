import { expect, test } from "bun:test";
import { HostClient } from "../src/host/client";
import type { Topic } from "../src/host/protocol";

/** A WebSocket that answers `subscribe` as the host does (pocket-server ws.rs): all or nothing,
 * an unknown topic refused with its index. */
class FakeHostSocket {
  static readonly OPEN = 1;
  static last: FakeHostSocket | null = null;
  readonly pushes = ["status", "world.changed", "events", "log", "history", "debug", "profile", "agent"];
  readonly asked: string[][] = [];
  readonly subscribed = new Set<string>();
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(readonly url: string) {
    FakeHostSocket.last = this;
    queueMicrotask(() => {
      this.readyState = FakeHostSocket.OPEN;
      this.onopen?.();
    });
  }

  send(raw: string) {
    const msg = JSON.parse(raw) as { id: number; method: string; params: { topics: string[] } };
    const topics = msg.params.topics;
    this.asked.push(topics);
    const bad = topics.findIndex((t) => !this.pushes.includes(t));
    const reply =
      bad >= 0
        ? { id: msg.id, error: { code: "request.invalid_value", message: `'${topics[bad]}' is not a topic`, detail: { path: `/topics/${bad}`, got: topics[bad] } } }
        : (topics.forEach((t) => this.subscribed.add(t)), { id: msg.id, result: { topics: [...this.subscribed] } });
    queueMicrotask(() => this.onmessage?.({ data: JSON.stringify(reply) }));
  }

  close() {}
}

test("a topic the host does not push is dropped and the rest are subscribed", async () => {
  const real = globalThis.WebSocket;
  const warn = console.warn;
  const warned: string[] = [];
  console.warn = (...a: unknown[]) => void warned.push(a.join(" "));
  (globalThis as { WebSocket: unknown }).WebSocket = FakeHostSocket;
  try {
    // An editor that asks for a topic this host never pushes (as `pick` once was).
    const client = new HostClient({ http: "", ws: "ws://fake/ws", label: "fake" }, ["status", "pick" as Topic, "profile", "log"]);
    client.connect();
    for (let i = 0; i < 20 && FakeHostSocket.last?.subscribed.size !== 3; i++) await Bun.sleep(1);
    const ws = FakeHostSocket.last!;
    expect(ws.asked).toEqual([
      ["status", "pick", "profile", "log"],
      ["status", "profile", "log"],
    ]);
    expect([...ws.subscribed].sort()).toEqual(["log", "profile", "status"]);
    expect(warned.some((w) => w.includes("'pick'"))).toBe(true);
  } finally {
    (globalThis as { WebSocket: unknown }).WebSocket = real;
    console.warn = warn;
  }
});
