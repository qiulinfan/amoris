// The debugger's end-to-end check over the Chrome DevTools Protocol (docs/spec/debugger.md 11):
// this script is the CDP client, with no UI, doing what a client that maps TypeScript does. It
// starts `debug_sailing --wait`, discovers the target as js-debug does (/json/version, /json/list),
// reads each script's source map from its data URL, maps a TypeScript line of
// samples/sailing/scripts/rules.ts to the JavaScript and sets a breakpoint there, then checks the
// pause (file, TypeScript line, caller frame), the locals, an evaluation on the frame, a step over,
// a conditional breakpoint, a logpoint (console), Debugger.pause and the resume.
//
//   node crates/pocket-debug/tests/cdp_e2e.mjs [--exe PATH] [--port 9239] [--log FILE]
//
// Node 24 (global WebSocket and fetch). Exit 0 when every check holds.

import { spawn } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
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
const PORT = Number(arg("--port", "9239"));
const LOG = arg("--log", null);
const RULES = join(root, "samples/sailing/scripts/rules.ts");
const TS = readFileSync(RULES, "utf8").split("\n");
const transcript = [];

/** 0-based TypeScript line holding `text`. */
const tsLine = (text) => {
  const i = TS.findIndex((l) => l.includes(text));
  if (i < 0) throw new Error(`no line with ${text}`);
  return i;
};

// ---- source maps, as a client reads them ----
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
function decodeMappings(mappings) {
  const out = []; // [genLine, genCol, srcLine, srcCol]
  let sl = 0, sc = 0, si = 0, ni = 0;
  mappings.split(";").forEach((line, gl) => {
    let gc = 0;
    for (const seg of line.split(",")) {
      if (!seg) continue;
      const f = [];
      let v = 0, shift = 0;
      for (const ch of seg) {
        const d = B64.indexOf(ch);
        v += (d & 31) << shift;
        if (d & 32) shift += 5;
        else { f.push(v & 1 ? -(v >> 1) : v >> 1); v = 0; shift = 0; }
      }
      gc += f[0];
      if (f.length >= 4) {
        si += f[1]; sl += f[2]; sc += f[3];
        if (f.length >= 5) ni += f[4];
        out.push([gl, gc, sl, sc]);
      }
    }
  });
  return out;
}
class SourceMap {
  constructor(dataUrl) {
    const b64 = dataUrl.slice(dataUrl.indexOf("base64,") + 7);
    this.raw = JSON.parse(Buffer.from(b64, "base64").toString("utf8"));
    this.segs = decodeMappings(this.raw.mappings);
  }
  /** TypeScript (line, col) of a generated position: the last segment at or before it. */
  original(line, col) {
    let best = null;
    for (const s of this.segs) {
      if (s[0] === line && s[1] <= col) best = s;
      if (s[0] === line && !best) best = s;
    }
    return best ? [best[2], best[3]] : null;
  }
  /** The first generated position of a TypeScript line. */
  generated(tsLine) {
    const hits = this.segs.filter((s) => s[2] === tsLine).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    return hits.length ? [hits[0][0], hits[0][1]] : null;
  }
}

// ---- the CDP client ----
class Client {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.next = 0;
    this.pending = new Map();
    this.events = [];
    this.waiters = [];
    this.ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      transcript.push(["<-", msg]);
      if (msg.id !== undefined && this.pending.has(msg.id)) {
        const { ok, fail, method } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? fail(new Error(`${method}: ${JSON.stringify(msg.error)}`)) : ok(msg.result);
      } else if (msg.method) {
        this.events.push(msg);
        this.flush();
      }
    };
  }
  open() {
    return new Promise((ok, fail) => { this.ws.onopen = ok; this.ws.onerror = fail; });
  }
  call(method, params = {}) {
    const id = ++this.next;
    transcript.push(["->", { id, method, params }]);
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((ok, fail) => this.pending.set(id, { ok, fail, method }));
  }
  flush() {
    for (const w of [...this.waiters]) {
      const i = this.events.findIndex((e) => e.method === w.method && w.pred(e.params));
      if (i >= 0) {
        const [e] = this.events.splice(i, 1);
        this.waiters.splice(this.waiters.indexOf(w), 1);
        clearTimeout(w.timer);
        w.ok(e.params);
      }
    }
  }
  event(method, pred = () => true, ms = 20000) {
    return new Promise((ok, fail) => {
      const w = { method, pred, ok };
      w.timer = setTimeout(() => {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        fail(new Error(`no ${method} within ${ms} ms`));
      }, ms);
      this.waiters.push(w);
      this.flush();
    });
  }
  /** Events seen so far with this method (consumed). */
  take(method) {
    const got = this.events.filter((e) => e.method === method).map((e) => e.params);
    this.events = this.events.filter((e) => e.method !== method);
    return got;
  }
}

