// The live profiler against a real host (docs/bench/profiler.md): headless Chrome (WebGPU on)
// opens the editor the host serves, Play starts through the host's API, the Profiler panel is
// shown, and after a few seconds the window, the panel and the viewport are captured with the
// panel's text and the viewport's stats overlay. Right after reading the panel, in the same Play
// with the editor connected, the host's own reads: `profile.frame` over the panel's window (180
// ticks, 3 s) and over 600 ticks, and with `--pocket` the CLI's `pocket profile` over the same
// windows (compare.json lines them up). `play-early.json` is `profile.frame` 3 s into Play, its first
// tick included.
//
//   pocket serve samples/sailing --port 7878     # editor/dist built (bun run build)
//   bun tools/profiler-capture.ts --url http://127.0.0.1:7878/ --out ../out/liveprof/capture \
//     [--pocket ../target/release/pocket.exe --project ../samples/sailing] [--seconds 15]
//
// CHROME=<path> picks the browser (default: Chrome's install path on Windows, macOS or Linux);
// CHROME_FLAGS adds switches (e.g. --force_high_performance_gpu on a two-GPU laptop).

import { existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { Cdp } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7878/");
const OUT = resolve(opt("--out", "../out/liveprof/capture"));
const SECONDS = Number(opt("--seconds", "15"));
const POCKET = opt("--pocket", "");
const PROJECT = opt("--project", "");
const W = 1600;
const H = 1000;
mkdirSync(OUT, { recursive: true });

function chromePath(): string {
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

const port = 9400 + Math.floor(Math.random() * 400);
const profile = join(tmpdir(), `pocket-profiler-${Date.now()}`);
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
    ...(process.env.CHROME_FLAGS ?? "").split(" ").filter(Boolean),
    "about:blank",
  ],
  { stdout: "ignore", stderr: "ignore" },
);
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
if (!pageWs) {
  proc.kill();
  throw new Error("Chrome did not start");
}
const cdp = await Cdp.connect(pageWs);
const logs: string[] = [];

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

type Rect = { x: number; y: number; width: number; height: number };

async function shot(name: string, clip?: Rect) {
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
  await Bun.write(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

/** The rect of the dock group showing a panel (tabs included). */
async function panelRect(id: string): Promise<Rect> {
  return js<Rect>(`
    const content = document.querySelector('.dock-panel.panel-${id}');
    const c = content.getBoundingClientRect();
    const group = [...document.querySelectorAll('.dv-groupview')].find((g) => {
      const r = g.getBoundingClientRect();
      return Math.abs(r.left - c.left) < 4 && Math.abs(r.right - c.right) < 4 && r.top <= c.top + 2 && r.bottom >= c.bottom - 2;
    });
    const r = (group ?? content).getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  `);
}

async function click(x: number, y: number) {
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"] as const) {
    await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: 1 });
  }
}

/** `pocket <args>` in the project (the CLI finds the host there): its output. */
async function cli(...rest: string[]): Promise<string> {
  const p = Bun.spawn([POCKET, ...rest], { cwd: PROJECT, stdout: "pipe", stderr: "pipe" });
  const [out, err] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text()]);
  await p.exited;
  return out + err;
}

type Summary = { tick: number; epoch?: number; ticks: number; tick_ms: number; busy_ms: number };

async function call(method: string, params: unknown = {}): Promise<unknown> {
  const r = (await (await fetch(new URL("/api/call", URL_), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method, params }) })).json()) as {
    result?: unknown;
    error?: { code: string; message: string };
  };
  if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
  return r.result;
}

