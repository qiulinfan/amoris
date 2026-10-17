// The wasm viewport across page loads, remounts and hot updates (docs/spec/editor.md 5):
// headless Chrome (WebGPU on) opens the editor at --url --loads times, each time as a new
// document, normally the Vite dev server (`bun run dev`, where React's StrictMode mounts every
// effect twice) proxying to a real host that serves the viewport (`pocket serve`). Then it resets
// the dock's layout (`window.pocket.resetLayout()`: the viewport panel unmounts and mounts again,
// its viewport disposed and a new one created on the same wasm instance) and, with --touch FILE,
// appends a comment to that editor source file and restores it: two hot updates, which must leave
// the page working (Fast Refresh keeps the viewport panel mounted: its effects do not run again).
// A load or a remount passes when the editor attaches a new wasm viewport (not the fallback) that
// draws the host's world, a hot update when Vite applied it and a viewport still draws, and every
// phase only when the page throws no exception and logs no error for --seconds. Drawing needs the
// renderer's stats (frames, entities) and a screenshot of the viewport's canvas that is not one
// flat color: the stats count frames whose command buffers the GPU rejected, which present
// nothing. An error is a console error or the renderer's own "[pocket ERROR]" line, which the wasm
// logger writes with console.log (wgpu's validation errors among them); "[pocket WARN]" lines are
// kept with the warnings, which do not fail a phase.
//
//   pocket serve samples/sailing --port 7890
//   POCKET_HOST=http://127.0.0.1:7890 EDITOR_PORT=5190 bun run dev
//   bun tools/viewport-loads.ts --url http://127.0.0.1:5190/ [--loads 3] [--seconds 10] \
//     [--touch src/panels/viewport/ViewportPanel.tsx] [--out ../out/viewport-loads]
//
// CHROME=<path> picks the browser; CHROME_FLAGS adds switches (--force_high_performance_gpu on a
// two-GPU laptop). Writes loads.json (each phase's state, messages and canvas check) and each
// phase's canvas screenshot into --out; the exit status is 1 when a phase fails. A run stopped while
// --touch's comment is in the file (a signal) restores the file, and the next run removes a comment
// a killed run left.

