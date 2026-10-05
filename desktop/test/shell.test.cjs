const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const http = require('node:http');
const { EventEmitter } = require('node:events');
const { PassThrough } = require('node:stream');
const { HostProcess, loopbackOrigin, alreadyServed } = require('../host.cjs');
const { allowedNavigation, authorizedSender, allowedClipboardWrite } = require('../security.cjs');
const { runtimePaths, validateResources } = require('../resources.cjs');

async function project(t) {
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'amoris-desktop-test-')));
  await fs.writeFile(path.join(root, 'project.toml'), '[project]\nname="test"\n');
  await fs.writeFile(path.join(root, 'scene.json'), '{}');
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  return root;
}

async function metadata(root, port, pid = 99999) {
  await fs.mkdir(path.join(root, '.pocket'), { recursive: true });
  await fs.writeFile(path.join(root, '.pocket/host.json'), JSON.stringify({
    project: root, port, pid, url: `http://127.0.0.1:${port}`,
  }));
}

async function server(t, handler) {
  const s = http.createServer(handler);
  await new Promise(resolve => s.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { s.closeAllConnections(); s.close(resolve); }));
  return s;
}

function fakeChild(root, behavior = 'ready') {
  const child = new EventEmitter();
  child.pid = behavior === 'spawn-error' ? undefined : 424242;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  child.begin = () => setImmediate(() => {
    if (behavior === 'spawn-error') {
      child.emit('error', Object.assign(new Error('spawn ENOENT'), { code: 'ENOENT' }));
      child.emit('close', -2, null);
    } else if (behavior === 'ready') child.stdout.write(JSON.stringify({
      url: 'http://127.0.0.1:31337', port: 31337, project: root, pid: child.pid,
    }) + '\n');
    else if (behavior === 'malformed') child.stdout.write('{not json}\n');
  });
  return child;
}

function supervisor(root, behavior, { ignoreTerm = false } = {}) {
  const child = fakeChild(root, behavior);
  const signals = [];
  const host = new HostProcess({ pocket: '/fixture/pocket', editor: '/fixture/editor',
    viewport: '/fixture/viewport', tsc: '/fixture/tsc' }, {
    startupMs: 20, shutdownMs: 10, spawnChild: () => { child.begin(); return child; },
    terminate: (_owned, signal) => { signals.push(signal);
      if (!ignoreTerm || signal === 'SIGKILL') { child.emit('exit', 0, signal); child.emit('close', 0, signal); }
    },
  });
  return { host, child, signals };
}

test('trusted renderer documents exclude same-origin project HTML and child frames', () => {
  const launcher = 'file:///app/launcher/index.html', origin = 'http://127.0.0.1:1234';
  assert.equal(allowedNavigation(origin + '/?viewport=wasm', launcher, origin), true);
  for (const url of [origin + '/assets/evil.html', origin + '/wasm/index.html',
    'http://127.0.0.1:9999/', 'https://evil.test/', 'file:///etc/passwd',
    'http://user:password@127.0.0.1:1234/']) {
    assert.equal(allowedNavigation(url, launcher, origin), false, url);
  }
  const mainFrame = { url: origin + '/' }, contents = { mainFrame };
  const window = { webContents: contents, isDestroyed: () => false };
  assert.equal(authorizedSender({ sender: contents, senderFrame: mainFrame }, window, launcher, origin), true);
  assert.equal(authorizedSender({ sender: contents, senderFrame: { url: origin + '/' } }, window, launcher, origin), false);
});

test('clipboard permits only sanitized writes from current editor main frame', () => {
  const launcher = 'file:///app/launcher/index.html', origin = 'http://127.0.0.1:1234';
  const contents = { mainFrame: { url: origin + '/?viewport=wasm' } };
  const window = { webContents: contents, isDestroyed: () => false };
  const details = { isMainFrame: true, requestingUrl: contents.mainFrame.url };
  assert.equal(allowedClipboardWrite(contents, 'clipboard-sanitized-write', details, window, launcher, origin), true);
  assert.equal(allowedClipboardWrite(contents, 'clipboard-read', details, window, launcher, origin), false);
  assert.equal(allowedClipboardWrite(contents, 'clipboard-sanitized-write', { ...details, isMainFrame: false }, window, launcher, origin), false);
  assert.equal(allowedClipboardWrite(contents, 'clipboard-sanitized-write', { ...details, requestingUrl: origin + '/assets/evil.html' }, window, launcher, origin), false);
});

