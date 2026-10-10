// Produce a self-contained application using the current platform's verified native toolchain.
import { packager } from '@electron/packager';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { runtimePaths, validateResources } = require('./resources.cjs');
const desktop = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(desktop, '..');
const output = path.join(repo, 'out/desktop');
const staging = path.join(output, 'staging');
const application = path.join(staging, 'app');
const runtime = path.join(staging, 'runtime');
const source = await validateResources(runtimePaths({ packaged: false, repo }));
const packageInfo = JSON.parse(await fs.readFile(path.join(desktop, 'package.json'), 'utf8'));
const executable = process.platform === 'win32' ? '.exe' : '';

// These are generated staging directories only. Project files and installed source dependencies
// never enter the bundle, and third-party scene assets are opened from the user's project.
await fs.rm(staging, { recursive: true, force: true });
await fs.mkdir(path.join(application, 'assets'), { recursive: true });
await fs.mkdir(runtime, { recursive: true });
for (const file of ['main.cjs', 'host.cjs', 'preload.cjs', 'security.cjs', 'resources.cjs']) {
  await fs.copyFile(path.join(desktop, file), path.join(application, file));
}
await fs.cp(path.join(desktop, 'launcher'), path.join(application, 'launcher'), { recursive: true });
const brandIcon = process.platform === 'darwin' ? 'amoris-icon-macos.png' : 'amoris-icon-morandi.png';
await fs.copyFile(path.join(repo, 'assets/branding', brandIcon),
  path.join(application, 'assets/icon.png'));
await fs.copyFile(path.join(repo, 'LICENSE'), path.join(application, 'LICENSE'));
await fs.writeFile(path.join(application, 'package.json'), JSON.stringify({
  name: packageInfo.name, productName: 'Amoris', version: packageInfo.version,
  description: packageInfo.description, author: packageInfo.author,
  license: packageInfo.license, main: 'main.cjs',
}, null, 2) + '\n');

const destinations = {
  pocket: path.join(runtime, 'pocket' + executable),
  editor: path.join(runtime, 'editor'), viewport: path.join(runtime, 'viewport'),
  typescript: path.join(runtime, 'typescript'),
};
await fs.copyFile(source.pocket, destinations.pocket);
await fs.chmod(destinations.pocket, 0o755);
await fs.cp(source.editor, destinations.editor, { recursive: true });
await fs.cp(source.viewport, destinations.viewport, { recursive: true });
await fs.cp(path.dirname(source.tsc), destinations.typescript, { recursive: true });
await fs.chmod(path.join(destinations.typescript, 'tsc' + executable), 0o755);
const tscPackage = path.dirname(path.dirname(source.tsc));
for (const name of ['LICENSE', 'NOTICE.txt']) {
  const license = path.join(tscPackage, name);
  if (await fs.stat(license).catch(() => null)) await fs.copyFile(license, path.join(destinations.typescript, name));
}
const compilerVersion = execFileSync(source.tsc, ['--version'], { encoding: 'utf8' }).trim();
if (compilerVersion !== 'Version 7.0.2') throw new Error(`Expected the pinned TypeScript 7.0.2, got ${compilerVersion}`);

let icon = path.join(application, 'assets/icon.png');
if (process.platform === 'darwin') {
  const iconset = path.join(staging, 'Amoris.iconset');
  await fs.mkdir(iconset);
  for (const size of [16, 32, 128, 256, 512]) {
    for (const scale of [1, 2]) {
      execFileSync('/usr/bin/sips', ['-z', String(size * scale), String(size * scale), icon,
        '--out', path.join(iconset, `icon_${size}x${size}${scale === 2 ? '@2x' : ''}.png`)], { stdio: 'ignore' });
    }
  }
  icon = path.join(staging, 'Amoris.icns');
  execFileSync('/usr/bin/iconutil', ['-c', 'icns', iconset, '-o', icon]);
}

const apps = await packager({
  dir: application, name: 'Amoris', out: output,
  platform: process.platform, arch: process.arch, electronVersion: packageInfo.devDependencies.electron,
  appBundleId: 'dev.amoris.editor', appCategoryType: 'public.app-category.developer-tools',
  appCopyright: 'Copyright © 2026 Qiulin Fan', icon,
  asar: false, prune: false, overwrite: true,
  extraResource: Object.values(destinations),
});
const receipt = { applications: apps, platform: process.platform, arch: process.arch,
  electron: packageInfo.devDependencies.electron, typescript: compilerVersion,
  distributionSigned: false, notarized: false };
await fs.writeFile(path.join(output, 'package-receipt.json'), JSON.stringify(receipt, null, 2) + '\n');
console.log(JSON.stringify(receipt, null, 2));
