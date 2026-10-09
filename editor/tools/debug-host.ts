// Evidence of the editor's script debugger against the real host (docs/spec/editor.md 10): drives
// the built editor that `pocket serve` serves, in headless Chrome over CDP, with real input only
// (clicks, keys, typing; nothing through dev hooks), through one edit-run-debug loop:
//
//   open rules.ts, click the gutter of a line in the `log` system, Play, the pause at that line,
//   the call stack and the scopes, two watch expressions, step over, set a closure's column value
//   (a component field) in the Variables tree, set an argument (`r`) and try a const (`speed`),
//   Continue to the next tick's pause (the value was committed), a data breakpoint from the
//   inspector, remove the breakpoints and Continue, edit the script while it runs, then Stop while
//   paused (the edit world takes the edited scripts) and Play again into the edited code.
//
//   pocket serve <copy of samples/sailing> --port 7911 --editor editor/dist
//   bun tools/debug-host.ts --url http://127.0.0.1:7911/ [--out ../docs/evidence/editor] [--line 27]
//
// Screenshots go to <out>/debug-host-*.png; what each step saw is printed (and kept beside them as
// debug-host.txt by the caller).

import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { Cdp, launchChrome } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7878/");
const OUT = resolve(opt("--out", resolve(import.meta.dir, "../../docs/evidence/editor")));
const LINE = Number(opt("--line", "27"));
const W = 1600;
// The editor's Mod: Cmd (CDP modifier 4) on macOS, Ctrl (2) elsewhere (src/commands/keys.ts).
const MOD = process.platform === "darwin" ? 4 : 2;
const H = 1000;
mkdirSync(OUT, { recursive: true });