import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { inflateSync } from "node:zlib";
import { Cdp } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:5190/");
const LOADS = Number(opt("--loads", "3"));
const SECONDS = Number(opt("--seconds", "10"));
const TOUCH = opt("--touch", "");
const OUT = resolve(opt("--out", "../out/viewport-loads"));
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
const profile = join(tmpdir(), `pocket-viewport-loads-${Date.now()}`);
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
    "--window-size=1600,1000",
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

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}`);
  return r.result.value as T;
}

// Exceptions, console errors and warnings, and the renderer's error and warning lines, stamped in
// seconds since the phase began: every one counted by kind, the first MESSAGES of a phase kept.
// A load hears only the documents created after it navigated: what the replaced document logged
// while it went counts as "previous-page", which does not fail the load.
type Message = { t: number; kind: string; text: string };
const MESSAGES = 60;
const FAILING = new Set(["exception", "error", "pocket-error"]);
let messages: Message[] = [];
let counts: Record<string, number> = {};
let contexts = new Set<number>();
let phaseStart = Date.now();
const stamp = () => Math.round((Date.now() - phaseStart) / 100) / 10;
const note = (kind: string, text: string, context?: number) => {
  if (context !== undefined && !contexts.has(context)) kind = "previous-page";
  counts[kind] = (counts[kind] ?? 0) + 1;
  if (messages.length < MESSAGES) messages.push({ t: stamp(), kind, text });
};
cdp.on("Runtime.executionContextCreated", (p) => {
  contexts.add((p as { context: { id: number } }).context.id);
});
cdp.on("Runtime.exceptionThrown", (p) => {
  const d = (p as { exceptionDetails: { text: string; executionContextId?: number; exception?: { description?: string } } }).exceptionDetails;
  note("exception", (d.exception?.description ?? d.text).slice(0, 600), d.executionContextId);
});
let hotUpdates = 0;
cdp.on("Runtime.consoleAPICalled", (p) => {
  const e = p as { type: string; executionContextId: number; args: { value?: unknown; description?: string }[] };
  const text = e.args.map((a) => String(a.value ?? a.description ?? "")).join(" ").slice(0, 600);
  if (text.startsWith("[vite] hot updated")) hotUpdates++;
  if (e.type === "error" || e.type === "warning") note(e.type, text, e.executionContextId);
  else if (text.startsWith("[pocket ERROR]")) note("pocket-error", text, e.executionContextId);
  else if (text.startsWith("[pocket WARN]")) note("pocket-warn", text, e.executionContextId);
});
await cdp.send("Page.enable");
await cdp.send("Runtime.enable");

type State = {
  pocket: boolean;
  viewport: string | null;
  engine: boolean;
  generation: number;
  frames: number;
  renderer: Record<string, unknown> | null;
  wasm_fetches: number;
};
// Each viewport the editor attaches is numbered once (on the engine object), so a remount shows.
// `wasm_fetches` counts the document's requests for the viewport's .wasm: wasm-bindgen's init
// fetches it once per instantiation, so more than one is a second instance.
const readState = () =>
  js<State>(`
    const p = window.pocket;
    const ctl = window.pocketEditorViewport;
    const e = ctl?.engine ?? null;
    if (e && !e.__loadsGeneration) e.__loadsGeneration = (window.__loadsGenerations = (window.__loadsGenerations ?? 0) + 1);
    const s = e?.stats?.() ?? null;
    return { pocket: !!p, viewport: p ? p.connection.getState().viewport : null, engine: !!e, generation: e?.__loadsGeneration ?? 0,
      frames: s?.frames ?? 0, renderer: s?.renderer ?? null,
      wasm_fetches: performance.getEntriesByType("resource").filter((r) => r.name.includes("pocket_viewport_bg.wasm")).length };
  `);

/** The RGBA pixels of a PNG as Chrome's screenshots write it (8 bits, RGB or RGBA, not interlaced). */
function decodePng(png: Buffer): { width: number; height: number; rgba: Uint8Array } {
  let width = 0;
  let height = 0;
  let channels = 0;
  const data: Buffer[] = [];
  for (let at = 8; at + 8 <= png.length; ) {
    const length = png.readUInt32BE(at);
    const kind = png.toString("latin1", at + 4, at + 8);
    const body = png.subarray(at + 8, at + 8 + length);
    if (kind === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      channels = body[9] === 6 ? 4 : body[9] === 2 ? 3 : 0;
      if (body[8] !== 8 || body[12] !== 0 || !channels) throw new Error(`PNG: depth ${body[8]}, color type ${body[9]}, interlace ${body[12]}`);
    } else if (kind === "IDAT") data.push(body);
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * channels;
  const rgba = new Uint8Array(width * height * 4);
  let prev = new Uint8Array(stride);
  let line = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)]!;
    const src = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels]! : 0;
      const b = prev[i]!;
      const c = i >= channels ? prev[i - channels]! : 0;
      const pa = Math.abs(b - c);
      const pb = Math.abs(a - c);
      const pc = Math.abs(a + b - 2 * c);
      const predict = [0, a, b, (a + b) >> 1, pa <= pb && pa <= pc ? a : pb <= pc ? b : c][filter] ?? 0;
      line[i] = (src[i]! + predict) & 255;
    }
    for (let x = 0; x < width; x++) {
      for (let k = 0; k < 3; k++) rgba[(y * width + x) * 4 + k] = line[x * channels + k]!;
      rgba[(y * width + x) * 4 + 3] = channels === 4 ? line[x * channels + 3]! : 255;
    }
    [prev, line] = [line, prev];
  }
  return { width, height, rgba };
}

// A drawn world is not one flat color: at most FLAT_MAX of the canvas's pixels may share the most
// common color (channels quantized to 16 levels). A viewport whose frames the GPU rejects presents
// a cleared canvas (the editor's overlays over it add only a few percent of other colors).
const FLAT_MAX = 0.8;
type Canvas = { width: number; height: number; flat: number; colors: number; shot: string };
async function canvasCheck(slug: string): Promise<Canvas | null> {
  const r = await js<{ x: number; y: number; width: number; height: number } | null>(`
    const c = document.querySelector("canvas.viewport-canvas");
    if (!c) return null;
    const b = c.getBoundingClientRect();
    return { x: b.x, y: b.y, width: b.width, height: b.height };
  `);
  if (!r || r.width < 1 || r.height < 1) return null;
  const shot = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png", clip: { ...r, scale: 1 } });
  const png = Buffer.from(shot.data, "base64");
  const file = join(OUT, `${slug}.png`);
  writeFileSync(file, png);
  const { width, height, rgba } = decodePng(png);
  const histogram = new Map<number, number>();
  for (let i = 0; i < rgba.length; i += 4) {
    const key = ((rgba[i]! >> 4) << 8) | ((rgba[i + 1]! >> 4) << 4) | (rgba[i + 2]! >> 4);
    histogram.set(key, (histogram.get(key) ?? 0) + 1);
  }
  const top = Math.max(...histogram.values());
  return { width, height, flat: Math.round((top / (width * height)) * 1000) / 1000, colors: histogram.size, shot: file };
}

type Phase = {
  name: string;
  ok: boolean;
  why: string[];
  ready_s: number | null;
  /** A viewport newer than the one before the phase was attached. */
  remounted: boolean;
  state: State | null;
  canvas: Canvas | null;
  /** Every message of the phase counted by kind, and the first MESSAGES of them. */
  counts: Record<string, number>;
  messages: Message[];
};
const phases: Phase[] = [];

/**
 * Runs `start`, waits until it took effect (a viewport newer than generation `after` is attached,
 * or with `hot`, Vite applied a hot update), then watches the page for SECONDS. Returns the
 * generation of the viewport then attached.
 */
async function phase(name: string, after: number, hot: boolean, start: () => Promise<void>): Promise<number> {
  messages = [];
  counts = {};
  phaseStart = Date.now();
  const updates = hotUpdates;
  await start();
  let state: State | null = null;
  let ready: number | null = null;
  const deadline = Date.now() + 40_000;
  while (Date.now() < deadline) {
    await Bun.sleep(250);
    try {
      state = await readState();
    } catch {
      continue; // the document is being replaced
    }
    const attached = state.viewport === "wasm" && state.engine;
    if (attached && (hot ? hotUpdates > updates : state.generation > after)) {
      ready = stamp();
      break;
    }
  }
  // Headless Chrome runs no animation frames for the page until an input event arrives.
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 8, y: 992, button: "none" });
  const until = phaseStart + (ready ?? 0) * 1000 + SECONDS * 1000;
  let drew = false;
  while (Date.now() < until) {
    await Bun.sleep(500);
    try {
      state = await readState();
    } catch {
      break;
    }
    drew ||= state.frames > 0 && Number(state.renderer?.entities ?? 0) > 0;
  }
  let canvas: Canvas | null = null;
  try {
    canvas = await canvasCheck(`phase-${phases.length + 1}`);
  } catch (e) {
    note("exception", `canvas check: ${e instanceof Error ? e.message : String(e)}`);
  }
  const why: string[] = [];
  if (ready === null) why.push(hot ? `no hot update applied: ${JSON.stringify(state)}` : `no new wasm viewport attached: ${JSON.stringify(state)}`);
  else if (!drew) why.push(`the viewport drew no frames of the world: ${JSON.stringify(state)}`);
  else if (!canvas || canvas.flat > FLAT_MAX) why.push(`the viewport's canvas shows no world: ${JSON.stringify(canvas)}`);
  const bad = Object.entries(counts).filter(([kind]) => FAILING.has(kind));
  if (bad.length) why.push(`errors: ${bad.map(([kind, n]) => `${n} ${kind}`).join(", ")}`);
  const remounted = (state?.generation ?? 0) > after;
  phases.push({ name, ok: why.length === 0, why, ready_s: ready, remounted, state, canvas, counts: { ...counts }, messages: [...messages] });
  const brief = state && { ...state, renderer: state.renderer && { entities: state.renderer.entities, draw_calls: state.renderer.draw_calls, backend: state.renderer.backend } };
  console.log(`${why.length ? "FAIL" : "ok  "} ${name}: ready ${ready ?? "-"} s, remounted ${remounted}, canvas flat ${canvas?.flat ?? "-"}, ${JSON.stringify(brief)}`);
  for (const w of why) console.log(`     ${w}`);
  for (const m of messages.slice(0, 8)) console.log(`     ${m.t}s [${m.kind}] ${m.text.split("\n")[0]}`);
  if (messages.length > 8) console.log(`     ... ${JSON.stringify(counts)}`);
  return state?.generation ?? after;
}

