// Clients held on a running host for a while, so the host's game thread can be profiled with them
// connected (docs/bench/profiler.md, "The serve loop"; tools/serve_loop_bench.py drives it):
//
// - `--mode sockets`: the editor's two sockets without a page: `/ws` subscribed to every topic the
//   editor takes, and `/render`; messages and bytes received are counted. `--reads` also sends
//   the editor's reads on each `world.changed` (host/sync.ts): `world.get` of every entity it
//   names and one `world.tree`.
// - `--mode editor`: headless Chrome (WebGPU on) on the editor the host serves, the mouse moved
//   once so the page runs animation frames; what the page sends and receives on its sockets is
//   counted (through CDP's Network domain), and the viewport's stats overlay is read at the end.
//
//   bun tools/serve-clients.ts --url http://127.0.0.1:7878/ --mode sockets|editor --seconds 15
//
// Prints `ready` once the clients are connected (and the viewport draws), then after `--seconds`
// one JSON line: what the clients received. CHROME=<path> picks the browser; CHROME_FLAGS adds
// switches (e.g. --force_high_performance_gpu on a two-GPU laptop).

import { existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { Cdp } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7878/");
const MODE = opt("--mode", "sockets");
const SECONDS = Number(opt("--seconds", "15"));
const READS = args.includes("--reads");
const TOPICS = ["status", "world.changed", "events", "log", "history", "debug", "profile", "agent"];

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

async function sockets() {
  const host = new URL(URL_).host;
  const count = { ws: { messages: 0, bytes: 0, by_topic: {} as Record<string, number> }, render: { messages: 0, bytes: 0 } };
  const ws = new WebSocket(`ws://${host}/ws`);
  const feed = new WebSocket(`ws://${host}/render`);
  feed.binaryType = "arraybuffer";
  let counting = false;
  let id = 10;
  let reads = 0;
  ws.onmessage = (e) => {
    if (!counting) return;
    const text = String(e.data);
    count.ws.messages += 1;
    count.ws.bytes += text.length;
    const m = JSON.parse(text) as { event?: string; data?: { changed?: [number, string][]; spawned?: number[] } };
    const topic = m.event ?? "reply";
    count.ws.by_topic[topic] = (count.ws.by_topic[topic] ?? 0) + 1;
    if (READS && topic === "world.changed" && m.data) {
      const ids = new Set([...(m.data.changed ?? []).map((c) => c[0]), ...(m.data.spawned ?? [])]);
      for (const entity of ids) ws.send(JSON.stringify({ id: ++id, method: "world.get", params: { entity } }));
      ws.send(JSON.stringify({ id: ++id, method: "world.tree", params: {} }));
      reads += ids.size + 1;
    }
  };
  feed.onmessage = (e) => {
    if (!counting) return;
    count.render.messages += 1;
    count.render.bytes += (e.data as ArrayBuffer).byteLength;
  };
  await Promise.all([new Promise((r) => (ws.onopen = r)), new Promise((r) => (feed.onopen = r))]);
  ws.send(JSON.stringify({ id: 1, method: "subscribe", params: { topics: TOPICS } }));
  await Bun.sleep(200);
  counting = true;
  console.log("ready");
  const t0 = performance.now();
  await Bun.sleep(SECONDS * 1000);
  const s = (performance.now() - t0) / 1000;
  ws.close();
  feed.close();
  return {
    mode: "sockets",
    reads,
    seconds: s,
    ws: { ...count.ws, per_s: count.ws.messages / s, bytes_per_s: count.ws.bytes / s },
    render: { ...count.render, per_s: count.render.messages / s, bytes_per_s: count.render.bytes / s },
  };
}

async function editor() {
  const port = 9400 + Math.floor(Math.random() * 400);
  const profile = join(tmpdir(), `pocket-serve-clients-${Date.now()}`);
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
      "--window-size=1600,1000",
      ...(process.env.CHROME_FLAGS ?? "").split(" ").filter(Boolean),
      "about:blank",
    ],
    { stdout: "ignore", stderr: "ignore" },
  );
  try {
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
    if (!pageWs) throw new Error("Chrome did not start");
    const cdp = await Cdp.connect(pageWs);
    const js = async <T>(expression: string): Promise<T> => {
      const r = await cdp.send<{ result: { value?: T } }>("Runtime.evaluate", { expression: `(async () => { ${expression} })()`, awaitPromise: true, returnByValue: true });
      return r.result.value as T;
    };
    await cdp.send("Page.enable");
    await cdp.send("Runtime.enable");
    await cdp.send("Network.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
    // What the page sends on its sockets (requests by method) and receives (pushes by topic),
    // counted from when the clients are ready.
    let counting = false;
    const sent: Record<string, number> = {};
    const received: Record<string, number> = {};
    let renderBytes = 0;
    cdp.on("Network.webSocketFrameSent", (p) => {
      if (!counting) return;
      const data = (p as { response: { payloadData: string } }).response.payloadData;
      let method = "?";
      try {
        method = (JSON.parse(data) as { method?: string }).method ?? "?";
      } catch {
        // binary
      }
      sent[method] = (sent[method] ?? 0) + 1;
    });
    cdp.on("Network.webSocketFrameReceived", (p) => {
      if (!counting) return;
      const r = (p as { response: { opcode: number; payloadData: string } }).response;
      if (r.opcode === 2) {
        received["/render"] = (received["/render"] ?? 0) + 1;
        renderBytes += (r.payloadData.length * 3) / 4;
        return;
      }
      let topic = "?";
      try {
        const m = JSON.parse(r.payloadData) as { event?: string };
        topic = m.event ?? "reply";
      } catch {
        // not JSON
      }
      received[topic] = (received[topic] ?? 0) + 1;
    });
    await cdp.send("Page.navigate", { url: URL_ });
    await cdp.once("Page.loadEventFired");
    await js(`localStorage.clear();`);
    await cdp.send("Page.reload");
    await cdp.once("Page.loadEventFired");
    // Headless Chrome runs no animation frame for the page until its first input event.
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 5, y: 5, button: "none", buttons: 0 });
    let renderer = "";
    for (let i = 0; i < 80 && renderer !== "WebGPU"; i++) {
      await Bun.sleep(250);
      renderer = await js<string>(`return document.querySelector('.vp-renderer')?.textContent ?? '';`);
    }
    console.log("ready");
    counting = true;
    await Bun.sleep(SECONDS * 1000);
    counting = false;
    const overlay = await js<string>(`return document.querySelector('.vp-stats')?.innerText ?? '';`);
    cdp.close();
    return { mode: "editor", sent, received, render_bytes: Math.round(renderBytes), renderer, overlay: overlay.replace(/\s+/g, " ").trim(), chrome_flags: process.env.CHROME_FLAGS ?? "" };
  } finally {
    proc.kill();
  }
}

const report = MODE === "editor" ? await editor() : await sockets();
console.log(JSON.stringify(report));
