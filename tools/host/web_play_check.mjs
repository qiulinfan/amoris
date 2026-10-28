// Plays an exported web game (cargo xtask export-web, docs/spec/web-export.md) in headless Chrome
// with WebGPU and checks it as a player would see it: node tools/host/web_play_check.mjs <folder>
// [--shots DIR] [--seconds S]. The folder is served without cross-origin isolation, as a static
// host (itch.io) serves it. The check waits for the player page to be ready (web/player.js: its
// view ready, the walker's camera known and the models and textures in, and the game run 60 ticks
// since then and since the last warming GPU water job that ran over 250 ms; READY_S= seconds,
// default 120), and fails `ready` at once if the game stops or its world halts (worker status
// `halted`, such as GPU water that failed or timed out). Then, as the seat `player` with the mouse
// captured (the page's `forceCapture`: headless Chrome has no pointer lock), holds W and checks the
// walker moved, turns the view down at the ground, clicks and checks the block in the crosshair
// went, and picks the hotbar's water (slot 8, WATER_SLOT=) and places it, and checks the water
// region's wet area grows as it runs off. It prints one JSON line:
// `{ok, checks: [{name, ok, ...}], frame_ms: {p50, p95, mean}, load: {...}, errors}`; WebGPU
// validation errors and uncaught exceptions fail it. Exit code 0 when every check passed.
// Chrome is found as tools/web_bench.mjs finds it (CHROME= overrides). VIEWPORT=WxH (default
// 1280x720) sets the page's size in CSS pixels; DPR= its device scale factor (default 1); QUERY=
// the page's query (`gpu_minimal=features,limits,timestamps`: WebGPU's baseline device).
// --windward checks the game-controlled camera, startup hold, banking/diving keys, restart,
// pause on release, and a complete ten-gate run by a scripted test pilot instead of voxel edits.
// --no-pointer-lock reproduces embedded browsers that refuse capture; the real click must still
// start a game-camera game with keyboard controls and drag-to-look, without forceCapture.
import { spawn } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";
import { chromeForTesting } from "../chrome_for_testing.mjs";

