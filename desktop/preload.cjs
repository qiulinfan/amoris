const { contextBridge, ipcRenderer } = require('electron');
const commands = new Set(['file.save', 'edit.undo', 'edit.redo', 'edit.palette',
  'game.play', 'game.pause', 'game.step', 'game.stop', 'help.shortcuts', 'help.about']);
contextBridge.exposeInMainWorld('amorisDesktop', Object.freeze({
  platform: process.platform,
  openProject: () => ipcRenderer.invoke('amoris:open-project'),
  listRecent: () => ipcRenderer.invoke('amoris:list-recent'),
  getIcon: () => ipcRenderer.invoke('amoris:icon'),
  openRecent: index => ipcRenderer.invoke('amoris:open-recent', index),
  setModified: modified => {
    if (typeof modified === 'boolean') ipcRenderer.send('amoris:modified', modified);
  },
  onCommand: callback => {
    if (typeof callback !== 'function') throw new TypeError('Expected a callback');
    const listener = (_event, command) => { if (commands.has(command)) callback(command); };
    ipcRenderer.on('amoris:command', listener);
    return () => ipcRenderer.removeListener('amoris:command', listener);
  },
  onLauncherError: callback => {
    if (typeof callback !== 'function') throw new TypeError('Expected a callback');
    const listener = (_event, message) => { if (typeof message === 'string') callback(message.slice(0, 8192)); };
    ipcRenderer.on('amoris:launcher-error', listener);
    return () => ipcRenderer.removeListener('amoris:launcher-error', listener);
  },
}));