const MARK = "// viewport-loads: a hot update\n";
const source = TOUCH ? resolve(TOUCH) : "";
let original = source ? readFileSync(source, "utf8") : "";
if (source && original.endsWith(MARK)) {
  original = original.slice(0, -MARK.length);
  writeFileSync(source, original);
  console.log(`removed the comment a stopped run left in ${TOUCH}`);
}
const restore = () => {
  if (source && readFileSync(source, "utf8") !== original) writeFileSync(source, original);
};
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
  process.on(signal, () => {
    restore();
    proc.kill();
    process.exit(130);
  });
}
try {
  let gen = 0;
  for (let i = 1; i <= LOADS; i++) {
    gen = await phase(`load ${i}`, 0, false, async () => {
      contexts = new Set();
      await cdp.send("Page.navigate", { url: URL_ });
    });
  }
  gen = await phase("layout reset (the viewport panel remounted)", gen, false, async () => {
    await js("window.pocket.resetLayout();");
  });
  if (source) {
    gen = await phase(`hot update 1 (${TOUCH})`, gen, true, async () => {
      writeFileSync(source, original + MARK);
    });
    await phase(`hot update 2 (${TOUCH} restored)`, gen, true, async () => {
      writeFileSync(source, original);
    });
  }
} finally {
  restore();
  cdp.close();
  proc.kill();
  await Promise.race([proc.exited, Bun.sleep(3000)]);
  try {
    rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  } catch {
    // Chrome still holds a file: the profile stays in the temporary directory
  }
}
const run = { url: URL_, loads: LOADS, seconds: SECONDS, touch: TOUCH || null };
writeFileSync(join(OUT, "loads.json"), JSON.stringify({ ...run, phases }, null, 2));
const failed = phases.filter((p) => !p.ok).length;
console.log(`${phases.length - failed} of ${phases.length} phases passed; ${join(OUT, "loads.json")}`);
process.exit(failed ? 1 : 0);
