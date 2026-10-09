// The desktop editor's smoke run (docs/spec/desktop.md, Verification), driven without a person:
// starts the Electron app from desktop/ on a copy of a sample, with Chromium's remote debugging on,
// and through the DevTools protocol checks that the editor loads with the WebAssembly/WebGPU
// viewport, that Play forks the world and runs ticks and Stop returns to the edit world, then closes
// the window as a person would (WM_CLOSE on Windows, SIGTERM elsewhere) and checks that the host
// process exited and removed its .pocket/host.json. Prints one JSON summary; with --out, writes it
// and half-size screenshots of the edit and play views there.
//
//   node tools/desktop_smoke.mjs [--sample samples/sailing] [--out docs/evidence/polish/desktop]
//
// Needs desktop/'s dependencies (cd desktop && npm ci), a built editor (editor/dist) and pocket;
// AMORIS_POCKET_BINARY, AMORIS_EDITOR_DIST, AMORIS_WEB_VIEWPORT and AMORIS_TSC pick other builds,
// as for the app itself.

import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs/promises';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
};
const sample = path.resolve(repo, arg('--sample', 'samples/sailing'));
const out = arg('--out', null);
const port = Number(arg('--port', '9333'));
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function until(what, timeoutMs, probe) {
  const end = Date.now() + timeoutMs;
  let last;
  while (Date.now() < end) {
    try {
      last = await probe();
      if (last) return last;
    } catch (e) { last = e; }
    await sleep(250);
  }
  throw new Error(`timed out waiting for ${what} (${last instanceof Error ? last.message : JSON.stringify(last)})`);
}

const report = { sample: path.relative(repo, sample), platform: `${process.platform}-${process.arch}`, steps: [] };
const step = (name, ok, detail = {}) => {
  report.steps.push({ name, ok, ...detail });
  console.error(`${ok ? 'ok  ' : 'FAIL'} ${name} ${JSON.stringify(detail)}`);
  if (!ok) throw new Error(name);
};

const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'amoris-desktop-smoke-'));
const project = path.join(temp, path.basename(sample));
await fs.cp(sample, project, { recursive: true });
await fs.rm(path.join(project, '.pocket'), { recursive: true, force: true });
const electron = createRequire(path.join(repo, 'desktop/package.json'))('electron');
const app = spawn(electron, [path.join(repo, 'desktop'), `--remote-debugging-port=${port}`,
  `--user-data-dir=${path.join(temp, 'user-data')}`, '--project', project],
{ stdio: ['ignore', 'pipe', 'pipe'], env: process.env });
let appLog = '';
app.stdout.on('data', c => { appLog = (appLog + c).slice(-4000); });
app.stderr.on('data', c => { appLog = (appLog + c).slice(-4000); });
const appExit = new Promise(resolve => app.once('exit', (code, signal) => resolve({ code, signal })));

let ws;
let id = 0;
const pending = new Map();
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const n = ++id;
  pending.set(n, { resolve, reject });
  ws.send(JSON.stringify({ id: n, method, params }));
});
const evaluate = async expression => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || 'evaluation failed');
  return r.result.value;
};
const shot = async name => {
  if (!out) return;
  const { cssVisualViewport: v } = await send('Page.getLayoutMetrics');
  const r = await send('Page.captureScreenshot', { format: 'png',
    clip: { x: 0, y: 0, width: v.clientWidth, height: v.clientHeight, scale: 0.5 } });
  await fs.writeFile(path.join(out, `${name}.png`), Buffer.from(r.data, 'base64'));
};

