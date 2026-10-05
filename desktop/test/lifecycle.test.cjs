const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { EventEmitter } = require('node:events');

// Exercise the actual main-process coordinator with native UI/process ports
// substituted. No Electron renderer, real projects or GUI state are mutated.
function application({ discard = false, startError = false, loadError = false,
                       dirtyDuringLoad = false, recentError = false } = {}) {
  const ipc = {}, calls = [], hosts = [];
  const contents = { mainFrame: { url: 'http://127.0.0.1:8000/?viewport=wasm' }, send: () => {} };
  const window = { webContents: contents, isDestroyed: () => false, show: () => {},
    setTitle: () => {}, setDocumentEdited: value => calls.push(['document-edited', value]),
    loadURL: async url => {
      calls.push(['load', url]);
      if (loadError && url.startsWith('http:')) throw new Error('load failed');
      contents.mainFrame.url = url;
      if (dirtyDuringLoad && url.startsWith('http:')) {
        ipc['amoris:modified']({ sender: contents, senderFrame: contents.mainFrame }, true);
      }
    }, close: () => calls.push(['close']) };
  class Host extends EventEmitter {
    constructor() { super(); this.exitResult = null; hosts.push(this); }
    async start() { if (startError) throw new Error('start failed'); return { origin: 'http://127.0.0.1:9000' }; }
    async stop() { calls.push(['stop', this]); this.exitResult = { code: 0 }; }
  }
  const app = { isPackaged: true, setName: () => {}, enableSandbox: () => {},
    requestSingleInstanceLock: () => true, on: () => {}, whenReady: () => new Promise(() => {}),
    getPath: () => '/fixture/user-data', addRecentDocument: () => {}, quit: () => calls.push(['quit']) };
  const fakeFs = { mkdir: async () => {}, rename: async () => {}, writeFile: async () => {
    if (recentError) throw new Error('disk full'); } };
  const electron = { app, BrowserWindow: class {}, nativeImage: {}, session: {},
    Menu: { buildFromTemplate: value => value, setApplicationMenu: () => {} },
    ipcMain: { handle: (name, fn) => { ipc[name] = fn; }, on: (name, fn) => { ipc[name] = fn; } },
    dialog: { showMessageBox: async (_win, options) => { calls.push(['dialog', options]); return { response: discard ? 1 : 0 }; },
              showErrorBox: (_title, message) => calls.push(['error', message]) } };
  const context = { module: { exports: {} }, __dirname: path.resolve(__dirname, '..'),
    console: { warn: () => calls.push(['warning']) }, process: { platform: 'darwin', env: {}, argv: [] },
    require: name => {
      if (name === 'electron') return electron;
      if (name === 'node:fs/promises') return fakeFs;
      if (name === './host.cjs') return { HostProcess: Host, validateProject: async p => p, alreadyServed: async () => false };
      if (name === './resources.cjs') return { runtimePaths: () => ({}), validateResources: async x => x };
      if (name === './security.cjs') return require('../security.cjs');
      return require(name);
    } };
  const source = fs.readFileSync(path.join(__dirname, '../main.cjs'), 'utf8');
  vm.runInNewContext(source + '\nmodule.exports={openProject,requestClose,setState(v){window=v.window;host=v.host;project=v.project;origin=v.origin;modified=v.modified;},state(){return {host,project,origin,modified,allowClose};}};', context);
  const main = context.module.exports;
  const previous = { exitResult: null, stop: async () => calls.push(['previous-stop']) };
  main.setState({ window, host: previous, project: '/fixture/old', origin: 'http://127.0.0.1:8000', modified: true });
  return { main, calls, hosts, previous, window, ipc };
}

test('Cancel on close keeps the host and unsaved editor alive', async () => {
  const a = application();
  await a.main.requestClose();
  assert.equal(a.main.state().host, a.previous);
  assert.equal(a.main.state().modified, true);
  assert.equal(a.calls.some(c => c[0] === 'previous-stop' || c[0] === 'close'), false);
});

test('Discard close stops the owned host before closing the window', async () => {
  const a = application({ discard: true });
  await a.main.requestClose();
  assert.ok(a.calls.findIndex(c => c[0] === 'previous-stop') < a.calls.findIndex(c => c[0] === 'close'));
  assert.equal(a.main.state().host, null);
});

test('Cancel on switch preserves the current project and host', async () => {
  const a = application();
  await a.main.openProject('/fixture/new');
  assert.equal(a.main.state().host, a.previous);
  assert.equal(a.main.state().project, '/fixture/old');
  assert.equal(a.hosts.length, 0);
});

test('new editor dirt received while switch completes is retained', async () => {
  const a = application({ discard: true, dirtyDuringLoad: true });
  await a.main.openProject('/fixture/new');
  assert.equal(a.main.state().project, '/fixture/new');
  assert.equal(a.main.state().modified, true);
  assert.ok(a.calls.findIndex(c => c[0] === 'previous-stop') < a.calls.findIndex(c => c[0] === 'load'));
});

test('failed candidate startup and page load both clean the candidate and return launcher', async () => {
  for (const option of [{ startError: true }, { loadError: true }]) {
    const a = application({ discard: true, ...option });
    await a.main.openProject('/fixture/new');
    assert.equal(a.main.state().host, null);
    assert.equal(a.main.state().project, null);
    assert.equal(a.main.state().origin, null);
    assert.ok(a.calls.some(c => c[0] === 'stop'));
    assert.ok(a.calls.some(c => c[0] === 'load' && c[1].startsWith('file:')));
  }
});

test('recent-list disk failure does not shut down a healthy newly opened project', async () => {
  const a = application({ discard: true, recentError: true });
  await a.main.openProject('/fixture/new');
  assert.equal(a.main.state().host, a.hosts[0]);
  assert.equal(a.main.state().project, '/fixture/new');
  assert.equal(a.calls.some(c => c[0] === 'stop'), false);
});

test('unexpected engine exit preserves modified Monaco document instead of navigating away', async () => {
  const a = application({ discard: true, dirtyDuringLoad: true });
  await a.main.openProject('/fixture/new');
  const editorUrl = a.window.webContents.mainFrame.url;
  a.hosts[0].exitResult = { code: 1 };
  a.hosts[0].emit('host-exit', { code: 1, signal: null });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(a.window.webContents.mainFrame.url, editorUrl);
  assert.equal(a.main.state().modified, true);
  assert.ok(a.calls.some(c => c[0] === 'dialog' && c[1].title === 'Engine stopped'));
});
