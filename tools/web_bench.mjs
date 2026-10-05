// Measures a page's frame times in Google Chrome (headless, WebGPU on), driven over the Chrome
// DevTools Protocol with Node's built-in WebSocket: node tools/web_bench.mjs <url> [seconds]
// [width] [height]. The page publishes `window.pocketSamples` ({frame_ms, gpu_ms, backend}).
// A temporary profile is used; the user's Chrome profile is never touched.
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [url, seconds = "10", width = "1280", height = "720"] = process.argv.slice(2);
const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const profile = mkdtempSync(join(tmpdir(), "pocket-bench-"));
const port = 9300 + Math.floor(Math.random() * 600);
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--disable-frame-rate-limit",
  "--disable-gpu-vsync", `--window-size=${width},${height}`, "--no-first-run", "--no-default-browser-check", "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    target = list.find((t) => t.type === "page");
  } catch {}
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const logs = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled") logs.push(`[${m.params.type}] ` + m.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
  if (m.method === "Runtime.exceptionThrown") logs.push("[exception] " + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
await send("Page.enable");
await send("Runtime.enable");
// INIT prepares a reproducible temporary-profile state before application modules read it.
if (process.env.INIT) await send("Page.addScriptToEvaluateOnNewDocument", { source: process.env.INIT });
await send("Page.navigate", { url });
await sleep(Number(seconds) * 1000);
// CLICK="x,y[;x,y...]": mouse clicks (CSS pixels) before measuring and the screenshot.
for (const c of (process.env.CLICK || "").split(";").filter(Boolean)) {
  const [x, y] = c.split(",").map(Number);
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
    await sleep(60);
  }
  await sleep(800);
}
const r = await send("Runtime.evaluate", { returnByValue: true, expression: `(() => {
  const s = (window.pocketSamples || []).slice(-300);
  const ft = s.map((x) => x.frame_ms).sort((a, b) => a - b);
  const mean = (a) => a.reduce((x, y) => x + y, 0) / Math.max(a.length, 1);
  return { url: location.href, frames: s.length, frame_ms_mean: mean(ft), frame_ms_p50: ft[Math.floor(ft.length / 2)],
    frame_ms_p95: ft[Math.floor(ft.length * 0.95)], gpu_ms_mean: mean(s.map((x) => x.gpu_ms || 0)),
    backend: s.length ? s[0].backend : null, passes: s.length ? s[s.length - 1].passes : null,
    canvas: (() => { const c = document.querySelector("canvas"); return c ? [c.width, c.height] : null; })(),
    gpu: navigator.gpu ? "yes" : "no" };
})()` });
console.log(JSON.stringify(r.result.result.value, null, 2));
if (process.env.EVAL) {
  const e = await send("Runtime.evaluate", { expression: process.env.EVAL, awaitPromise: true, returnByValue: true });
  console.log("eval:", JSON.stringify(e.result.result.value ?? e.result.exceptionDetails ?? e.result.result));
}
if (process.env.SHOT) {
  const shot = await send("Page.captureScreenshot", { format: "png" });
  (await import("node:fs")).writeFileSync(process.env.SHOT, Buffer.from(shot.result.data, "base64"));
  console.log("screenshot:", process.env.SHOT);
}
if (process.env.LOGS) for (const l of logs.slice(-40)) console.log(l.slice(0, 400));
ws.close();
proc.kill();
if (proc.exitCode === null && proc.signalCode === null) {
  await Promise.race([new Promise((resolve) => proc.once("exit", resolve)), sleep(2000)]);
}
rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
