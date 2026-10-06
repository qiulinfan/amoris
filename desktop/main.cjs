const { app, BrowserWindow, Menu, dialog, ipcMain, nativeImage, session } = require('electron');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { HostProcess, validateProject, alreadyServed } = require('./host.cjs');
const { runtimePaths, validateResources } = require('./resources.cjs');
const { COMMANDS, allowedNavigation, authorizedSender, allowedClipboardWrite } = require('./security.cjs');

app.setName('Amoris');
app.enableSandbox();
let window, host = null, origin = null, project = null, modified = false;
let busy = false, allowClose = false, allowQuit = false, recents = [], operation = Promise.resolve();
const launcherUrl = pathToFileURL(path.join(__dirname, 'launcher/index.html')).href;
const brandIcon = process.platform === 'darwin' ? 'amoris-icon-macos.png' : 'amoris-icon-morandi.png';
const icon = app.isPackaged ? path.join(__dirname, 'assets/icon.png') :
  process.env.AMORIS_BRAND_ICON || path.join(__dirname, '../assets/branding', brandIcon);

function serialize(action) {
  const next = operation.then(action, action);
  operation = next.catch(() => {});
  return next;
}

function checkSender(event, launcherOnly = false) {
  if (!window || !authorizedSender(event, window, launcherUrl, origin) ||
      (launcherOnly && event.senderFrame.url !== launcherUrl)) throw new Error('Unauthorized desktop request');
}

async function persistRecent(root) {
  recents = [root, ...recents.filter(item => item !== root)].slice(0, 12);
  const directory = app.getPath('userData');
  await fs.mkdir(directory, { recursive: true });
  const file = path.join(directory, 'recent-projects.json');
  await fs.writeFile(file + '.tmp', JSON.stringify(recents), { mode: 0o600 });
  await fs.rename(file + '.tmp', file);
  app.addRecentDocument(root);
}

async function discardConfirmed() {
  if (!modified) return true;
  const result = await dialog.showMessageBox(window, {
    type: 'warning', title: 'Unsaved changes', message: 'Discard unsaved changes?',
    detail: 'Unsaved script buffers and scene edits will be lost when this project closes.',
    buttons: ['Cancel', 'Discard'], defaultId: 0, cancelId: 0, noLink: true,
  });
  return result.response === 1;
}

function command(id) {
  if (!COMMANDS.includes(id) || !host || host.exitResult || busy) return;
  window.webContents.send('amoris:command', id);
}

