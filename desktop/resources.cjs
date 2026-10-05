const fs = require('node:fs/promises');
const path = require('node:path');

function runtimePaths({ packaged, resourcesPath, repo, env = process.env }) {
  const base = packaged ? resourcesPath : repo;
  const suffix = process.platform === 'win32' ? '.exe' : '';
  return {
    pocket: env.AMORIS_POCKET_BINARY || path.join(base, packaged ? `pocket${suffix}` : `target/release/pocket${suffix}`),
    editor: env.AMORIS_EDITOR_DIST || path.join(base, packaged ? 'editor' : 'editor/dist'),
    viewport: env.AMORIS_WEB_VIEWPORT || path.join(base, packaged ? 'viewport' : 'web/viewport'),
    tsc: env.AMORIS_TSC || path.join(base, packaged ? `typescript/tsc${suffix}` :
      `sdk/node_modules/@typescript/typescript-${process.platform}-${process.arch}/lib/tsc${suffix}`),
  };
}

async function validateResources(resources) {
  const files = [resources.pocket, path.join(resources.editor, 'index.html'),
    path.join(resources.viewport, 'pkg/pocket_viewport_bg.wasm'),
    path.join(resources.viewport, 'pkg/pocket_viewport.js'), resources.tsc,
    path.join(resources.viewport, 'pocket_web.js'),
    path.join(path.dirname(resources.tsc), 'lib.esnext.d.ts')];
  for (const file of files) {
    if (!(await fs.stat(file).catch(() => null))?.isFile()) {
      throw new Error(`Required desktop resource is missing: ${file}`);
    }
  }
  return Object.fromEntries(await Promise.all(Object.entries(resources).map(async ([key, value]) =>
    [key, await fs.realpath(value)])));
}

module.exports = { runtimePaths, validateResources };
