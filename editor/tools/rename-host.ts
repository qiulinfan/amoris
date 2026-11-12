// The editor's rename against a real host (editor.md 8 item 2 and 10; server.md 3.1, Names):
// headless Chrome opens the editor that `amoris serve` serves and, with real input only (clicks,
// keys, typed text), renames an entity in the hierarchy with F2, undoes and redoes it, renames it
// again in the inspector's name field, and tries a name of 66 bytes, which the host refuses. After
// each step it asks the host what the entity is called (`/api/call`, not the page) and reads what
// the page shows.
//
//   amoris serve <copy of samples/sailing> --port 7975 --editor editor/dist
//   bun tools/rename-host.ts --url http://127.0.0.1:7975/ [--chrome-port 9345] [--out <dir>]
//
// Exits 0 only when every step held; what each step saw is printed, with two screenshots in <out>.

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
const URL_ = opt("--url", "http://127.0.0.1:7975/");
const OUT = resolve(opt("--out", resolve(import.meta.dir, "../../out/bench-runs/editor")));
const W = 1600;
const H = 1000;
// The editor's Mod: Cmd (CDP modifier 4) on macOS, Ctrl (2) elsewhere (src/commands/keys.ts).
const MOD = process.platform === "darwin" ? 4 : 2;
const SHIFT = 8;
mkdirSync(OUT, { recursive: true });

const port = Number(opt("--chrome-port", "0")) || (await freePort());
const profile = mkdtempSync(join(tmpdir(), "amoris-rename-"));
const proc = Bun.spawn(
  [chromePath(), "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", `--window-size=${W},${H}`, ...chromeFlags(), "about:blank"],
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
/** Ends the Chrome this tool started and removes its profile, locked until the process exits. */
async function closeChrome() {
  proc.kill();
  await proc.exited;
  rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
if (!pageWs) {
  await closeChrome();
  throw new Error("Chrome did not start");
}
const cdp = await Cdp.connect(pageWs);
const t0 = Date.now();
const failures: string[] = [];
function check(ok: boolean, what: string) {
  console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s] ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures.push(what);
}

async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`,
    awaitPromise: true,
    returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}\n${expression}`);
  return r.result.value as T;
}

/** A call to the host itself, beside the page. */
async function call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
  const r = (await (await fetch(new URL("/api/call", URL_), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id: 1, method, params }) })).json()) as {
    result?: T;
    error?: { code: string; message: string };
  };
  if (r.error) throw new Error(`${method}: ${r.error.code} ${r.error.message}`);
  return r.result as T;
}

/** What the host calls entity `id`. */
const hostName = async (id: number) => (await call<{ name: string | null }[]>("world.tree", { root: id }))[0]?.name;

type History = { undo: string[]; redo: string[]; entries: { undo: { label: string; source: unknown }[] } };

async function until(what: string, test: () => Promise<boolean>, tries = 100): Promise<boolean> {
  for (let i = 0; i < tries; i++) {
    if (await test().catch(() => false)) return true;
    await Bun.sleep(100);
  }
  console.log(`  timed out: ${what}`);
  return false;
}

async function mouse(type: "mouseMoved" | "mousePressed" | "mouseReleased", x: number, y: number, clickCount = 1) {
  await cdp.send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : clickCount });
}

async function click(x: number, y: number, count = 1) {
  await mouse("mouseMoved", x, y);
  for (let n = 1; n <= count; n++) {
    await mouse("mousePressed", x, y, n);
    await mouse("mouseReleased", x, y, n);
  }
}

