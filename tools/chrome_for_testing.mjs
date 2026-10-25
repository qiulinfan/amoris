// Chrome for Testing as Puppeteer installs it (`npx @puppeteer/browsers install chrome@stable`, under
// `PUPPETEER_CACHE_DIR` or ~/.cache/puppeteer), newest version first. The headless checks prefer it to
// the installed Google Chrome: Google Chrome clones its app bundle into /var/folders on every launch
// and leaves the clone behind when a check kills it, while Chrome for Testing, which never
// auto-updates, makes no clone.
import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const executables = {
  darwin: [
    ["chrome-mac-arm64", "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"],
    ["chrome-mac-x64", "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"],
  ],
  win32: [["chrome-win64", "chrome.exe"]],
  linux: [["chrome-linux64", "chrome"]],
}[process.platform] || [];

const version = (dir) => dir.split("-").pop().split(".").map(Number);
const newestFirst = (a, b) => {
  const [x, y] = [version(a), version(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) if ((x[i] || 0) !== (y[i] || 0)) return (y[i] || 0) - (x[i] || 0);
  return 0;
};

export function chromeForTesting() {
  const root = join(process.env.PUPPETEER_CACHE_DIR || join(homedir(), ".cache", "puppeteer"), "chrome");
  if (!existsSync(root)) return [];
  return readdirSync(root)
    .sort(newestFirst)
    .flatMap((dir) => executables.map(([folder, exe]) => join(root, dir, folder, exe)))
    .filter((path) => existsSync(path));
}
