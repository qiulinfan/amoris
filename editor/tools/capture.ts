// Evidence capture: drives the editor in headless Chrome (CDP) through an edit, play and debug
// session against a running dev server, and saves crisp screenshots of the whole window and of each
// panel. Real input (mouse drags on the gizmo, keys, typing) where it matters; the dev-only
// `window.pocket` hooks to set up the rest.
//
//   bun run dev:mock            # in another terminal (mock host + Vite)
//   bun tools/capture.ts --url http://127.0.0.1:5173/ --out ../docs/evidence/editor

import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { Cdp, launchChrome } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:5173/");
const OUT = resolve(opt("--out", resolve(import.meta.dir, "../../docs/evidence/editor")));
const W = 1600;
const H = 1000;
mkdirSync(OUT, { recursive: true });

const { proc, pageWs } = await launchChrome(9339);
const cdp = await Cdp.connect(pageWs);
const sleep = (ms: number) => Bun.sleep(ms);

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
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", {
    format: "png",
    ...(clip ? { clip: { ...clip, scale: 1 } } : {}),
  });
  await Bun.write(`${OUT}/${name}.png`, Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

/** The rect of the dock group showing a panel (tabs included). */
async function panelRect(id: string, pad = 0): Promise<Rect> {
  return js<Rect>(`
    const content = document.querySelector('.dock-panel.panel-${id}');
    const c = content.getBoundingClientRect();
    const group = [...document.querySelectorAll('.dv-groupview')].find((g) => {
      const r = g.getBoundingClientRect();
      return Math.abs(r.left - c.left) < 4 && Math.abs(r.right - c.right) < 4 && r.top <= c.top + 2 && r.bottom >= c.bottom - 2;
    });
    const r = (group ?? content).getBoundingClientRect();
    return { x: Math.max(0, r.left - ${pad}), y: Math.max(0, r.top - ${pad}), width: r.width + ${pad * 2}, height: r.height + ${pad * 2} };
  `);
}

function union(a: Rect, b: Rect): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, width: Math.max(a.x + a.width, b.x + b.width) - x, height: Math.max(a.y + a.height, b.y + b.height) - y };
}

async function mouse(type: "mousePressed" | "mouseMoved" | "mouseReleased", x: number, y: number, button: "left" | "right" | "none" = "left", buttons = 1, clickCount = 1) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button, buttons: type === "mouseReleased" ? 0 : buttons, clickCount });
}

async function click(x: number, y: number, button: "left" | "right" = "left") {
  await mouse("mouseMoved", x, y, "none", 0);
  await mouse("mousePressed", x, y, button, button === "left" ? 1 : 2);
  await mouse("mouseReleased", x, y, button, 0);
}

async function key(keyName: string, code: string, modifiers = 0, keyCode = 0) {
  await cdp.send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: keyName, code, modifiers, windowsVirtualKeyCode: keyCode });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: keyName, code, modifiers, windowsVirtualKeyCode: keyCode });
}

async function type(text: string) {
  await cdp.send("Input.insertText", { text });
}

async function center(selector: string): Promise<[number, number]> {
  return js<[number, number]>(`const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2];`);
}

/** Screen position of a gizmo handle (the middle of its first shape). */
async function handlePos(handle: string): Promise<[number, number]> {
  return js<[number, number]>(`
    const ctl = window.pocketEditorViewport;
    const v = ctl.view();
    const g = ctl.gizmo;
    const s = document.querySelector('.viewport-surface').getBoundingClientRect();
    const sh = g.shapes.find((x) => x.handle === ${JSON.stringify(handle)} && x.kind === 'line') ?? g.shapes.find((x) => x.handle === ${JSON.stringify(handle)});
    const pts = sh.points.map((q) => v.project(q));
    const m = pts[Math.floor(pts.length * 0.6)] ?? pts[0];
    return [s.left + m.x, s.top + m.y];
  `);
}

const META = 4;

