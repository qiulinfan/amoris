// Real gizmo drags against a real host (docs/bench/gizmo.md; tools/gizmo_bench.py runs it):
// headless Chrome (WebGPU on) opens the editor at --url (the host serving the editor's production
// build made with VITE_AMORIS_EXPOSE=1, or the Vite dev server proxying to the host), selects an
// entity and drags the translate gizmo's x handle with CDP mouse events. The page is instrumented
// before it loads: pointer moves, the `world.edit`s the editor sends and their replies on /ws, the
// render feed's messages on /render, the first requestAnimationFrame callback of every frame (the
// viewport draws what it was pushed in that callback), `history` pushes and long tasks. For each
// pointer move during the drag, the latency from the move's input timestamp (the event's
// `timeStamp`: when Chrome took the CDP input, before it queued the event for the page) to the
// frame that draws the first feed message after the first edit sent after the page's listener saw
// the move: the edit world is paused, so the host pushes a feed message only after a write (the
// idle count checks it). The frame is the requestAnimationFrame callback that draws it; when that
// frame reaches the screen (compositing, presentation) is not measured.
//
// First, without the browser: the host's floor, 40 grouped edits sent from this process over /ws
// (each after the last one's feed message), to the host itself (--host) and to --url's host,
// timing each edit's reply and its feed message on /render. Then the drags: one streamed (the
// catalog's `group`, editor.md 6) and timed, the same drag back timed as an editor without `group`
// does it (previewed locally, sent on release, as before 2026-10-15), both again with a screenshot
// halfway, and a streamed drag cancelled with Escape; last, an edit through the HTTP API (the CLI's
// client) and the History panel marking it. The entity is fetched before it is framed, and the
// camera's orbit is recorded, so every run drags at the same framing.
//
//   amoris serve samples/sailing --port 7890          # AMORIS_EDITOR_DIST=editor/dist to serve it
//   bun tools/gizmo-drag.ts --url http://127.0.0.1:7890/ --out ../out/gizmo/drag \
//     [--host 127.0.0.1:7890] [--entity Crate1] [--moves 90] [--hz 60] [--px 3] [--paced]
//     [--profile]
//
// Mouse moves are dispatched at --hz without waiting for each one's acknowledgement (a real mouse
// does not wait either); `--paced` waits for each, as a slower pointer. `--profile` saves the
// page's JavaScript CPU profile of the timed drag (drag.cpuprofile, for DevTools). CHROME=<path>
// picks the browser; CHROME_FLAGS adds switches (--force_high_performance_gpu on a two-GPU
// laptop). Writes drag.json (samples and summaries), console.log and screenshots into --out.

import { existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { Cdp } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:5190/");
const OUT = resolve(opt("--out", "../out/gizmo/drag"));
const ENTITY = opt("--entity", "Crate1");
const MOVES = Number(opt("--moves", "90"));
const HZ = Number(opt("--hz", "60"));
const PX = Number(opt("--px", "3"));
const HOST = opt("--host", "127.0.0.1:7890");
const PACED = args.includes("--paced");
const PROFILE = args.includes("--profile");
const W = 1600;
const H = 1000;
mkdirSync(OUT, { recursive: true });

function chromePath(): string {
  if (process.env.CHROME) return process.env.CHROME;
  const candidates =
    process.platform === "win32"
      ? [
          join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google\\Chrome\\Application\\chrome.exe"),
          join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Google\\Chrome\\Application\\chrome.exe"),
          join(process.env.LOCALAPPDATA ?? "", "Google\\Chrome\\Application\\chrome.exe"),
        ]
      : process.platform === "darwin"
        ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
        : ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"];
  const found = candidates.find((p) => existsSync(p));
  if (!found) throw new Error(`no Chrome found (tried ${candidates.join(", ")}); set CHROME=<path>`);
  return found;
}

/** Installed before the page's own scripts: timestamps on the page's clock (performance.now()). */
const INSTRUMENT = `(() => {
  const gz = { moves: [], sends: [], replies: [], feeds: [], frames: [], history: [], ids: new Set() };
  window.__gz = gz;
  const Real = window.WebSocket;
  class Instrumented extends Real {
    constructor(url, protocols) {
      super(url, protocols);
      const feed = String(url).includes("/render");
      this.addEventListener("message", (e) => {
        const t = performance.now();
        if (feed) {
          gz.feeds.push({ t, bytes: e.data && e.data.byteLength !== undefined ? e.data.byteLength : 0 });
          return;
        }
        if (typeof e.data !== "string") return;
        if (!e.data.startsWith('{"id"')) {
          if (e.data.includes('"event":"history"')) gz.history.push(t);
          return;
        }
        try {
          const m = JSON.parse(e.data);
          if (gz.ids.has(m.id)) gz.replies.push({ t, id: m.id, ok: !m.error });
        } catch {}
      });
    }
    send(data) {
      if (typeof data === "string" && data.includes('"world.edit"')) {
        try {
          const m = JSON.parse(data);
          gz.ids.add(m.id);
          gz.sends.push({ t: performance.now(), id: m.id, group: m.params.group ?? null, ops: m.params.ops.length });
        } catch {}
      }
      return super.send(data);
    }
  }
  window.WebSocket = Instrumented;
  const raf = window.requestAnimationFrame.bind(window);
  let last = -1;
  window.requestAnimationFrame = (cb) => raf((ts) => {
    if (ts !== last) { last = ts; gz.frames.push(performance.now()); }
    cb(ts);
  });
  window.addEventListener("pointermove", (e) => gz.moves.push({ t: performance.now(), stamp: e.timeStamp, buttons: e.buttons }), true);
  gz.long = [];
  try {
    new PerformanceObserver((l) => { for (const e of l.getEntries()) gz.long.push({ t: e.startTime, ms: e.duration }); }).observe({ type: "longtask", buffered: false });
  } catch {}
})();`;

const port = 9400 + Math.floor(Math.random() * 400);
const profile = join(tmpdir(), `amoris-gizmo-${Date.now()}`);
const proc = Bun.spawn(
  [
    chromePath(),
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    "--enable-unsafe-webgpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--hide-scrollbars",
    `--window-size=${W},${H}`,
    ...(process.env.CHROME_FLAGS ?? "").split(" ").filter(Boolean),
    "about:blank",
  ],
  { stdout: "ignore", stderr: "ignore" },
);
let pageWs = "";
for (let i = 0; i < 100 && !pageWs; i++) {
  try {
    const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
    pageWs = list.find((t) => t.type === "page")?.webSocketDebuggerUrl ?? "";
  } catch {
    // not up yet
  }
  if (!pageWs) await Bun.sleep(100);
}
if (!pageWs) {
  proc.kill();
  throw new Error("Chrome did not start");
}
const cdp = await Cdp.connect(pageWs);

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

async function shot(name: string) {
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

const mouse = (type: "mouseMoved" | "mousePressed" | "mouseReleased", x: number, y: number, down: boolean) =>
  cdp.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" && !down ? "none" : "left", buttons: down ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : 1 });

async function call(method: string, params: unknown = {}): Promise<unknown> {
  const r = (await (await fetch(new URL("/api/call", URL_), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method, params }) })).json()) as {
    result?: unknown;
    error?: { code: string; message: string };
  };
  if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
  return r.result;
}

type Entry = { label: string; source?: unknown; session?: string; group?: string };
type History = { undo: string[]; redo: string[]; entries?: { undo: Entry[]; redo: Entry[] } };
const history = () => call("history.list") as Promise<History>;
const position = async (): Promise<number[]> =>
  ((await call("world.get", { entity: ENTITY, components: ["Transform"] })) as { components: { Transform: { position: number[] } } }).components.Transform.position;

/** A point on the x handle's shaft, in page pixels, that hovering finds (the shaft is tried from
 * 70% of its length inwards and outwards, away from the plane handles near the centre). */
async function handle(): Promise<{ x: number; y: number; ok: boolean }> {
  const line = await js<[number, number, number, number]>(`
    const ctl = window.amorisEditorViewport;
    const pts = ctl.gizmo.shapes.filter((s) => s.handle === "x").flatMap((s) => s.points);
    const view = ctl.view();
    const c = view.project(ctl.gizmo.position);
    const far = pts.map((p) => view.project(p)).sort((a, b) => Math.hypot(b.x - c.x, b.y - c.y) - Math.hypot(a.x - c.x, a.y - c.y))[0];
    const r = document.querySelector(".viewport-surface").getBoundingClientRect();
    return [r.left + c.x, r.top + c.y, r.left + far.x, r.top + far.y];
  `);
  const [cx, cy, fx, fy] = line;
  let last = { x: cx, y: cy };
  for (const f of [0.7, 0.6, 0.8, 0.5, 0.9, 0.45]) {
    const at = { x: cx + (fx - cx) * f, y: cy + (fy - cy) * f };
    last = at;
    await mouse("mouseMoved", at.x, at.y, false);
    await Bun.sleep(60);
    if ((await js<string | null>(`return window.amorisEditorViewport.hoverHandle;`)) === "x") return { ...at, ok: true };
  }
  return { ...last, ok: false };
}

