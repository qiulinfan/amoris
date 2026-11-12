// A minimal Chrome DevTools Protocol client over Bun's WebSocket, and a headless Chrome launcher,
// for the editor's browser checks. No dependency beyond a local Chrome (chrome.ts finds it).

import type { Subprocess } from "bun";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromeFlags, chromePath } from "./chrome";

export class Cdp {
  private id = 0;
  private pending = new Map<number, { resolve(v: unknown): void; reject(e: unknown): void }>();
  private handlers = new Map<string, ((p: unknown) => void)[]>();

  private constructor(private ws: WebSocket) {
    ws.onmessage = (ev) => {
      const msg = JSON.parse(String(ev.data)) as { id?: number; result?: unknown; error?: { message: string }; method?: string; params?: unknown };
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) p?.reject(new Error(msg.error.message));
        else p?.resolve(msg.result);
      } else if (msg.method) {
        for (const h of this.handlers.get(msg.method) ?? []) h(msg.params);
      }
    };
  }

  static connect(url: string): Promise<Cdp> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.onopen = () => resolve(new Cdp(ws));
      ws.onerror = (e) => reject(e);
    });
  }

  send<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Calls `fn` with every event `method` from now on. */
  on(method: string, fn: (p: unknown) => void) {
    this.handlers.set(method, [...(this.handlers.get(method) ?? []), fn]);
  }

  once(method: string): Promise<unknown> {
    return new Promise((resolve) => {
      const list = this.handlers.get(method) ?? [];
      const h = (p: unknown) => {
        this.handlers.set(method, (this.handlers.get(method) ?? []).filter((x) => x !== h));
        resolve(p);
      };
      list.push(h);
      this.handlers.set(method, list);
    });
  }

  close() {
    this.ws.close();
  }
}

/** Whether something accepts connections on a local TCP port. */
function listening(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = connect({ port, host: "127.0.0.1" });
    socket.once("connect", () => {
      socket.destroy();
      done(true);
    });
    socket.once("error", () => done(false));
  });
}

/** The process id of the browser whose debugger answers on `port`, once one does. */
async function browserPid(port: number): Promise<number | null> {
  try {
    const version = (await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()) as { webSocketDebuggerUrl: string };
    const browser = await Cdp.connect(version.webSocketDebuggerUrl);
    try {
      const info = await browser.send<{ processInfo: { type: string; id: number }[] }>("SystemInfo.getProcessInfo");
      return info.processInfo.find((p) => p.type === "browser")?.id ?? null;
    } finally {
      browser.close();
    }
  } catch {
    return null;
  }
}

/**
 * Starts a headless Chrome of the tool's own: the browser chrome.ts finds (CHROME) with its
 * CHROME_FLAGS and `extra` switches, on a fresh temporary profile, so it never touches a Chrome
 * somebody else runs. `port` 0 lets Chrome pick a free debugging port, which it writes into its
 * profile (`DevToolsActivePort`). A given port is refused when something already listens on it: a
 * second Chrome cannot bind it, and the tool would drive the browser that holds it. Either way the
 * browser that answers on the port must be the process started here before its page is handed out.
 * `close` ends that process and removes the profile, which stays locked until the process has
 * exited.
 */
export async function launchChrome(port = 0, extra: string[] = []): Promise<{ proc: Subprocess; pageWs: string; close(): Promise<void> }> {
  if (port && (await listening(port))) {
    throw new Error(`the CDP port ${port} is taken: something already listens on it (another tool's Chrome?); pass a free --chrome-port, or none for a free one`);
  }
  const profile = mkdtempSync(join(tmpdir(), "amoris-chrome-"));
  const proc = Bun.spawn(
    [
      chromePath(),
      "--headless=new",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--hide-scrollbars",
      "--window-size=1600,1000",
      ...extra,
      ...chromeFlags(),
      "about:blank",
    ],
    { stdout: "ignore", stderr: "ignore" },
  );
  const close = async () => {
    proc.kill();
    await proc.exited;
    rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  };
  /** The port this Chrome's debugger listens on: the one given, else the first line of the file
   * Chrome writes once it listens on the port it picked. */
  const debugPort = (): number | null => {
    if (port) return port;
    try {
      const n = Number(readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0]);
      return Number.isInteger(n) && n > 0 ? n : null;
    } catch {
      return null;
    }
  };
  for (let i = 0; i < 300 && proc.exitCode === null; i++) {
    const at = debugPort();
    const pid = at === null ? null : await browserPid(at);
    if (at !== null && pid !== null) {
      if (pid !== proc.pid) {
        // Bound by another browser between the check above and this Chrome's start.
        await close();
        throw new Error(`the CDP port ${at} is answered by another browser (process ${pid}), not the Chrome started here (${proc.pid}); pass a free --chrome-port`);
      }
      const list = (await (await fetch(`http://127.0.0.1:${at}/json/list`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
      const page = list.find((t) => t.type === "page");
      if (page) return { proc, pageWs: page.webSocketDebuggerUrl, close };
    }
    await Bun.sleep(100);
  }
  await close();
  throw new Error(port ? `Chrome did not open the CDP port ${port} (is it taken?)` : "Chrome did not start");
}