try {
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 2, mobile: false });
  await cdp.send("Page.navigate", { url: URL_ });
  await cdp.once("Page.loadEventFired");
  await js(`localStorage.clear();`);
  await cdp.send("Page.reload");
  await cdp.once("Page.loadEventFired");
  await sleep(2500);
  console.log(`capturing ${URL_} → ${OUT}`);

  // ---- Edit mode ---------------------------------------------------------------------------------
  await js(`pocket.selection.getState().set([3]);`);
  await sleep(700);
  await shot("editor-edit");

  // A real gizmo drag on Crate2 along X: live preview, one undo entry on release.
  await js(`pocket.selection.getState().set([5]); pocket.viewport.getState().set({ tool: 'translate' });`);
  await sleep(400);
  const [hx, hy] = await handlePos("x");
  await mouse("mouseMoved", hx, hy, "none", 0);
  await sleep(150);
  await mouse("mousePressed", hx, hy);
  for (let i = 1; i <= 12; i++) {
    await mouse("mouseMoved", hx + i * 9, hy + i * 2);
    await sleep(30);
  }
  await sleep(250);
  await shot("viewport-gizmo-drag", await panelRect("viewport"));
  await mouse("mouseReleased", hx + 108, hy + 24);
  await sleep(600);

  // Rotate tool on Crate3, local space.
  await js(`pocket.selection.getState().set([6]); pocket.viewport.getState().set({ tool: 'rotate', space: 'local' });`);
  await sleep(500);
  await shot("viewport-rotate", await panelRect("viewport"));
  await js(`pocket.viewport.getState().set({ tool: 'translate', space: 'world' });`);

  // Inspector on the sloop.
  await js(`pocket.selection.getState().set([3]);`);
  await sleep(600);
  await shot("inspector", await panelRect("inspector"));
  // Add Component popover.
  await js(`document.querySelector('.inspector .panel-body').scrollTop = 1e6;`);
  await sleep(200);
  const [ax, ay] = await center(".add-component");
  await click(ax, ay);
  await sleep(400);
  await type("li");
  await sleep(300);
  const popover = await js<Rect>(`const r = document.querySelector('.add-menu').getBoundingClientRect(); return { x: r.left - 8, y: r.top - 8, width: r.width + 16, height: r.height + 16 };`);
  await shot("inspector-add-component", union(await panelRect("inspector"), popover));
  await key("Escape", "Escape", 0, 27);
  await js(`document.querySelector('.inspector .panel-body').scrollTop = 0;`);

  // Hierarchy: search and the context menu on a crate.
  const row = await js<[number, number]>(`const el = [...document.querySelectorAll('.tree-row')].find((r) => r.textContent.startsWith('Crate2')); const b = el.getBoundingClientRect(); return [b.left + 50, b.top + b.height / 2];`);
  await click(row[0], row[1], "right");
  await sleep(300);
  const [hmx, hmy] = await js<[number, number]>(`const r = [...document.querySelectorAll('.menu-item')].find((m) => m.textContent.includes('Add Component')).getBoundingClientRect(); return [r.left + 30, r.top + r.height / 2];`);
  await mouse("mouseMoved", hmx, hmy, "none", 0);
  await sleep(300);
  const menuRect = await js<Rect>(`const ms = [...document.querySelectorAll('.menu')]; let x0=1e9,y0=1e9,x1=0,y1=0; for (const m of ms) { const r = m.getBoundingClientRect(); x0=Math.min(x0,r.left); y0=Math.min(y0,r.top); x1=Math.max(x1,r.right); y1=Math.max(y1,r.bottom); } return {x:x0-8,y:y0-8,width:x1-x0+16,height:y1-y0+16};`);
  await shot("hierarchy-context-menu", union(await panelRect("hierarchy"), menuRect));
  await key("Escape", "Escape", 0, 27);
  await click(10, 990);

  // The Edit menu with its Create submenu.
  const [ex, ey] = await js<[number, number]>(`const r = [...document.querySelectorAll('.menubar-item')].find((m) => m.textContent === 'Edit').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2];`);
  await click(ex, ey);
  await sleep(250);
  const [cx_, cy_] = await js<[number, number]>(`const r = [...document.querySelectorAll('.menu-item')].find((m) => m.textContent.startsWith('Create')).getBoundingClientRect(); return [r.left + 40, r.top + r.height / 2];`);
  await mouse("mouseMoved", cx_, cy_, "none", 0);
  await sleep(300);
  await shot("menu-edit", { x: 0, y: 0, width: 760, height: 560 });
  await key("Escape", "Escape", 0, 27);
  await sleep(200);

  // Command palette: fuzzy search, then a host method call on the selected entity.
  await js(`pocket.selection.getState().set([3]);`);
  await key("k", "KeyK", META);
  await sleep(300);
  await type("tog");
  await sleep(300);
  await shot("palette", { x: 400, y: 60, width: 800, height: 560 });
  await js(`pocket.ui.getState().set({ paletteOpen: false });`);
  await sleep(200);
  await key("k", "KeyK", META);
  await sleep(250);
  await type("call world.get");
  await sleep(300);
  await key("Enter", "Enter", 0, 13);
  await sleep(300);
  await key("Enter", "Enter", 0, 13);
  await sleep(600);
  await shot("palette-call", { x: 400, y: 60, width: 800, height: 760 });
  await key("Escape", "Escape", 0, 27);
  await js(`pocket.ui.getState().set({ paletteOpen: false });`);

  // Assets.
  await shot("assets", await panelRect("assets"));

  // ---- Play (at 4x, so the crew takes every crate) ---------------------------------------------
  await js(`await pocket.api.time.control({ speed: 4 });`);
  await key("p", "KeyP", META);
  await sleep(9000);
  await js(`await pocket.api.time.control({ speed: 1 }); pocket.selection.getState().set([3]);`);
  await js(`pocket.openPanel('events'); const ev = pocket.events.getState().events; const e = ev.findLast((x) => x.name === 'crates.all') ?? ev.findLast((x) => x.name === 'crate.taken'); if (e) { pocket.events.getState().select(e.seq, await pocket.api.events.why(e.seq)); }`);
  await sleep(700);
  await shot("editor-play");
  await shot("events", await panelRect("events"));
  await js(`pocket.openPanel('timeline');`);
  await sleep(1200);
  await shot("timeline", await panelRect("timeline"));
  await js(`pocket.openPanel('profiler');`);
  await sleep(1500);
  await shot("profiler", await panelRect("profiler"));
  await js(`pocket.openPanel('agent'); pocket.toggleMaximize('agent');`);
  await sleep(500);
  await shot("agent", { x: 0, y: 0, width: W, height: H });
  await js(`pocket.toggleMaximize();`);
  await sleep(300);

  // ---- Debug: a breakpoint in rules.ts pauses the running game ---------------------------------
  await js(`
    const text = await pocket.api.scripts.read('scripts/rules.ts');
    const line = text.split('\\n').findIndex((l) => l.includes('const set = b.hoist_now')) + 1;
    await pocket.actions.debug.toggleBreakpoint('scripts/rules.ts', line);
    pocket.actions.debug.addWatch('speed * 3.6');
    pocket.actions.debug.addWatch('ctx.tick % 60');
  `);
  await sleep(2000);
  await key("F10", "F10", 0, 121);
  await sleep(800);
  await shot("debug-paused");
  await shot("debug", await panelRect("debug"));
  await shot("scripts", await panelRect("scripts"));
  // The console evaluates on the paused frame (the host evaluates only while the debugger holds the game).
  await js(`pocket.openPanel('console');`);
  await sleep(300);
  const [ix, iy] = await center(".console-input input");
  await click(ix, iy);
  await type("({ speed, knots: speed * 1.944, tick: ctx.tick })");
  await key("Enter", "Enter", 0, 13);
  await sleep(600);
  await shot("console", await panelRect("console"));
  await js(`await pocket.actions.debug.clearAllBreakpoints(); await pocket.actions.debug.resume();`);
  await sleep(500);

  // ---- Back to Edit: history shows the agent's and the editor's edits ---------------------------
  await key("p", "KeyP", META);
  await sleep(1000);
  await js(`pocket.openPanel('history');`);
  await sleep(400);
  await shot("history", await panelRect("history"));
  await js(`pocket.ui.getState().set({ shortcutsOpen: true });`);
  await sleep(400);
  await shot("shortcuts", { x: 360, y: 60, width: 880, height: 880 });
  console.log("done");
} finally {
  cdp.close();
  proc.kill();
}