type Gz = {
  /** `t` when the page's listener ran, `stamp` the event's input timestamp (same clock). */
  moves: { t: number; stamp: number; buttons: number }[];
  sends: { t: number; id: number; group: string | null }[];
  replies: { t: number; id: number; ok: boolean }[];
  feeds: { t: number; bytes: number }[];
  frames: number[];
  /** `history` pushes on the editor's socket. */
  history: number[];
  /** Long tasks (50 ms or more) on the page's main thread. */
  long: { t: number; ms: number }[];
};

const reset = () => js(`const g = window.__gz; g.moves = []; g.sends = []; g.replies = []; g.feeds = []; g.frames = []; g.history = []; g.long = []; return 0;`);
const take = () => js<Gz>(`const g = window.__gz; return { moves: g.moves, sends: g.sends, replies: g.replies, feeds: g.feeds, frames: g.frames, history: g.history, long: g.long };`);

const first = <T>(xs: T[], ok: (x: T) => boolean) => xs.find(ok);
function stats(xs: number[]) {
  const s = [...xs].sort((a, b) => a - b);
  const q = (p: number) => (s.length ? s[Math.min(s.length - 1, Math.floor(p * (s.length - 1) + 0.5))]! : null);
  const r = (v: number | null) => (v === null ? null : Math.round(v * 10) / 10);
  return { n: s.length, median: r(q(0.5)), p95: r(q(0.95)), min: r(s[0] ?? null), max: r(s.at(-1) ?? null), mean: r(s.length ? s.reduce((a, b) => a + b, 0) / s.length : null) };
}

/** Per pointer move while the button is down: input -> listener -> send -> (reply) -> feed ->
 * frame. `total_ms` starts at the input timestamp, `from_listener_ms` at the listener (the
 * provisional runs before 2026-10-15's review started there). */
function latencies(g: Gz, releasedAt: number) {
  const rows: { move: number; listener: number; wait: number; host: number; draw: number; total: number; rtt: number | null }[] = [];
  for (const m of g.moves.filter((m) => m.buttons === 1 && m.t < releasedAt)) {
    const send = first(g.sends, (s) => s.t >= m.t);
    if (!send) continue;
    const feed = first(g.feeds, (f) => f.t >= send.t);
    if (!feed) continue;
    const frame = first(g.frames, (f) => f >= feed.t);
    if (frame === undefined) continue;
    const reply = g.replies.find((r) => r.id === send.id);
    const input = Math.min(m.stamp, m.t);
    rows.push({ move: m.t, listener: m.t - input, wait: send.t - m.t, host: feed.t - send.t, draw: frame - feed.t, total: frame - input, rtt: reply ? reply.t - send.t : null });
  }
  return {
    samples: rows.length,
    total_ms: stats(rows.map((r) => r.total)),
    from_listener_ms: stats(rows.map((r) => r.total - r.listener)),
    listener_ms: stats(rows.map((r) => r.listener)),
    wait_ms: stats(rows.map((r) => r.wait)),
    host_ms: stats(rows.map((r) => r.host)),
    draw_ms: stats(rows.map((r) => r.draw)),
    reply_ms: stats(rows.flatMap((r) => (r.rtt === null ? [] : [r.rtt]))),
    rows,
  };
}

/** One drag of the x handle by MOVES moves at HZ, to the right (`dir` 1) or left (-1), so the
 * entity stays in view; `during` runs halfway with the button down. */
