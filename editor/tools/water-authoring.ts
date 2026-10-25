// Water authored in the real editor against a real host (editor.md 6.1, water.md 65): headless
// Chrome (WebGPU on) opens the editor at --url (the host serving the editor's production build made
// with VITE_AMORIS_EXPOSE=1), and with real mouse and key input creates a terrain from the
// viewport's context menu, draws a river along it with the river tool and keeps it with Enter,
// creates water over the terrain and pours some into the river with the pour tool. It checks each
// edit on the host, saves the edit world as the editor shows it (the water previewed from the
// terrain's beds and the pour) and, after Play, the water running.
//
//   amoris serve <a project with a camera and a light> --port 7891   # AMORIS_EDITOR_DIST=editor/dist
//   bun tools/water-authoring.ts --url http://127.0.0.1:7891/ --out ../out/editor/water

import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { Cdp } from "./cdp";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7891/");
const OUT = resolve(opt("--out", "../out/editor/water"));
const W = 1600;
const H = 1000;
mkdirSync(OUT, { recursive: true });

const chrome = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const port = 9347;
const proc = Bun.spawn(
  [
    chrome,
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${process.env.TMPDIR ?? "/tmp"}/amoris-water-${Date.now()}`,
    "--enable-unsafe-webgpu",
    "--no-first-run",
    "--hide-scrollbars",
    `--window-size=${W},${H}`,
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
const errors: string[] = [];
cdp.on("Runtime.consoleAPICalled", (p) => {
  const e = p as { type: string; args: { value?: unknown; description?: string }[] };
  const text = e.args.map((a) => String(a.value ?? a.description ?? "")).join(" ");
  if (e.type === "error" || /validation error/i.test(text)) errors.push(text);
});
cdp.on("Runtime.exceptionThrown", (p) => errors.push(JSON.stringify((p as { exceptionDetails: unknown }).exceptionDetails)));

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

async function shot(name: string) {
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

async function call(method: string, params: unknown = {}): Promise<unknown> {
  const r = (await (await fetch(new URL("/api/call", URL_), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ method, params }) })).json()) as {
    result?: unknown;
    error?: { code: string; message: string };
  };
  if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
  return r.result;
}

async function mouse(type: "mouseMoved" | "mousePressed" | "mouseReleased", x: number, y: number, button: "left" | "right" | "none" = "left") {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button, buttons: type === "mousePressed" ? (button === "right" ? 2 : 1) : 0, clickCount: type === "mouseMoved" ? 0 : 1 });
}

async function click(x: number, y: number, button: "left" | "right" = "left") {
  await mouse("mouseMoved", x, y, "none");
  await mouse("mousePressed", x, y, button);
  await mouse("mouseReleased", x, y, button);
}

async function key(name: string, code: number) {
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: name, code: name, windowsVirtualKeyCode: code });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code: name, windowsVirtualKeyCode: code });
}

/** The centre of the visible menu item labelled `label`, or of the button whose label starts so. */
async function find(selector: string, label: string): Promise<{ x: number; y: number }> {
  for (let i = 0; i < 40; i++) {
    const at = await js<{ x: number; y: number } | null>(`
      const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => {
        const text = (e.querySelector(".menu-label")?.textContent ?? e.getAttribute("aria-label") ?? "").trim();
        return text.startsWith(${JSON.stringify(label)}) && e.getBoundingClientRect().width > 0;
      });
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    `);
    if (at) return at;
    await Bun.sleep(100);
  }
  throw new Error(`no ${selector} labelled ${label}`);
}

/** World point `p` on the page, through the editor's camera. */
async function onScreen(p: number[]): Promise<{ x: number; y: number }> {
  return js(`
    const ctl = window.amorisEditorViewport;
    const s = ctl.view().project(${JSON.stringify(p)});
    const r = document.querySelector(".viewport-surface").getBoundingClientRect();
    return { x: r.left + s.x, y: r.top + s.y };
  `);
}

/** The entities holding `component`, with that component, as the host has them. */
async function holding(component: string): Promise<{ id: number; value: Record<string, unknown> }[]> {
  const ids = await js<number[]>(`
    const w = window.amoris.world.getState();
    return w.order;
  `);
  const out = [];
  for (const id of ids) {
    const r = (await call("world.get", { entity: id, components: [component] })) as { components?: Record<string, Record<string, unknown>> };
    const value = r.components?.[component];
    if (value) out.push({ id, value });
  }
  return out;
}

async function until(what: string, test: () => Promise<boolean>, tries = 80) {
  for (let i = 0; i < tries; i++) {
    if (await test()) return;
    await Bun.sleep(150);
  }
  throw new Error(`timed out: ${what}`);
}

const result: Record<string, unknown> = {};
try {
  await cdp.send("Runtime.enable");
  await cdp.send("Page.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  const target = new URL(URL_);
  target.searchParams.set("viewport", "wasm");
  await cdp.send("Page.navigate", { url: target.toString() });
  await cdp.once("Page.loadEventFired");
  await until("the editor connects with the wasm viewport", () =>
    js<boolean>(`
      const p = window.amoris;
      if (!p || !window.amorisEditorViewport?.engine) return false;
      const c = p.connection.getState();
      return c.info.state === "open" && c.viewport === "wasm" && p.world.getState().order.length > 0;
    `),
  );
  // Headless Chrome runs the page's animation frames once input arrives.
  await mouse("mouseMoved", 8, H - 8, "none");
  const surface = await js<{ x: number; y: number; w: number; h: number }>(`
    const r = document.querySelector(".viewport-surface").getBoundingClientRect();
    return { x: r.left, y: r.top, w: r.width, h: r.height };
  `);
  const middle = { x: surface.x + surface.w / 2, y: surface.y + surface.h / 2 };

  // 1. A terrain from the viewport's context menu, on the ground under the click.
  await click(middle.x, middle.y, "right");
  const create = await find(".menu-item", "Create");
  await mouse("mouseMoved", create.x, create.y, "none");
  await click(create.x, create.y);
  const terrainItem = await find(".menu-item", "Terrain");
  await click(terrainItem.x, terrainItem.y);
  await until("a terrain", async () => (await holding("Terrain")).length === 1);
  const terrain = (await holding("Terrain"))[0]!;
  const origin = ((await call("world.get", { entity: terrain.id, components: ["Transform"] })) as { components: { Transform: { position: number[] } } }).components.Transform.position;
  result.terrain = { id: terrain.id, origin };
  // Seen from above at a slant, the terrain filling the view.
  await js(`
    window.amoris.selection.getState().set([${terrain.id}]);
    window.amorisEditorViewport.setOrbit({ target: [${origin[0]! + 32}, ${origin[1]! + 2}, ${origin[2]! + 32}], yaw: 0.3, pitch: 0.9, distance: 75 });
    return 0;
  `);
  await Bun.sleep(1500);
  await shot("1-terrain");

  // 2. A river: four points across the terrain with the river tool, kept with Enter.
  const tool = await find("button", "Draw a river");
  await click(tool.x, tool.y);
  const course = [
    [origin[0]! + 6, origin[1]! + 2, origin[2]! + 30],
    [origin[0]! + 24, origin[1]! + 2, origin[2]! + 22],
    [origin[0]! + 42, origin[1]! + 2, origin[2]! + 40],
    [origin[0]! + 58, origin[1]! + 2, origin[2]! + 34],
  ];
  for (const p of course) {
    const s = await onScreen(p);
    await click(s.x, s.y);
    await Bun.sleep(250);
  }
  result.drawn = await js<number>(`return window.amorisEditorViewport.river?.points.length ?? 0;`);
  await shot("2-river-drawn");
  await key("Enter", 13);
  await until("the river carved", async () => {
    const t = (await holding("Terrain"))[0]!;
    return ((t.value.features as unknown[]) ?? []).some((f) => typeof f === "object" && f !== null && "Channel" in f);
  });
  const channel = ((await holding("Terrain"))[0]!.value.features as Record<string, { path: number[][] }>[]).find((f) => "Channel" in f)!.Channel;
  result.channel = channel;
  await Bun.sleep(1500);
  await shot("3-river-carved");

  // 3. Water over the terrain from its context menu, and some poured into the river.
  await key("Escape", 27);
  await click(middle.x, middle.y, "right");
  const waterItem = await find(".menu-item", "Water over Terrain");
  await click(waterItem.x, waterItem.y);
  await until("a water region", async () => (await holding("WaterRegion")).length === 1);
  const region = (await holding("WaterRegion"))[0]!;
  await js(`window.amoris.selection.getState().set([${region.id}]); return 0;`);
  await Bun.sleep(300);
  const pourTool = await find("button", "Pour water");
  await click(pourTool.x, pourTool.y);
  // Into the river's bed, carved under its second point, seen from above so the click reaches
  // the bed rather than a bank: the pour fills the channel there.
  const bed = channel.path[1]!;
  await js(`window.amorisEditorViewport.setOrbit({ pitch: 1.45 }); return 0;`);
  await Bun.sleep(800);
  const at = await onScreen([origin[0]! + bed[0]!, origin[1]! + bed[1]!, origin[2]! + bed[2]!]);
  await click(at.x, at.y);
  if (process.env.WATER_DEBUG) {
    result.pick = await js(`
      const r = document.querySelector(".viewport-surface").getBoundingClientRect();
      const hit = await window.amorisEditorViewport.engine.pick(${at.x} - r.left, ${at.y} - r.top);
      return { hit, aim: ${JSON.stringify([origin[0]! + bed[0]!, origin[1]! + bed[1]!, origin[2]! + bed[2]!])} };
    `);
  }
  await until("the pour", async () => (((await holding("WaterRegion"))[0]!.value.fill as unknown[]) ?? []).length === 1);
  result.fill = (await holding("WaterRegion"))[0]!.value.fill;
  await js(`window.amorisEditorViewport.setOrbit({ pitch: 0.9 }); return 0;`);
  await Bun.sleep(2000);
  await shot("4-poured-edit-world");

  // 4. Play: the water runs.
  await call("play.start", {});
  await Bun.sleep(6000);
  await shot("5-playing");
  await call("play.stop", {});
  result.errors = errors;
  result.ok = errors.length === 0 && (result.drawn as number) === 4 && channel.path.length === 4;
  console.log(JSON.stringify(result));
} finally {
  cdp.close();
  proc.kill();
}