const checks = [];
function check(name, ok, detail = "") {
  checks.push([name, !!ok]);
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail !== "" ? ": " + JSON.stringify(detail) : ""}`);
}

async function scope(c, frame, type) {
  const s = frame.scopeChain.find((x) => x.type === type);
  const props = (await c.call("Runtime.getProperties", { objectId: s.object.objectId, ownProperties: true })).result;
  return Object.fromEntries(props.map((p) => [p.name, p.value?.value ?? p.value?.description ?? p.value?.type]));
}

async function main() {
  const game = spawn(EXE, ["--port", String(PORT), "--wait", "--speed", "1"], { stdio: ["ignore", "pipe", "pipe"] });
  const stderr = [];
  game.stderr.on("data", (d) => stderr.push(String(d)));
  const lines = createInterface({ input: game.stdout });
  const first = await new Promise((ok) => lines.once("line", ok));
  const hello = JSON.parse(first);
  try {
    const version = await (await fetch(`http://127.0.0.1:${PORT}/json/version`)).json();
    const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
    check("discovery: /json/version has no webSocketDebuggerUrl, /json/list has the pocket-game target",
      !("webSocketDebuggerUrl" in version) && list[0].id === "pocket-game" && list[0].webSocketDebuggerUrl === hello.ws,
      list[0].webSocketDebuggerUrl);
    const refused = await fetch(`http://127.0.0.1:${PORT}/json/list`, { headers: { Origin: "http://evil.example" } });
    check("a non-loopback Origin is refused", refused.status === 403, refused.status);

    const c = new Client(list[0].webSocketDebuggerUrl);
    await c.open();
    await c.call("Runtime.enable");
    await c.call("Debugger.enable");
    await c.call("Debugger.setPauseOnExceptions", { state: "none" });
    const parsed = c.take("Debugger.scriptParsed");
    const rules = parsed.find((s) => s.url === "pocket:///scripts/rules.js");
    check("scriptParsed announces pocket:///scripts/rules.js with a data-URL source map",
      rules && rules.sourceMapURL.startsWith("data:application/json"), parsed.map((s) => s.url));
    const map = new SourceMap(rules.sourceMapURL);
    check("the source map's source is rules.ts beside it, with the TypeScript embedded",
      map.raw.sources[0] === "rules.ts" && map.raw.sourcesContent[0] === TS.join("\n"), map.raw.sources);
    const js = (await c.call("Debugger.getScriptSource", { scriptId: rules.scriptId })).scriptSource;
    check("getScriptSource returns the JavaScript QuickJS-ng runs", js.includes("system(") && !js.includes(": number"), js.length);

    // A breakpoint on a TypeScript line, set where the source map puts it in the JavaScript.
    const speedTs = tsLine("const speed = Math.abs(b.speed[r]);");
    const [jsLine, jsCol] = map.generated(speedTs);
    const bp1 = await c.call("Debugger.setBreakpointByUrl", { url: rules.url, lineNumber: jsLine, columnNumber: jsCol });
    check("setBreakpointByUrl resolves on the JavaScript line of the TypeScript line",
      bp1.locations.length === 1 && bp1.locations[0].lineNumber === jsLine, bp1.locations);
    const t0 = performance.now();
    await c.call("Runtime.runIfWaitingForDebugger");
    let p = await c.event("Debugger.paused");
    console.log(`     first pause after runIfWaitingForDebugger: ${(performance.now() - t0).toFixed(1)} ms`);
    let top = p.callFrames[0];
    let at = map.original(top.location.lineNumber, top.location.columnNumber);
    check("pause 1: rules.js at the TypeScript line of `const speed`", top.url === rules.url && at[0] === speedTs,
      { ts_line: at[0] + 1, want: speedTs + 1 });
    check("pause 1: hitBreakpoints names the breakpoint", p.hitBreakpoints.includes(bp1.breakpointId), p.hitBreakpoints);
    const caller = p.callFrames[1];
    const callerTs = caller && map.original(caller.location.lineNumber, caller.location.columnNumber);
    check("pause 1: the caller frame is the system's run at `boats.each`",
      caller && caller.functionName === "run" && callerTs[0] === tsLine("boats.each((r, e) => {"),
      caller && { fn: caller.functionName, ts_line: callerTs[0] + 1 });
    let local = await scope(c, top, "local");
    check("pause 1: locals r = 0 and e an entity id; speed not yet assigned",
      local.r === 0 && typeof local.e === "number" && local.e > 0 && local.speed === "undefined", local);
    const closure = await scope(c, top, "closure");
    check("pause 1: closure variables b, l and ctx", "b" in closure && "l" in closure && "ctx" in closure, Object.keys(closure));
    let e = await c.call("Debugger.evaluateOnCallFrame", { callFrameId: top.callFrameId, expression: "ctx.system + ':' + typeof b.speed[r]", returnByValue: true });
    check("evaluateOnCallFrame sees ctx (closure) and r (argument)", e.result.value === "log:number", e.result);
    e = await c.call("Debugger.evaluateOnCallFrame", { callFrameId: caller.callFrameId, expression: "boats.len", returnByValue: true });
    check("evaluateOnCallFrame on the caller frame: boats.len = 1", e.result.value === 1, e.result);
    const ctxObj = (await c.call("Runtime.getProperties", { objectId: top.scopeChain.find((s) => s.type === "closure").object.objectId }))
      .result.find((x) => x.name === "ctx").value;
    const ctxProps = (await c.call("Runtime.getProperties", { objectId: ctxObj.objectId, ownProperties: true })).result.map((x) => x.name);
    check("getProperties expands an object (ctx)", ["tick", "dt", "world", "rng"].every((k) => ctxProps.includes(k)), ctxProps);
    const tick1 = (await c.call("Debugger.evaluateOnCallFrame", { callFrameId: top.callFrameId, expression: "ctx.tick", returnByValue: true })).result.value;

    // Step over to the next TypeScript statement.
    const t1 = performance.now();
    await c.call("Debugger.stepOver");
    p = await c.event("Debugger.paused");
    console.log(`     step over (request to paused): ${(performance.now() - t1).toFixed(1)} ms`);
    top = p.callFrames[0];
    at = map.original(top.location.lineNumber, top.location.columnNumber);
    check("step over: the next TypeScript line", p.reason === "step" && at[0] === tsLine("l.distance[r] = l.distance[r]"), { reason: p.reason, ts_line: at[0] + 1 });
    local = await scope(c, top, "local");
    check("step over: speed now holds a number", typeof local.speed === "number", local.speed);

    // A conditional breakpoint: an odd tick.
    await c.call("Debugger.removeBreakpoint", { breakpointId: bp1.breakpointId });
    const setTs = tsLine("const set = b.hoist_now[r] >= 0.5;");
    const [setJs] = map.generated(setTs);
    const bp2 = await c.call("Debugger.setBreakpointByUrl", { url: rules.url, lineNumber: setJs, condition: `ctx.tick > ${tick1 + 3} && ctx.tick % 2 === 1` });
    await c.call("Debugger.resume");
    await c.event("Debugger.resumed");
    p = await c.event("Debugger.paused");
    top = p.callFrames[0];
    const tick2 = (await c.call("Debugger.evaluateOnCallFrame", { callFrameId: top.callFrameId, expression: "ctx.tick", returnByValue: true })).result.value;
    at = map.original(top.location.lineNumber, top.location.columnNumber);
    check("conditional breakpoint: stops at `const set` only on an odd tick after it",
      at[0] === setTs && tick2 > tick1 + 3 && tick2 % 2 === 1, { tick1, tick2, ts_line: at[0] + 1 });

    // A logpoint as DevTools and js-debug set one: a condition that logs and is false.
    await c.call("Debugger.removeBreakpoint", { breakpointId: bp2.breakpointId });
    const bp3 = await c.call("Debugger.setBreakpointByUrl", { url: rules.url, lineNumber: setJs, condition: 'console.log("logpoint tick", ctx.tick), false' });
    c.take("Runtime.consoleAPICalled");
    await c.call("Debugger.resume");
    const log = await c.event("Runtime.consoleAPICalled", (x) => String(x.args[0].value).startsWith("logpoint tick"));
    const logAt = log.stackTrace.callFrames[0];
    check("logpoint: Runtime.consoleAPICalled with its text, located in rules.js, and no pause",
      logAt && logAt.url === rules.url && map.original(logAt.lineNumber, logAt.columnNumber)[0] === setTs && !c.events.some((x) => x.method === "Debugger.paused"),
      log.args[0].value);
    await c.call("Debugger.removeBreakpoint", { breakpointId: bp3.breakpointId });

    // Debugger.pause stops the running game at its next statement.
    await new Promise((r) => setTimeout(r, 300));
    await c.call("Debugger.pause");
    p = await c.event("Debugger.paused");
    check("Debugger.pause stops in a script", p.callFrames.length > 0 && p.callFrames[0].url.startsWith("pocket:///scripts/"), p.callFrames[0]?.url);
    await c.call("Debugger.resume");
    await c.event("Debugger.resumed");

    // Detaching resumes and the game runs on.
    c.ws.close();
    await new Promise((r) => setTimeout(r, 1500));
    const status = [];
    lines.on("line", (l) => status.push(JSON.parse(l)));
    await new Promise((r) => setTimeout(r, 2200));
    const last = status.at(-1);
    check("after the client leaves the game runs on, not paused", last && !last.paused_in_debugger && last.tick > tick2, last);
  } finally {
    game.kill();
    if (LOG) writeFileSync(LOG, transcript.map(([d, m]) => `${d} ${JSON.stringify(m)}`).join("\n") + "\n");
  }
  const failed = checks.filter(([, ok]) => !ok);
  console.log(`${checks.length - failed.length}/${checks.length} checks passed`);
  if (failed.length) console.log(stderr.join(""));
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(2); });
