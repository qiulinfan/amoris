// Plays an exported web game (cargo xtask export-web, docs/spec/web-export.md) in headless Chrome
// with WebGPU and checks it as a player would see it: node tools/web_play_check.mjs <folder>
// [--shots DIR] [--seconds S]. The folder is served without cross-origin isolation, as a static
// host (itch.io) serves it. The check waits for the player page to be ready (the walker's camera
// known, the models and textures in), then, as the seat `player` with the mouse captured (the
// page's `forceCapture`: headless Chrome has no pointer lock), holds W and checks the walker moved,
// turns the view down at the ground, clicks and checks the block in the crosshair went, and
// picks the hotbar's water (slot 8, WATER_SLOT=) and places it, and checks the water region's
// wet area grows as it runs off. It prints one JSON line:
// `{ok, checks: [{name, ok, ...}], frame_ms: {p50, p95, mean}, load: {...}, errors}`; WebGPU
// validation errors and uncaught exceptions fail it. Exit code 0 when every check passed.
// Chrome is found as tools/web_bench.mjs finds it (CHROME= overrides). VIEWPORT=WxH (default
// 1280x720) sets the page's size in CSS pixels; DPR= its device scale factor (default 1); QUERY=
// the page's query (`gpu_minimal=features,limits,timestamps`: WebGPU's baseline device).
import { spawn } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";

const args = process.argv.slice(2);
const folder = resolve(args.find((a) => !a.startsWith("--")) || "");
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const shots = opt("--shots", null);
const settle = Number(opt("--seconds", "6"));
if (!existsSync(join(folder, "index.html"))) {
  console.error("usage: node tools/web_play_check.mjs <exported folder> [--shots DIR] [--seconds S]");
  process.exit(2);
}
if (shots) mkdirSync(shots, { recursive: true });

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json",
  ".png": "image/png", ".jpg": "image/jpeg", ".gltf": "model/gltf+json", ".glb": "model/gltf-binary", ".bin": "application/octet-stream" };
const server = createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
  let file = join(folder, path || "index.html");
  if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
  if (!file.startsWith(folder) || !existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const url = `http://127.0.0.1:${server.address().port}/${process.env.QUERY ? "?" + process.env.QUERY : ""}`;

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
const profile = mkdtempSync(join(tmpdir(), "amoris-play-"));
const port = 9300 + Math.floor(Math.random() * 600);
const [vw, vh] = (process.env.VIEWPORT || "1280x720").split("x").map(Number);
// WebGPU as a player's Chrome has it (no --enable-unsafe-webgpu), held to the display's rate.
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  ...(process.platform === "win32" ? [] : ["--enable-features=Vulkan"]),
  `--window-size=${vw},${vh}`, "--no-first-run", "--no-default-browser-check",
  ...(process.env.CHROME_FLAGS || "").split(" ").filter(Boolean), "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try {
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page");
  } catch {}
}
let browser = null;
try { browser = (await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).Browser; } catch {}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const errors = [];
const seen = new Set();
const note = (line) => {
  if (seen.has(line)) return;
  seen.add(line);
  if (errors.length < 20) errors.push(line.slice(0, 1200));
};
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled") {
    const text = m.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
    if (m.params.type === "error" || /ERROR|Validation|panicked/.test(text)) note(text);
  }
  if (m.method === "Runtime.exceptionThrown") {
    note("[exception] " + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
  }
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text);
  return r.result.result.value;
};
const shot = async (name) => {
  if (!shots) return;
  const s = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(shots, `${name}.png`), Buffer.from(s.result.data, "base64"));
};
const key = async (code, ms) => {
  const k = { code, key: code.replace(/^Key/, "").toLowerCase(), windowsVirtualKeyCode: code.startsWith("Key") ? code.charCodeAt(3) : 0 };
  await send("Input.dispatchKeyEvent", { type: "keyDown", ...k });
  await sleep(ms);
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...k });
};
const click = async (button) => {
  const at = { x: Math.round(vw / 2), y: Math.round(vh / 2), button, clickCount: 1 };
  await send("Input.dispatchMouseEvent", { type: "mousePressed", ...at });
  await sleep(40);
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", ...at });
};

await send("Page.enable");
await send("Runtime.enable");
await send("Emulation.setDeviceMetricsOverride", { width: vw, height: vh, deviceScaleFactor: Number(process.env.DPR || 1), mobile: false });
const started = Date.now();
await send("Page.navigate", { url });

const checks = [];
const check = (name, ok, extra = {}) => checks.push({ name, ok: Boolean(ok), ...extra });
// What the world says (developer `world.get`s answered by the game worker): the walker's position,
// the water region's wet cells and their total depth, and the block in the view's crosshair.
const world = async () => evaluate(`(async () => {
  const p = window.amorisPlayer;
  const a = p.amoris;
  const q = await a.command("world.query", { with: ["Walker"] }).catch((e) => ({ error: e.message }));
  const walker = Array.isArray(q) ? q[0] : null;
  const w = walker ? await a.command("world.get", { entity: walker.id, components: ["Transform"] }) : null;
  const wq = await a.command("world.query", { with: ["WaterRegion"] }).catch(() => ({}));
  let water = null;
  if (Array.isArray(wq) && wq.length) {
    const r = (await a.command("world.get", { entity: wq[0].id, components: ["WaterRegion"] })).components.WaterRegion;
    let wet = 0, depth = 0;
    for (const t of r.tiles || []) for (const d of t.depth || []) if (d > r.dry_below) { wet++; depth += d; }
    water = { wet, depth: Number(depth.toFixed(3)) };
  }
  return { walker: w ? w.components.Transform.position : null, query: q.error || null, water,
    target: Array.from(p.viewport.raw.voxel_target(5)) };
})()`);

