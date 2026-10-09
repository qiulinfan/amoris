// node drive.mjs URL OUTDIR: runs the harness page (Cargo.toml says how to build and serve it) in
// headless Chrome with a temporary profile, screenshots the canvas after 90 frames of quads
// (OUTDIR/web_quad.png) and after 90 frames of tiles (web_tile.png), and prints each mode's last
// pass timings and splat stats. CHROME overrides the browser's path.
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [url, out] = process.argv.slice(2);
const chrome =
  process.env.CHROME ??
  {
    win32: "C:/Program Files/Google/Chrome/Application/chrome.exe",
    darwin: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  }[process.platform] ??
  "google-chrome";
const profile = mkdtempSync(join(tmpdir(), "splat-web-"));
const port = 9800 + Math.floor(Math.random() * 90);
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--enable-unsafe-webgpu", "--enable-features=WebGPU", "--window-size=1280,720",
  "--force-device-scale-factor=1", "--no-first-run", "--no-default-browser-check", "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 75 && !target; i++) {
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
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) {
    pending.get(d.id)(d);
    pending.delete(d.id);
  }
};
const send = (method, params = {}) =>
  new Promise((r) => {
    const i = ++id;
    pending.set(i, r);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
const evaluate = async (expression) =>
  (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;
const waitPhase = async (want) => {
  for (let i = 0; i < 300; i++) {
    const p = await evaluate("window.phase");
    if (p === want || String(p).startsWith("failed")) return p;
    await sleep(200);
  }
  return "timeout";
};
const shot = async (name) => {
  const r = await send("Page.captureScreenshot", { format: "png", clip: { x: 0, y: 0, width: 1280, height: 720, scale: 1 } });
  writeFileSync(join(out, name), Buffer.from(r.result.data, "base64"));
};
await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url });
console.log("phase:", await waitPhase("quads"));
await shot("web_quad.png");
await evaluate("window.next()");
console.log("phase:", await waitPhase("tiles"));
await shot("web_tile.png");
console.log("quads:", await evaluate("window.quads"));
console.log("tiles:", await evaluate("window.tiles"));
console.log("logs:", JSON.stringify(await evaluate("window.logs"), null, 1));
ws.close();
proc.kill();
await sleep(500);
try {
  rmSync(profile, { recursive: true, force: true });
} catch {}