async function drag(dir: 1 | -1, during?: () => Promise<void>): Promise<{ g: Gz; pressedAt: number; releasedAt: number; end: { x: number; y: number } }> {
  const h = await handle();
  if (!h.ok) throw new Error("the x handle is not under the pointer");
  await reset();
  await mouse("mousePressed", h.x, h.y, true);
  const pressedAt = await js<number>(`return performance.now();`);
  const step = 1000 / HZ;
  const t0 = performance.now();
  let x = h.x;
  const sent: Promise<unknown>[] = [];
  for (let i = 1; i <= MOVES; i++) {
    x = h.x + dir * i * PX;
    const moved = mouse("mouseMoved", x, h.y, true);
    if (PACED) await moved;
    sent.push(moved);
    if (during && i === Math.floor(MOVES / 2)) {
      await Promise.all(sent);
      await during();
    }
    const wait = t0 + i * step - performance.now();
    if (wait > 0) await Bun.sleep(wait);
  }
  await Promise.all(sent);
  const releasedAt = await js<number>(`return performance.now();`);
  await mouse("mouseReleased", x, h.y, false);
  await Bun.sleep(600);
  return { g: await take(), pressedAt, releasedAt, end: { x, y: h.y } };
}


/** The host's floor without a browser: `n` grouped edits of Crate3 over /ws at `host`, each sent
 * after the last one's feed message; per edit, its reply and its first feed message on /render. */
async function floor(host: string, n = 40) {
  const ws = new WebSocket(`ws://${host}/ws`);
  const feed = new WebSocket(`ws://${host}/render`);
  feed.binaryType = "arraybuffer";
  let feeds: number[] = [];
  feed.onmessage = () => feeds.push(performance.now());
  const replies = new Map<number, (t: number) => void>();
  ws.onmessage = (e) => {
    const m = JSON.parse(String(e.data)) as { id?: number };
    if (m.id !== undefined) replies.get(m.id)?.(performance.now());
  };
  await new Promise((r) => (ws.onopen = r));
  await Bun.sleep(500);
  const reply: number[] = [];
  const toFeed: number[] = [];
  for (let i = 1; i <= n; i++) {
    feeds = [];
    const t0 = performance.now();
    const answered = new Promise<number>((r) => replies.set(i, r));
    ws.send(JSON.stringify({ id: i, method: "world.edit", params: { ops: [{ set: { entity: "Crate3", component: "Transform", value: { "position.1": 0.001 * i } } }], label: "floor", group: "floor" } }));
    reply.push((await answered) - t0);
    while (!feeds.length && performance.now() - t0 < 1000) await Bun.sleep(1);
    if (feeds.length) toFeed.push(feeds[0]! - t0);
    await Bun.sleep(20);
  }
  ws.close();
  feed.close();
  return { edits: n, reply_ms: stats(reply), feed_ms: stats(toFeed) };
}