let ready = null;
for (let i = 0; i < Number(process.env.READY_S || 120) * 2 && !ready; i++) {
  await sleep(500);
  ready = await evaluate("window.amorisPlayer && window.amorisPlayer.ready_ms ? window.amorisPlayer.ready_ms : null").catch(() => null);
  const stopped = await evaluate("window.amorisPlayer && window.amorisPlayer.amoris.stopped && window.amorisPlayer.amoris.stopped.message").catch(() => null);
  if (stopped) { note("[stopped] " + stopped); break; }
}
const waited = ready ? null : await evaluate(`(() => { const p = window.amorisPlayer; if (!p) return null;
  const s = p.stats || {}; return { frames: p.frames, look: p.look, pending_assets: s.pending_assets, instances: s.instances,
  entities: s.entities, tick: s.tick, latest: p.amoris.latest() }; })()`).catch((e) => String(e));
check("ready", ready, { ready_ms: ready, wall_ms: Date.now() - started, ...(waited ? { waited } : {}) });
if (ready) {
  await sleep(settle * 1000);
  await shot("1-ready");
  const veil = await evaluate("!document.getElementById('veil').hidden && document.getElementById('status').textContent");
  check("click to play shown", veil === "Click to play", { status: veil });
  await evaluate("window.amorisPlayer.forceCapture(true)");
  await sleep(300);
  const before = await world();
  await key("KeyW", 1500);
  await sleep(500);
  const after = await world();
  const moved = before.walker && after.walker ? Math.hypot(after.walker[0] - before.walker[0], after.walker[2] - before.walker[2]) : 0;
  check("walks forward", moved > 1, { moved_m: Number(moved.toFixed(2)), from: before.walker, to: after.walker });
  await shot("2-walked");
  // Look at the ground at the walker's feet, then break the block in the crosshair.
  await evaluate("window.amorisPlayer.input.dy += 400");
  await sleep(500);
  const aimed = await world();
  await click("left");
  await sleep(800);
  const broke = await world();
  const changed = aimed.target.length && (!broke.target.length || broke.target.some((v, i) => v !== aimed.target[i]));
  check("breaks a block", changed, { target_before: aimed.target, target_after: broke.target });
  await shot("3-broke");
  // Place a block back where it went (right button), which hits the cell below it again.
  await click("right");
  await sleep(800);
  const placed = await world();
  check("places a block", placed.target.length && broke.target.some((v, i) => v !== placed.target[i]), { target_after: placed.target });
  // The water: pour a water block (the hotbar's water slot) on the ground and watch the region's
  // wet area grow as it runs off.
  await key(`Digit${process.env.WATER_SLOT || "8"}`, 60);
  await sleep(200);
  const dry = await world();
  await click("right");
  const flow = [];
  for (let i = 0; i < 6; i++) {
    await sleep(500);
    flow.push((await world()).water);
  }
  await shot("4-water");
  const wet = flow.map((f) => f && f.wet);
  check("water pours and spreads", dry.water && wet[0] > dry.water.wet && wet[wet.length - 1] > wet[0], {
    before: dry.water, after: flow });
  const frames = await evaluate(`(() => { const s = window.amorisPlayer.samples.slice(-300).sort((a, b) => a - b);
    const st = window.amorisPlayer.stats; return { n: s.length, p50: s[Math.floor(s.length / 2)], p95: s[Math.floor(s.length * 0.95)],
    mean: s.reduce((a, b) => a + b, 0) / Math.max(1, s.length), gpu_ms: st.gpu_ms, render_ms: st.render_ms, instances: st.instances, draw_path: st.draw_path,
    backend: st.backend, canvas: [document.getElementById('view').width, document.getElementById('view').height] }; })()`);
  check("draws frames", frames.n > 30, frames);
  // The game worker's tick timings (amoris-web's perf: the loop's work per tick in WebAssembly).
  globalThis.perf = await evaluate(`window.amorisPlayer.amoris.perf().then((p) => {
    const q = (a) => { const s = a.slice(-600).sort((x, y) => x - y); return s.length ? { p50: s[Math.floor(s.length / 2)],
      p95: s[Math.floor(s.length * 0.95)], max: s[s.length - 1] } : null; };
    return { ticks: p.ticks, step_ms: q(p.step_ms), publish_ms: q(p.publish_ms) }; })`).catch((e) => String(e));
  globalThis.frames = frames;
}
check("no WebGPU or script errors", errors.length === 0);
const load = await evaluate(`(() => { const e = performance.getEntriesByType("resource");
  return { requests: e.length, transferred: e.reduce((a, r) => a + (r.transferSize || 0), 0),
    loaded_ms: window.amorisPlayer && window.amorisPlayer.loaded_ms }; })()`).catch(() => null);
const ok = checks.every((c) => c.ok);
console.log(JSON.stringify({ ok, checks, frame_ms: globalThis.frames || null, perf: globalThis.perf || null, load, errors, browser }));
ws.close();
proc.kill();
server.close();
await sleep(500);
rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
process.exit(ok ? 0 : 1);
