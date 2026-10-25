// Plays water-pool through the shipped first_person page using CDP keyboard/mouse events.
// node tools/water_pool_check.mjs <exported folder | http(s) URL> [--out DIR] [--timeout-s 90]
// --selftest is CPU-only. This check captures evidence; human visual acceptance remains separate.
import {spawn} from "node:child_process";
import {createReadStream, createWriteStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync} from "node:fs";
import {createServer} from "node:http";
import {tmpdir} from "node:os";
import {extname, join, resolve, sep} from "node:path";

const ensure = (ok, message, detail) => { if (!ok) throw Error(message + (detail === undefined ? "" : ": " + JSON.stringify(detail))); };
const sleep = ms => new Promise(ready => setTimeout(ready, ms));
function stats(values) { ensure(Array.isArray(values) && values.length > 0 && values.every(x => Number.isFinite(x) && x >= 0), "finite timing samples required"); const s = values.slice().sort((a, b) => a - b); return {n: s.length, p50: s[Math.floor(s.length / 2)], p95: s[Math.min(s.length - 1, Math.ceil(s.length * 0.95) - 1)], max: s.at(-1), mean: s.reduce((a, b) => a + b, 0) / s.length}; }
function summarizePerf(p) {
  ensure(p && Array.isArray(p.step_ms) && p.step_ms.length > 0 && p.external_ms?.length === p.step_ms.length && p.publish_ms?.length === p.step_ms.length, "aligned active/external/publication timing arrays required");
  const total = p.step_ms.map((v, i) => v + p.external_ms[i] + p.publish_ms[i]);
  return {ticks: p.ticks, step_ms: stats(p.step_ms), external_ms: stats(p.external_ms), publish_ms: stats(p.publish_ms), full_tick_ms: stats(total),
    external_nonzero_samples: p.external_ms.filter(x => x > 0).length, scope: "worker active step + external compute wait + publication, paired by the same retained tick; real-time route with read-only instrumentation"};
}
function home(s) { const p = s.position; return p[0] >= 14 && p[0] <= 18 && p[2] >= 22 && p[2] <= 26 && s.grounded && !s.wet; }
function marker(s) { return s.eye_wet && s.position[1] <= 2.3 && Math.hypot(s.position[0] - 16, s.position[2] - 8) <= 1.5; }
function controls(s) {
  if (s.stage >= 4) return [];
  if (s.stage === 3) return ["KeyS", "Space"];
  if (s.stage === 2) return ["Space"];
  if (s.position[2] > 8.25) return ["KeyW"];
  if (s.position[2] < 7.75) return ["KeyS"];
  return [];
}
function poolEventKind(record) { const kind = record?.event?.kind; return typeof kind === "string" && kind.startsWith("pool.") ? kind : null; }
function selftest() {
  const p = summarizePerf({ticks: 3, step_ms: [1, 2, 3], external_ms: [10, 20, 30], publish_ms: [0.5, 1, 1.5]});
  ensure(p.full_tick_ms.max === 34.5 && p.external_nonzero_samples === 3, "full tick timing excluded external/publication or mispaired ticks");
  let bad = false; try { summarizePerf({step_ms: [1, 2], external_ms: [1], publish_ms: [1, 2]}); } catch { bad = true; } ensure(bad, "unaligned worker timings were accepted");
  const deck = {position: [16, 5.62, 24], grounded: true, wet: false, eye_wet: false, stage: 0};
  ensure(home(deck) && !home({...deck, wet: true}) && !home({...deck, position: [16, 5.62, 21]}) && controls(deck).join() === "KeyW", "dry grounded return-pad geometry is not enforced");
  const gold = {...deck, position: [16, 2, 8], wet: true, eye_wet: true, stage: 2};
  ensure(marker(gold) && !marker({...gold, eye_wet: false}) && !marker({...gold, position: [16, 2.31, 8]}) && !marker({...gold, position: [18, 2, 8]}) && controls(gold).join() === "Space", "underwater marker accepts the wrong eye/pose");
  ensure(controls({...gold, stage: 3}).join() === "KeyS,Space" && controls({...deck, stage: 4}).length === 0, "return input direction or completed-input release differs");
  const finished = {seq: 12, id: 37, event: {seq: 37, tick: 420, kind: "pool.finished", subject: 5, cause: null, data: {attempt: 1}}, epoch: 0};
  const retried = {seq: 13, id: 38, event: {seq: 38, tick: 450, kind: "pool.retried", subject: 5, cause: null, data: {attempt: 2}}, epoch: 3};
  const wrongOuterKind = {seq: 14, id: 39, kind: "pool.finished", event: {kind: "other.event"}, epoch: 3};
  const flatLegacy = {seq: 15, id: 40, kind: "pool.retried", epoch: 3};
  const source = [finished, wrongOuterKind, flatLegacy, null, retried], retained = source.filter(record => poolEventKind(record) !== null);
  ensure(retained.length === 2 && retained[0] === finished && retained[1] === retried && retained[0].epoch === 0 && retained[1].epoch === 3
    && retained[0].id === 37 && retained[1].seq === 13 && retained.some(record => poolEventKind(record) === "pool.finished") && retained.some(record => poolEventKind(record) === "pool.retried"),
    "worker event envelope kind/epoch/identity was flattened, read at the wrong level, or lost");
  return {passed: true, worker_event_envelope_regression: true, scope: "CPU-only route geometry, complete-tick timing and actual worker event envelope bookkeeping; no Chrome/GPU or gameplay claim"};
}
const args = process.argv.slice(2), option = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback; };
if (args.includes("--selftest")) console.log(JSON.stringify(selftest()));
else {
  const input = args[0], out = resolve(option("--out", "out/water-pool/check")), timeoutS = Number(option("--timeout-s", "90")), readyS = Number(process.env.READY_S || 120), pollMs = 70;
  ensure(input && !input.startsWith("--") && Number.isFinite(timeoutS) && timeoutS > 0 && Number.isFinite(readyS) && readyS > 0, "usage: node tools/water_pool_check.mjs <exported folder | URL> [--out DIR] [--timeout-s 90]");
  mkdirSync(out, {recursive: true});
  let server, proc, ws, profile, chromeLog; const pressed = new Set(), pending = new Map(), errors = [], seen = new Set(), observations = [], shots = [], checks = [];
  let requestId = 0, browser = null, url, last = null, processError = null;
  const report = {ok: false, input, out, scope: "automated first_person player input route over resident GPU water; screenshots require human visual review", human_visual_acceptance: "pending", checks, observations, shots, errors};
  const note = text => { text = String(text).slice(0, 2000); if (!seen.has(text)) { seen.add(text); errors.push(text); } };
  const check = (name, ok, detail = {}) => { checks.push({name, ok: !!ok, ...detail}); ensure(ok, name, detail); };
  function send(method, params = {}, timeoutMs = 20000) {
    ensure(ws?.readyState === WebSocket.OPEN, "owned CDP page is not connected", method);
    return new Promise((resolve, reject) => {
      const id = ++requestId, timer = setTimeout(() => { pending.delete(id); reject(Error(`CDP timeout ${method} after ${timeoutMs} ms`)); }, timeoutMs);
      pending.set(id, {resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); }}); ws.send(JSON.stringify({id, method, params}));
    });
  }
  async function evaluate(expression, timeoutMs = 20000) {
    const r = await send("Runtime.evaluate", {expression, awaitPromise: true, returnByValue: true}, timeoutMs);
    if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text); return r.result?.value;
  }
  async function keys(next) {
    for (const code of [...pressed]) if (!next.includes(code)) { await send("Input.dispatchKeyEvent", {type: "keyUp", code, key: code === "Space" ? " " : code.slice(3).toLowerCase(), windowsVirtualKeyCode: code === "Space" ? 32 : code.charCodeAt(3)}); pressed.delete(code); }
    for (const code of next) if (!pressed.has(code)) { await send("Input.dispatchKeyEvent", {type: "keyDown", code, key: code === "Space" ? " " : code.slice(3).toLowerCase(), windowsVirtualKeyCode: code === "Space" ? 32 : code.charCodeAt(3)}); pressed.add(code); }
  }
  async function state() {
    const s = await evaluate(`(async () => {
      const p=window.amorisPlayer;if(!p)return null;const g=p.amoris;
      if(g.stopped)throw Error(JSON.stringify(g.stopped));
      const tickBefore=g.latest()?.tick;
      const [swimmer,water]=await Promise.all([
        g.command("world.get",{entity:5,components:["Walker","Transform","PoolRun"]}),
        g.command("world.get",{entity:4,fields:["WaterRegion.solver","WaterRegion.tiles","WaterResident.completed_tick","WaterResident.recipe.digest","WaterResident.stats"]})
      ]);
      const c=swimmer.components,w=c.Walker,r=c.PoolRun,f=water.fields;
      if(!w||!r||!c.Transform)throw Error("Swimmer components missing");
      const tiles=f["WaterRegion.tiles"];
      return {name:swimmer.name,water_name:water.name,position:c.Transform.position,wet:w.in_liquid,eye_wet:w.eye_in_liquid,grounded:w.grounded,vertical_mps:w.vertical_mps,yaw:w.yaw,pitch:w.pitch,
        stage:r.stage,elapsed_s:r.elapsed_s,underwater_s:r.underwater_s,attempt:r.attempt,best_s:r.best_s,
        tick_before:tickBefore,tick_after:g.latest()?.tick,resident_tick:f["WaterResident.completed_tick"],solver:f["WaterRegion.solver"],tiles_empty:Array.isArray(tiles)&&tiles.length===0,
        digest:f["WaterResident.recipe.digest"],water_stats:f["WaterResident.stats"],frames:p.frames,look:p.look,seat:{acts:p.seat.acts,refused:p.seat.refused},problems:g.problems(),
        pending_frontend_commands:g.waiting.size,pending_frontend_requests:g.requests.size};
    })()`);
    ensure(s && s.position?.length === 3 && s.position.every(Number.isFinite) && Number.isInteger(s.stage) && s.stage >= 0 && s.stage <= 4, "invalid player observation", s);
    ensure(s.name === "Swimmer" && s.water_name === "PoolWater" && s.solver === "gpu_resident" && s.tiles_empty, "sample identity or resident empty-tile contract differs", s);
    ensure(Number.isInteger(s.resident_tick) && s.resident_tick > 0 && Array.isArray(s.digest) && s.digest.length === 32 && s.digest.some(x => x !== 0), "water has no completed authoritative GPU field", s);
    ensure(s.seat.refused === 0 && s.problems.length === 0 && errors.length === 0, "player command, runtime or WebGPU error", {seat: s.seat, problems: s.problems, errors});
    ensure(Math.abs(s.yaw) < 0.05, "forward/back route lost its authored yaw without mouse input", s.yaw);
    ensure(!last || s.resident_tick >= last.resident_tick, "resident completed water stamp moved backwards without a restore", {before: last?.resident_tick, after: s.resident_tick});
    s.wall_ms = Date.now(); last = s; observations.push(s); return s;
  }
  async function stageShot(name, s) {
    const shot = await send("Page.captureScreenshot", {format: "png", captureBeyondViewport: false}); ensure(shot.data, "stage screenshot missing pixels", name);
    const path = join(out, `${name}.png`); writeFileSync(path, Buffer.from(shot.data, "base64")); const after = await state(); const metadata = {name, path, before: s, after}; shots.push(metadata); writeFileSync(join(out, `${name}.json`), JSON.stringify(metadata, null, 2));
  }
  try {
    if (/^https?:\/\//.test(input)) url = input;
    else {
      const folder = resolve(input); ensure(existsSync(join(folder, "index.html")), "exported player index.html is missing", folder);
      const types = {".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json", ".png": "image/png", ".gltf": "model/gltf+json", ".glb": "model/gltf-binary"};
      server = createServer((request, response) => {
        let file; try { file = resolve(folder, "." + decodeURIComponent(new URL(request.url, "http://x").pathname)); } catch { response.writeHead(400).end(); return; }
        if (file !== folder && !file.startsWith(folder + sep)) { response.writeHead(403).end(); return; }
        if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
        if (!existsSync(file) || !statSync(file).isFile()) { response.writeHead(404).end(); return; }
        response.writeHead(200, {"Content-Type": types[extname(file)] || "application/octet-stream", "Cache-Control": "no-store"}); createReadStream(file).pipe(response);
      });
      await new Promise((ready, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", ready); }); url = `http://127.0.0.1:${server.address().port}/`;
    }
    if (process.env.QUERY) { const u = new URL(url); for (const [k, v] of new URLSearchParams(process.env.QUERY)) u.searchParams.set(k, v); url = u.href; }
    report.url = url;
    const candidates = process.platform === "darwin" ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"] : process.platform === "win32" ? [join(process.env.PROGRAMFILES || "C:\\Program Files", "Google/Chrome/Application/chrome.exe"), join(process.env["PROGRAMFILES(X86)"] || "C:\\Program Files (x86)", "Google/Chrome/Application/chrome.exe"), join(process.env.LOCALAPPDATA || "", "Google/Chrome/Application/chrome.exe")] : ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"];
    const executable = process.env.CHROME || candidates.find(existsSync); ensure(executable, "Chrome not found; set CHROME");
    profile = mkdtempSync(join(tmpdir(), "amoris-pool-")); const [vw, vh] = (process.env.VIEWPORT || "1280x720").split("x").map(Number); ensure(vw > 0 && vh > 0, "invalid VIEWPORT");
    chromeLog = createWriteStream(join(out, "chrome.log"));
    proc = spawn(executable, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, ...(process.platform === "win32" ? [] : ["--enable-features=Vulkan"]), `--window-size=${vw},${vh}`, "--no-first-run", "--no-default-browser-check", ...(process.env.CHROME_FLAGS || "").split(" ").filter(Boolean), "about:blank"], {stdio: ["ignore", "ignore", "pipe"]}); proc.once("error", error => { processError = String(error); }); proc.stderr.pipe(chromeLog);
    let target; const connectEnd = Date.now() + 20000;
    while (!target && Date.now() < connectEnd) {
      ensure(!processError && proc.exitCode === null && proc.signalCode === null, "owned Chrome exited or could not start before its CDP target opened", processError);
      try { const port = Number(readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0]); browser = (await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).Browser; target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(t => t.type === "page"); } catch {} if (!target) await sleep(100);
    }
    ensure(target, "owned Chrome CDP target did not open"); ws = new WebSocket(target.webSocketDebuggerUrl);
    await new Promise((ready, reject) => { ws.onopen = ready; ws.onerror = reject; });
    ws.onmessage = e => {
      const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); return; }
      if (m.method === "Runtime.exceptionThrown") note("[exception] " + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
      if (m.method === "Runtime.consoleAPICalled") { const text = m.params.args.map(x => x.value ?? x.description ?? "").join(" "); if (m.params.type === "error" || /Validation|panicked|amoris ERROR/i.test(text)) note(text); }
    };
    const disconnected = () => { for (const p of pending.values()) p.reject(Error("owned CDP connection closed")); pending.clear(); }; ws.onclose = disconnected; ws.onerror = disconnected;
    await send("Page.enable"); await send("Runtime.enable"); await send("Emulation.setDeviceMetricsOverride", {width: vw, height: vh, deviceScaleFactor: Number(process.env.DPR || 1), mobile: false}); await send("Page.navigate", {url});
    const start = Date.now(), readyEnd = start + readyS * 1000; let ready;
    while (!ready && Date.now() < readyEnd) { ready = await evaluate("window.amorisPlayer?.ready_ms || null"); const stopped = await evaluate("window.amorisPlayer?.amoris.stopped || null"); ensure(!stopped, "game stopped before ready", stopped); const halted = await evaluate("window.amorisPlayer?.amoris.status?.state === 'halted' ? (window.amorisPlayer.amoris.status.error || {}) : null"); ensure(!halted, "game halted before ready", halted); if (!ready) await sleep(150); }
    check("player_ready", !!ready, {ready_ms: ready, wall_ms: Date.now() - start}); await sleep(200);
    await evaluate(`(() => { const p=window.amorisPlayer, poolKind=${poolEventKind.toString()}; window.__poolCheck={events:[],missed:0,frame_delivery_ms:[],last_frame_delivery:null};
      const append=p.samples.push;p.samples.push=function(...values){const now=performance.now(),c=window.__poolCheck;if(c.last_frame_delivery!==null&&c.frame_delivery_ms.length<10000)c.frame_delivery_ms.push(now-c.last_frame_delivery);c.last_frame_delivery=now;return append.apply(this,values);};
      p.amoris.onEvents((records,missed)=>{window.__poolCheck.missed+=missed||0;for(const record of records)if(poolKind(record)!==null)window.__poolCheck.events.push(record);});p.forceCapture(true);return true;})()`);
    await sleep(100); const initial = await state();
    check("authored_start_on_green_pad", initial.stage === 0 && home(initial) && Math.hypot(initial.position[0] - 16, initial.position[2] - 24) < 0.15 && Math.abs(initial.position[1] - 5.62) < 0.1, {initial}); await stageShot("00-deck", initial);
    let waterline = false, underwater = false, surfaced = false, finished = null; const routeEnd = Date.now() + timeoutS * 1000;
    while (Date.now() < routeEnd) {
      const s = await state(); await keys(s.stage === 2 && !underwater ? [] : controls(s));
      if (!waterline && s.stage === 1 && s.wet && Math.abs(s.position[1] - 3.5) <= 0.4) { waterline = true; await stageShot("01-waterline", s); }
      if (!underwater && s.stage === 2) { check("submerged_gold_marker", marker(s), {state: s}); await keys([]); await stageShot("02-underwater-marker", s); underwater = true; await keys(["Space"]); }
      if (!surfaced && s.stage === 3) { check("real_eye_emerged_while_body_wet", s.wet && !s.eye_wet, {state: s}); await stageShot("03-surfaced", s); surfaced = true; }
      if (s.stage === 4) { await keys([]); finished = s; break; }
      await sleep(pollMs);
    }
    check("completed_route_before_watchdog", !!finished, {last, timeout_s: timeoutS}); check("waterline_and_underwater_evidence_captured", waterline && underwater && surfaced, {waterline, underwater, surfaced});
    check("returned_to_dry_grounded_pad", home(finished) && finished.underwater_s > 0 && finished.elapsed_s > 0 && finished.best_s > 0, {finished}); await stageShot("04-return-pad", finished);
    await sleep(250); const stable = await state(); check("finish_timer_stopped", stable.stage === 4 && home(stable) && stable.elapsed_s === finished.elapsed_s, {stable});
    const at = {x: Math.floor(vw / 2), y: Math.floor(vh / 2), button: "right", clickCount: 1}; await send("Input.dispatchMouseEvent", {type: "mousePressed", ...at}); await sleep(50); await send("Input.dispatchMouseEvent", {type: "mouseReleased", ...at});
    let retried; const retryEnd = Date.now() + 5000; while (Date.now() < retryEnd) { const s = await state(); if (s.stage === 0 && s.attempt === finished.attempt + 1) { retried = s; break; } await sleep(pollMs); }
    check("right_click_retry_preserves_best", retried && home(retried) && retried.best_s === finished.best_s && retried.elapsed_s === 0 && retried.underwater_s === 0, {retried, best_before: finished.best_s}); await stageShot("05-retried", retried);
    report.pool_events = await evaluate("window.__poolCheck"); check("route_emits_finish_and_retry", report.pool_events.events.some(record => poolEventKind(record) === "pool.finished") && report.pool_events.events.some(record => poolEventKind(record) === "pool.retried") && report.pool_events.missed === 0, report.pool_events);
    report.raw_worker_perf = await evaluate("window.amorisPlayer.amoris.perf()"); report.perf = summarizePerf(report.raw_worker_perf); check("actual_external_gpu_work_completed", report.perf.external_nonzero_samples > 0 && retried.resident_tick > initial.resident_tick && retried.frames > initial.frames, {initial_tick: initial.resident_tick, final_tick: retried.resident_tick});
    report.sample_residual_evidence = {on_return_not_wet: !finished.wet, on_retry_not_wet: !retried.wet, resident_cpu_tiles_empty_throughout: observations.every(s => s.tiles_empty), pending_frontend_commands: retried.pending_frontend_commands, pending_frontend_requests: retried.pending_frontend_requests,
      scope: "current-pose contact, authoritative resident completion stamps, and outside-field dry return observed; internal derived Walker sample-cache entries are not exposed by the page and are not claimed empty"};
    report.frame_ms = await evaluate("(() => {const p=window.amorisPlayer;return {samples:p.samples,stats:p.stats,frames:p.frames};})()"); if (report.frame_ms.samples.length) report.frame_ms.statistics = stats(report.frame_ms.samples);
    report.frame_ms.scope="Viewport's 120-drawn-frame rolling mean of requestAnimationFrame intervals; distribution of these rolling means is not raw frame-time tails.";
    report.drawn_frame_intervals={...stats(report.pool_events.frame_delivery_ms),scope:"Wall intervals between actual viewport stats deliveries after drawn frames; includes CPU work, route instrumentation and screenshots, not GPU timestamps or an isolated rendering benchmark."};
    check("no_runtime_or_gpu_errors", errors.length === 0 && retried.problems.length === 0 && retried.seat.refused === 0); report.ok = checks.every(c => c.ok); report.browser = browser;
  } catch (error) {
    report.error = String(error?.stack || error); report.last = last; report.browser = browser;
    if (ws?.readyState === WebSocket.OPEN) { try { const s = await send("Page.captureScreenshot", {format: "png"}, 5000); if (s.data) writeFileSync(join(out, "failure.png"), Buffer.from(s.data, "base64")); } catch {} }
  } finally {
    if (ws?.readyState === WebSocket.OPEN) { try { await keys([]); await evaluate("window.amorisPlayer?.forceCapture(false)", 2000); } catch {} ws.close(); }
    if (proc && proc.exitCode === null && proc.signalCode === null) { proc.kill(); await Promise.race([new Promise(ready => proc.once("exit", ready)), sleep(3000)]); if (proc.exitCode === null && proc.signalCode === null) proc.kill("SIGKILL"); }
    if (server) { server.closeAllConnections(); await new Promise(done => server.close(done)); }
    if (chromeLog) chromeLog.end(); if (profile) { await sleep(150); rmSync(profile, {recursive: true, force: true, maxRetries: 5, retryDelay: 200}); }
    writeFileSync(join(out, "summary.json"), JSON.stringify(report, null, 2)); console.log(JSON.stringify({ok: report.ok, checks: checks.map(c => ({name: c.name, ok: c.ok})), error: report.error || null, output: join(out, "summary.json"), human_visual_acceptance: "pending"}));
  }
  if (!report.ok) process.exitCode = 1;
}