const result: Record<string, unknown> = { url: URL_, host: HOST, entity: ENTITY, moves: MOVES, hz: HZ, paced: PACED, px_per_move: PX, at: new Date().toISOString() };
const logs: string[] = [];
try {
  cdp.on("Runtime.consoleAPICalled", (p) => {
    const m = p as { type: string; args: { value?: unknown; description?: string }[] };
    logs.push(`[${m.type}] ${m.args.map((a) => String(a.value ?? a.description ?? "")).join(" ")}`);
  });
  cdp.on("Runtime.exceptionThrown", (p) => {
    const e = p as { exceptionDetails: { text: string; exception?: { description?: string } } };
    logs.push(`[exception] ${e.exceptionDetails.text} ${e.exceptionDetails.exception?.description ?? ""}`);
  });
  result.floor = { direct: await floor(HOST), proxied: await floor(new URL(URL_).host) };
  console.log(`floor: ${JSON.stringify(result.floor)}`);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: INSTRUMENT });
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  const target = new URL(URL_);
  target.searchParams.set("viewport", "wasm");
  await cdp.send("Page.navigate", { url: target.toString() });
  await cdp.once("Page.loadEventFired");
  let ready = false;
  // A cold Vite optimizes the editor's dependencies on the first load and reloads the page.
  for (let i = 0; i < 360 && !ready; i++) {
    await Bun.sleep(250);
    ready = await js<boolean>(`
      const p = window.amoris;
      if (!p || !window.amorisEditorViewport?.engine) return false;
      const c = p.connection.getState();
      return c.info.state === "open" && c.viewport === "wasm" && p.world.getState().order.length > 0;
    `);
  }
  if (!ready) {
    const state = await js(`
      const p = window.amoris;
      if (!p) return { amoris: false, title: document.title };
      const c = p.connection.getState();
      return { info: c.info, viewport: c.viewport, engine: !!window.amorisEditorViewport?.engine, entities: p.world.getState().order.length };
    `);
    throw new Error(`the editor did not connect with the wasm viewport: ${JSON.stringify(state)}`);
  }
  // Headless Chrome runs no requestAnimationFrame for this page (the viewport draws nothing and the
  // camera's framing animation stands still) until the first input event; one mouse move in a
  // corner starts its frames.
  await mouse("mouseMoved", 8, H - 8, false);
  const drawing = await js<boolean>(`return await Promise.race([new Promise((r) => requestAnimationFrame(() => r(true))), new Promise((r) => setTimeout(() => r(false), 2000))]);`);
  if (!drawing) throw new Error("the page runs no animation frames");
  result.edit_groups = await js<boolean>(`return window.amoris.connection.getState().editGroups;`);
  if (!result.edit_groups) throw new Error("the host's world.edit has no group: drags would not stream");
  // Select the entity with its components fetched (framing reads its Transform; a selection the
  // editor has not fetched yet would leave the camera where it was), frame it, translate tool in
  // world space without snapping.
  await js(`
    const p = window.amoris;
    const w = p.world.getState();
    const id = w.order.find((i) => w.entities[i]?.name === ${JSON.stringify(ENTITY)});
    p.world.getState().setEntity(await p.api.world.get(id));
    if (!p.world.getState().entities[id]?.components?.Transform) throw new Error("no Transform for " + id);
    p.selection.getState().set([id]);
    p.viewport.getState().set({ tool: "translate", space: "world" });
    p.viewport.getState().setSnap({ enabled: false });
    window.amorisEditorViewport.frameSelection();
    return id;
  `);
  await Bun.sleep(800);
  // From the front, a little above: the x handle runs across the screen.
  await js(`window.amorisEditorViewport.setOrbit({ yaw: 0, pitch: 0.45 }); return 0;`);
  await Bun.sleep(700);
  result.orbit = await js(`
    const o = window.amorisEditorViewport.orbit;
    const w = window.amoris.world.getState();
    const id = w.order.find((i) => w.entities[i]?.name === ${JSON.stringify(ENTITY)});
    return { target: o.target, distance: o.distance, yaw: o.yaw, pitch: o.pitch, entity: w.entities[id].components.Transform.position };
  `);
  {
    const o = result.orbit as { target: number[]; entity: number[] };
    if (Math.hypot(...o.target.map((v, i) => v - o.entity[i]!)) > 0.01) throw new Error(`the camera is not framed on ${ENTITY}: ${JSON.stringify(o)}`);
  }
  result.renderer = await js(`return window.amorisEditorViewport.engine.stats?.().renderer ?? null;`);
  // Which GPU Chrome's WebGPU picks for the request the viewport makes (high performance, as
  // amoris-render's gpu.rs asks for it).
  result.browser = await js(`
    const a = await navigator.gpu?.requestAdapter({ powerPreference: "high-performance" });
    const i = a?.info ?? {};
    return { user_agent: navigator.userAgent, adapter: { vendor: i.vendor, architecture: i.architecture, device: i.device, description: i.description } };
  `);
  // Feed messages while nothing is edited (the edit world is paused): attribution needs none.
  await reset();
  await Bun.sleep(1000);
  result.idle_feeds_per_s = (await take()).feeds.length;
  await shot("01-before");

  // 1. Streamed and timed (no screenshot during it): one entry, the object follows the pointer.
  const page = (d: Awaited<ReturnType<typeof drag>>) => ({
    frames: d.g.frames.length,
    drag_ms: Math.round(d.releasedAt - d.pressedAt),
    moves_seen: d.g.moves.filter((m) => m.buttons === 1).length,
    long_tasks: d.g.long.length,
    long_task_ms: Math.round(d.g.long.reduce((a, l) => a + l.ms, 0)),
    history_pushes: d.g.history.length,
    frame_interval_ms: stats(d.g.frames.slice(1).map((f, i) => f - d.g.frames[i]!)),
  });
  const h0 = await history();
  const p0 = await position();
  if (PROFILE) {
    await cdp.send("Profiler.enable");
    await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
    await cdp.send("Profiler.start");
  }
  const streamed = await drag(1);
  if (PROFILE) {
    const { profile } = await cdp.send<{ profile: unknown }>("Profiler.stop");
    await Bun.write(join(OUT, "drag.cpuprofile"), JSON.stringify(profile));
  }
  const h1 = await history();
  const p1 = await position();
  result.streamed = {
    undo_before: h0.undo.length,
    undo_after: h1.undo.length,
    last_entry: h1.entries?.undo.at(-1) ?? null,
    position_before: p0,
    position_after: p1,
    sends: streamed.g.sends.length,
    feeds: streamed.g.feeds.length,
    ...page(streamed),
    latency: latencies(streamed.g, streamed.releasedAt),
  };
  await shot("02-after-streamed");

  // 2. The same drag back, timed, as an editor without `group` does it: previewed locally and sent
  // once on release (the gizmo moves, the object does not until the release).
  await js(`window.amoris.connection.getState().set({ editGroups: false }); return 0;`);
  const before = (await history()).undo.length;
  const local = await drag(-1);
  await js(`window.amoris.connection.getState().set({ editGroups: true }); return 0;`);
  const firstFeed = local.g.feeds[0]?.t ?? null;
  const firstMove = local.g.moves.find((m) => m.buttons === 1)?.t ?? null;
  result.on_release = {
    sends: local.g.sends.length,
    feeds: local.g.feeds.length,
    first_move_to_first_feed_ms: firstFeed !== null && firstMove !== null ? Math.round(firstFeed - firstMove) : null,
    release_to_first_feed_ms: firstFeed !== null ? Math.round(firstFeed - local.releasedAt) : null,
    undo_added: (await history()).undo.length - before,
    ...page(local),
  };

  // Evidence: both again with the button held for a screenshot halfway (streamed, the viewport
  // draws the object where the host has it; on release, only the gizmo has moved).
  await drag(1, async () => {
    await Bun.sleep(200);
    await shot("03-mid-drag-streamed");
  });
  await js(`window.amoris.connection.getState().set({ editGroups: false }); return 0;`);
  await drag(-1, async () => {
    await Bun.sleep(200);
    await shot("04-mid-drag-on-release");
  });
  await js(`window.amoris.connection.getState().set({ editGroups: true }); return 0;`);

  // 3. Escape during a streamed drag: the whole drag undone, nothing left in the history.
  const h2 = await history();
  const p2 = await position();
  await drag(1, async () => {
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  });
  await Bun.sleep(400);
  const h3 = await history();
  const p3 = await position();
  // The drag moves to the redo stack (Redo brings it back); its own entry is the next to redo.
  result.escape = {
    undo_before: h2.undo.length,
    undo_after: h3.undo.length,
    redo_after: h3.redo.length,
    next_redo: h3.entries?.redo?.at(-1) ?? null,
    position_before: p2,
    position_after: p3,
    restored: JSON.stringify(p2) === JSON.stringify(p3),
  };

  // 4. Another client's edit: the HTTP API's (the CLI's), marked in the History panel.
  await call("world.edit", { ops: [{ set: { entity: "Crate2", component: "Cargo", value: { value: 5 } } }], label: "raise Crate2's cargo" });
  await Bun.sleep(300);
  await js(`window.amoris.openPanel("history"); return 0;`);
  await Bun.sleep(400);
  result.history_panel = await js<string>(`return document.querySelector(".history-panel")?.innerText ?? "";`);
  result.history = await history();
  await shot("05-history-panel");
  // A page whose wasm panicked (or threw) is not a valid measurement: the table leaves it out.
  result.page_errors = logs.filter((l) => /panicked|RuntimeError|\[exception\]/.test(l)).length;
  await Bun.write(join(OUT, "drag.json"), `${JSON.stringify(result, null, 2)}\n`);
  const s = result.streamed as { undo_before: number; undo_after: number; latency: ReturnType<typeof latencies> };
  console.log(`streamed: undo ${s.undo_before} -> ${s.undo_after}; input->frame ${JSON.stringify(s.latency.total_ms)}`);
  console.log(`on release: ${JSON.stringify(result.on_release)}`);
  console.log(`escape: ${JSON.stringify(result.escape)}`);
  if (result.page_errors) console.log(`page errors: ${result.page_errors} (see console.log)`);
} finally {
  await Bun.write(join(OUT, "console.log"), logs.map((l) => `${l}\n`).join(""));
  cdp.close();
  proc.kill();
}
