// The editor's edit loop against a real host (editor.md 6, 8 item 12 and 10): headless Chrome opens
// the editor that `amoris serve` serves (the build made with VITE_AMORIS_EXPOSE=1) and, with real
// input only (clicks, keys, typed text), creates each primitive, each light and a camera from the
// hierarchy's Create menu, renames, edits a field, adds a component, duplicates, deletes, undoes
// and redoes, places a model and a splat from the Assets panel, changes the speed, plays, pauses,
// reads an event's cause chain and resumes, and last plays a world that halts (a tick that fails
// halfway) and stops it. After each step it asks the host itself what happened (`/api/call`:
// `world.tree`, `world.get`, `history.list`, `capture`'s instance count, `time.control`,
// `events.why`, `log.since`) and then reads what the page shows.
//
//   mkdir -p out/gapfix-scratch && cp -R samples/anim out/gapfix-scratch/edit-host
//   amoris serve out/gapfix-scratch/edit-host --port 7971 --editor editor/dist
//   bun tools/edit-host.ts --url http://127.0.0.1:7971/ [--out ../out/editor/edit-host]
//       [--chrome-port 9341] [--viewport wasm|fallback] [--scribble]
//
// On a real host the tool first writes into the served project, over `/api/call` and the file
// system: scripts/main.ts (two systems that emit an event and an event it causes, since no sample
// emits one with a cause) and splats/edit-host.splat (four splats, since samples/anim has none).
// That is setup, not the flow under test, and it is refused for a project that is not a scratch
// copy (no `out` segment in its root) unless --scribble says so. The project is a copy of
// samples/anim: the probe script replaces the project's scripts, which a project whose scripts
// declare components refuses, and the flow places a model, which the project must list; either
// is a wrong setup (exit 2). Use a fresh copy for each run: the edits accumulate.
//
// Against the mock (`bun run mock -- --port <n> --no-agent --dist`) the same steps run with the
// fallback view; what only a renderer can show is printed as `not checked`.
//
// Exits 0 only when every check held, 1 on a failed check, 2 on a wrong setup. What each step saw
// is printed, with four screenshots in <out>.

import { copyFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Cdp, launchChrome } from "./cdp";
import { componentInfo as mockComponent } from "../mock/schemas";
import { defaultComponent, enumValue } from "../src/host/schema";
import type { ComponentInfo } from "../src/host/protocol";

const args = process.argv.slice(2);
const opt = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1]! : fallback;
};
const URL_ = opt("--url", "http://127.0.0.1:7971/");
const OUT = resolve(opt("--out", resolve(import.meta.dir, "../../out/editor/edit-host")));
const W = 1600;
const H = 1000;
// The editor's Mod: Cmd (CDP modifier 4) on macOS, Ctrl (2) elsewhere (src/commands/keys.ts).
const MOD = process.platform === "darwin" ? 4 : 2;
const SHIFT = 8;
mkdirSync(OUT, { recursive: true });

