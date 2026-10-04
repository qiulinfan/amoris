// Chrome DevTools against the game (docs/spec/debugger.md 11): the DevTools frontend that ships in
// Google Chrome, opened in a headless Chrome at http://127.0.0.1:<chrome port>/devtools/js_app.html
// (the page chrome://inspect opens for a Node target) with ws= pointing at debug_sailing's endpoint.
// The frontend connects by itself; this script then works it from inside, through its own
// modules, as a user's clicks would: it waits for rules.ts (from the source map) in the Sources
// workspace, sets a breakpoint on a TypeScript line with DevTools' BreakpointManager, waits for the
// pause, reads the paused frame's TypeScript location and scope through DevTools' bindings, shows
// the Sources panel and takes a screenshot, steps over, and resumes.
//
//   node crates/pocket-debug/tests/devtools_chrome.mjs [--exe PATH] [--port 9243]
//        [--chrome-port 9333] [--chrome APP] [--shots DIR] [--game-log FILE]
//
// Exit 0 when every check holds; screenshots go to --shots (default: docs/evidence/debug).

import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : dflt;
};
const target = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const EXE = arg("--exe", join(target, "debug/examples/debug_sailing"));
const PORT = Number(arg("--port", "9243"));
const CPORT = Number(arg("--chrome-port", "9333"));
const CHROME = arg("--chrome", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
const SHOTS = arg("--shots", join(root, "docs/evidence/debug"));
// The game's stderr (with POCKET_CDP_LOG=1 in the environment, every CDP message).
const GAME_LOG = arg("--game-log", null);
const RULES = join(root, "samples/sailing/scripts/rules.ts");
const TS = readFileSync(RULES, "utf8").split("\n");
const line0 = (text) => TS.findIndex((l) => l.includes(text));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const checks = [];
function check(name, ok, detail = "") {
  checks.push([name, !!ok]);
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail !== "" ? ": " + JSON.stringify(detail) : ""}`);
}

async function until(f, ms = 20000, what = "condition") {
  const deadline = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}

class Page {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.id = 0;
    this.pending = new Map();
    this.ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.id && this.pending.has(msg.id)) {
        this.pending.get(msg.id)(msg);
        this.pending.delete(msg.id);
      }
    };
  }
  open() {
    return new Promise((r) => (this.ws.onopen = r));
  }
  call(method, params = {}) {
    return new Promise((r) => {
      const i = ++this.id;
      this.pending.set(i, r);
      this.ws.send(JSON.stringify({ id: i, method, params }));
    });
  }
  /** Evaluates an async function body in the DevTools page; its value, or throws. */
  async run(body) {
    const r = await this.call("Runtime.evaluate", { expression: `(async () => { ${body} })()`, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description ?? JSON.stringify(r.result.exceptionDetails));
    return r.result?.result?.value;
  }
  async shot(file) {
    const r = await this.call("Page.captureScreenshot", { format: "png" });
    writeFileSync(file, Buffer.from(r.result.data, "base64"));
  }
}

// DevTools' own modules, imported inside its page.
const MODULES = `
  const W = await import('./models/workspace/workspace.js');
  const BP = await import('./models/breakpoints/breakpoints.js');
  const SDK = await import('./core/sdk/sdk.js');
  const B = await import('./models/bindings/bindings.js');
  const Common = await import('./core/common/common.js');
  const UI = await import('./ui/legacy/legacy.js');
  const rules = () => W.Workspace.WorkspaceImpl.instance().uiSourceCodes().find((u) => u.url() === 'pocket:///scripts/rules.ts');
  const model = () => SDK.TargetManager.TargetManager.instance().models(SDK.DebuggerModel.DebuggerModel)[0];
  const where = async (frame) => {
    const ui = await B.DebuggerWorkspaceBinding.DebuggerWorkspaceBinding.instance().rawLocationToUILocation(frame.location());
    return ui && { url: ui.uiSourceCode.url(), line: ui.lineNumber, fn: frame.functionName };
  };
`;

async function main() {
  mkdirSync(SHOTS, { recursive: true });
  const game = spawn(EXE, ["--port", String(PORT), "--wait"], { stdio: ["ignore", "pipe", "pipe"] });
  const gameErr = [];
  game.stderr.on("data", (d) => gameErr.push(String(d)));
  const lines = createInterface({ input: game.stdout });
  await new Promise((ok) => lines.once("line", ok));
  const profile = mkdtempSync(join(tmpdir(), "pocket-devtools-"));
  const chrome = spawn(CHROME, ["--headless=new", `--remote-debugging-port=${CPORT}`, `--user-data-dir=${profile}`,
    "--no-first-run", "--no-default-browser-check", "--window-size=1600,1000", "about:blank"], { stdio: "ignore" });
  try {
    const version = await until(async () => {
      try { return await (await fetch(`http://127.0.0.1:${CPORT}/json/version`)).json(); } catch { return null; }
    }, 20000, "Chrome");
    const browser = new Page(version.webSocketDebuggerUrl);
    await browser.open();
    const url = `http://127.0.0.1:${CPORT}/devtools/js_app.html?experiments=true&v8only=true&ws=127.0.0.1:${PORT}/devtools/game`;
    const created = await browser.call("Target.createTarget", { url });
    if (created.error) throw new Error(`Target.createTarget: ${JSON.stringify(created.error)}`);
    const list = await until(async () => (await (await fetch(`http://127.0.0.1:${CPORT}/json/list`)).json()).find((t) => t.url.includes("js_app.html")), 10000, "the DevTools page");
    const dt = new Page(list.webSocketDebuggerUrl);
    await dt.open();
    await dt.call("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
    console.log(`     ${version.Browser}: ${url}`);

    const sources = await until(() => dt.run(`${MODULES} return rules() ? W.Workspace.WorkspaceImpl.instance().uiSourceCodes().map((u) => u.url()) : null;`), 20000, "rules.ts in the workspace");
    check("DevTools' workspace has the TypeScript from the source maps", ["main.ts", "rules.ts", "components.ts"].every((f) => sources.includes(`pocket:///scripts/${f}`)), sources.filter((s) => s.endsWith(".ts")));
    const content = await dt.run(`${MODULES} return (await rules().requestContentData()).text;`);
    check("rules.ts's content in DevTools is the TypeScript file", content === TS.join("\n"), content?.length);

    const bpLine = line0("const speed = Math.abs(b.speed[r]);");
    await dt.run(`${MODULES}
      await BP.BreakpointManager.BreakpointManager.instance().setBreakpoint(rules(), ${bpLine}, undefined,
        BP.BreakpointManager.EMPTY_BREAKPOINT_CONDITION, true, false, BP.BreakpointManager.BreakpointOrigin.USER_ACTION);
      return true;`);
    const paused = await until(() => dt.run(`${MODULES} const m = model(); if (!m || !m.isPaused()) return null;
      const d = m.debuggerPausedDetails(); return { frames: await Promise.all(d.callFrames.map(where)), reason: d.reason };`), 20000, "the pause");
    check("DevTools pauses at the breakpoint's TypeScript line in rules.ts",
      paused.frames[0].url === "pocket:///scripts/rules.ts" && paused.frames[0].line === bpLine, paused.frames[0]);
    check("DevTools' call stack: the callback, then run at boats.each",
      paused.frames.some((f) => f.fn === "run" && f.url.endsWith("rules.ts") && f.line === line0("boats.each((r, e) => {")), paused.frames);
    const scope = await dt.run(`${MODULES} const f = model().debuggerPausedDetails().callFrames[0];
      const local = f.scopeChain()[0]; const props = await local.object().getAllProperties(false, false);
      return { type: local.type(), names: props.properties.map((p) => p.name + '=' + (p.value?.description ?? '')) };`);
    check("DevTools' Scope pane: Local has r = 0 and e", scope.names.includes("r=0") && scope.names.some((n) => n.startsWith("e=")), scope);
    const value = await dt.run(`${MODULES} const r = await model().debuggerPausedDetails().callFrames[0].evaluate({ expression: 'ctx.system + ":" + r', objectGroup: 'console', includeCommandLineAPI: true, silent: false, returnByValue: true, generatePreview: false });
      return r.object?.value;`);
    check("DevTools' console evaluates on the paused frame", value === "log:0", value);

    // The Sources panel, showing the pause in the TypeScript.
    await dt.run(`${MODULES}
      await UI.ViewManager.ViewManager.instance().showView('sources');
      const f = model().debuggerPausedDetails().callFrames[0];
      const ui = await B.DebuggerWorkspaceBinding.DebuggerWorkspaceBinding.instance().rawLocationToUILocation(f.location());
      await Common.Revealer.reveal(ui);
      return true;`);
    await sleep(1500);
    await dt.shot(join(SHOTS, "devtools-paused.png"));

    await dt.run(`${MODULES} model().stepOver(); return true;`);
    const stepped = await until(() => dt.run(`${MODULES} const m = model(); if (!m.isPaused()) return null;
      const w = await where(m.debuggerPausedDetails().callFrames[0]); return w.line === ${bpLine} ? null : w;`), 10000, "the step");
    check("DevTools step over: the next TypeScript line", stepped.url.endsWith("rules.ts") && stepped.line === line0("l.distance[r] = l.distance[r]"), stepped);
    await sleep(800);
    await dt.shot(join(SHOTS, "devtools-stepped.png"));

    await dt.run(`${MODULES}
      for (const b of BP.BreakpointManager.BreakpointManager.instance().allBreakpointLocations()) await b.breakpoint.remove(false);
      model().resume(); return true;`);
    await until(() => dt.run(`${MODULES} return !model().isPaused();`), 10000, "the resume");
    const status = [];
    lines.on("line", (l) => status.push(JSON.parse(l)));
    await sleep(2200);
    check("after DevTools resumes the game runs on", status.length && !status.at(-1).paused_in_debugger, status.at(-1));
    dt.ws.close();
    browser.ws.close();
  } finally {
    chrome.kill();
    game.kill();
    if (GAME_LOG) writeFileSync(GAME_LOG, gameErr.join(""));
  }
  const failed = checks.filter(([, ok]) => !ok);
  console.log(`${checks.length - failed.length}/${checks.length} checks passed`);
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
