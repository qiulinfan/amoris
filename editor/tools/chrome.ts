// Finding and starting headless Chrome for the editor's browser checks: CHROME picks the browser,
// CHROME_FLAGS adds switches (--force_high_performance_gpu on a two-GPU Windows laptop).

import { existsSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

export function chromePath(): string {
  if (process.env.CHROME) return process.env.CHROME;
  const candidates =
    process.platform === "win32"
      ? [
          join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google\\Chrome\\Application\\chrome.exe"),
          join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Google\\Chrome\\Application\\chrome.exe"),
          join(process.env.LOCALAPPDATA ?? "", "Google\\Chrome\\Application\\chrome.exe"),
        ]
      : process.platform === "darwin"
        ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
        : ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"];
  const found = candidates.find((p) => existsSync(p));
  if (!found) throw new Error(`no Chrome found (tried ${candidates.join(", ")}); set CHROME=<path>`);
  return found;
}

/** The extra switches CHROME_FLAGS names. */
export function chromeFlags(): string[] {
  return (process.env.CHROME_FLAGS ?? "").split(" ").filter(Boolean);
}

/** A local TCP port free at the time of asking, for --remote-debugging-port. */
export async function freePort(): Promise<number> {
  return await new Promise((done, fail) => {
    const server = createServer();
    server.once("error", fail);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : 0;
      server.close(() => done(port));
    });
  });
}