const t0 = Date.now();
const stamp = () => `[${((Date.now() - t0) / 1000).toFixed(1)}s]`;
const failures: string[] = [];
function check(ok: boolean, what: string) {
  console.log(`${stamp()} ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures.push(what);
}
const note = (what: string) => console.log(`${stamp()} --   ${what}`);
function wrongSetup(why: string): never {
  console.log(`edit-host: ${why}`);
  process.exit(2);
}

/** A call to the host itself, beside the page. */
async function call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
  const r = (await (
    await fetch(new URL("/api/call", URL_), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id: 1, method, params }),
      // A debug host's first capture compiles the renderer's shaders; scripts.apply runs tsc.
      signal: AbortSignal.timeout(180_000),
    })
  ).json()) as { result?: T; error?: { code: string; message: string } };
  if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
  return r.result as T;
}

/** Polls `test` every 100 ms; no step waits a fixed time for its effect. */
async function until(what: string, test: () => Promise<boolean>, tries = 600): Promise<boolean> {
  for (let i = 0; i < tries; i++) {
    if (await test().catch(() => false)) return true;
    await Bun.sleep(100);
  }
  console.log(`  timed out: ${what}`);
  return false;
}

const same = (a: unknown, b: unknown) => JSON.stringify(sorted(a)) === JSON.stringify(sorted(b));
function sorted(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sorted);
  if (v && typeof v === "object") return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted((v as Record<string, unknown>)[k])]));
  return v;
}

// ---- The host, before the page ------------------------------------------------------------------

type Node = { id: number; name: string | null; components: string[]; children: Node[] };
type Entity = { id: number; name: string | null; components: Record<string, Record<string, unknown>> };
type History = { undo: string[]; redo: string[]; entries: { undo: { label: string; source: unknown }[] } };
type Asset = { path: string; kind: string };
type HostEvent = { seq: number; id?: number; name: string; cause: number | null };
type Pacing = "stepped" | { real_time: { speed: number } };

async function tree(): Promise<Node[]> {
  const out: Node[] = [];
  const walk = (n: Node) => {
    out.push(n);
    n.children.forEach(walk);
  };
  (await call<Node[]>("world.tree")).forEach(walk);
  return out;
}
const entity = (id: number) => call<Entity>("world.get", { entity: id });
const history = () => call<History>("history.list");
const status = () => call<{ mode: string; paused: boolean; pacing: Pacing }>("time.control", {});

const info = await call<{ name: string; root: string; host?: string }>("project.info").catch((e) => wrongSetup(`no host answers at ${URL_}: ${e instanceof Error ? e.message : String(e)}`));
const mock = info.host === "mock";
const catalogRaw = (await (await fetch(new URL("/api/catalog", URL_))).json()) as { name: string }[] | { commands: { name: string }[] };
const catalog = Array.isArray(catalogRaw) ? catalogRaw : catalogRaw.commands;
const canCapture = catalog.some((c) => c.name === "capture");
const schema = Object.fromEntries((await call<ComponentInfo[]>("world.schema")).map((c) => [c.name, c]));
const MODE = opt("--viewport", mock ? "fallback" : "wasm");
if (MODE !== "wasm" && MODE !== "fallback") wrongSetup(`--viewport is wasm or fallback, not '${MODE}'`);
console.log(`edit-host: ${mock ? "the mock" : "amoris serve"} at ${URL_}, project ${info.name} (${info.root}), viewport ${MODE}${canCapture ? "" : ", no capture"} → ${OUT}`);

const CAMERA = { position: [8, 6, 10], look_at: [0, 0.5, 0], width: 640, height: 360 };
type Capture = { instances: number; pending_assets: number; path: string };
/**
 * What the host's own renderer holds for the world shown: the instance slots it has filled (a
 * mesh that resolved has one; a mesh name that imports nothing has none) and the assets it still
 * waits for. The count is the renderer's high-water mark: it rises with what is added and does
 * not fall when an entity goes, so only additions are read from it.
 */
const capture = (camera: object = CAMERA) => call<Capture>("capture", camera);

const PROBE_SCRIPT = `// The edit loop's probe (editor/tools/edit-host.ts): an event and one it causes, every 30 ticks.
import { game, system } from "amoris";

const knock = system({
  name: "knock",
  phase: "update",
  doc: "Emits probe.knock every 30 ticks.",
  when: { every: 30 },
  run(ctx) {
    ctx.emit("probe.knock", { tick: ctx.tick });
  },
});

const echo = system({
  name: "echo",
  phase: "update",
  doc: "Answers each probe.knock with a probe.echo that names it as its cause.",
  events: ["probe."],
  run(ctx) {
    for (const e of ctx.events) {
      if (e.kind === "probe.knock") ctx.emit("probe.echo", { of: e.seq }, { cause: e.seq });
    }
  },
});

export default game({ components: [], systems: [knock, echo] });
`;

/** Four splats in antimatter15's 32-byte records: position f32x3, scale f32x3, colour and opacity
 * u8x4, rotation u8x4 as w x y z (crates/amoris-render/src/splat/loader.rs). */
function splatFile(): Uint8Array {
  const bytes = new Uint8Array(4 * 32);
  const view = new DataView(bytes.buffer);
  for (let i = 0; i < 4; i++) {
    const o = i * 32;
    [i * 0.4 - 0.6, 0.5, 0].forEach((v, k) => view.setFloat32(o + k * 4, v, true));
    [0.15, 0.15, 0.15].forEach((v, k) => view.setFloat32(o + 12 + k * 4, v, true));
    bytes.set([230, 120 + i * 30, 60, 255], o + 24);
    bytes.set([255, 128, 128, 128], o + 28);
  }
  return bytes;
}

let baseline: Capture | null = null;
if (!mock) {
  console.log("setup (not the flow under test):");
  if (!info.root.split(/[\\/]/).includes("out") && !args.includes("--scribble")) {
    wrongSetup(`the served project ${info.root} is not a scratch copy (no 'out' in its path), and this tool writes scripts/main.ts and a splat into it; serve a copy, or pass --scribble`);
  }
  // The mock's hand-copied schemas against this host's: a copy that drifted fails here.
  for (const name of ["Model", "Light", "Camera", "Splat"]) {
    const host = schema[name]?.schema as { properties?: Record<string, { default?: unknown }>; $defs?: Record<string, { oneOf?: { const?: unknown }[] }> } | undefined;
    const copy = mockComponent(name)?.schema as typeof host;
    const fields = (s: typeof host) => Object.keys(s?.properties ?? {}).sort();
    const defaults = (s: typeof host) => fields(s).map((f) => [f, s!.properties![f]!.default ?? "(none)"]);
    const consts = (s: typeof host) => Object.entries(s?.$defs ?? {}).map(([k, d]) => [k, (d.oneOf ?? []).map((a) => a.const)]);
    const differs = !host ? "the host registers none" : !copy ? "the mock registers none" : !same(fields(host), fields(copy)) ? `fields ${fields(host)} / ${fields(copy)}` : !same(defaults(host), defaults(copy)) ? `defaults ${JSON.stringify(defaults(host))} / ${JSON.stringify(defaults(copy))}` : !same(consts(host), consts(copy)) ? `enum values ${JSON.stringify(consts(host))} / ${JSON.stringify(consts(copy))}` : null;
    check(differs === null, `the mock's ${name} schema is this host's: its fields, their defaults, its enum values${differs ? ` (${differs})` : ""}`);
  }
  const anim = "serve a scratch copy of samples/anim (cp -R samples/anim out/gapfix-scratch/edit-host)";
  if (!(await call<Asset[]>("assets.list")).some((a) => a.kind === "model")) wrongSetup(`the served project ${info.root} lists no model asset to place; ${anim}`);
  const refusedSetup = (e: unknown) => wrongSetup(`the served project ${info.root} does not take the probe script, which replaces its scripts (${e instanceof Error ? e.message : String(e)}); ${anim}`);
  await call("scripts.write", { path: "scripts/main.ts", text: PROBE_SCRIPT }).catch(refusedSetup);
  const applied = await call<{ bundle: string | null; diagnostics: { severity: string; message: string }[] }>("scripts.apply", {}).catch(refusedSetup);
  const refusedBy = applied.diagnostics.filter((d) => d.severity === "error");
  if (!applied.bundle || refusedBy.length) wrongSetup(`scripts.apply refused the probe script: ${JSON.stringify(refusedBy.length ? refusedBy : applied)}; ${anim}`);
  console.log(`  scripts/main.ts: two systems, knock and echo, applied as bundle ${applied.bundle.slice(0, 8)}…`);
  mkdirSync(join(info.root, "splats"), { recursive: true });
  writeFileSync(join(info.root, "splats/edit-host.splat"), splatFile());
  console.log(`  splats/edit-host.splat: 4 splats written into ${info.root}`);
  if (canCapture) {
    baseline = await capture();
    console.log(`  capture (warm-up): ${baseline.instances} instances, ${baseline.pending_assets} pending assets`);
  }
}
const assets = await call<Asset[]>("assets.list");
const modelAsset = assets.find((a) => a.kind === "model");
const splatAsset = assets.find((a) => a.kind === "splat");
const started = await tree();

// ---- The page -----------------------------------------------------------------------------------

const { pageWs, close: closeChrome } = await launchChrome(Number(opt("--chrome-port", "0")), ["--enable-unsafe-webgpu"]);
const cdp = await Cdp.connect(pageWs);
const pageErrors: string[] = [];
cdp.on("Runtime.consoleAPICalled", (p) => {
  const e = p as { type: string; args: { value?: unknown; description?: string }[] };
  if (e.type === "error") pageErrors.push(`console.error: ${e.args.map((a) => String(a.value ?? a.description ?? "")).join(" ")}`);
});
cdp.on("Runtime.exceptionThrown", (p) => {
  const d = (p as { exceptionDetails: { text: string; exception?: { description?: string } } }).exceptionDetails;
  pageErrors.push(`exception: ${d.exception?.description ?? d.text}`);
});

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

async function mouse(type: "mouseMoved" | "mousePressed" | "mouseReleased", x: number, y: number, clickCount = 1) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : clickCount });
}

async function click(at: { x: number; y: number }, count = 1) {
  await mouse("mouseMoved", at.x, at.y);
  for (let n = 1; n <= count; n++) {
    await mouse("mousePressed", at.x, at.y, n);
    await mouse("mouseReleased", at.x, at.y, n);
  }
}

async function key(name: string, code: string, keyCode: number, modifiers = 0) {
  // Enter carries its text, as a keyboard's does.
  const down = name === "Enter" && !modifiers ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : { type: "rawKeyDown" };
  await cdp.send("Input.dispatchKeyEvent", { ...down, key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
}
const enter = () => key("Enter", "Enter", 13);
const undoKey = () => key("z", "KeyZ", 90, MOD);
const redoKey = () => key("z", "KeyZ", 90, MOD | SHIFT);

const type = (text: string) => cdp.send("Input.insertText", { text });

async function shot(name: string) {
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

type Point = { x: number; y: number };

/** The centre of the first element `find` (page JavaScript, an expression) gives, scrolled into
 * view, once it is there. */
async function locate(what: string, find: string): Promise<Point> {
  let at: Point | null = null;
  await until(what, async () => {
    at = await js<Point | null>(`
      const el = ${find};
      if (!el) return null;
      el.scrollIntoView({ block: "nearest" });
      const r = el.getBoundingClientRect();
      return r.width > 0 ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null;
    `);
    return at !== null;
  });
  if (!at) throw new Error(`the page shows no ${what}`);
  return at;
}

/** The visible menu item labelled `label`, and whether the menu offers it. */
const menuItem = (label: string) => `[...document.querySelectorAll(".menu .menu-item")].find((m) => m.querySelector(".menu-label")?.textContent === ${JSON.stringify(label)})`;

type Row = Point & { label: string; icon: string; primary: boolean };

/** The hierarchy row of entity `id` (its `#id` cell): where to click it, the name and the icon it
 * shows, and whether it is the selection. The tree draws only the rows in view, so the row is
 * scrolled there first. */
async function row(id: number): Promise<Row | null> {
  return js(`
    const find = () => [...document.querySelectorAll(".hierarchy .tree-row")].find((r) => r.querySelector(".row-id")?.textContent === "#${id}");
    let row = find();
    const tree = document.querySelector(".hierarchy .tree");
    if (!row && tree) {
      const i = window.amoris.world.getState().order.indexOf(${id});
      if (i < 0) return null;
      tree.scrollTop = Math.max(0, i * 24 - tree.clientHeight / 2);
      await new Promise((r) => setTimeout(r, 60));
      row = find();
    }
    if (!row) return null;
    row.scrollIntoView({ block: "nearest" });
    const label = row.querySelector(".tree-label");
    const r = (label ?? row).getBoundingClientRect();
    return {
      x: r.left + Math.min(r.width / 2, 30),
      y: r.top + r.height / 2,
      label: label?.textContent ?? "",
      icon: row.querySelector(".tree-icon svg")?.getAttribute("class") ?? "",
      primary: row.classList.contains("is-primary"),
    };
  `);
}

const rowShows = (id: number, name: string) => until(`the hierarchy shows #${id} as ${name}`, async () => (await row(id))?.label === name);

/** Clicks the entity's hierarchy row and waits until the page has it selected. */
async function selectRow(id: number) {
  const at = await row(id);
  if (!at) throw new Error(`the hierarchy has no row #${id}`);
  await click(at);
  await until(`#${id} selected`, async () => (await row(id))?.primary === true);
}

/** The inspector's name field, as the page shows it. */
const nameField = () => js<string | null>(`return document.querySelector(".insp-name input")?.value ?? null;`);
const nameFieldReads = (name: string) => until(`the inspector's name field reads ${name}`, async () => (await nameField()) === name);

/** The inspector's card of a component, when it shows one. */
const card = (name: string) => `[...document.querySelectorAll(".inspector .card")].find((c) => c.querySelector(".card-title")?.textContent === ${JSON.stringify(name)})`;

/** What the page's own renderer holds (the wasm viewport's stats, read from the store the viewport
 * panel fills: `engine.stats()` would consume the frames it counts): its instance slots, as the
 * host's capture counts them, and the entities its scene draws now, which fall when one goes. */
const pageStat = (field: "instances" | "entities") => js<number | null>(`return window.amoris.render.getState().stats?.renderer?.${field} ?? null;`);
const pageInstances = () => pageStat("instances");

/** The page's undo and redo stacks as its history store holds them: keys act on what the page has
 * heard from the host, so a key waits for the push. */
const pageHistory = () => js<{ undo: string[]; redo: number }>(`const h = window.amoris.history.getState(); return { undo: h.undo.map((e) => e.label), redo: h.redo.length };`);
const pageUndoTop = (label: string) => until(`the page's history ends with '${label}'`, async () => (await pageHistory()).undo.at(-1) === label);

const speedButton = () => js<string>(`return document.querySelector(".speed-btn")?.textContent?.trim() ?? "";`);
const modeBadge = () => js<string>(`return document.querySelector(".mode-badge")?.textContent?.trim() ?? "";`);

/**
 * Waits until the page has heard two more `status` pushes (the host sends one a second, changed
 * or not). What the transport shows right after a click is the answer to the page's own call; what
 * it shows after these is what the host goes on saying.
 */
async function afterStatusPushes(): Promise<boolean> {
  return js<boolean>(`
    const session = window.amoris.session;
    let left = 2;
    return await new Promise((done) => {
      const timer = setTimeout(() => (stop(), done(false)), 30000);
      const stop = session.subscribe((now, was) => {
        if (now.receivedAt === was.receivedAt || --left > 0) return;
        clearTimeout(timer);
        stop();
        done(true);
      });
    });
  `);
}

type Drawn = { host: number | null; page: number | null; entities: number | null };

/** What the host's renderer and the page's hold, when each can be asked. */
async function drawn(): Promise<Drawn> {
  const wasm = MODE === "wasm";
  return { host: canCapture ? (await capture()).instances : null, page: wasm ? await pageInstances() : null, entities: wasm ? await pageStat("entities") : null };
}

const NO_PAGE_COUNT = MODE === "wasm" ? "the viewport reports no count" : "the fallback view draws from the editor's own data";

/** Checks that an entity with a mesh was added: the host's renderer and the page's each hold one
 * instance more than `before`. */
async function drawnAdded(what: string, before: Drawn) {
  if (before.host === null) note(`not checked: ${what} is drawn by the host (this host has no capture)`);
  else {
    let c: Capture = { instances: -1, pending_assets: -1, path: "" };
    await until(`${what}: the host's capture`, async () => ((c = await capture()), c.pending_assets === 0 && c.instances === before.host! + 1), 20);
    check(c.instances === before.host + 1 && c.pending_assets === 0, `${what}: the host's capture holds ${c.instances} instances (${before.host} before), ${c.pending_assets} assets pending`);
  }
  if (before.page === null) note(`not checked: ${what} is drawn by the page (${NO_PAGE_COUNT})`);
  else {
    const ok = await until(`${what}: the page's viewport`, async () => (await pageInstances()) === before.page! + 1);
    check(ok, `${what}: the page's viewport holds ${await pageInstances()} instances (${before.page} before)`);
  }
}

/** Checks that the page's renderer draws `by` entities more (or fewer) than `before`. The host's
 * capture cannot say: its count does not fall when an entity goes. */
async function entitiesChanged(what: string, before: Drawn, by: number) {
  if (before.host !== null) note(`not checked: ${what} in the host's capture (its instance count is a high-water mark)`);
  if (before.entities === null) note(`not checked: ${what} in what the page draws (${NO_PAGE_COUNT})`);
  else {
    const ok = await until(`${what}: the page's viewport`, async () => (await pageStat("entities")) === before.entities! + by);
    check(ok, `${what}: the page's viewport draws ${await pageStat("entities")} entities (${before.entities} before)`);
  }
}

/** The history entries the host added since `before`. */
async function newEntries(before: History): Promise<{ label: string; source: unknown }[]> {
  return (await history()).entries.undo.slice(before.entries.undo.length);
}

/** An axis of a number or vector field of a component's card in the inspector. */
const fieldInput = (component: string, name: string, axis = 0) =>
  `[...(${card(component)}?.querySelectorAll(".field-row") ?? [])].find((r) => (r.querySelector(".field-label")?.getAttribute("data-tip") ?? "").split(":")[0] === ${JSON.stringify(name)})?.querySelectorAll(".num-input")[${axis}]`;

/** Types a number into a field of the selected entity's inspector and commits it with Enter. */
async function typeNumber(component: string, name: string, axis: number, value: number) {
  const input = fieldInput(component, name, axis);
  await click(await locate(`the ${component} card's ${name} field`, input), 3);
  await until("the field has the focus", () => js<boolean>(`return document.activeElement === (${input});`));
  await type(String(value));
  await enter();
  // Enter commits and leaves the field; the next step starts once the page shows the value.
  await until(`${component}.${name} shows ${value}`, () => js<boolean>(`return document.activeElement !== (${input}) && Number((${input})?.value) === ${value};`));
}

/** Error toasts go after a few seconds: keeps each one the page shows from now on (again after a
 * reload, which starts the page anew). */
async function watchToasts() {
  await js(`
    window.__editHostToasts = [];
    const seen = new Set();
    const keep = (s) => {
      for (const t of s.toasts) if (t.kind === "error" && !seen.has(t.id)) { seen.add(t.id); window.__editHostToasts.push(t.title + (t.body ? ": " + t.body : "")); }
    };
    keep(window.amoris.ui.getState());
    window.amoris.ui.subscribe(keep);
    return 0;
  `);
}
/** The error toasts the page showed since `watchToasts`. */
const errorToasts = () => js<string[]>(`return window.__editHostToasts;`);

/** Add Component on the selected entity `id`: the name typed into the search, whose first answer
 * it must be, and Enter. Answers the component as the host then has it. */
async function addComponent(id: number, name: string): Promise<Record<string, unknown> | undefined> {
  await click(await locate("Add Component", `document.querySelector(".inspector .add-component")`));
  await until("the component search", () => js<boolean>(`return document.activeElement?.classList.contains("entity-search") ?? false;`));
  await type(name);
  const first = () => js<string>(`return document.querySelector(".add-menu .menu-item.is-active .menu-label")?.textContent ?? "";`);
  check(await until("the search's first answer", async () => (await first()) === name), `Add Component: '${name}' typed, the first answer is ${await first()}`);
  await enter();
  let value: Record<string, unknown> | undefined;
  await until(`the host adds ${name}`, async () => ((value = (await entity(id)).components[name]), value !== undefined));
  return value;
}

const created: { label: string; id: number; name: string }[] = [];

/** Create > `label` from the hierarchy's + menu; answers the new entity as the host has it. */
async function create(label: string): Promise<Entity | null> {
  const before = new Set((await tree()).map((n) => n.id));
  const h = await history();
  await click(await locate("the hierarchy's Create button", `document.querySelector('.hierarchy button[aria-label="Create entity"]')`));
  const item = await locate(`Create > ${label}`, menuItem(label));
  const disabled = await js<boolean>(`return ${menuItem(label)}?.classList.contains("is-disabled") ?? true;`);
  check(!disabled, `Create > ${label} is offered`);
  if (disabled) {
    await key("Escape", "Escape", 27);
    return null;
  }
  await click(item);
  let fresh: Node | undefined;
  await until(`the host spawns ${label}`, async () => ((fresh = (await tree()).find((n) => !before.has(n.id))), fresh !== undefined));
  if (!fresh) {
    check(false, `Create > ${label}: the host has a new entity`);
    return null;
  }
  const name = fresh.name ?? "";
  check(name === label || name.startsWith(`${label} `), `Create > ${label}: the host has a new entity #${fresh.id} called ${name}`);
  const entries = await newEntries(h);
  check(entries.length === 1 && entries[0]!.label === `Create ${name}` && entries[0]!.source === "editor", `Create > ${label}: one undo entry from the editor: ${JSON.stringify(entries)}`);
  check((await rowShows(fresh.id, name)) && (await nameFieldReads(name)), `Create > ${label}: the hierarchy row and the inspector's name field show ${name}`);
  created.push({ label, id: fresh.id, name });
  return entity(fresh.id);
}

try {
  await cdp.send("Runtime.enable");
  await cdp.send("Page.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  const target = new URL(URL_);
  target.searchParams.set("viewport", MODE);
  await cdp.send("Page.navigate", { url: target.toString() });
  await cdp.once("Page.loadEventFired");
  if (!(await until("window.amoris", () => js<boolean>(`return !!window.amoris;`), 100))) {
    cdp.close();
    await closeChrome();
    wrongSetup("the page has no window.amoris: build the editor with VITE_AMORIS_EXPOSE=1 (cd editor && VITE_AMORIS_EXPOSE=1 bun run build)");
  }
  await watchToasts();
  // Headless Chrome runs the page's animation frames once input arrives.
  await mouse("mouseMoved", 8, H - 8);
  check(
    await until("the editor shows the host's world", () => js<boolean>(`return window.amoris.world.getState().order.length === ${started.length} && document.querySelectorAll(".hierarchy .tree-row").length > 0;`)),
    `the editor connected: its hierarchy holds the host's ${started.length} entities`,
  );
  if (MODE === "wasm") {
    check(await until("the wasm viewport", () => js<boolean>(`return window.amoris.connection.getState().viewport === "wasm";`)), "the page draws with the engine's renderer (the wasm viewport, WebGPU)");
    // The render feed's first frame fills the scene: the count is the host's once it has come.
    const ok = await until("the viewport's instance count", async () => {
      const n = await pageInstances();
      return n !== null && n > 0 && (baseline === null || n === baseline.instances);
    });
    check(ok, `the page's viewport holds ${await pageInstances()} instances${baseline ? `, the host's capture ${baseline.instances}` : ""}`);
  }

  // ---- 1. Create: the four primitives, the three lights, a camera -------------------------------
  // Each is created where the viewport looks, so one inside the other: its position is typed into
  // the inspector next, a row of four in front of the scene, the plane lifted off the ground.
  for (const [label, mesh, place] of [
    ["Cube", "cube", [-3, 0.5, 4]],
    ["Sphere", "sphere", [-1, 0.5, 4]],
    ["Cylinder", "cylinder", [1, 0.5, 4]],
    ["Plane", "plane", [3, 0.6, 4]],
  ] as const) {
    const before = await drawn();
    const e = await create(label);
    if (!e) continue;
    check(e.components.Model?.mesh === mesh, `Create > ${label}: the host's Model.mesh is '${String(e.components.Model?.mesh)}'`);
    await drawnAdded(`Create > ${label}`, before);
    const was = e.components.Transform?.position as number[];
    for (const axis of [0, 1, 2]) if (was[axis] !== place[axis]) await typeNumber("Transform", "position", axis, place[axis]!);
    const at = async () => (await entity(e.id)).components.Transform?.position;
    check(await until(`${label} moved`, async () => same(await at(), place)), `Move ${label}: the host's Transform.position is ${JSON.stringify(await at())}`);
  }
  if (canCapture) {
    // The host's own picture of the four, from in front of them.
    const c = await capture({ position: [0, 3, 10], look_at: [0, 0.6, 3.5], width: 960, height: 540 });
    if (existsSync(c.path)) {
      copyFileSync(c.path, join(OUT, "edit-host-1-host-capture.png"));
      console.log("  edit-host-1-host-capture.png (the host's capture)");
    }
  }
  for (const [label, kind] of [["Directional Light", "directional"], ["Point Light", "point"], ["Spot Light", "spot"]] as const) {
    const e = await create(label);
    if (!e) continue;
    // The value the host's own schema spells for this kind: nothing here is a literal.
    const want = enumValue(schema.Light, "kind", kind);
    check(want !== null && e.components.Light?.kind === want, `Create > ${label}: the host's Light.kind is '${String(e.components.Light?.kind)}' (its schema's value for ${kind}: '${want}')`);
    const icon = (await row(e.id))?.icon ?? "";
    check(icon.includes("lucide-sun") === (kind === "directional"), `Create > ${label}: the row's icon is ${kind === "directional" ? "the sun" : "not the sun"} (${icon})`);
  }
  {
    const e = await create("Camera");
    const fov = (schema.Camera?.schema as { properties?: Record<string, { default?: unknown }> } | undefined)?.properties?.fov_deg?.default;
    if (e) check(e.components.Camera !== undefined && e.components.Camera.fov_deg === fov, `Create > Camera: the host's entity holds Camera with fov_deg ${String(e.components.Camera?.fov_deg)} (the schema's default ${String(fov)})`);
  }
  note("not checked: that a light lights or a camera shows (their components on the host are)");
  await shot("edit-host-1-created");

  // ---- 2. Edit the cube: rename, a field, a component, duplicate, delete, undo, redo ------------
  const cube = created.find((c) => c.label === "Cube");
  if (!cube) throw new Error("no cube was created: the edits that follow need it");
  const hostName = async (id: number) => (await tree()).find((n) => n.id === id)?.name;

  // (a) Rename: F2 on its row.
  await selectRow(cube.id);
  await key("F2", "F2", 113);
  check(await until("the rename field", () => js<boolean>(`return document.activeElement?.classList.contains("tree-rename") ?? false;`)), "F2 opens the row's rename field");
  await type("Block");
  await enter();
  check(await until("the host renames", async () => (await hostName(cube.id)) === "Block"), `Rename: the host calls #${cube.id} Block`);
  check((await rowShows(cube.id, "Block")) && (await nameFieldReads("Block")), "Rename: the hierarchy row and the name field show Block");

  // (b) A field: Model.metallic typed into the inspector.
  {
    const h = await history();
    const field = fieldInput("Model", "metallic");
    await typeNumber("Model", "metallic", 0, 0.5);
    check(await until("the host's Model.metallic", async () => (await entity(cube.id)).components.Model?.metallic === 0.5), "Edit a field: the host's Model.metallic is 0.5");
    await pageUndoTop("Set Model.metallic on Block");
    check(await js<boolean>(`return document.activeElement !== (${field}) && Number((${field})?.value) === 0.5;`), "Edit a field: Enter left the field, which shows 0.5");
    const entries = await newEntries(h);
    check(entries.length === 1 && entries[0]!.label === "Set Model.metallic on Block", `Edit a field: exactly one undo entry: ${JSON.stringify(entries.map((e) => e.label))}`);
  }

  // (c) Add Component: the search's first answer to "Light" is Light, and Enter adds it.
  {
    const h = await history();
    const light = await addComponent(cube.id, "Light");
    check(light !== undefined && same(light, defaultComponent(schema.Light!)), `Add Component: the host's Block holds Light at its schema's defaults: ${JSON.stringify(light)}`);
    check(await until("the Light card", () => js<boolean>(`return !!(${card("Light")});`)), "Add Component: the inspector shows a Light card");
    const entries = await newEntries(h);
    // One entry, and no second commit of the field edited before it.
    check(entries.length === 1 && entries[0]!.label === "Add Light to Block", `Add Component: one undo entry: ${JSON.stringify(entries.map((e) => e.label))}`);
    check((await entity(cube.id)).components.Model?.metallic === 0.5, "Add Component: the host's Model.metallic is still 0.5");
    await pageUndoTop("Add Light to Block");
    await selectRow(cube.id);
    await undoKey();
    check(await until("the undo", async () => (await entity(cube.id)).components.Light === undefined), "Mod+Z: the Light is gone from Block on the host");
    check(await until("the Light card goes", () => js<boolean>(`return !(${card("Light")});`)), "Mod+Z: the inspector shows no Light card");
  }

  // (c2) A component without fields (a marker): its value is {}, which the host takes.
  if (!schema.Occluder) note("not checked: adding a component without fields (this host registers no Occluder)");
  else {
    const h = await history();
    await pageUndoTop("Set Model.metallic on Block");
    const marker = await addComponent(cube.id, "Occluder");
    check(marker !== undefined && same(marker, {}), `Add Component: the host's Block holds Occluder, a component without fields: ${JSON.stringify(marker)}`);
    check(await until("the Occluder card", () => js<boolean>(`return !!(${card("Occluder")});`)), "Add Component: the inspector shows an Occluder card");
    const entries = await newEntries(h);
    check(entries.length === 1 && entries[0]!.label === "Add Occluder to Block", `Add Component: one undo entry: ${JSON.stringify(entries.map((e) => e.label))}`);
    await pageUndoTop("Add Occluder to Block");
    await selectRow(cube.id);
    await undoKey();
    check(await until("the undo", async () => (await entity(cube.id)).components.Occluder === undefined), "Mod+Z: the Occluder is gone from Block on the host");
  }

  // (d) Duplicate with Mod+D.
  let copy: Node | undefined;
  {
    const before = await drawn();
    const ids = new Set((await tree()).map((n) => n.id));
    await pageUndoTop("Set Model.metallic on Block");
    await selectRow(cube.id);
    await key("d", "KeyD", 68, MOD);
    await until("the host duplicates", async () => ((copy = (await tree()).find((n) => !ids.has(n.id))), copy !== undefined));
    if (!copy) throw new Error("Mod+D spawned nothing on the host");
    const { Name: _a, ...original } = (await entity(cube.id)).components;
    const { Name: _b, ...twin } = (await entity(copy.id)).components;
    check(copy.name === "Block 2" && same(original, twin), `Mod+D: the host has #${copy.id} ${copy.name} with Block's components (${Object.keys(twin).join(", ")})`);
    await drawnAdded("Mod+D", before);
    check((await nameFieldReads("Block 2")) && (await row(copy.id))?.primary === true, "Mod+D: the copy is selected and the name field reads Block 2");
  }

  // (e) Delete, (f) undo, redo, undo.
  {
    const id = copy.id;
    const has = async () => (await tree()).some((n) => n.id === id);
    let before = await drawn();
    await pageUndoTop("Duplicate Block");
    await key("Delete", "Delete", 46);
    check(await until("the host deletes", async () => !(await has())), `Delete: #${id} left the host's world`);
    await entitiesChanged("Delete", before, -1);
    check(await until("the row goes", async () => (await row(id)) === null), "Delete: the hierarchy has no row for it");

    before = await drawn();
    await pageUndoTop("Delete Block 2");
    await selectRow(cube.id);
    await undoKey();
    check(await until("the undo", has), `Mod+Z: Block 2 is back on the host under the same id #${id}`);
    await entitiesChanged("Mod+Z", before, 1);
    check(await rowShows(id, "Block 2"), "Mod+Z: the hierarchy shows Block 2 again");

    before = await drawn();
    // Redo is enabled from the page's history store: the key waits for the host's push.
    await until("the page can redo", async () => (await pageHistory()).redo > 0);
    await redoKey();
    check(await until("the redo", async () => !(await has())), "Mod+Shift+Z: Block 2 is gone from the host again");
    await entitiesChanged("Mod+Shift+Z", before, -1);

    await pageUndoTop("Delete Block 2");
    await undoKey();
    check(await until("the second undo", has), "Mod+Z again: Block 2 is back on the host");
    check(await rowShows(id, "Block 2"), "Mod+Z again: the hierarchy shows Block 2");
    await selectRow(cube.id);
  }

  // (g) A field that was only focused writes nothing when it is left: another client changes the
  // value meanwhile (over /api/call, as an agent's edit comes), and leaving the field keeps it.
  {
    const field = fieldInput("Model", "metallic");
    await click(await locate("the Model card's metallic field", field));
    await until("the field has the focus", () => js<boolean>(`return document.activeElement === (${field});`));
    const h = await history();
    const label = "edit-host: another client's edit";
    await call("world.edit", { ops: [{ set: { entity: cube.id, component: "Model", value: { metallic: 0.9 } } }], label });
    await until("the page hears of it", () => js<boolean>(`return window.amoris.world.getState().entities[${cube.id}]?.components.Model?.metallic === 0.9;`));
    await click(await locate("the inspector's name field", `document.querySelector(".insp-name input")`));
    await until("the field lost the focus", () => js<boolean>(`return document.activeElement !== (${field});`));
    // A call of the page's own, answered by the game thread after any edit the blur had sent.
    await js(`await window.amoris.api.time.control({}); return 0;`);
    const entries = await newEntries(h);
    const metallic = (await entity(cube.id)).components.Model?.metallic;
    check(metallic === 0.9 && entries.length === 1 && entries[0]!.label === label, `Leave an untouched field: the host's Model.metallic is ${String(metallic)} (the other client's 0.9), undo entries since: ${JSON.stringify(entries.map((e) => e.label))}`);
    check(await until("the field shows it", () => js<boolean>(`return Number((${field})?.value) === 0.9;`)), "Leave an untouched field: the field shows 0.9");
  }

  // (h) Mod+Z with the focus still in a select undoes the choice just made there.
  const pointLight = created.find((c) => c.label === "Point Light");
  const spot = enumValue(schema.Light, "kind", "spot");
  if (!pointLight || spot === null) note("not checked: Mod+Z with the focus in a select (no Point Light was created)");
  else {
    const before = (await entity(pointLight.id)).components.Light?.kind;
    await selectRow(pointLight.id);
    const select = `[...(${card("Light")}?.querySelectorAll(".field-row") ?? [])].find((r) => (r.querySelector(".field-label")?.getAttribute("data-tip") ?? "").split(":")[0] === "kind")?.querySelector("select")`;
    await locate("the Light card's kind select", select);
    // The option is chosen by script (a native select's popup is not driven here); the key is real.
    await js(`const s = ${select}; s.focus(); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(s, ${JSON.stringify(spot)}); s.dispatchEvent(new Event("change", { bubbles: true })); return 0;`);
    check(await until("the host's kind", async () => (await entity(pointLight.id)).components.Light?.kind === spot), `Choose a kind: the host's Light.kind is ${spot}`);
    await pageUndoTop(`Set Light.kind on ${pointLight.name}`);
    const focused = await js<string>(`return document.activeElement?.tagName ?? "";`);
    await undoKey();
    check(focused === "SELECT" && (await until("the undo", async () => (await entity(pointLight.id)).components.Light?.kind === before)), `Mod+Z with the focus in the select (${focused}): the host's Light.kind is ${String(before)} again`);
  }

  // (i) The focus passing through a colour's swatch writes nothing: the hex field is clicked,
  // Shift+Tab moves the focus to the swatch and Tab leaves it again.
  {
    await selectRow(cube.id);
    const hex = `${card("Model")}?.querySelector(".color-field .hex-input")`;
    await click(await locate("the Model card's colour hex field", hex));
    await until("the hex field has the focus", () => js<boolean>(`return document.activeElement === (${hex});`));
    const h = await history();
    const color = (await entity(cube.id)).components.Model?.color;
    await key("Tab", "Tab", 9, SHIFT);
    const on = await js<string>(`return document.activeElement?.getAttribute("type") ?? "";`);
    await key("Tab", "Tab", 9);
    await until("the swatch lost the focus", () => js<boolean>(`return document.activeElement?.getAttribute("type") !== "color";`));
    await js(`await window.amoris.api.time.control({}); return 0;`);
    const entries = await newEntries(h);
    const now = (await entity(cube.id)).components.Model?.color;
    check(on === "color" && entries.length === 0 && same(now, color), `Tab through a colour's swatch (the focus was on an input of type '${on}'): the host's Model.color is still ${JSON.stringify(now)}, undo entries since: ${JSON.stringify(entries.map((e) => e.label))}`);
    await selectRow(cube.id);
  }
  await shot("edit-host-2-inspector");

  // ---- 3. Place a model and a splat from the Assets panel ---------------------------------------
  const assetCard = (path: string) => `[...document.querySelectorAll(".assets-panel .asset-card")].find((c) => (c.getAttribute("data-tip") ?? "").startsWith(${JSON.stringify(`${path} ·`)}))`;
  /** Double-clicks the asset's card; answers the entity the host spawned. */
  async function place(path: string): Promise<Entity | null> {
    const ids = new Set((await tree()).map((n) => n.id));
    const h = await history();
    await click(await locate(`the asset card of ${path}`, assetCard(path)), 2);
    let fresh: Node | undefined;
    await until(`the host places ${path}`, async () => ((fresh = (await tree()).find((n) => !ids.has(n.id))), fresh !== undefined));
    if (!fresh) {
      check(false, `Place ${path}: the host has a new entity`);
      return null;
    }
    const entries = await newEntries(h);
    check(entries.length === 1 && entries[0]!.label === `Place ${path}`, `Place ${path}: one undo entry: ${JSON.stringify(entries.map((e) => e.label))}`);
    check((await rowShows(fresh.id, fresh.name ?? "")) && (await nameFieldReads(fresh.name ?? "")), `Place ${path}: the hierarchy and the name field show ${fresh.name}`);
    created.push({ label: path, id: fresh.id, name: fresh.name ?? "" });
    return entity(fresh.id);
  }
  if (!modelAsset) check(false, "the project lists a model asset to place");
  else {
    const before = await drawn();
    const e = await place(modelAsset.path);
    if (e) {
      check(e.components.Model?.mesh === modelAsset.path && e.components.Splat === undefined, `Place a model: the host's #${e.id} ${e.name} holds Model.mesh '${String(e.components.Model?.mesh)}'`);
      if (before.host === null) note("not checked: the placed model is drawn by the host (this host has no capture)");
      else {
        let c: Capture = { instances: -1, pending_assets: -1, path: "" };
        await until("the model's meshes", async () => ((c = await capture()), c.pending_assets === 0 && c.instances > before.host!), 50);
        check(c.pending_assets === 0 && c.instances > before.host, `Place a model: the host's capture holds ${c.instances} instances (${before.host} before), ${c.pending_assets} assets pending`);
      }
      if (before.page === null) note("not checked: the placed model is drawn by the page");
      else check(await until("the page draws the model", async () => ((await pageInstances()) ?? 0) > before.page!), `Place a model: the page's viewport holds ${await pageInstances()} instances (${before.page} before)`);
    }
  }
  if (!mock) {
    // The splat was written after the host started: the panel lists it after a refresh.
    await click(await locate("the Assets panel's Refresh", `document.querySelector('.assets-panel button[aria-label="Refresh"]')`));
  }
  if (!splatAsset) check(false, "the project lists a splat asset to place");
  else {
    const e = await place(splatAsset.path);
    if (e) {
      check(e.components.Splat?.asset === splatAsset.path && e.components.Model === undefined, `Place a splat: the host's #${e.id} ${e.name} holds Splat.asset '${String(e.components.Splat?.asset)}' and no Model`);
      note("not checked: that the placed splat is drawn (a splat is no instance; its component on the host is)");
    }
  }
  await shot("edit-host-3-placed");

  // ---- 4. The speed, Play at it, a cause chain, Stop --------------------------------------------
  /** Chooses a speed from the transport's menu. */
  async function chooseSpeed(label: string) {
    await click(await locate("the speed button", `document.querySelector(".speed-btn")`));
    await click(await locate(`the speed ${label}`, menuItem(label)));
  }
  const speedIs = (speed: number) => until(`the host's speed ${speed}`, async () => same((await status()).pacing, { real_time: { speed } }));
  await chooseSpeed("2×");
  check(await speedIs(2), `Speed: the host's edit world paces at ${JSON.stringify((await status()).pacing)}`);
  check(await until("the button", async () => (await speedButton()) === "2×"), `Speed: the transport's button reads ${await speedButton()}`);
  check((await afterStatusPushes()) && (await speedButton()) === "2×", `Speed: after the host's next status pushes the button still reads ${await speedButton()}`);

  const eventsAfter = async (seq: number) => (await call<{ events: HostEvent[] }>("events.since", { seq, limit: 5000 })).events;
  // The newest event before this Play: a cause chain is read from an event this Play emits.
  const lastBefore = (await call<{ last: number }>("events.since", { limit: 1 })).last;
  await click(await locate("the transport's Play button", `document.querySelector('.transport button[aria-label^="Play"]')`));
  check(await until("Play", async () => (await status()).mode === "play"), "Play: the host is in Play");
  // The page too, before any further click: its Pause acts on the mode it has heard of.
  check(await until("the badge in Play", async () => (await modeBadge()) === "PLAY"), `Play: the transport's badge reads ${await modeBadge()}`);
  // Each world has its own pacing: the fork runs at 2 only because the editor asked for it.
  check(same((await status()).pacing, { real_time: { speed: 2 } }), `Play: the fork paces at ${JSON.stringify((await status()).pacing)} (the editor sent play.start {speed: 2})`);
  check(await until("the button in Play", async () => (await speedButton()) === "2×"), `Play: the transport's button still reads ${await speedButton()}`);

  let caused: HostEvent | undefined;
  check(await until("an event with a cause", async () => ((caused = (await eventsAfter(lastBefore)).find((e) => e.cause !== null)), caused !== undefined), 1200), `Play: the game emitted an event with a cause: ${JSON.stringify(caused)}`);
  if (caused) {
    // Paused, so the list under the pointer stands still.
    await click(await locate("the transport's Pause button", `document.querySelector('.transport button[aria-label="Pause"]')`));
    check(await until("the pause", async () => (await status()).paused), "Pause: the host's Play world is paused");
    check((await until("the badge", async () => (await modeBadge()) === "PAUSED")) && (await afterStatusPushes()) && (await modeBadge()) === "PAUSED", `Pause: the transport's badge reads ${await modeBadge()}, also after the host's next status pushes`);
    const held = (await eventsAfter(0)).length;
    await click(await locate("the dock's Events tab", `[...document.querySelectorAll(".dock-tab")].find((t) => t.querySelector(".dock-tab-title")?.textContent === "Events")`));
    const eventRow = `[...document.querySelectorAll(".events-panel .ev-body .ev-row")].find((r) => r.firstElementChild?.textContent === "${caused.seq}")`;
    check(await until("the page's events", () => js<boolean>(`return window.amoris.events.getState().events.length === ${held} && !!(${eventRow});`)), `Events: the panel lists the host's ${held} events`);
    // On its name: the row's subject is a link that selects the entity instead.
    await click(await locate(`the row of event ${caused.seq}`, `(${eventRow})?.querySelector(".ev-name")`));
    const why = await call<{ event: HostEvent; causes: HostEvent[]; complete: boolean }>("events.why", { seq: caused.seq });
    // The host answers the causes nearest first; the panel shows the chain oldest first.
    const want = [...why.causes].reverse().concat(why.event).map((e) => e.name);
    const shown = () => js<{ names: string[]; self: boolean[] }>(`
      const items = [...document.querySelectorAll(".events-panel .why .chain li")];
      return { names: items.map((li) => li.querySelector(".chain-name")?.textContent ?? ""), self: items.map((li) => li.classList.contains("is-self")) };
    `);
    const ok = await until("the cause chain", async () => same((await shown()).names, want));
    const chain = await shown();
    check(ok && want.length >= 2 && chain.self.at(-1) === true && chain.self.filter(Boolean).length === 1, `Events: the chain reads ${chain.names.join(" → ")}, the host's events.why ${want.join(" → ")} (complete: ${why.complete}), the event itself last`);
    await shot("edit-host-4-cause-chain");
    // The same button resumes: it reads the pause from the host's status.
    await click(await locate("the transport's Resume button", `document.querySelector('.transport button[aria-label="Resume"]')`));
    check((await until("the resume", async () => !(await status()).paused)) && (await until("the badge", async () => (await modeBadge()) === "PLAY")), `Resume: the host's Play world runs again, the badge reads ${await modeBadge()}`);
  }

  await click(await locate("the transport's Stop button", `document.querySelector('.transport button[aria-label^="Stop"]')`));
  check(await until("Stop", async () => (await status()).mode === "edit"), "Stop: the host is back in Edit");
  const after = new Set((await tree()).map((n) => n.id));
  const kept = created.filter((c) => after.has(c.id));
  check(kept.length === created.length, `Stop: the edit world still holds the ${created.length} entities made here (${kept.length} found)`);
  check((await speedIs(2)) && (await until("the button after Stop", async () => (await speedButton()) === "2×")), `Stop: the edit world's speed is shown again: ${await speedButton()}`);
  await chooseSpeed("1×");
  check((await speedIs(1)) && (await until("the button", async () => (await speedButton()) === "1×")), `Speed: back to 1 on the host, the button reads ${await speedButton()}`);

  // ---- 5. A Play world that halts ---------------------------------------------------------------
  // A Vessel whose collider spans no volume fails the first tick halfway (water.vessel_invalid):
  // the world is poisoned, the host runs no further tick of it, and its own status then carries
  // `world_hash: null`. Both components are added at the editor's defaults, in the inspector.
  /** The error toasts that say a world halted; every other one fails the run at the end. */
  const haltToast = (t: string) => /^(Play|The edit world) halted at tick \d+/.test(t);
  /** The error toasts of the page before its reload in this step. */
  let earlier: string[] = [];
  if (mock) note("not checked: a Play world that halts (the mock's worlds never do)");
  else if (!schema.Vessel || !schema.Collider) note("not checked: a Play world that halts (this host registers no Vessel or no Collider)");
  else {
    type Thread = { mode: string; paused: boolean; halted?: boolean; poisoned?: number | null; world_hash: string | null };
    const thread = () => call<Thread>("time.control", {});
    /** What is left of the page: a blank one has no child under #root; a part that failed to
     * draw shows a `.boundary` in its place. */
    const pageUp = () => js<{ root: number; rows: number; failed: number; statusbar: boolean }>(`
      return {
        root: document.querySelector("#root")?.children.length ?? 0,
        rows: document.querySelectorAll(".hierarchy .tree-row").length,
        failed: document.querySelectorAll(".boundary").length,
        statusbar: !!document.querySelector(".statusbar"),
      };
    `);
    const up = (p: { root: number; rows: number; failed: number; statusbar: boolean }) => p.root > 0 && p.rows > 0 && p.failed === 0 && p.statusbar;
    const raft = await create("Cube");
    if (!raft) throw new Error("no cube was created for the world that halts");
    for (const name of ["Vessel", "Collider"]) {
      const added = await addComponent(raft.id, name);
      check(added !== undefined, `Add Component: the host's ${raft.name} holds ${name}`);
      await pageUndoTop(`Add ${name} to ${raft.name}`);
    }
    const logBefore = (await call<{ last: number }>("log.since", { limit: 1 })).last;
    await click(await locate("the transport's Play button", `document.querySelector('.transport button[aria-label^="Play"]')`));
    let st: Thread = await thread();
    const halts = await until("the host's Play world halts", async () => ((st = await thread()), st.mode === "play" && st.halted === true), 300);
    check(halts && typeof st.poisoned === "number" && st.world_hash === null, `Play a Vessel whose collider spans no volume: the host's Play world is halted (poisoned at tick ${String(st.poisoned)}, world_hash ${String(st.world_hash)})`);
    if (halts) {
      // The host says why on its log; the page shows that line and says the world halted.
      const hostLines = (await call<{ lines: { level: string; source: string; message: string; tick?: number }[] }>("log.since", { seq: logBefore, limit: 200 })).lines;
      const why = hostLines.find((l) => l.level === "error" && l.tick === st.poisoned);
      check(why !== undefined, `Halted: the host's log has the tick's error: ${why ? `${why.source}: ${why.message}` : JSON.stringify(hostLines)}`);
      check(await until("the badge", async () => (await modeBadge()) === "HALTED"), `Halted: the transport's badge reads ${await modeBadge()}`);
      const buttons = await js<Record<string, boolean>>(`return Object.fromEntries([...document.querySelectorAll(".transport-buttons button")].map((b) => [(b.getAttribute("aria-label") ?? "").split(" (")[0], b.disabled]));`);
      check(buttons.Stop === false && buttons.Resume === true && buttons["Step one tick"] === true, `Halted: Stop is offered, Resume and Step are not: ${JSON.stringify(buttons)}`);
      await click(await locate("the dock's Console tab", `[...document.querySelectorAll(".dock-tab")].find((t) => t.querySelector(".dock-tab-title")?.textContent === "Console")`));
      const consoleErrors = () => js<string[]>(`return [...document.querySelectorAll(".console .log-row.lvl-error")].map((r) => (r.querySelector(".log-source")?.textContent ?? "") + ": " + (r.querySelector(".log-msg")?.textContent ?? ""));`);
      const said = await until("the Console's lines", async () => {
        const lines = await consoleErrors();
        return lines.some((l) => why !== undefined && l === `${why.source}: ${why.message}`) && lines.some((l) => l.startsWith(`editor: Play halted at tick ${st.poisoned}. `));
      });
      check(said, `Halted: the Console shows the host's error and the editor's own line: ${JSON.stringify(await consoleErrors())}`);
      earlier = await errorToasts();
      const said1 = earlier.filter(haltToast);
      check(said1.length === 1 && said1[0]!.startsWith(`Play halted at tick ${st.poisoned}: `), `Halted: one toast says so, with the host's tick: ${JSON.stringify(said1)}`);
      // A click on the disabled Resume asks for nothing. A speed from the menu is a call whose
      // answer is the game thread's own status, with world_hash null: the page keeps each status
      // it takes with one, so the check below knows that answer reached it.
      await js(`
        window.__editHostNullHash = 0;
        window.amoris.session.subscribe((s) => { if (s.status && s.status.world_hash === null) window.__editHostNullHash++; });
        return 0;
      `);
      await click(await locate("the transport's Resume button", `document.querySelector('.transport button[aria-label="Resume"]')`));
      await chooseSpeed("4×");
      check(await speedIs(4), `Halted: a speed chosen is taken by the host: ${JSON.stringify((await status()).pacing)}`);
      check(await until("the button", async () => (await speedButton()) === "4×"), `Halted: the transport's button reads ${await speedButton()}`);
      const taken = await until("the answer without a hash", () => js<boolean>(`return window.__editHostNullHash > 0;`));
      let page = await pageUp();
      check(taken && up(page) && (await modeBadge()) === "HALTED", `Halted: the page took a status with world_hash null and is all there (${page.rows} hierarchy rows, ${page.failed} parts failed, the badge ${await modeBadge()})`);
      check((await thread()).halted === true, "Halted: the host's Play world is still halted");
      await shot("edit-host-5-halted");
      // A reload reads the same status first.
      const loaded = cdp.once("Page.loadEventFired");
      await cdp.send("Page.reload");
      await loaded;
      await until("window.amoris", () => js<boolean>(`return !!window.amoris;`), 100);
      await watchToasts();
      await mouse("mouseMoved", 8, H - 8);
      const back = await until("the page after a reload", async () => ((page = await pageUp()), up(page) && (await modeBadge()) === "HALTED"));
      check(back, `Halted: a reloaded page comes up (${page.rows} hierarchy rows, ${page.failed} parts failed) and its badge reads ${await modeBadge()}`);
    }
    await click(await locate("the transport's Stop button", `document.querySelector('.transport button[aria-label^="Stop"]')`));
    check(await until("Stop", async () => ((st = await thread()), st.mode === "edit" && st.halted === false && st.world_hash !== null)), `Stop: the host is back in an edit world that is not halted (world_hash ${String(st.world_hash).slice(0, 8)})`);
    check(await until("the badge", async () => (await modeBadge()) === "EDIT"), `Stop: the transport's badge reads ${await modeBadge()}`);
    // The entity that halts Play is deleted again, so the world plays after this run.
    await selectRow(raft.id);
    await key("Delete", "Delete", 46);
    check(await until("the host deletes", async () => !(await tree()).some((n) => n.id === raft.id)), `Delete: ${raft.name} left the host's world`);
  }

  const toasts = earlier.concat(await errorToasts()).filter((t) => !haltToast(t));
  check(toasts.length === 0, `the page showed no error toast${toasts.length ? `: ${toasts.join(" | ")}` : ""}`);
  check(pageErrors.length === 0, `the page threw nothing and logged no error${pageErrors.length ? `: ${pageErrors.slice(0, 5).join(" | ")}` : ""}`);

  // ---- 6. A part of the window that cannot draw -------------------------------------------------
  // Not input: a status the transport cannot draw is put into the page's store (a `tick_ms` that
  // is no number), as a host's unexpected value would arrive. The top bar says it failed, the
  // rest of the window stays, and Try again brings the bar back once a status it can draw came.
  {
    await js(`const s = window.amoris.session.getState(); s.setStatus({ ...s.status, tick_ms: "fast" }); return 0;`);
    const failedParts = () => js<string[]>(`return [...document.querySelectorAll(".boundary .boundary-title")].map((b) => b.textContent ?? "");`);
    check(await until("the top bar's boundary", async () => same(await failedParts(), ["The top bar failed to draw"])), `A status the transport cannot draw: ${JSON.stringify(await failedParts())}`);
    const rest = await js<{ rows: number; statusbar: boolean; dock: boolean }>(`return { rows: document.querySelectorAll(".hierarchy .tree-row").length, statusbar: !!document.querySelector(".statusbar"), dock: !!document.querySelector(".dock") };`);
    check(rest.rows > 0 && rest.statusbar && rest.dock, `The rest of the window is still there: ${JSON.stringify(rest)}`);
    await js(`window.amoris.session.getState().setStatus(await window.amoris.api.time.control({})); return 0;`);
    await click(await locate("the boundary's Try again", `document.querySelector(".boundary button")`));
    check(await until("the top bar", () => js<boolean>(`return !document.querySelector(".boundary") && !!document.querySelector(".transport .mode-badge");`)), `Try again: the top bar is back, its badge reads ${await modeBadge()}`);
  }
} catch (e) {
  check(false, `the driver failed: ${e instanceof Error ? e.message : String(e)}`);
  await shot("edit-host-failure").catch(() => undefined);
} finally {
  cdp.close();
  await closeChrome();
}
console.log(`\n${failures.length} failures`);
process.exit(failures.length ? 1 : 0);
