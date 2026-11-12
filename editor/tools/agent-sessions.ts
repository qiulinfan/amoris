// The Agent panel against a real host (editor.md 3, server.md 3.5): an editor that connects after
// agents called the host shows the host's sessions and what they called. Before any page exists,
// this script registers a bridge session over `/api/call`, makes one call that succeeds and one
// that is refused with its handle, and opens and closes a second session. Then headless Chrome
// opens the editor at --url (the host serving the editor's production build made with
// VITE_AMORIS_EXPOSE=1) and the Agent panel is compared, row by row and line by line, with what the
// host answers to `agent.session.list` and `agent.session.events`. One more call must raise the
// first session's count by a push, and closing it must take its live dot away.
//
//   amoris serve <a scratch copy of a project> --port 7950 --editor <editor/dist>
//   bun tools/agent-sessions.ts --url http://127.0.0.1:7950/ --out ../out/editor/agent \
//     [--cdp-port 9330]
//
// CHROME=<path> picks the browser; CHROME_FLAGS adds switches. Writes agent-sessions.png and
// agent-sessions.json into --out; exits 1 when a check fails.

import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Cdp } from "./cdp";
import { chromeFlags, chromePath, freePort } from "./chrome";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7950/");
const OUT = resolve(opt("--out", "../out/editor/agent"));
const CDP_PORT = Number(opt("--cdp-port", "0"));
const W = 1600;
const H = 1000;
mkdirSync(OUT, { recursive: true });

interface Row {
  id: string;
  transport: string;
  label: string;
  seat: string | null;
  calls: number;
  failed_calls: number;
  live: boolean;
}

/** `POST /api/call`: the result, or the refusal's code. */
async function call(method: string, params: unknown = {}, handle?: string): Promise<{ result?: unknown; error?: { code: string; message: string } }> {
  const body = JSON.stringify(handle === undefined ? { id: 1, method, params } : { id: 1, method, params, handle });
  const r = await fetch(new URL("/api/call", URL_), { method: "POST", headers: { "content-type": "application/json" }, body });
  return (await r.json()) as { result?: unknown; error?: { code: string; message: string } };
}

async function ok<T>(method: string, params: unknown = {}, handle?: string): Promise<T> {
  const a = await call(method, params, handle);
  if (a.error) throw new Error(`${method}: ${a.error.code}: ${a.error.message}`);
  return a.result as T;
}

const failures: string[] = [];
function check(what: string, pass: boolean, detail: unknown = "") {
  console.log(`  ${pass ? "ok  " : "FAIL"} ${what}${pass || detail === "" ? "" : `: ${JSON.stringify(detail)}`}`);
  if (!pass) failures.push(what);
}

// ---- Before any editor: sessions and their calls ------------------------------------------------

console.log(`agent sessions at ${URL_}`);
const before = await ok<{ sessions: Row[]; last: number }>("agent.session.list");
const first = await ok<Row & { handle: string }>("agent.session.open", { label: "agent-sessions harness 1" });
await ok("world.tree", {}, first.handle);
const refused = await call("world.get", { entity: "Nobody-at-all" }, first.handle);
check("the session's refused call is refused", refused.error !== undefined, refused);
const second = await ok<Row & { handle: string }>("agent.session.open", { label: "agent-sessions harness 2" });
await ok("agent.session.close", { handle: second.handle });
console.log(`  opened ${first.id} and ${second.id} (closed) beside ${before.sessions.length} sessions already listed`);

// ---- The editor, opened afterwards -------------------------------------------------------------

const port = CDP_PORT || (await freePort());
const profile = mkdtempSync(join(tmpdir(), "amoris-agent-"));
const proc = Bun.spawn(
  [
    chromePath(),
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    "--enable-unsafe-webgpu",
    "--no-first-run",
    "--no-default-browser-check",
    "--hide-scrollbars",
    `--window-size=${W},${H}`,
    ...chromeFlags(),
    "about:blank",
  ],
  { stdout: "ignore", stderr: "ignore" },
);
/** Ends Chrome (this script's own process) and removes its profile. */
async function closeChrome() {
  proc.kill();
  await proc.exited;
  rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}