try {
  cdp.on("Runtime.consoleAPICalled", (p) => {
    const m = p as { type: string; args: { value?: unknown; description?: string }[] };
    logs.push(`[${m.type}] ${m.args.map((a) => String(a.value ?? a.description ?? "")).join(" ")}`);
  });
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.navigate", { url: URL_ });
  await cdp.once("Page.loadEventFired");
  await js(`localStorage.clear();`);
  await cdp.send("Page.reload");
  await cdp.once("Page.loadEventFired");
  // The wasm viewport takes a moment to load and get its first full frame.
  let renderer = "";
  for (let i = 0; i < 60 && renderer !== "WebGPU"; i++) {
    await Bun.sleep(250);
    renderer = await js<string>(`return document.querySelector('.vp-renderer')?.textContent ?? '';`);
  }
  console.log(`capturing ${URL_} (viewport: ${renderer || "none"}) -> ${OUT}`);
  await call("play.start", {});
  const played = Date.now();
  const tab = await js<[number, number] | null>(`
    const t = [...document.querySelectorAll('.dv-tab')].find((e) => e.textContent.trim() === 'Profiler');
    if (!t) return null;
    const r = t.getBoundingClientRect();
    return [r.left + r.width / 2, r.top + r.height / 2];
  `);
  if (tab) await click(tab[0], tab[1]);
  // Give the bottom dock room for the profiler's columns: drag the sash above it up.
  const sash = await js<[number, number] | null>(`
    const g = document.querySelector('.dock-panel.panel-profiler').getBoundingClientRect();
    const s = [...document.querySelectorAll('.dv-sash')].map((e) => e.getBoundingClientRect())
      .find((r) => r.width > r.height && Math.abs(r.top + r.height / 2 - g.top) < 48 && r.left < g.right && r.right > g.left);
    return s ? [g.left + g.width / 2, s.top + s.height / 2] : null;
  `);
  if (sash) {
    const [x, y] = sash;
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y, button: "none", buttons: 0 });
    await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
    for (let i = 1; i <= 10; i++) await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y: y - i * 32, button: "left", buttons: 1 });
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y: y - 320, button: "left", buttons: 0, clickCount: 1 });
  }
  // Play's first ticks, the first one among them.
  await Bun.sleep(Math.max(0, played + 3000 - Date.now()));
  await Bun.write(join(OUT, "play-early.json"), `${JSON.stringify(await call("profile.frame", { ticks: 600 }), null, 2)}\n`);
  await Bun.sleep(Math.max(0, played + SECONDS * 1000 - Date.now()));
  // The panel's numbers and the host's over the same window, one right after the other.
  const text = await js<string>(`return document.querySelector('.profiler')?.innerText ?? '';`);
  const at = Date.now();
  const frame180 = (await call("profile.frame", { ticks: 180 })) as Summary;
  const cli180 = POCKET ? (JSON.parse(await cli("profile", "--ticks", "180", "--json")) as Summary) : null;
  const cliMs = Date.now() - at;
  const frame = (await call("profile.frame", { ticks: 600 })) as Summary;
  const table = POCKET ? await cli("profile", "--ticks", "600") : "";
  const overlay = await js<string>(`return document.querySelector('.vp-stats')?.innerText ?? '';`);
  await shot("editor-profiler");
  await shot("profiler-panel", await panelRect("profiler"));
  await shot("viewport", await panelRect("viewport"));
  const panel = /tick ([\d.]+) ms · thread ([\d.]+) ms/.exec(text);
  const lastTick = /tick (\d+)\s*$/m.exec(text.split("\n").slice(0, 3).join("\n"));
  const row = (s: Summary | null) => (s ? { tick: s.tick, epoch: s.epoch, ticks: s.ticks, tick_ms: s.tick_ms, busy_ms: s.busy_ms } : null);
  const compare = {
    note: "the panel: tick-weighted over its last 12 pushes (3 s); profile.frame and the CLI read 180 ticks right after it",
    panel: panel ? { tick_ms: Number(panel[1]), busy_ms: Number(panel[2]), last_tick: lastTick ? Number(lastTick[1]) : null } : null,
    profile_frame_180: row(frame180),
    cli_180: row(cli180),
    cli_read_ms_after_panel: cliMs,
    profile_frame_600: row(frame),
  };
  await Bun.write(join(OUT, "profiler-panel.txt"), `${text}\n`);
  await Bun.write(join(OUT, "viewport-stats.txt"), `${overlay}\n`);
  await Bun.write(join(OUT, "profile-frame.json"), `${JSON.stringify(frame, null, 2)}\n`);
  await Bun.write(join(OUT, "compare.json"), `${JSON.stringify(compare, null, 2)}\n`);
  if (table) await Bun.write(join(OUT, "pocket-profile.txt"), table);
  await Bun.write(join(OUT, "console.txt"), `${logs.join("\n")}\n`);
  console.log(overlay.replace(/\s+/g, " "));
  console.log(JSON.stringify(compare));
  if (table) console.log(table);
  await call("play.stop", {});
} finally {
  cdp.close();
  proc.kill();
}