async function key(name: string, code: string, keyCode: number, modifiers = 0) {
  // Enter carries its text, as a keyboard's does.
  const down = name === "Enter" && !modifiers ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : { type: "rawKeyDown" };
  await cdp.send("Input.dispatchKeyEvent", { ...down, key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
}

const type = (text: string) => cdp.send("Input.insertText", { text });

/** The centre of the hierarchy row of entity `id` (its `#id` cell), and the name the row shows. */
async function row(id: number): Promise<{ x: number; y: number; label: string } | null> {
  return js(`
    const row = [...document.querySelectorAll(".hierarchy .tree-row")].find((r) => r.querySelector(".row-id")?.textContent === "#${id}");
    if (!row) return null;
    row.scrollIntoView({ block: "nearest" });
    const label = row.querySelector(".tree-label");
    const r = (label ?? row).getBoundingClientRect();
    return { x: r.left + Math.min(r.width / 2, 30), y: r.top + r.height / 2, label: label?.textContent ?? "" };
  `);
}

const rowShows = (id: number, name: string) => until(`the hierarchy shows #${id} as ${name}`, async () => (await row(id))?.label === name);

/** The inspector's name field: its centre and its text. */
async function nameField(): Promise<{ x: number; y: number; value: string } | null> {
  return js(`
    const el = document.querySelector(".insp-name input");
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2, value: el.value };
  `);
}

const toasts = () => js<string[]>(`return [...document.querySelectorAll(".toast-error")].map((t) => t.innerText.replace(/\\s+/g, " ").trim());`);

async function shot(name: string) {
  const r = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
  console.log(`  ${name}.png`);
}

const ENTITY = 4; // Crate1 in samples/sailing
try {
  await cdp.send("Runtime.enable");
  await cdp.send("Page.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
  const target = new URL(URL_);
  target.searchParams.set("viewport", "fallback");
  await cdp.send("Page.navigate", { url: target.toString() });
  await cdp.once("Page.loadEventFired");
  const was = await hostName(ENTITY);
  check(was === "Crate1", `the host calls #${ENTITY} ${was}`);
  check(await rowShows(ENTITY, "Crate1"), "the editor connected: the hierarchy lists Crate1");

  // 1. F2 in the hierarchy, a new name, Enter.
  let at = (await row(ENTITY))!;
  await click(at.x, at.y);
  await key("F2", "F2", 113);
  check(await until("the rename field", () => js<boolean>(`return document.activeElement?.classList.contains("tree-rename") ?? false;`)), "F2 opens the row's rename field");
  await type("CrateA");
  await key("Enter", "Enter", 13);
  check(await until("the host renames", async () => (await hostName(ENTITY)) === "CrateA"), "F2 rename: the host calls it CrateA");
  check(await rowShows(ENTITY, "CrateA"), "F2 rename: the hierarchy shows CrateA");
  const h = await call<History>("history.list");
  const entry = h.entries.undo.at(-1);
  check(entry?.label === "Rename Crate1 → CrateA" && entry.source === "editor", `one undo entry: ${JSON.stringify(entry)}`);
  check((await toasts()).length === 0, "no error toast");
  await shot("rename-host-1-renamed");

  // 2. Undo and redo with the keyboard.
  at = (await row(ENTITY))!;
  await click(at.x, at.y);
  await key("z", "KeyZ", 90, MOD);
  check(await until("the undo", async () => (await hostName(ENTITY)) === "Crate1"), "Mod+Z: the host calls it Crate1 again");
  check(await rowShows(ENTITY, "Crate1"), "Mod+Z: the hierarchy shows Crate1");
  await key("z", "KeyZ", 90, MOD | SHIFT);
  check(await until("the redo", async () => (await hostName(ENTITY)) === "CrateA"), "Mod+Shift+Z: the host calls it CrateA");
  check(await rowShows(ENTITY, "CrateA"), "Mod+Shift+Z: the hierarchy shows CrateA");

  // 3. The inspector's name field: select its text, type, Enter.
  check(await until("the inspector shows the entity", async () => (await nameField())?.value === "CrateA"), "the inspector's name field reads CrateA");
  let field = (await nameField())!;
  await click(field.x, field.y, 3);
  await type("CrateB");
  await key("Enter", "Enter", 13);
  check(await until("the host renames", async () => (await hostName(ENTITY)) === "CrateB"), "name field: the host calls it CrateB");
  check(await rowShows(ENTITY, "CrateB"), "name field: the hierarchy shows CrateB");

  // 4. 22 CJK characters fit the field's 64 characters and are 66 bytes: the host refuses.
  const long = "界".repeat(22);
  field = (await nameField())!;
  await click(field.x, field.y, 3);
  await type(long);
  check((await nameField())?.value === long, "the name field takes 22 CJK characters");
  await key("Enter", "Enter", 13);
  check(await until("the refusal's toast", async () => (await toasts()).some((t) => t.includes("sim.name_too_long"))), `a name of 66 bytes: the editor shows the host's refusal (${(await toasts()).join(" | ")})`);
  check((await hostName(ENTITY)) === "CrateB", "a name of 66 bytes: the host still calls it CrateB");
  check(await until("the field shows the name again", async () => (await nameField())?.value === "CrateB"), "a name of 66 bytes: the name field reads CrateB again");
  await shot("rename-host-2-refused");
  const undo = (await call<History>("history.list")).undo;
  check(undo.at(-1) === "Rename CrateA → CrateB" && undo.filter((l) => l.startsWith("Rename")).length === 2, `the history holds the two renames: ${JSON.stringify(undo)}`);
} catch (e) {
  check(false, `the driver failed: ${e instanceof Error ? e.message : String(e)}`);
} finally {
  cdp.close();
  await closeChrome();
}
console.log(`\n${failures.length} failures`);
process.exit(failures.length ? 1 : 0);
