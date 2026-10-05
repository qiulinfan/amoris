const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { HostProcess } = require('../host.cjs');

test('packaged runtime edits, type-checks, loads viewport assets, and stops an external project', {
  skip: !process.env.AMORIS_TEST_RESOURCES, timeout: 60000,
}, async () => {
  const resourcesDir = process.env.AMORIS_TEST_RESOURCES;
  const resources = {
    pocket: path.join(resourcesDir, 'pocket'), editor: path.join(resourcesDir, 'editor'),
    viewport: path.join(resourcesDir, 'viewport'), tsc: path.join(resourcesDir, 'typescript/tsc'),
  };
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'amoris-desktop-runtime-'));
  const project = path.join(temporary, 'Sailing');
  await fs.cp(path.resolve(__dirname, '../../samples/sailing'), project, { recursive: true });
  const host = new HostProcess(resources);
  try {
    const ready = await host.start(project);
    const call = async (method, params = {}) => {
      const response = await fetch(ready.origin + '/api/call', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: 1, method, params }), signal: AbortSignal.timeout(10000),
      });
      assert.equal(response.status, 200);
      const data = await response.json();
      assert.equal(data.error, undefined, JSON.stringify(data.error));
      return data.result;
    };
    assert.equal((await call('project.info')).root, await fs.realpath(project));
    await assert.rejects(new HostProcess(resources).start(project), /already open/);
    const module = await fetch(ready.origin + '/wasm/pocket_web.js');
    assert.equal(module.status, 200);
    assert.match(await module.text(), /createViewport/);
    const wasm = await fetch(ready.origin + '/wasm/pkg/pocket_viewport_bg.wasm');
    assert.equal(wasm.status, 200);
    assert.deepEqual([...new Uint8Array(await wasm.arrayBuffer()).slice(0, 4)], [0, 97, 115, 109]);
    await call('scripts.types', { tsconfig: true });
    const check = await call('scripts.check');
    assert.equal(check.typecheck, 'ok', JSON.stringify(check));
    assert.equal(check.tsc_version, '7.0.2');
    const before = await call('time.control');
    await call('world.edit', { label: 'Desktop test', ops: [{ spawn: { name: 'Desktop test entity' } }] });
    assert.notEqual((await call('time.control')).world_hash, before.world_hash);
    await call('history.undo');
    // Undo restores content while intentionally preserving entity allocator history.
    assert.equal((await call('world.tree')).some(entity => entity.name === 'Desktop test entity'), false);
    const edit = await call('time.control');
    assert.equal((await call('play.start')).mode, 'play');
    await call('time.control', { pause: true });
    const stepped = await call('time.step', { ticks: 3 });
    assert.ok(stepped.tick >= 3);
    const stopped = await call('play.stop');
    assert.equal(stopped.mode, 'edit');
    assert.equal(stopped.world_hash, edit.world_hash);
    await host.stop();
    assert.ok(host.exitResult);
    assert.equal(await fs.stat(path.join(project, '.pocket/host.json')).catch(() => null), null);
  } finally {
    await host.stop();
    await fs.rm(temporary, { recursive: true, force: true });
  }
});
