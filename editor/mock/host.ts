// The mock host: enough of docs/spec/host-protocol.md for the editor to be fully usable before the
// real pocket-server lands. `bun mock/host.ts [--port 7879] [--no-agent] [--dist]`
//
//   GET  /api/catalog   the command catalog
//   POST /api/call      {id?, method, params} -> {id, result} | {id, error}
//   GET  /ws            requests, responses and pushed events (status, world.changed, events, log,
//                       history, debug, profile, agent)
//   GET  /              editor/dist when built (with --dist, or whenever it exists)
//
// Like the real host it binds 127.0.0.1 and refuses a non-loopback Origin or Host.

import type { ServerWebSocket } from "bun";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { CATALOG } from "./catalog";
import { MockAgent } from "./agent";
import { Session } from "./session";
import { MockError } from "./util";

const args = process.argv.slice(2);
const flag = (name: string) => args.includes(name);
const option = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};

const PORT = Number(option("--port", process.env.MOCK_PORT ?? "7879"));
const DIST = resolve(import.meta.dir, "../dist");
const ALL_TOPICS = ["status", "world.changed", "events", "log", "history", "debug", "profile", "agent", "pick"];

interface Client {
  topics: Set<string>;
}

const clients = new Set<ServerWebSocket<Client>>();
// Recent log lines and agent events, replayed to a client that subscribes (the mock's choice, so a
// freshly opened editor is not empty; the protocol does not require it).
const recentLogs: unknown[] = [];
const recentAgent: unknown[] = [];

function broadcast(topic: string, data: unknown) {
  if (topic === "log" || topic === "agent") {
    const ring = topic === "log" ? recentLogs : recentAgent;
    ring.push(data);
    if (ring.length > 300) ring.shift();
  }
  const frame = JSON.stringify({ event: topic, data });
  for (const ws of clients) if (topic === "status" || ws.data.topics.has(topic)) ws.send(frame);
}

const session = new Session(broadcast);
const agent = new MockAgent(session, broadcast);

function answer(id: unknown, method: string, params: unknown) {
  try {
    return { id, result: session.call(method, params) };
  } catch (e) {
    if (e instanceof MockError) return { id, error: e.toWire() };
    console.error(e);
    return { id, error: { code: "host.internal", message: e instanceof Error ? e.message : String(e), detail: {} } };
  }
}

const LOOPBACK = /^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/;

function loopback(req: Request): boolean {
  const host = req.headers.get("host") ?? "";
  const origin = req.headers.get("origin");
  if (!LOOPBACK.test(host)) return false;
  if (origin && origin !== "null") {
    try {
      if (!LOOPBACK.test(new URL(origin).host)) return false;
    } catch {
      return false;
    }
  }
  return true;
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const server = Bun.serve<Client>({
  hostname: "127.0.0.1",
  port: PORT,
  async fetch(req, srv) {
    if (!loopback(req)) return json({ code: "host.not_loopback", message: "Only loopback clients may connect." }, 403);
    const url = new URL(req.url);
    const path = url.pathname;
    if (path === "/ws") {
      if (srv.upgrade(req, { data: { topics: new Set<string>() } })) return undefined;
      return new Response("WebSocket expected", { status: 400 });
    }
    if (path === "/render" || path.startsWith("/devtools")) {
      return json({ code: "host.unsupported", message: "The mock host has no render feed or CDP endpoint; the editor uses its fallback viewport." }, 501);
    }
    if (path === "/api/catalog") return json({ version: 1, host: "mock", commands: CATALOG });
    if (path === "/api/call" && req.method === "POST") {
      let body: { id?: unknown; method?: string; params?: unknown };
      try {
        body = (await req.json()) as typeof body;
      } catch {
        return json({ error: { code: "request.invalid_json", message: "The body is not JSON.", detail: {} } }, 400);
      }
      return json(answer(body.id ?? null, String(body.method ?? ""), body.params));
    }
    if (path === "/mcp") return json({ code: "host.unsupported", message: "The mock host speaks no MCP; its agent is simulated." }, 501);
    if (path.startsWith("/assets/") || path.startsWith("/wasm/")) return new Response("Not found", { status: 404 });
    if (flag("--dist") || existsSync(DIST)) {
      const file = Bun.file(join(DIST, path === "/" ? "index.html" : path));
      if (path !== "/" && (await file.exists())) return new Response(file);
      const index = Bun.file(join(DIST, "index.html"));
      if (await index.exists()) return new Response(index);
    }
    return new Response("aipocket2 mock host: build the editor (bun run build) or run it with vite (bun run dev:mock).", {
      status: 404,
    });
  },
  websocket: {
    open(ws) {
      clients.add(ws);
      ws.send(JSON.stringify({ event: "status", data: session.status() }));
    },
    close(ws) {
      clients.delete(ws);
    },
    message(ws, raw) {
      let msg: { id?: unknown; method?: string; params?: { topics?: string[] } };
      try {
        msg = JSON.parse(String(raw));
      } catch {
        ws.send(JSON.stringify({ id: null, error: { code: "request.invalid_json", message: "The frame is not JSON.", detail: {} } }));
        return;
      }
      if (msg.method === "subscribe") {
        const topics = (msg.params?.topics ?? []).filter((t) => ALL_TOPICS.includes(t));
        const fresh = topics.filter((t) => !ws.data.topics.has(t));
        topics.forEach((t) => ws.data.topics.add(t));
        ws.send(JSON.stringify({ id: msg.id, result: { topics: [...ws.data.topics] } }));
        if (fresh.includes("log")) for (const l of recentLogs) ws.send(JSON.stringify({ event: "log", data: l }));
        if (fresh.includes("agent")) for (const a of recentAgent) ws.send(JSON.stringify({ event: "agent", data: a }));
        if (fresh.includes("history")) ws.send(JSON.stringify({ event: "history", data: session.world.history() }));
        return;
      }
      ws.send(JSON.stringify(answer(msg.id, String(msg.method ?? ""), msg.params)));
    },
  },
});

// The loops: the game at 60 Hz, status and world.changed at 10 Hz, profile at 4 Hz.
setInterval(() => session.frame(), 1000 / 60);
setInterval(() => {
  broadcast("status", session.status());
  const c = session.takeChanges();
  if (c.spawned.size || c.despawned.size || c.changed.size) {
    broadcast("world.changed", {
      tick: session.tick,
      spawned: [...c.spawned],
      despawned: [...c.despawned],
      changed: [...c.changed].map((k) => {
        const [id, comp] = k.split("|");
        return [Number(id), comp];
      }),
    });
  }
}, 100);
setInterval(() => broadcast("profile", session.profile()), 250);

session.log({ level: "info", source: "host", message: `Opened project sailing (${session.world.entities.size} entities, ${session.scripts.files.size} scripts) — mock host` });
session.log({ level: "info", source: "host", message: `Editor WebSocket at ws://127.0.0.1:${PORT}/ws; catalog of ${CATALOG.length} commands` });
session.log({ level: "warn", source: "render", message: "No wasm viewport is served by the mock (/wasm/pocket_web.js); the editor draws its fallback view" });
session.log({ level: "debug", source: "scripts", message: "Type-checked 3 scripts: 0 errors (mock diagnostics)" });

if (!flag("--no-agent")) agent.start();

console.log(`aipocket2 mock host on http://127.0.0.1:${server.port} (ws /ws, catalog /api/catalog)`);
