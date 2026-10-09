// Measures a page's frame times in Google Chrome (headless, WebGPU on), driven over the Chrome
// DevTools Protocol with Node's built-in WebSocket: node tools/web_bench.mjs <url> [seconds]
// [width] [height]. The page publishes `window.pocketSamples` ({frame_ms, gpu_ms, backend}).
// A temporary profile is used; the user's Chrome profile is never touched. Chrome is found at its
// default install path on macOS, Windows and Linux, or at `CHROME=<path>`. `CHROME_FLAGS="..."`
// adds switches (e.g. `--force_high_performance_gpu`); `VSYNC=1` keeps Chrome's vsync and frame-rate
// limit, which the page otherwise runs without.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [url, seconds = "10", width = "1280", height = "720"] = process.argv.slice(2);
const candidates = {
  darwin: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
  win32: [
    join(process.env.PROGRAMFILES || "C:\\Program Files", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env["PROGRAMFILES(X86)"] || "C:\\Program Files (x86)", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env.LOCALAPPDATA || "", "Google\\Chrome\\Application\\chrome.exe"),
  ],
  linux: ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"],
}[process.platform] || [];
const chrome = process.env.CHROME || candidates.find((p) => existsSync(p));
if (!chrome) throw new Error(`no Chrome found (tried ${candidates.join(", ")}); set CHROME=<path>`);
const profile = mkdtempSync(join(tmpdir(), "pocket-bench-"));
const port = 9300 + Math.floor(Math.random() * 600);
// Chrome's WebGPU runs on Metal (macOS), D3D12 (Windows) or Vulkan (Linux). The Vulkan feature
// switch (kept from the macOS runs) is left out on Windows, where it would move Chrome's own
// compositing to Vulkan beside Dawn's D3D12.
const platformFlags = process.platform === "win32" ? [] : ["--enable-features=Vulkan"];
// The frame loop runs uncapped (no vsync, no frame-rate limit) unless VSYNC=1.
const uncapped = process.env.VSYNC === "1" ? [] : ["--disable-frame-rate-limit", "--disable-gpu-vsync"];
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--enable-unsafe-webgpu", ...platformFlags, ...uncapped,
  `--window-size=${width},${height}`, "--no-first-run", "--no-default-browser-check",
  ...(process.env.CHROME_FLAGS || "").split(" ").filter(Boolean), "about:blank",
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
    render_ms_mean: mean(s.map((x) => x.render_ms || 0)),
    backend: s.length ? s[0].backend : null, passes: s.length ? s[s.length - 1].passes : null,
    draw_path: s.length ? s[s.length - 1].draw_path : null, draw_calls: s.length ? s[s.length - 1].draw_calls : null,
    instances: s.length ? s[s.length - 1].instances : null,
    occlusion: s.length ? s[s.length - 1].occlusion : null, occluded: s.length ? s[s.length - 1].occluded : null,
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