const args = process.argv.slice(2);
const folder = resolve(args.find((a) => !a.startsWith("--")) || "");
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const shots = opt("--shots", null);
const settle = Number(opt("--seconds", "6"));
const windward = args.includes("--windward");
const emberlight = args.includes("--emberlight");
const noPointerLock = args.includes("--no-pointer-lock");
if (!existsSync(join(folder, "index.html"))) {
  console.error("usage: node tools/host/web_play_check.mjs <exported folder> [--shots DIR] [--seconds S]");
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

const candidates = [...chromeForTesting(), ...({
  darwin: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
  win32: [
    join(process.env.PROGRAMFILES || "C:\\Program Files", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env["PROGRAMFILES(X86)"] || "C:\\Program Files (x86)", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env.LOCALAPPDATA || "", "Google\\Chrome\\Application\\chrome.exe"),
  ],
  linux: ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"],
}[process.platform] || [])];
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
if (noPointerLock) await send("Page.addScriptToEvaluateOnNewDocument", { source:
  'HTMLCanvasElement.prototype.requestPointerLock = function () { return Promise.reject(new DOMException("Unavailable in this embedded browser", "NotSupportedError")); };'
});
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
  // A poisoned world halts without stopping the worker (GPU water that failed or timed out).
  const halted = await evaluate("window.amorisPlayer && window.amorisPlayer.amoris.status && window.amorisPlayer.amoris.status.state === 'halted' ? JSON.stringify(window.amorisPlayer.amoris.status.error || null) : null").catch(() => null);
  if (halted) { note("[halted] " + halted); break; }
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
  if (emberlight) {
    await click("left");
    await sleep(200);
    const active = await evaluate("window.amorisPlayer.input.captured() && !window.amorisPlayer.input.forced && document.getElementById('veil').hidden");
    check("real click starts first-person play", active);
    const before = await world();
    await key("KeyW", 400);
    const after = await world();
    check("keeper walks with the keyboard", Math.hypot(after.walker[0]-before.walker[0], after.walker[2]-before.walker[2]) > 0.1);
    await shot("2-walked");
    // The sample's own puzzle input fixture: real player turns, with travel between rooms elided.
    const fixture = readFileSync(new URL("../../samples/emberlight/checks/inputs.jsonl", import.meta.url), "utf8")
      .trim().split("\n").map(JSON.parse);
    const result = await evaluate(`(async () => {
      const p=window.amorisPlayer, a=p.amoris;
      await a.command("player.pacing", {pacing:{pacing:"stepped"}});
      // Stop input while the deterministic test drives its own controls.
      if(document.pointerLockElement) document.exitPointerLock();
      p.input.release();
      await a.command("snapshots.restore", {tick:0});
      let tick=0;
      for(const call of ${JSON.stringify(fixture)}) {
        if(call.tick>tick+1) { await a.command("time.step", {ticks:call.tick-tick-1}); tick=call.tick-1; }
        await a.command(call.name,call.params, {source:call.source});
      }
      await a.command("time.step", {ticks:600-tick});
      return (await a.command("world.query", {with:["Progress"],fields:["Progress.lamps_lit","Progress.dawn","Progress.finale"]}))[0];
    })()`);
    check("five lanterns and the dawn in the browser", result["Progress.lamps_lit"]===5 && result["Progress.dawn"]===true && result["Progress.finale"]===1,
      {lamps:result["Progress.lamps_lit"],dawn:result["Progress.dawn"],finale:result["Progress.finale"]});
    await sleep(200);
    await shot("3-dawn");
  } else if (windward) {
    const tick = await evaluate("window.amorisPlayer.amoris.command('status').then(s => s.tick)");
    check("holds the start while loading", tick === 1, { tick });
    const flight = () => evaluate(`window.amorisPlayer.amoris.command("world.query", {
      with: ["Flight"], fields: ["Flight.heading", "Flight.pitch", "Transform.position"]
    }).then(r => r[0])`);
    const initial = await flight();
    check("game camera and control legend", await evaluate(`window.amorisPlayer.gameCamera
      && document.getElementById("help").textContent.includes("bank")`));
    await click("left");
    await sleep(300);
    const startedByClick = await evaluate(`(() => { const p = window.amorisPlayer;
      return { captured: p.input.captured(), forced: p.input.forced, unlocked: p.input.unlocked,
        veil: document.getElementById("veil").hidden, pointer: !!document.pointerLockElement };
    })()`);
    check("a real click starts the game", startedByClick.captured && !startedByClick.forced && startedByClick.veil, startedByClick);
    if (noPointerLock) check("keyboard play when pointer lock is refused", startedByClick.unlocked && !startedByClick.pointer);
    await key("KeyA", 600);
    const turned = await flight();
    check("keyboard banks the glider", Math.abs(turned["Flight.heading"] - initial["Flight.heading"]) > 0.05);
    await key("KeyW", 500);
    const dived = await flight();
    check("keyboard dives the glider", dived["Flight.pitch"] < -0.1);
    await shot("2-flight");
    await click("right");
    await sleep(100);
    const restart = await flight();
    check("right click restarts", restart["Transform.position"][0] < 30);
    await evaluate("window.amorisPlayer.input.dx += 120");
    await sleep(120);
    const orbit = await evaluate(`window.amorisPlayer.amoris.command("world.query", {
      with: ["Cam"], fields: ["Cam.orbit_yaw"]
    }).then(r => r[0]["Cam.orbit_yaw"])`);
    check("mouse reaches the game's chase camera", Math.abs(orbit) > 0.1, { orbit });
    if (startedByClick.unlocked) await key("Escape", 50);
    else await evaluate("document.exitPointerLock()");
    await sleep(200);
    const paused = await evaluate("window.amorisPlayer.amoris.command('status').then(s => s.tick)");
    await sleep(500);
    const held = await evaluate("window.amorisPlayer.amoris.command('status').then(s => s.tick)");
    check("release pauses the game", paused === held, { tick: held });
    // A scripted test pilot, distinct from the model recording: exercise every gate in WASM.
    const complete = await evaluate(`(async () => {
      const p = window.amorisPlayer, a = p.amoris;
      const act = actions => a.command("player.act", { actions }, { source: "player" });
      await act([{ do: "pulse", control: "secondary" }]);
      await a.command("time.step", { ticks: 1 });
      const wrap = x => x - Math.PI * 2 * Math.round(x / (Math.PI * 2));
      const gates = await a.command("world.query", { with: ["Gate"], fields: ["Gate.index", "Transform.position"] });
      gates.sort((x,y) => x["Gate.index"] - y["Gate.index"]);
      let course;
      for (let i = 0; i < 1200; i++) {
        course = (await a.command("world.query", { with: ["Course"], fields: ["Course.gates", "Course.medal"] }))[0];
        if (course["Course.gates"] === 10) break;
        const f = (await a.command("world.query", { with: ["Flight"],
          fields: ["Transform.position", "Flight.heading", "Flight.speed", "Flight.boost"] }))[0];
        const pos = f["Transform.position"], target = gates[course["Course.gates"]]["Transform.position"];
        const dx = target[0]-pos[0], dz = target[2]-pos[2], horizontal = Math.hypot(dx,dz);
        const error = wrap(Math.atan2(-dx,-dz) - f["Flight.heading"]);
        let pitch = Math.max(-0.6, Math.min(0.55, Math.atan2(target[1]-pos[1], Math.max(horizontal,30))));
        if (f["Flight.speed"] < 16) pitch = Math.min(pitch,-0.1);
        await act([{ do: "set", controls: { strafe: Math.max(-1,Math.min(1,-error*2.6)),
          forward: Math.max(-1,Math.min(1,-pitch/0.62)), run: f["Flight.boost"]>0.35 && horizontal>150 } }]);
        await a.command("time.step", { ticks: 4 });
      }
      await a.command("time.step", { ticks: 6 });
      return course;
    })()`);
    check("completes all ten gates in the browser", complete["Course.gates"] === 10,
      { gates: complete["Course.gates"], medal: complete["Course.medal"] });
    await sleep(200);
    await shot("3-finish");
  } else {
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