const { proc, pageWs } = await launchChrome(Number(opt("--chrome-port", "9351")));
const cdp = await Cdp.connect(pageWs);
const sleep = (ms: number) => Bun.sleep(ms);
const t0 = Date.now();
const note = (msg: string) => console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s] ${msg}`);

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

async function waitFor(what: string, expression: string, timeoutMs = 15000): Promise<unknown> {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const v = await js(expression).catch(() => undefined);
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}

type Rect = { x: number; y: number; width: number; height: number };

async function shot(name: string, clip?: Rect) {
  await mouse("mouseMoved", W - 4, H - 40, "none", 0); // no hover tooltip in the picture
  await sleep(150);
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
  await Bun.write(`${OUT}/${name}.png`, Buffer.from(r.data, "base64"));
  note(`screenshot ${name}.png`);
}

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

async function mouse(type: "mousePressed" | "mouseMoved" | "mouseReleased", x: number, y: number, button: "left" | "right" | "none" = "left", buttons = 1, clickCount = 1) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button, buttons: type === "mouseReleased" ? 0 : buttons, clickCount });
}

async function click(x: number, y: number, button: "left" | "right" = "left", clickCount = 1) {
  await mouse("mouseMoved", x, y, "none", 0);
  await mouse("mousePressed", x, y, button, button === "left" ? 1 : 2, clickCount);
  await mouse("mouseReleased", x, y, button, 0, clickCount);
}

async function dblclick(x: number, y: number) {
  await click(x, y, "left", 1);
  await click(x, y, "left", 2);
}

async function key(keyName: string, code: string, keyCode: number, modifiers = 0) {
  // Enter carries its text, as a keyboard's does (Monaco inserts the line break from it).
  const down = keyName === "Enter" && !modifiers ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : { type: "rawKeyDown" };
  await cdp.send("Input.dispatchKeyEvent", { ...down, key: keyName, code, modifiers, windowsVirtualKeyCode: keyCode });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: keyName, code, modifiers, windowsVirtualKeyCode: keyCode });
}

async function type(text: string) {
  await cdp.send("Input.insertText", { text });
}

/** The center of the first element matching `selector` whose text matches `text` (a regex source). */
async function at(selector: string, text = "", nth = 0): Promise<[number, number]> {
  return js<[number, number]>(`
    const re = new RegExp(${JSON.stringify(text)});
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => re.test(e.textContent) && e.getBoundingClientRect().width > 0);
    const el = els[${nth}];
    if (!el) throw new Error('no ' + ${JSON.stringify(selector)} + ' matching ' + re);
    el.scrollIntoView({ block: 'nearest' });
    const r = el.getBoundingClientRect();
    return [r.left + r.width / 2, r.top + r.height / 2];
  `);
}

const text = (selector: string) => js<string[]>(`return [...document.querySelectorAll(${JSON.stringify(selector)})].map((e) => e.innerText.replace(/\\s+/g, ' ').trim());`);

async function describe(label: string) {
  const banner = await text(".pause-banner");
  const frames = await text(".debug-panel .frame-row");
  const vars = await text(".debug-panel .debug-col:nth-child(2) .var-row, .debug-panel .debug-col:nth-child(2) .scope-name");
  const watches = await text(".debug-panel .debug-col:nth-child(3) .var-row");
  const bps = await text(".debug-panel .bp-row");
  const status = await text(".statusbar .sb-paused");
  const pausedLine = await js<number | null>(`const g = document.querySelector('.code-editor .paused-line'); if (!g) return null; const top = g.parentElement.getBoundingClientRect().top; const n = [...document.querySelectorAll('.code-editor .line-numbers')].find((x) => Math.abs(x.getBoundingClientRect().top - g.getBoundingClientRect().top) < 3); return n ? Number(n.textContent) : null;`);
  console.log(`--- ${label}`);
  console.log(`  banner:      ${banner.join(" | ") || "(none)"}`);
  console.log(`  status bar:  ${status.join(" | ") || "(not paused)"}`);
  console.log(`  paused line: ${pausedLine ?? "(none shown)"}`);
  console.log(`  call stack:  ${frames.join(" | ") || "(empty)"}`);
  console.log(`  variables:   ${vars.join(" | ") || "(empty)"}`);
  console.log(`  watches:     ${watches.join(" | ") || "(none)"}`);
  console.log(`  breakpoints: ${bps.join(" | ") || "(none)"}`);
}

/** The line numbers Monaco shows (sticky scroll's excluded). */
const shownLines = () =>
  js<number[]>(`return [...document.querySelectorAll('.code-editor .lines-content .line-numbers, .code-editor .margin-view-overlays .line-numbers')].filter((n) => !n.closest('.sticky-widget')).map((n) => Number(n.textContent)).filter((n) => n > 0);`);

/** Clicks the glyph margin (breakpoint gutter) of a line of the shown script. */
async function gutter(line: number) {
  // Bring the line into view as a person would: click into the text, then arrow keys.
  let shown = await shownLines();
  if (!shown.includes(line) || line > Math.max(...shown) - 2 || line < Math.min(...shown) + 2) {
    const [tx, ty] = await js<[number, number]>(`
      const n = [...document.querySelectorAll('.code-editor .margin-view-overlays .line-numbers')].filter((x) => !x.closest('.sticky-widget'));
      const first = n.reduce((a, b) => (a.getBoundingClientRect().top < b.getBoundingClientRect().top ? a : b));
      const m = document.querySelector('.code-editor .margin').getBoundingClientRect();
      const r = first.getBoundingClientRect();
      return [m.right + 40, r.top + r.height / 2];
    `);
    await click(tx, ty);
    const from = await js<number>(`return Number(document.querySelector('.code-status')?.textContent.match(/Ln (\\d+)/)?.[1] ?? 1);`);
    const down = line + 4 - from;
    for (let i = 0; i < Math.abs(down); i++) await key(down > 0 ? "ArrowDown" : "ArrowUp", down > 0 ? "ArrowDown" : "ArrowUp", down > 0 ? 40 : 38);
    await sleep(300);
    shown = await shownLines();
  }
  const [x, y] = await js<[number, number]>(`
    const n = [...document.querySelectorAll('.code-editor .margin-view-overlays .line-numbers')].filter((x) => !x.closest('.sticky-widget')).find((x) => Number(x.textContent) === ${line});
    if (!n) throw new Error('line ${line} is not shown');
    const m = document.querySelector('.code-editor .margin').getBoundingClientRect();
    const r = n.getBoundingClientRect();
    return [m.left + 9, r.top + r.height / 2];
  `);
  await click(x, y);
}

async function openTab(title: string) {
  const [x, y] = await at(".dock-tab-title", `^${title}$`);
  await click(x, y);
  await sleep(300);
}

async function pausedAt(line: number, timeoutMs = 15000) {
  await waitFor(`a pause at line ${line}`, `const b = document.querySelector('.pause-banner'); return b && new RegExp(':${line}(:|\\\\b)').test(b.textContent);`, timeoutMs);
  await sleep(500);
}

const VARS = ".debug-panel .debug-col:nth-child(2) .var-row";

/** A Variables row's center (scrolled into view), by its name; `after`: the first such row below that one. */
async function varRow(name: string, after?: string): Promise<[number, number]> {
  return js<[number, number]>(`
    const rows = [...document.querySelectorAll(${JSON.stringify(VARS)})];
    const named = (n, from) => rows.findIndex((r, i) => i >= from && r.querySelector('.var-name')?.textContent === n);
    const start = ${after === undefined ? 0 : `named(${JSON.stringify(after)}, 0) + 1`};
    const row = rows[named(${JSON.stringify(name)}, start)];
    if (!row) throw new Error('no variable row ${name}');
    row.scrollIntoView({ block: 'center' });
    const r = row.getBoundingClientRect();
    return [r.left + 40, r.top + r.height / 2];
  `);
}

/** Opens a variable's children unless they are shown. */
async function expand(name: string, after?: string) {
  const open = await js<boolean>(`
    const rows = [...document.querySelectorAll(${JSON.stringify(VARS)})];
    const named = (n, from) => rows.findIndex((r, i) => i >= from && r.querySelector('.var-name')?.textContent === n);
    const start = ${after === undefined ? 0 : `named(${JSON.stringify(after)}, 0) + 1`};
    const row = rows[named(${JSON.stringify(name)}, start)];
    return !!row?.querySelector('.var-twisty.open');
  `);
  if (open) return;
  const [x, y] = await varRow(name, after);
  await click(x, y);
  await sleep(250);
}

/** Makes the bottom dock group (Console, ..., Debug) taller by dragging its sash up. */
async function enlargeBottom(toY: number) {
  const sash = await js<[number, number] | null>(`
    const c = document.querySelector('.dock-panel.panel-debug').getBoundingClientRect();
    const group = [...document.querySelectorAll('.dv-groupview')].map((g) => g.getBoundingClientRect())
      .find((r) => Math.abs(r.left - c.left) < 4 && r.top <= c.top + 2 && r.bottom >= c.bottom - 2);
    const s = [...document.querySelectorAll('.dv-sash')].map((x) => x.getBoundingClientRect())
      .find((r) => r.width > 200 && r.height < 12 && Math.abs(r.top + r.height / 2 - group.top) < 10);
    return s ? [s.left + s.width / 2, s.top + s.height / 2] : null;
  `);
  if (!sash) {
    note("no sash found above the Debug panel's group");
    return;
  }
  await mouse("mouseMoved", sash[0], sash[1], "none", 0);
  await mouse("mousePressed", sash[0], sash[1]);
  for (let i = 1; i <= 10; i++) await mouse("mouseMoved", sash[0], sash[1] + ((toY - sash[1]) * i) / 10);
  await mouse("mouseReleased", sash[0], toY);
  await sleep(400);
}

try {
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: false });
  await cdp.send("Page.navigate", { url: URL_ });
  await cdp.once("Page.loadEventFired");
  await js(`localStorage.clear();`);
  await cdp.send("Page.reload");
  await cdp.once("Page.loadEventFired");
  await waitFor("the host connection", `return document.querySelector('.statusbar.conn-open') !== null;`);
  note(`connected to ${URL_}`);
  await sleep(1500);

  // ---- 1. Open rules.ts and click the gutter of a line in the `log` system ------------------------
  await openTab("Scripts");
  const [fx, fy] = await at(".file-row", "rules\\.ts");
  await click(fx, fy);
  await waitFor("rules.ts in the code editor", `const t = document.querySelector('.code-tab.is-active'); return t && t.textContent.includes('rules.ts') && document.querySelectorAll('.code-editor .line-numbers').length > 5;`);
  await sleep(800);
  await openTab("Debug");
  await enlargeBottom(440);
  await openTab("Scripts");
  await gutter(LINE);
  await waitFor("the breakpoint in the Debug panel", `return [...document.querySelectorAll('.bp-row')].some((r) => r.textContent.includes('rules.ts:${LINE}'));`);
  await sleep(600);
  await describe(`1. breakpoint set by clicking the gutter of rules.ts:${LINE} (Edit mode)`);
  await shot("debug-host-1-breakpoint");

  // ---- 2. Play: the fork runs the scripts and stops at the breakpoint ----------------------------
  const [px, py] = await at('button[aria-label^="Play"]');
  await click(px, py);
  note("Play");
  await pausedAt(LINE);
  await describe("2. Play hit the breakpoint");
  await shot("debug-host-2-paused");

  // ---- 3. Expand scopes, add watch expressions ---------------------------------------------------
  await expand("ctx");
  await expand("world", "ctx");
  for (const expr of ["b.speed[r] * 3.6", "l.distance[r]", "ctx.tick"]) {
    const [wx, wy] = await at(".debug-panel .watch-input");
    await click(wx, wy);
    await type(expr);
    await key("Enter", "Enter", 13);
    await sleep(400);
  }
  await sleep(600);
  await js(`document.querySelector('.debug-panel .debug-col:nth-child(2)').scrollTop = 0;`);
  await describe("3. scopes expanded, watches evaluated on the paused frame");
  await shot("debug-host-3-scopes-watches", await panelRect("debug"));
  // Fold ctx again, so the next views show the locals.
  const [cx0, cy0] = await varRow("ctx");
  await click(cx0, cy0);
  await sleep(200);

  // ---- 4. Step over -----------------------------------------------------------------------------
  await click(10, 990); // out of the watch input; F10 is global anyway
  await key("F10", "F10", 121);
  note("F10 (step over)");
  await pausedAt(LINE + 1);
  await describe("4. stepped over to the next line");
  await shot("debug-host-4-stepped");

  // ---- 5. Set a value: the boat's Log.distance column, through the closure's `l` -----------------
  await expand("l");
  await expand("distance", "l");
  const [ix, iy] = await js<[number, number]>(`
    const rows = [...document.querySelectorAll(${JSON.stringify(VARS)})];
    const i = rows.findIndex((r) => r.querySelector('.var-name')?.textContent === 'distance');
    const row = rows[i + 1];
    row.scrollIntoView({ block: 'center' });
    const v = row.querySelector('.var-value').getBoundingClientRect();
    return [v.left + 6, v.top + v.height / 2];
  `);
  await dblclick(ix, iy);
  await waitFor("the value editor", `return document.querySelector('.var-input') !== null;`, 5000);
  await js(`document.querySelector('.var-input').select();`);
  await type("1000");
  await key("Enter", "Enter", 13);
  await waitFor("the new value", `return [...document.querySelectorAll('.debug-panel .debug-col:nth-child(3) .var-row')].some((r) => r.querySelector('.var-name')?.textContent === 'l.distance[r]' && r.querySelector('.var-value')?.textContent === '1000');`);
  await sleep(500);
  await describe("5. l.distance[0] set to 1000 in the Variables tree");
  await shot("debug-host-5-set-value", await panelRect("debug"));

  // ---- 5b. Set a variable of the frame (the argument r) and try a const (speed) -----------------
  /** Double-clicks a top-level Variables row's value, types `value` and presses Enter. */
  const setVar = async (name: string, value: string) => {
    const [vx, vy] = await js<[number, number]>(`
      const row = [...document.querySelectorAll(${JSON.stringify(VARS)})].find((r) => r.querySelector('.var-name')?.textContent === ${JSON.stringify(name)});
      row.scrollIntoView({ block: 'center' });
      const v = row.querySelector('.var-value').getBoundingClientRect();
      return [v.left + 6, v.top + v.height / 2];
    `);
    await dblclick(vx, vy);
    await waitFor("the value editor", `return document.querySelector('.var-input') !== null;`, 5000);
    await js(`document.querySelector('.var-input').select();`);
    await type(value);
    await key("Enter", "Enter", 13);
  };
  const watchReads = (expr: string, value: string) =>
    `return [...document.querySelectorAll('.debug-panel .debug-col:nth-child(3) .var-row')].some((r) => r.querySelector('.var-name')?.textContent === ${JSON.stringify(expr)} && r.querySelector('.var-value')?.textContent === ${JSON.stringify(value)});`;
  await setVar("r", "1");
  await waitFor("the watch on the frame's new r", watchReads("l.distance[r]", "undefined"));
  await setVar("speed", "5");
  await waitFor("the const refused", `return [...document.querySelectorAll('.toast')].some((t) => /constant/.test(t.textContent));`, 5000);
  await key("Escape", "Escape", 27);
  await sleep(500);
  await describe("5b. r set to 1 (l.distance[r] reads undefined), speed refused as a const");
  console.log(`  toasts: ${(await text(".toast")).join(" | ")}`);
  await shot("debug-host-5b-set-local");
  await setVar("r", "0");
  await waitFor("r set back", watchReads("l.distance[r]", "1000"));
  await sleep(300);

  // ---- 6. Step over the line that adds to it, then Continue to the next tick's pause -------------
  await key("F10", "F10", 121);
  await pausedAt(LINE + 2);
  await describe("6a. stepped over the line that adds speed * dt");
  await key("F5", "F5", 116);
  note("F5 (continue)");
  await waitFor("the next tick's pause", `const b = document.querySelector('.pause-banner'); return b && /:${LINE}:/.test(b.textContent);`);
  await sleep(800);
  await expand("l");
  await expand("distance", "l");
  await describe("6. continued: the next tick stops at the breakpoint again and reads the committed value");
  await shot("debug-host-6-next-tick");

  // ---- 7. A data breakpoint from the inspector (Break When Written) -----------------------------
  const [bx, by] = await at(".debug-panel .bp-row .var-remove");
  await click(bx, by);
  await sleep(300);
  const [hx, hy] = await at(".tree-row", "^\\s*Sloop");
  await click(hx, hy);
  await sleep(600);
  const [cx, cy] = await at(".card .card-title", "^Log$");
  await click(cx, cy, "right");
  await sleep(300);
  const [mx, my] = await at(".menu-item", "Break When Written");
  await mouse("mouseMoved", mx, my, "none", 0);
  await sleep(400);
  const [sx, sy] = await at(".menu-item", "^\\s*top_speed\\s*$");
  await click(sx, sy);
  await waitFor("the data breakpoint", `return [...document.querySelectorAll('.bp-row')].some((r) => r.textContent.includes('Log.top_speed'));`);
  // Pause on uncaught exceptions (a native <select>: its popup is not drawn headless, so the change
  // is dispatched on the element; the host answers the mode, which the select then shows).
  await js(`const s = document.querySelector('.debug-exceptions select'); s.value = 'uncaught'; s.dispatchEvent(new Event('change', { bubbles: true }));`);
  await sleep(400);
  const mode = await js<unknown>(`const r = await fetch('/api/call', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ method: 'debug.state' }) }); return (await r.json()).result.exceptions;`);
  note(`debug.state.exceptions on the host: ${JSON.stringify(mode)}`);
  await key("F5", "F5", 116);
  note("F5 (continue) with a data breakpoint on Sloop.Log.top_speed");
  await waitFor("the data breakpoint's pause", `const b = document.querySelector('.pause-banner'); return b && /data breakpoint/.test(b.textContent);`, 30000);
  await sleep(800);
  await describe("7. paused by the data breakpoint");
  await shot("debug-host-7-data-breakpoint");

  // ---- 8. Remove it, Continue: the game runs; the inspector shows the committed Log ---------------
  const [rx, ry] = await at(".debug-panel .bp-row .var-remove");
  await click(rx, ry);
  await sleep(300);
  await key("F5", "F5", 116);
  await waitFor("running", `return document.querySelector('.pause-banner') === null;`);
  await sleep(1500);
  await describe("8. running again, no breakpoints");
  const log = await js<unknown>(`const r = await fetch('/api/call', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ method: 'world.query', params: { with: ['Log'], fields: ['Log.distance', 'Log.top_speed'] } }) }); return (await r.json()).result;`);
  console.log(`  world.query Log (the host): ${JSON.stringify(log)}`);
  const shown = await js<string[]>(`const c = [...document.querySelectorAll('.card')].find((x) => x.querySelector('.card-title')?.textContent === 'Log'); if (!c) return []; c.scrollIntoView({ block: 'end' }); return [...c.querySelectorAll('input')].map((i) => i.value);`);
  console.log(`  inspector Log card inputs: ${JSON.stringify(shown)}`);
  await openTab("Viewport");
  await sleep(1500);
  await shot("debug-host-8-running");

  // ---- 9. Edit the script while it runs, save (write + apply), and debug the new line ------------
  await openTab("Scripts");
  await sleep(300);
  const [ex, ey] = await js<[number, number]>(`
    const n = [...document.querySelectorAll('.code-editor .line-numbers')].find((x) => Number(x.textContent) === ${LINE});
    const m = document.querySelector('.code-editor .margin').getBoundingClientRect();
    const r = n.getBoundingClientRect();
    return [m.right + 420, r.top + r.height / 2];
  `);
  await click(ex, ey);
  await key("End", "End", 35);
  await key("Enter", "Enter", 13);
  await type("const knots = speed * 1.944;");
  await key("Escape", "Escape", 27);
  await sleep(300);
  await key("s", "KeyS", 83, MOD); // Mod+S: scripts.write, then scripts.apply (a hot swap)
  note("typed a new line after rules.ts:" + LINE + " and pressed Mod+S");
  await waitFor("the scripts applied", `return [...document.querySelectorAll('.toast')].some((t) => t.textContent.includes('Scripts applied'));`, 30000);
  const toast = await text(".toast");
  console.log(`  toast: ${toast.join(" | ")}`);
  const edited = /Bundle (\w+)/.exec(toast.join(" "))?.[1] ?? "";
  await sleep(1000);
  await gutter(LINE + 2);
  note(`breakpoint on rules.ts:${LINE + 2} (the line after the new one)`);
  await pausedAt(LINE + 2);
  await describe("9. the new code runs: a breakpoint after the inserted line stops with `knots` in Local");
  await shot("debug-host-9-edited-script");

  // ---- 10. Stop while paused: Stop continues the pause; the edit world takes the edited scripts --
  await waitFor("the earlier toasts gone", `return document.querySelectorAll('.toast').length === 0;`, 15000);
  const [tx, ty] = await at('button[aria-label^="Stop"]');
  const stopAt = Date.now();
  await click(tx, ty);
  note("Stop, while paused at rules.ts:" + (LINE + 2));
  await waitFor("Edit mode", `return document.querySelector('button[aria-label^="Play"]') !== null;`, 15000);
  note(`Edit mode ${((Date.now() - stopAt) / 1000).toFixed(1)} s after Stop`);
  await waitFor("the scripts applied to the edit world", `return [...document.querySelectorAll('.toast')].some((t) => t.textContent.includes('Scripts applied'));`, 30000);
  const editStatus = await js<{ mode: string; bundle: string }>(`const r = await fetch('/api/call', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ method: 'time.control', params: {} }) }); return (await r.json()).result;`);
  console.log(`  edit world: mode ${editStatus.mode}, bundle ${editStatus.bundle} (step 9 applied ${edited})`);
  if (!edited || !editStatus.bundle.startsWith(edited.replace(/\W+$/, ""))) throw new Error("the edit world does not run the edited scripts");
  console.log(`  toasts: ${(await text(".toast")).join(" | ")}`);
  await sleep(500);
  await describe("10. stopped while paused: Edit mode, the edit world runs the edited bundle");
  await shot("debug-host-10-stopped");

  // ---- 11. Play again: the fork runs the edited scripts, the breakpoint stops in the new code -----
  const [p2x, p2y] = await at('button[aria-label^="Play"]');
  await click(p2x, p2y);
  note("Play again");
  await pausedAt(LINE + 2);
  const locals = await text(VARS);
  if (!locals.some((l) => l.startsWith("knots"))) throw new Error("the new Play does not run the edited line (no knots in Local)");
  await describe("11. the new Play stops at rules.ts:" + (LINE + 2) + " with knots in Local");
  await shot("debug-host-11-replay");

  // Remove the breakpoint and Stop (paused again: Stop continues it).
  const [zx, zy] = await at(".debug-panel .bp-row .var-remove");
  await click(zx, zy);
  await sleep(300);
  const [sx2, sy2] = await at('button[aria-label^="Stop"]');
  await click(sx2, sy2);
  await waitFor("Edit mode", `return document.querySelector('button[aria-label^="Play"]') !== null;`, 15000);
  await waitFor("not paused", `return document.querySelector('.pause-banner') === null;`);
  console.log("done");
} catch (e) {
  console.log(`FAILED: ${e instanceof Error ? e.message : String(e)}`);
  console.log(`  toasts: ${(await text(".toast").catch(() => [])).join(" | ")}`);
  await describe("at the failure").catch(() => undefined);
  await shot("debug-host-failure").catch(() => undefined);
  process.exitCode = 1;
} finally {
  cdp.close();
  proc.kill();
}
