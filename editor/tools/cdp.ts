// A minimal Chrome DevTools Protocol client over Bun's WebSocket, and a headless Chrome launcher,
// for tools/capture.ts. No dependency beyond a local Chrome.

import type { Subprocess } from "bun";

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

export async function launchChrome(port: number): Promise<{ proc: Subprocess; pageWs: string }> {
  const chrome = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  const profile = `${process.env.TMPDIR ?? "/tmp"}/pocket-capture-${Date.now()}`;
  const proc = Bun.spawn(
    [chrome, "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", "--window-size=1600,1000", "about:blank"],
    { stdout: "ignore", stderr: "ignore" },
  );
  for (let i = 0; i < 100; i++) {
    try {
      const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
      const page = list.find((t) => t.type === "page");
      if (page) return { proc, pageWs: page.webSocketDebuggerUrl };
    } catch {
      // not up yet
    }
    await Bun.sleep(100);
  }
  proc.kill();
  throw new Error("Chrome did not start");
}