let exit = 1;
try {
  let pageWs = "";
  for (let i = 0; i < 100 && !pageWs; i++) {
    try {
      const list = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) as { type: string; webSocketDebuggerUrl: string }[];
      pageWs = list.find((t) => t.type === "page")?.webSocketDebuggerUrl ?? "";
    } catch {
      // not up yet
    }
    if (!pageWs) await Bun.sleep(100);
  }
  if (!pageWs) throw new Error("Chrome did not start");
  const cdp = await Cdp.connect(pageWs);

  async function js<T = unknown>(expression: string): Promise<T> {
    const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
      expression: `(async () => { ${expression} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
    return r.result.value as T;
  }

  /** Waits until `expression` (a page expression) is truthy; false after `ms`. */
  async function until(expression: string, ms = 15000): Promise<boolean> {
    const end = Date.now() + ms;
    while (Date.now() < end) {
      if (await js<boolean>(`return !!(${expression});`)) return true;
      await Bun.sleep(100);
    }
    return false;
  }

  /** The panel's session rows as the page shows them. */
  const shownRows = () =>
    js<{ id: string; live: boolean; text: string }[]>(`
      return [...document.querySelectorAll('.agent-panel .ag-session')].map((el) => ({
        id: el.getAttribute('data-session'),
        live: !!el.querySelector('.ag-live.is-live'),
        text: el.textContent,
      }));
    `);
  const shownLines = () => js<number>(`return document.querySelectorAll('.agent-panel .ag-line').length;`);

  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.navigate", { url: URL_ });
  if (!(await until("window.amoris && window.amoris.connection.getState().info.state === 'open'", 30000))) {
    throw new Error("the editor did not connect (is the host serving a build made with VITE_AMORIS_EXPOSE=1?)");
  }
  await js(`amoris.openPanel('agent'); amoris.toggleMaximize('agent');`);
  check("the Agent panel lists sessions", await until(`document.querySelector('.agent-panel .ag-session[data-session="${first.id}"]')`));

  // What the host says, and what the page shows.
  const listed = await ok<{ sessions: Row[]; last: number }>("agent.session.list");
  const stream = await ok<{ events: { seq: number }[]; last: number }>("agent.session.events", { limit: 2000 });
  const rows = await shownRows();
  check(
    "the panel shows the host's sessions",
    JSON.stringify(rows.map((r) => r.id).sort()) === JSON.stringify(listed.sessions.map((s) => s.id).sort()),
    { shown: rows.map((r) => r.id), host: listed.sessions.map((s) => s.id) },
  );
  // The feed shows its newest 400 lines.
  const lines = await shownLines();
  check("the panel shows the host's stream", lines === Math.min(400, stream.events.length), { shown: lines, host: stream.events.length });
  const one = rows.find((r) => r.id === first.id);
  check("the first session shows its label", !!one && one.text.includes("agent-sessions harness 1"), one);
  check("the first session shows 2 calls, 1 failed", !!one && one.text.includes("2 calls") && one.text.includes("1 failed"), one);
  check("the first session is live", !!one && one.live, one);
  const two = rows.find((r) => r.id === second.id);
  check("the closed session is listed and not live", !!two && !two.live, two);
  for (const s of listed.sessions) {
    const shown = rows.find((r) => r.id === s.id);
    check(`${s.id} shows the host's live state`, !!shown && shown.live === s.live, { shown, host: s });
  }
  const png = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(join(OUT, "agent-sessions.png"), Buffer.from(png.data, "base64"));
  console.log("  agent-sessions.png");

  // What follows arrives by push.
  await ok("world.tree", {}, first.handle);
  check(
    "one more call raises the count by a push",
    await until(`[...document.querySelectorAll('.agent-panel .ag-session[data-session="${first.id}"]')].some((el) => el.textContent.includes('3 calls'))`),
    await shownRows(),
  );
  check("its two lines are in the feed", (await shownLines()) === Math.min(400, stream.events.length + 2), await shownLines());
  await ok("agent.session.close", { handle: first.handle });
  check(
    "closing the session takes its live dot away",
    await until(`(() => { const el = document.querySelector('.agent-panel .ag-session[data-session="${first.id}"]'); return el && !el.querySelector('.ag-live.is-live'); })()`),
    await shownRows(),
  );
  const after = await ok<{ sessions: Row[]; last: number }>("agent.session.list");
  await Bun.write(
    join(OUT, "agent-sessions.json"),
    JSON.stringify({ url: URL_, opened: [first.id, second.id], host: after, shown: await shownRows(), failures }, null, 2),
  );
  cdp.close();
  exit = failures.length === 0 ? 0 : 1;
} finally {
  // Leave no session of this run live, whatever happened.
  await call("agent.session.close", { handle: first.handle }).catch(() => undefined);
  await closeChrome();
}
console.log(failures.length === 0 ? "agent sessions: every check passed" : `agent sessions: ${failures.length} failed: ${failures.join("; ")}`);
process.exit(exit);