function buildMenu() {
  const active = Boolean(host && !host.exitResult && !busy);
  const item = (label, id, accelerator) => ({ label, accelerator, enabled: active, click: () => command(id) });
  const template = [
    ...(process.platform === 'darwin' ? [{ label: 'Amoris', submenu: [
      { label: 'About Amoris', click: () => active ? command('help.about') : app.showAboutPanel() },
      { type: 'separator' }, { role: 'services' }, { type: 'separator' },
      { role: 'hide' }, { role: 'hideOthers' }, { role: 'unhide' }, { type: 'separator' }, { role: 'quit' },
    ] }] : []),
    { label: 'File', submenu: [
      { label: 'Open Project…', accelerator: 'CmdOrCtrl+O', click: () => pickProject() },
      { label: 'Open Recent', submenu: recents.length ? recents.map(root => ({
        label: root, click: () => openProject(root),
      })) : [{ label: 'No recent projects', enabled: false }] },
      { type: 'separator' }, item('Save', 'file.save', 'CmdOrCtrl+S'),
      { type: 'separator' }, { role: 'close' },
    ] },
    { label: 'Edit', submenu: [item('Undo Scene', 'edit.undo'),
      item('Redo Scene', 'edit.redo'), { type: 'separator' },
      { role: 'cut' }, { role: 'copy' }, { role: 'paste' }] },
    { label: 'View', submenu: [item('Command Palette…', 'edit.palette', 'CmdOrCtrl+K'),
      { type: 'separator' }, { role: 'togglefullscreen' }] },
    { label: 'Game', submenu: [item('Play', 'game.play'), item('Pause', 'game.pause'),
      item('Step', 'game.step'), item('Stop', 'game.stop')] },
    { role: 'windowMenu' },
    { label: 'Help', submenu: [item('Keyboard Shortcuts', 'help.shortcuts'),
      { label: 'About Amoris', click: () => active ? command('help.about') : app.showAboutPanel() }] },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

async function showLauncher(error = '') {
  origin = null;
  project = null;
  modified = false;
  window.setDocumentEdited(false);
  window.setTitle('Amoris');
  await window.loadURL(launcherUrl);
  if (error) window.webContents.send('amoris:launcher-error', error);
  buildMenu();
}

async function openProject(input) {
  return serialize(async () => {
    let next = null;
    try {
      const root = await validateProject(input);
      if (root === project && host && !host.exitResult) { window.show(); return; }
      const resources = await validateResources(runtimePaths({ packaged: app.isPackaged,
        resourcesPath: process.resourcesPath, repo: path.resolve(__dirname, '..') }));
      if (await alreadyServed(root)) throw new Error('Project already open in another host. Close that host first.');
      if (!await discardConfirmed()) return;
      busy = true;
      buildMenu();
      const previous = host;
      host = null;
      if (previous) await previous.stop();
      origin = null;
      project = null;
      modified = false;
      window.setDocumentEdited(false);
      next = new HostProcess(resources);
      host = next;
      next.on('host-exit', async ({ code, signal }) => {
        if (host !== next) return;
        await next.stop();
        buildMenu();
        const text = `The engine stopped unexpectedly (${signal || code}).`;
        if (modified) {
          await dialog.showMessageBox(window, { type: 'error', title: 'Engine stopped',
            message: text, detail: 'The editor remains open so you can copy unsaved script text. Use Open Project when ready.' });
        } else await showLauncher(text);
      });
      const ready = await next.start(root);
      origin = ready.origin;
      project = root;
      window.setTitle(`Amoris — ${path.basename(root)}`);
      await window.loadURL(origin + '/?viewport=wasm');
      await persistRecent(root).catch(() => console.warn('Could not store recent projects.'));
    } catch (error) {
      if (next) {
        if (host === next) host = null;
        await next.stop();
        origin = null;
        project = null;
      }
      dialog.showErrorBox('Could not open project', error.message);
      if (!project) await showLauncher();
    } finally { busy = false; buildMenu(); }
  });
}

async function pickProject() {
  const selected = await dialog.showOpenDialog(window, { title: 'Open Amoris project',
    buttonLabel: 'Open Project', properties: ['openDirectory'] });
  if (!selected.canceled && selected.filePaths[0]) await openProject(selected.filePaths[0]);
}

function requestClose(quit = false) {
  return serialize(async () => {
    if (!await discardConfirmed()) return;
    busy = true;
    const previous = host;
    host = null;
    if (previous) await previous.stop();
    allowClose = true;
    if (quit) { allowQuit = true; app.quit(); }
    else window.close();
  });
}

ipcMain.handle('amoris:open-project', async event => { checkSender(event); await pickProject(); });
ipcMain.handle('amoris:list-recent', event => {
  checkSender(event, true);
  return recents.map((root, index) => ({ index, name: path.basename(root), path: root }));
});
ipcMain.handle('amoris:icon', event => {
  checkSender(event, true);
  return nativeImage.createFromPath(icon).toDataURL();
});
ipcMain.handle('amoris:open-recent', async (event, index) => {
  checkSender(event, true);
  if (!Number.isInteger(index) || index < 0 || index >= recents.length) throw new Error('Invalid recent project');
  await openProject(recents[index]);
});
ipcMain.on('amoris:modified', (event, value) => {
  try {
    checkSender(event);
    if (typeof value !== 'boolean' || !project) return;
    modified = value;
    window.setDocumentEdited(value);
  } catch {}
});

if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on('second-instance', () => { if (window) { window.show(); window.focus(); } });
  app.on('before-quit', event => { if (!allowQuit && window && !window.isDestroyed()) {
    event.preventDefault(); requestClose(true);
  } });
  app.on('window-all-closed', () => { if (process.platform !== 'darwin') { allowQuit = true; app.quit(); } });
  app.on('activate', () => { if (!window || window.isDestroyed()) createWindow(); else window.show(); });
  app.whenReady().then(async () => {
    app.setAboutPanelOptions({ applicationName: 'Amoris', applicationVersion: app.getVersion(),
      copyright: 'Copyright © 2026 Qiulin Fan', iconPath: icon });
    if (process.platform === 'darwin') app.dock.setIcon(nativeImage.createFromPath(icon));
    session.defaultSession.setPermissionRequestHandler((contents, permission, callback, details) =>
      callback(allowedClipboardWrite(contents, permission, details, window, launcherUrl, origin)));
    session.defaultSession.setPermissionCheckHandler((contents, permission, _requestingOrigin, details) =>
      allowedClipboardWrite(contents, permission, details, window, launcherUrl, origin));
    session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
      const headers = { ...details.responseHeaders };
      if (origin && new URL(details.url).origin === origin) {
        headers['Content-Security-Policy'] = ["default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ws://127.0.0.1:*; worker-src 'self' blob:; object-src 'none'; frame-src 'none'"];
      }
      callback({ responseHeaders: headers });
    });
    try { recents = JSON.parse(await fs.readFile(path.join(app.getPath('userData'), 'recent-projects.json'), 'utf8'))
      .filter(item => typeof item === 'string').slice(0, 12); } catch {}
    await createWindow();
    const args = process.argv.slice(app.isPackaged ? 1 : 2);
    const index = args.indexOf('--project');
    if (index >= 0 && args[index + 1]) await openProject(args[index + 1]);
    else if (args[0] && !args[0].startsWith('-')) await openProject(args[0]);
  });
}

async function createWindow() {
  allowClose = false;
  window = new BrowserWindow({ width: 1440, height: 920, minWidth: 960, minHeight: 620,
    title: 'Amoris', backgroundColor: '#171a1f', icon,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true,
      nodeIntegration: false, sandbox: true, webSecurity: true, webviewTag: false } });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event, url) => {
    if (!allowedNavigation(url, launcherUrl, origin)) event.preventDefault();
  });
  window.webContents.on('before-input-event', (event, input) => {
    if ((input.meta || input.control) && input.key.toLowerCase() === 'r') event.preventDefault();
  });
  window.webContents.on('render-process-gone', async () => {
    const previous = host; host = null;
    if (previous) await previous.stop();
    dialog.showErrorBox('Editor stopped', 'The editor renderer stopped unexpectedly. Open the project again when ready.');
    if (!modified) await showLauncher();
  });
  window.on('close', event => { if (!allowClose) { event.preventDefault(); requestClose(); } });
  await showLauncher();
}