try {
  if (out) await fs.mkdir(out, { recursive: true });
  const page = await until('the editor page', 120000, async () => {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    return list.find(t => t.type === 'page' && /^http:\/\/127\.0\.0\.1:\d+\/\?viewport=wasm/.test(t.url));
  });
  const origin = new URL(page.url).origin;
  step('editor page loaded from the host', true, { url: page.url });
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  ws.onmessage = m => {
    const d = JSON.parse(m.data);
    const p = pending.get(d.id);
    if (!p) return;
    pending.delete(d.id);
    d.error ? p.reject(new Error(d.error.message)) : p.resolve(d.result);
  };
  const hostFile = JSON.parse(await fs.readFile(path.join(project, '.pocket/host.json'), 'utf8'));
  step('host.json names the project as fs.realpath does', hostFile.project === await fs.realpath(project),
    { project: hostFile.project, pid: hostFile.pid });
  const call = async (method, params = {}) => {
    const r = await fetch(origin + '/api/call', { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: 1, method, params }) });
    const d = await r.json();
    if (d.error) throw new Error(JSON.stringify(d.error));
    return d.result;
  };
  const status = await until('the status bar to say WebGPU (wasm)', 60000, () =>
    evaluate(`(() => { const t = document.body.innerText; return t.includes('WebGPU (wasm)') ? 'WebGPU (wasm)' : null; })()`));
  step('viewport is the WebAssembly/WebGPU one', status === 'WebGPU (wasm)');
  const adapter = await evaluate(`(async () => { const a = await navigator.gpu?.requestAdapter();
    return a ? { vendor: a.info?.vendor, architecture: a.info?.architecture, description: a.info?.description } : null; })()`);
  const canvas = await evaluate(`(() => { const c = [...document.querySelectorAll('canvas')]
    .map(c => ({ w: c.width, h: c.height })).sort((a, b) => b.w * b.h - a.w * a.h)[0]; return c || null; })()`);
  step('a canvas draws', !!canvas && canvas.w > 0 && canvas.h > 0, { canvas, adapter });
  await sleep(1500);
  await shot('edit');
  const edit = await call('status');
  step('the host starts in the edit world', edit.mode === 'edit', { tick: edit.tick });
  const click = label => evaluate(`(() => { const b = document.querySelector('button[aria-label^="${label}"]');
    if (!b) return false; b.click(); return true; })()`);
  step('Play clicked', await click('Play'));
  const play = await until('Play', 20000, async () => {
    const s = await call('status');
    return s.mode === 'play' ? s : null;
  });
  if (play.paused) await call('time.control', { pause: false });
  const ran = await until('ticks in Play', 20000, async () => {
    const s = await call('status');
    return s.tick >= play.tick + 30 ? s : null;
  });
  step('Play forks the world and runs ticks', true, { from: play.tick, to: ran.tick });
  const badge = await evaluate(`document.querySelector('.mode-badge')?.innerText`);
  await shot('play');
  step('the editor shows PLAY', /PLAY|PAUSED/.test(badge || ''), { badge });
  step('Stop clicked', await click('Stop'));
  const back = await until('Stop', 20000, async () => {
    const s = await call('status');
    return s.mode === 'edit' ? s : null;
  });
  step('Stop returns to the edit world', back.world_hash === edit.world_hash,
    { edit_hash: edit.world_hash, after: back.world_hash });
  ws.close();
  // Close the window as a person does: the app stops its host before it quits.
  if (process.platform === 'win32') execFileSync('taskkill', ['/PID', String(app.pid)], { stdio: 'ignore' });
  else app.kill('SIGTERM');
  const exit = await Promise.race([appExit, sleep(30000).then(() => null)]);
  step('the app exits on close', !!exit, { exit });
  const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
  await until('the host to exit', 10000, async () => !alive(hostFile.pid));
  step('the host process exited', !alive(hostFile.pid), { pid: hostFile.pid });
  const left = await fs.stat(path.join(project, '.pocket/host.json')).catch(() => null);
  step('host.json was removed', !left);
  report.ok = true;
} catch (e) {
  report.ok = false;
  report.error = e.message;
  report.app_log = appLog;
} finally {
  if (app.exitCode === null) {
    if (process.platform === 'win32') {
      try { execFileSync('taskkill', ['/F', '/T', '/PID', String(app.pid)], { stdio: 'ignore' }); } catch {}
    } else app.kill('SIGKILL');
  }
  await sleep(500);
  await fs.rm(temp, { recursive: true, force: true }).catch(() => {});
}
if (out) await fs.writeFile(path.join(out, 'summary.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exit(report.ok ? 0 : 1);
