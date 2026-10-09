// Validates the splat WGSL in Chrome's WebGPU (Tint) at WebGPU's default limits:
//   python tools/splats/wgsl_check/compose.py . tools/splats/wgsl_check/shaders.js
//   python3 -m http.server -d tools/splats/wgsl_check 8731 --bind 127.0.0.1   (another shell)
//   node tools/splats/wgsl_check/check.mjs http://127.0.0.1:8731/index.html
// Opens the URL in headless Chrome (a temporary profile; the user's is never touched) over the
// DevTools protocol and prints the JSON of `await window.result` (index.html). CHROME overrides the
// browser's path.
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const url = process.argv[2];
const chrome =
  process.env.CHROME ??
  {
    win32: "C:/Program Files/Google/Chrome/Application/chrome.exe",
    darwin: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  }[process.platform] ??
  "google-chrome";
const profile = mkdtempSync(join(tmpdir(), "splat-wgsl-"));
const port = 9900 + Math.floor(Math.random() * 90);
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--enable-unsafe-webgpu", "--no-first-run", "--no-default-browser-check", "about:blank",
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
await send("Page.enable");
await send("Page.navigate", { url });
await sleep(3000);
const r = await send("Runtime.evaluate", {
  expression: "window.result.then((x) => JSON.stringify(x, null, 1))",
  awaitPromise: true,
  returnByValue: true,
});
console.log(r.result?.result?.value ?? JSON.stringify(r));
ws.close();
proc.kill();
await sleep(500);
try {
  rmSync(profile, { recursive: true, force: true });
} catch {}
