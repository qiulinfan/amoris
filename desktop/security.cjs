const COMMANDS = Object.freeze(['file.save', 'edit.undo', 'edit.redo', 'edit.palette',
  'game.play', 'game.pause', 'game.step', 'game.stop', 'help.shortcuts', 'help.about']);

function allowedNavigation(url, launcherUrl, origin) {
  if (url === launcherUrl) return true;
  try {
    const parsed = new URL(url);
    return Boolean(origin && !parsed.username && !parsed.password && parsed.origin === origin &&
      ['/', '/index.html'].includes(parsed.pathname) &&
      ['', '?viewport=wasm'].includes(parsed.search));
  } catch { return false; }
}

function authorizedSender(event, window, launcherUrl, origin) {
  return !window.isDestroyed() && event.sender === window.webContents &&
    event.senderFrame === window.webContents.mainFrame &&
    allowedNavigation(event.senderFrame.url, launcherUrl, origin);
}

function allowedClipboardWrite(contents, permission, details, window, launcherUrl, origin) {
  return Boolean(window && !window.isDestroyed() && origin &&
    permission === 'clipboard-sanitized-write' && contents === window.webContents &&
    details?.isMainFrame === true && details.requestingUrl === contents.mainFrame.url &&
    details.requestingUrl !== launcherUrl &&
    allowedNavigation(details.requestingUrl, launcherUrl, origin));
}

module.exports = { COMMANDS, allowedNavigation, authorizedSender, allowedClipboardWrite };