test('readiness requires exact numeric loopback authority', () => {
  assert.equal(loopbackOrigin('http://127.0.0.1:31337'), 'http://127.0.0.1:31337');
  for (const url of ['https://127.0.0.1:31337', 'http://localhost:31337',
    'http://127.0.0.1:31337/asset', 'http://127.0.0.1:31337#x', 'http://user@127.0.0.1:31337']) {
    assert.equal(loopbackOrigin(url), null);
  }
});

test('duplicate native authority is refused without spawning or killing it', async t => {
  const root = await project(t);
  const s = await server(t, (_req, res) => res.end(JSON.stringify({ result: { root } })));
  await metadata(root, s.address().port);
  let spawned = false;
  const host = new HostProcess({}, { spawnChild: () => { spawned = true; throw new Error(); } });
  await assert.rejects(host.start(root), /already open/);
  await host.stop();
  assert.equal(spawned, false);
  assert.equal(JSON.parse(await fs.readFile(path.join(root, '.pocket/host.json'))).pid, 99999);
});

test('queued breakpoint or unreadable reachable authority is occupied after bounded deadline', async t => {
  const root = await project(t);
  const s = await server(t, () => {});
  await metadata(root, s.address().port);
  const start = Date.now();
  assert.equal(await alreadyServed(root, 30), true);
  assert.ok(Date.now() - start < 500);
});

test('definitively dead stale host metadata permits a new host', async t => {
  const root = await project(t);
  const s = http.createServer();
  await new Promise(resolve => s.listen(0, '127.0.0.1', resolve));
  const port = s.address().port;
  await new Promise(resolve => s.close(resolve));
  await metadata(root, port);
  assert.equal(await alreadyServed(root, 100), false);
});

test('startup JSON failure and timeout always stop the owned child', async t => {
  const root = await project(t);
  for (const behavior of ['malformed', 'silent']) {
    const { host, signals } = supervisor(root, behavior);
    await assert.rejects(host.start(root));
    await host.stop();
    assert.deepEqual(signals, ['SIGTERM']);
  }
});

test('spawn ENOENT without PID closes cleanly and never signals another process', async t => {
  const root = await project(t);
  const { host, signals } = supervisor(root, 'spawn-error');
  await assert.rejects(host.start(root), /ENOENT/);
  await host.stop();
  assert.deepEqual(signals, []);
});

test('owned stop is idempotent, forced when needed, and leaves foreign metadata intact', async t => {
  const root = await project(t);
  const { host, signals } = supervisor(root, 'ready', { ignoreTerm: true });
  await host.start(root);
  await metadata(root, 31337, 77777);
  await Promise.all([host.stop(), host.stop()]);
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
  assert.equal(JSON.parse(await fs.readFile(path.join(root, '.pocket/host.json'))).pid, 77777);
});

test('owned metadata is cleaned and an unexpected exit is reported', async t => {
  const root = await project(t);
  const { host, child } = supervisor(root, 'ready');
  await host.start(root);
  let crash;
  host.on('host-exit', result => { crash = result; });
  await metadata(root, 31337, child.pid);
  child.emit('exit', 1, null);
  await host.stop();
  assert.deepEqual(crash, { code: 1, signal: null });
  await assert.rejects(fs.stat(path.join(root, '.pocket/host.json')), { code: 'ENOENT' });
});

test('packaged resource paths never depend on the checkout; viewport wrapper and TS libs required', async t => {
  const root = await project(t);
  const resources = runtimePaths({ packaged: true, resourcesPath: root, repo: '/absent/checkout', env: {} });
  const files = [resources.pocket, path.join(resources.editor, 'index.html'),
    path.join(resources.viewport, 'pkg/pocket_viewport.js'), path.join(resources.viewport, 'pkg/pocket_viewport_bg.wasm'),
    resources.tsc, path.join(path.dirname(resources.tsc), 'lib.esnext.d.ts')];
  for (const file of files) { await fs.mkdir(path.dirname(file), { recursive: true }); await fs.writeFile(file, 'fixture'); }
  await assert.rejects(validateResources(resources), /pocket_web.js/);
  await fs.writeFile(path.join(resources.viewport, 'pocket_web.js'), 'fixture');
  await validateResources(resources);
  assert.ok(Object.values(resources).every(p => p.startsWith(root)));
});
