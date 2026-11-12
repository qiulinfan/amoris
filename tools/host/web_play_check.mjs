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
// --third-person checks a third_person seat (docs/spec/player.md 5.1, samples/thornkeep): it walks
// and sprints along the camera, the mouse orbits the camera, the wheel zooms, the right button
// guards, C dodges, the left button acts and Q locks on to the nearest foe in front, which turns
// the camera to it; the drawn camera stays near the world's (its lag is the page's alone).
// --no-pointer-lock reproduces embedded browsers that refuse capture; the real click must still
// start a game-camera game with keyboard controls and drag-to-look, without forceCapture.
// --gamepad plays a first-person game with a synthetic standard gamepad (navigator.getGamepads
// replaced before the page loads; docs/spec/input.md 7): a button starts play without the lock and
// without jumping, the left stick walks, the right stick turns the look, South jumps and Start
// pauses. --touch plays it with touch events (CDP's touch emulation): a tap on the veil starts play
// and shows the touch controls, the touch stick walks, a drag on the game turns the look, the jump
// button jumps and the pause button pauses (input.md 5).
// --ui presses the game UI's buttons (docs/spec/ui.md 6, tests/fixtures/scenes/ui-hud): the viewport
// finds every enabled button and no disabled one, a mouse click and a touch tap on a button through
// the veil each send one `ui.press` that the world counts, a tap on the disabled one presses
// nothing, and none of it captures the pointer.
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
const thirdPerson = args.includes("--third-person");
const noPointerLock = args.includes("--no-pointer-lock");
const gamepad = args.includes("--gamepad");
const touchscreen = args.includes("--touch");
const uiPresses = args.includes("--ui");
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
const keys = async (codes, ms) => {
  const k = (code) => ({ code, key: code.replace(/^Key/, "").toLowerCase(), windowsVirtualKeyCode: code.startsWith("Key") ? code.charCodeAt(3) : 0 });
  for (const c of codes) await send("Input.dispatchKeyEvent", { type: "keyDown", ...k(c) });
  await sleep(ms);
  for (const c of codes) await send("Input.dispatchKeyEvent", { type: "keyUp", ...k(c) });
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
// A standard gamepad the check holds: window.__pad.set({0: 1}, [0, -1, 0, 0]).
if (gamepad) await send("Page.addScriptToEvaluateOnNewDocument", { source: `(() => {
  const pad = { id: "Amoris check pad (STANDARD GAMEPAD)", index: 0, connected: true, mapping: "standard",
    timestamp: 0, axes: [0, 0, 0, 0], vibrationActuator: null,
    buttons: Array.from({ length: 17 }, () => ({ pressed: false, touched: false, value: 0 })) };
  Object.defineProperty(navigator, "getGamepads", { value: () => [pad, null, null, null] });
  window.__pad = { pad, set(buttons = {}, axes = [0, 0, 0, 0]) {
    pad.buttons.forEach((b, i) => { const v = buttons[i] || 0; b.value = v; b.pressed = b.touched = v > 0; });
    pad.axes = axes.slice();
    pad.timestamp = performance.now();
  } };
  addEventListener("load", () => dispatchEvent(Object.assign(new Event("gamepadconnected"), { gamepad: pad })));
})();` });
if (touchscreen || uiPresses) await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
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
  if (!uiPresses) check("click to play shown", veil === "Click to play", { status: veil });
  if (uiPresses) {
    // The buttons as the world lists them, and where the viewport draws each: the bounds of the
    // page points its hit test gives the button's id.
    const listed = async () => (await evaluate(`window.amorisPlayer.amoris.command("ui.list", {})`))
      .elements.filter((e) => e.kind === "button");
    const buttons = await listed();
    const boxes = await evaluate(`(() => { const v = window.amorisPlayer.viewport, b = {};
      for (let y = 2; y < innerHeight; y += 4) for (let x = 2; x < innerWidth; x += 4) {
        const id = v.uiHit(x, y); if (!id) continue;
        const r = b[id] || (b[id] = [x, y, x, y]);
        r[0] = Math.min(r[0], x); r[1] = Math.min(r[1], y); r[2] = Math.max(r[2], x); r[3] = Math.max(r[3], y);
      }
      return b; })()`);
    const centre = (b) => { const r = boxes[b.entity]; return [(r[0] + r[2]) / 2, (r[1] + r[3]) / 2]; };
    const enabled = buttons.filter((b) => b.enabled);
    const disabled = buttons.filter((b) => !b.enabled);
    check("the viewport hits every enabled button and no disabled one", enabled.length >= 2
      && enabled.every((b) => boxes[b.entity]) && disabled.every((b) => !boxes[b.entity]),
      { buttons: buttons.map((b) => [b.entity, b.label, b.enabled]), hit: Object.keys(boxes) });
    const presses = async (b) => (await listed()).find((x) => x.entity === b.entity).presses;
    const free = () => evaluate(`!document.pointerLockElement && !window.amorisPlayer.input.captured()`);
    const [clicked, tapped] = enabled;
    const [cx, cy] = centre(clicked);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: cx, y: cy });
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x: cx, y: cy, button: "left", clickCount: 1 });
    await sleep(60);
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: cx, y: cy, button: "left", clickCount: 1 });
    await sleep(600);
    check("a click on a button presses it once", await presses(clicked) === clicked.presses + 1 && await free(),
      { button: clicked.label, presses: await presses(clicked) });
    await shot("2-clicked");
    const tap = async ([x, y]) => {
      const at = { x: Math.round(x), y: Math.round(y), id: 0, radiusX: 4, radiusY: 4, force: 1 };
      await send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
      await sleep(60);
      await send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await sleep(600);
    };
    await tap(centre(tapped));
    check("a tap on a button presses it once", await presses(tapped) === tapped.presses + 1 && await free(),
      { button: tapped.label, presses: await presses(tapped), touch: await evaluate("window.amorisPlayer.touch") });
    if (disabled.length) {
      // The disabled button's place: below the last enabled one in the column.
      const last = boxes[enabled[enabled.length - 1].entity];
      await tap([(last[0] + last[2]) / 2, last[3] + (last[3] - last[1]) / 2 + 12]);
      check("a tap on the disabled button presses nothing", await presses(disabled[0]) === disabled[0].presses,
        { presses: await presses(disabled[0]) });
    }
    const ui = await evaluate("({ presses: window.amorisPlayer.ui.presses, refused: window.amorisPlayer.ui.refused })");
    check("the page sent two presses, none refused", ui.presses === 2 && ui.refused === 0, ui);
  } else if (thirdPerson) {
    // The seat's walker as the world has it, and the camera as the page draws it.
    const seat = () => evaluate(`(async () => {
      const p = window.amorisPlayer, a = p.amoris;
      const q = await a.command("world.query", { with: ["ThirdPerson", "Walker"], fields: [
        "Transform.position", "Walker.yaw", "ThirdPerson.zoom", "ThirdPerson.guard", "ThirdPerson.lock",
        "ThirdPerson.facing", "Fighter.action", "Fighter.swings"] });
      const r = q[0];
      const cam = Array.from(p.viewport.raw.camera());
      const rig = (await a.command("world.query", { with: ["CameraRig"], fields: ["Transform.position"] }))[0];
      return { at: r["Transform.position"], yaw: r["Walker.yaw"], zoom: r["ThirdPerson.zoom"], guard: r["ThirdPerson.guard"],
        lock: r["ThirdPerson.lock"], facing: r["ThirdPerson.facing"], action: r["Fighter.action"], swings: r["Fighter.swings"],
        drawn: cam.slice(0, 3), camera: rig["Transform.position"], tick: (await a.command("status")).tick };
    })()`);
    check("third-person seat and game camera", await evaluate("window.amorisPlayer.thirdPerson && window.amorisPlayer.gameCamera"));
    await evaluate("window.amorisPlayer.forceCapture(true)");
    await sleep(500);
    const s0 = await seat();
    await keys(["KeyW"], 2000);
    await sleep(300);
    const s1 = await seat();
    const walked = Math.hypot(s1.at[0] - s0.at[0], s1.at[2] - s0.at[2]);
    check("walks away from the camera", walked > 1.5 && s1.at[2] < s0.at[2], { walked_m: Number(walked.toFixed(2)), from: s0.at, to: s1.at });
    const lag = Math.hypot(...s1.drawn.map((v, i) => v - s1.camera[i]));
    check("the drawn camera follows the rig's", lag < 1.5 && s1.camera[2] > s1.at[2], { lag_m: Number(lag.toFixed(2)), drawn: s1.drawn, rig: s1.camera });
    await shot("2-walked");
    await evaluate("window.amorisPlayer.input.dx += 200");
    await sleep(400);
    const s2 = await seat();
    check("the mouse orbits the camera", Math.abs(s2.yaw - s1.yaw) > 0.3, { yaw_before: s1.yaw, yaw_after: s2.yaw });
    await evaluate("window.amorisPlayer.input.dx -= 200");
    await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: vw / 2, y: vh / 2, deltaX: 0, deltaY: 300 });
    await sleep(400);
    const s3 = await seat();
    check("the wheel zooms out", s3.zoom > 1.2, { zoom: s3.zoom });
    await send("Input.dispatchMouseEvent", { type: "mousePressed", x: vw / 2, y: vh / 2, button: "right", clickCount: 1 });
    await sleep(300);
    const guarding = await seat();
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: vw / 2, y: vh / 2, button: "right", clickCount: 1 });
    await sleep(300);
    const lowered = await seat();
    check("the right button guards while held", guarding.guard === true && lowered.guard === false, { held: guarding.guard, released: lowered.guard });
    // Sprint toward the gate's brigands until one is in lock range, then lock on.
    await keys(["ShiftLeft", "KeyW"], 2400);
    await sleep(200);
    await key("KeyQ", 60);
    await sleep(700);
    const locked = await seat();
    check("Q locks on and turns the camera to the foe", locked.lock !== null && locked.lock !== undefined, { lock: locked.lock, at: locked.at, yaw: locked.yaw });
    await shot("3-locked");
    await key("KeyC", 60);
    await sleep(150);
    const rolled = await seat();
    check("C dodges (the game's roll)", rolled.action === 2, { action: rolled.action });
    await sleep(1200);
    await click("left");
    await sleep(300);
    const swung = await seat();
    check("the left button swings", swung.swings > rolled.swings, { swings: swung.swings });
    await shot("4-swung");
    const frames = await evaluate(`(() => { const s = window.amorisPlayer.samples.slice(-300).sort((a, b) => a - b);
      return { n: s.length, p50: s[Math.floor(s.length / 2)], p95: s[Math.floor(s.length * 0.95)] }; })()`);
    check("draws frames", frames.n > 30, frames);
    globalThis.frames = frames;
  } else if (emberlight) {
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
  } else if (gamepad || touchscreen) {
    // The seat's state as the page and the world see it.
    const seen = async () => {
      const w = await world();
      return { ...w, look: await evaluate("window.amorisPlayer.look"), state: await evaluate(`(() => {
        const p = window.amorisPlayer; return { captured: p.input.captured(), forced: p.input.forced,
        lock: !!document.pointerLockElement, veil: !document.getElementById("veil").hidden, pad: p.pad, touch: p.touch,
        controls: !document.getElementById("touch").hidden, acts: p.seat.acts, refused: p.seat.refused }; })()`) };
    };
    // The highest the walker's eye gets over `ms`, sampled as fast as the world answers.
    const highest = async (ms) => {
      let top = -Infinity;
      for (const end = Date.now() + ms; Date.now() < end;) top = Math.max(top, (await world()).walker[1]);
      return top;
    };
    const pad = (buttons, axes) => evaluate(`window.__pad.set(${JSON.stringify(buttons)}, ${JSON.stringify(axes)})`);
    const touch = (type, points) => send("Input.dispatchTouchEvent", { type,
      touchPoints: points.map(([x, y], id) => ({ x: Math.round(x), y: Math.round(y), id, radiusX: 4, radiusY: 4, force: 1 })) });
    // A finger from one point to another in `steps`, held there `hold` ms, then lifted.
    const swipe = async (from, to, steps, hold) => {
      await touch("touchStart", [from]);
      for (let i = 1; i <= steps; i++) {
        await sleep(30);
        await touch("touchMove", [[from[0] + (to[0] - from[0]) * i / steps, from[1] + (to[1] - from[1]) * i / steps]]);
      }
      await sleep(hold);
      await touch("touchEnd", []);
    };
    const tap = async (at) => { await touch("touchStart", [at]); await sleep(60); await touch("touchEnd", []); };
    const start = await seen();
    if (gamepad) {
      check("the page sees the gamepad", start.state.pad && start.state.pad.includes("STANDARD GAMEPAD"), { pad: start.state.pad });
      await pad({ 0: 1 }, [0, 0, 0, 0]);
      await sleep(200);
      await pad({}, [0, 0, 0, 0]);
    } else {
      await tap([vw / 2, vh / 2]);
    }
    await sleep(500);
    const playing = await seen();
    check(gamepad ? "a gamepad button starts play" : "a tap starts play", playing.state.captured && !playing.state.forced
      && !playing.state.lock && !playing.state.veil, playing.state);
    check("the starting press does not jump", Math.abs(playing.walker[1] - start.walker[1]) < 0.05,
      { from: start.walker, to: playing.walker });
    if (touchscreen) check("the touch controls show", playing.state.touch && playing.state.controls, playing.state);
    await shot("2-playing");
    if (gamepad) {
      await pad({}, [0, -1, 0, 0]);
      await sleep(1500);
      await pad({}, [0, 0, 0, 0]);
    } else {
      const at = [150, vh - 160];
      await swipe(at, [at[0], at[1] - 70], 6, 1500);
    }
    await sleep(400);
    const walked = await seen();
    const moved = Math.hypot(walked.walker[0] - playing.walker[0], walked.walker[2] - playing.walker[2]);
    check(gamepad ? "the left stick walks" : "the touch stick walks", moved > 1,
      { moved_m: Number(moved.toFixed(2)), from: playing.walker, to: walked.walker });
    await shot("3-walked");
    await sleep(300);
    const still = (await world()).walker;
    const drift = Math.hypot(still[0] - walked.walker[0], still[2] - walked.walker[2]);
    check("letting go stops the walker", drift < 0.05, { drift_m: Number(drift.toFixed(3)) });
    if (gamepad) {
      await pad({}, [0, 0, 1, 0]);
      await sleep(500);
      await pad({}, [0, 0, 0, 0]);
    } else {
      await swipe([vw * 0.7, vh * 0.3], [vw * 0.7 + 150, vh * 0.3], 8, 50);
    }
    await sleep(300);
    const turned = await seen();
    const turn = Math.abs(turned.look[0] - walked.look[0]);
    check(gamepad ? "the right stick turns the look" : "a drag turns the look", turn > 0.3,
      { yaw_before: walked.look[0], yaw_after: turned.look[0] });
    const ground = (await world()).walker[1];
    if (gamepad) {
      await pad({ 0: 1 }, [0, 0, 0, 0]);
      await sleep(80);
      await pad({}, [0, 0, 0, 0]);
    } else {
      const b = await evaluate(`(() => { const r = document.querySelector('.touch-button[data-input="jump"]').getBoundingClientRect();
        return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
      await tap(b);
    }
    const top = await highest(800);
    check(gamepad ? "South jumps" : "the jump button jumps", top > ground + 0.2, { ground, top });
    if (gamepad) {
      await pad({ 9: 1 }, [0, 0, 0, 0]);
      await sleep(100);
      await pad({}, [0, 0, 0, 0]);
    } else {
      const b = await evaluate(`(() => { const r = document.getElementById("touch-pause").getBoundingClientRect();
        return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
      await tap(b);
    }
    await sleep(300);
    const paused = await seen();
    check(gamepad ? "Start pauses" : "the pause button pauses", !paused.state.captured && paused.state.veil, paused.state);
    check("every act accepted", paused.state.acts > 0 && paused.state.refused === 0, paused.state);
    await shot("4-paused");
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
