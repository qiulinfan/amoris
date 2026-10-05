// VS Code's own JavaScript debugger against the game (docs/spec/debugger.md 11): js-debug as VS
// Code ships it (Visual Studio Code.app's extensions/ms-vscode.js-debug, version in its
// package.json), run headless. VS Code's installation has no standalone DAP server, so this loads
// the extension's bundle under a small stand-in for the `vscode` module, takes the debug adapter
// factory it registers, resolves editors/vscode/launch.json's attach configuration through the
// configuration provider it registers, and speaks the Debug Adapter Protocol to the adapter over
// the named pipe the factory opens, as VS Code does: the root session, then the child session
// js-debug starts for the target. It checks a breakpoint set in samples/sailing/scripts/rules.ts
// (by path, as the editor sets it), the stop, the stack's TypeScript source and line, the
// variables, an evaluation, a step over, and the disconnect.
//
//   node crates/pocket-debug/tests/jsdebug_dap.mjs [--exe PATH] [--port 9241] [--vscode APP]
//        [--log FILE] [--game-log FILE]

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";
import net from "node:net";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : dflt;
};
const target = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const EXE = arg("--exe", join(target, "debug/examples/debug_sailing"));
const PORT = Number(arg("--port", "9241"));
const APP = arg("--vscode", "/Applications/Visual Studio Code.app");
const LOG = arg("--log", null);
// The game's stderr (with POCKET_CDP_LOG=1 in the environment, every CDP message).
const GAME_LOG = arg("--game-log", null);
const EXT = join(APP, "Contents/Resources/app/extensions/ms-vscode.js-debug");
const RULES = join(root, "samples/sailing/scripts/rules.ts");
const TS = readFileSync(RULES, "utf8").split("\n");
const line1 = (text) => TS.findIndex((l) => l.includes(text)) + 1;
const transcript = [];
const log = (...a) => transcript.push(a.map((x) => (typeof x === "string" ? x : JSON.stringify(x))).join(" "));

// ---- a stand-in for the `vscode` module: what js-debug's activation and adapter touch ----
function anything(name) {
  const f = function () {};
  return new Proxy(f, {
    get(_t, prop) {
      if (prop === "then") return undefined;
      if (prop === Symbol.toPrimitive) return () => "";
      if (prop === Symbol.iterator) return function* () {};
      if (prop === "dispose") return () => {};
      return anything(`${name}.${String(prop)}`);
    },
    apply() {
      return anything(`${name}()`);
    },
    construct() {
      return anything(`new ${name}`);
    },
  });
}
class EventEmitter {
  constructor() {
    this.listeners = new Set();
    this.event = (fn) => {
      this.listeners.add(fn);
      return { dispose: () => this.listeners.delete(fn) };
    };
  }
  fire(v) {
    for (const l of [...this.listeners]) l(v);
  }
  dispose() {
    this.listeners.clear();
  }
}
const noEvent = () => ({ dispose() {} });
class Uri {
  constructor(scheme, path) {
    this.scheme = scheme;
    this.path = path;
    this.fsPath = path;
    this.authority = "";
    this.query = "";
    this.fragment = "";
  }
  static file(p) {
    return new Uri("file", p);
  }
  static parse(s) {
    const u = new URL(s);
    return new Uri(u.protocol.replace(":", ""), decodeURIComponent(u.pathname));
  }
  static joinPath(base, ...parts) {
    return new Uri(base.scheme, join(base.path, ...parts));
  }
  with(change) {
    return new Uri(change.scheme ?? this.scheme, change.path ?? this.path);
  }
  toString() {
    return this.scheme === "file" ? `file://${this.path}` : `${this.scheme}:${this.path}`;
  }
  toJSON() {
    return { scheme: this.scheme, path: this.path, fsPath: this.fsPath };
  }
}
const descriptors = [];
const providers = new Map();
const childLaunches = [];
const folder = { uri: Uri.file(root), name: "amoris", index: 0 };
const configuration = (section) => ({
  get: (_key, dflt) => dflt,
  has: () => false,
  inspect: () => undefined,
  update: async () => {},
});
const memento = () => {
  const m = new Map();
  return { get: (k, d) => (m.has(k) ? m.get(k) : d), update: async (k, v) => m.set(k, v), keys: () => [...m.keys()], setKeysForSync() {} };
};
let factory = null;
const vscode = new Proxy(
  {
    version: "1.112.0",
    EventEmitter,
    Uri,
    Disposable: class {
      constructor(fn) {
        this.fn = fn;
      }
      static from(...d) {
        return { dispose: () => d.forEach((x) => x?.dispose?.()) };
      }
      dispose() {
        this.fn?.();
      }
    },
    CancellationError: class extends Error {},
    ExtensionKind: { UI: 1, Workspace: 2 },
    DebugConsoleMode: { Separate: 0, MergeWithParent: 1 },
    DebugConfigurationProviderTriggerKind: { Initial: 1, Dynamic: 2 },
    ConfigurationTarget: { Global: 1, Workspace: 2, WorkspaceFolder: 3 },
    UIKind: { Desktop: 1, Web: 2 },
    DebugAdapterNamedPipeServer: class {
      constructor(path) {
        this.path = path;
      }
    },
    DebugAdapterServer: class {
      constructor(port, host) {
        this.port = port;
        this.host = host;
      }
    },
    l10n: { t: (m, ...a) => (typeof m === "string" ? m.replace(/\{(\d+)\}/g, (_, i) => String(a[i])) : m.message), bundle: undefined },
    env: { machineId: "pocket-test", sessionId: "pocket-test", language: "en", appName: "Visual Studio Code", appRoot: join(APP, "Contents/Resources/app"), uiKind: 1, remoteName: undefined, isTelemetryEnabled: false, onDidChangeTelemetryEnabled: noEvent, uriScheme: "vscode", openExternal: async () => true, asExternalUri: async (u) => u, clipboard: { writeText: async () => {} } },
    extensions: { getExtension: () => undefined, all: [], onDidChange: noEvent },
    workspace: {
      getConfiguration: configuration,
      workspaceFolders: [folder],
      getWorkspaceFolder: () => folder,
      onDidChangeConfiguration: noEvent,
      onDidChangeWorkspaceFolders: noEvent,
      onDidGrantWorkspaceTrust: noEvent,
      isTrusted: true,
      fs: { readFile: async (u) => readFileSync(u.fsPath), stat: async () => ({}) },
      registerFileSystemProvider: () => ({ dispose() {} }),
      registerTextDocumentContentProvider: () => ({ dispose() {} }),
      textDocuments: [],
      onDidOpenTextDocument: noEvent,
      onDidCloseTextDocument: noEvent,
      onDidChangeTextDocument: noEvent,
      onDidSaveTextDocument: noEvent,
      createFileSystemWatcher: () => ({ onDidChange: noEvent, onDidCreate: noEvent, onDidDelete: noEvent, dispose() {} }),
    },
    debug: {
      registerDebugAdapterDescriptorFactory: (type, f) => {
        factory = f;
        descriptors.push(type);
        return { dispose() {} };
      },
      registerDebugConfigurationProvider: (type, p) => {
        if (!providers.has(type)) providers.set(type, []);
        providers.get(type).push(p);
        return { dispose() {} };
      },
      registerDebugAdapterTrackerFactory: () => ({ dispose() {} }),
      startDebugging: async (_folder, config, options) => {
        log("startDebugging", config);
        childLaunches.push(config);
        return true;
      },
      onDidStartDebugSession: noEvent,
      onDidTerminateDebugSession: noEvent,
      onDidChangeActiveDebugSession: noEvent,
      onDidReceiveDebugSessionCustomEvent: noEvent,
      onDidChangeBreakpoints: noEvent,
      activeDebugSession: undefined,
      breakpoints: [],
    },
    commands: { registerCommand: () => ({ dispose() {} }), executeCommand: async () => undefined },
    window: anything("window"),
    languages: anything("languages"),
    ...Object.fromEntries(
      ["MarkdownString", "Position", "Range", "RelativePattern", "Selection", "SnippetString",
       "SourceBreakpoint", "TerminalProfile", "ThemeColor", "ThemeIcon", "TreeItem", "WorkspaceEdit"]
        .map((n) => [n, class { constructor(...a) { this.args = a; } }]),
    ),
    FileSystemError: class extends Error {
      static FileNotFound(m) { return new this(String(m)); }
      static FileExists(m) { return new this(String(m)); }
      static NoPermissions(m) { return new this(String(m)); }
    },
    ...Object.fromEntries(
      ["CompletionItemKind", "FileChangeType", "FileType", "PortAutoForwardAction", "ProgressLocation",
       "QuickPickItemKind", "StatusBarAlignment", "TreeItemCheckboxState", "TreeItemCollapsibleState", "ViewColumn"]
        .map((n) => [n, new Proxy({}, { get: (_t, k) => (typeof k === "string" ? k : undefined) })]),
    ),
    tasks: anything("tasks"),
    tests: anything("tests"),
  },
  {
    get(t, prop) {
      if (prop in t) return t[prop];
      return anything(`vscode.${String(prop)}`);
    },
  },
);

// ---- the Debug Adapter Protocol over a pipe ----
class Dap {
  constructor(name, path) {
    this.name = name;
    this.seq = 0;
    this.pending = new Map();
    this.events = [];
    this.waiters = [];
    this.requests = [];
    this.buf = Buffer.alloc(0);
    this.sock = net.connect(path);
    this.sock.on("data", (d) => this.data(d));
  }
  ready() {
    return new Promise((ok, fail) => {
      this.sock.once("connect", ok);
      this.sock.once("error", fail);
    });
  }
  data(d) {
    this.buf = Buffer.concat([this.buf, d]);
    for (;;) {
      const head = this.buf.indexOf("\r\n\r\n");
      if (head < 0) return;
      const len = Number(/Content-Length: (\d+)/i.exec(this.buf.slice(0, head).toString())[1]);
      if (this.buf.length < head + 4 + len) return;
      const msg = JSON.parse(this.buf.slice(head + 4, head + 4 + len).toString());
      this.buf = this.buf.slice(head + 4 + len);
      log(`${this.name} <-`, msg);
      if (msg.type === "response" && this.pending.has(msg.request_seq)) {
        const { ok, fail } = this.pending.get(msg.request_seq);
        this.pending.delete(msg.request_seq);
        msg.success ? ok(msg.body ?? {}) : fail(new Error(`${msg.command}: ${msg.message} ${JSON.stringify(msg.body ?? {})}`));
      } else if (msg.type === "event") {
        this.events.push(msg);
        this.flush();
      } else if (msg.type === "request") {
        // Reverse requests (runInTerminal, startDebugging): answered as VS Code would.
        this.requests.push(msg);
        this.reply(msg);
      }
    }
  }
  reply(req) {
    const body = {};
    this.write({ type: "response", seq: ++this.seq, request_seq: req.seq, command: req.command, success: true, body });
    if (req.command === "startDebugging") childLaunches.push(req.arguments.configuration);
  }
  write(msg) {
    log(`${this.name} ->`, msg);
    const s = JSON.stringify(msg);
    this.sock.write(`Content-Length: ${Buffer.byteLength(s)}\r\n\r\n${s}`);
  }
  request(command, args = {}) {
    const seq = ++this.seq;
    this.write({ type: "request", seq, command, arguments: args });
    return new Promise((ok, fail) => this.pending.set(seq, { ok, fail }));
  }
  flush() {
    for (const w of [...this.waiters]) {
      const i = this.events.findIndex((e) => e.event === w.event && w.pred(e.body ?? {}));
      if (i >= 0) {
        const [e] = this.events.splice(i, 1);
        this.waiters.splice(this.waiters.indexOf(w), 1);
        clearTimeout(w.timer);
        w.ok(e.body ?? {});
      }
    }
  }
  event(event, pred = () => true, ms = 30000) {
    return new Promise((ok, fail) => {
      const w = { event, pred, ok };
      w.timer = setTimeout(() => fail(new Error(`${this.name}: no ${event} within ${ms} ms`)), ms);
      this.waiters.push(w);
      this.flush();
    });
  }
}

const checks = [];
function check(name, ok, detail = "") {
  checks.push([name, !!ok]);
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail !== "" ? ": " + JSON.stringify(detail) : ""}`);
}

async function session(name, config) {
  const s = { id: `${name}-${Date.now()}`, type: config.type, name: config.name ?? name, configuration: config, workspaceFolder: folder, customRequest: async () => ({}) };
  const d = await factory.createDebugAdapterDescriptor(s);
  const dap = new Dap(name, d.path);
  await dap.ready();
  await dap.request("initialize", { clientID: "vscode", clientName: "Visual Studio Code", adapterID: config.type, pathFormat: "path", linesStartAt1: true, columnsStartAt1: true, supportsVariableType: true, supportsVariablePaging: true, supportsRunInTerminalRequest: true, locale: "en", supportsProgressReporting: true, supportsInvalidatedEvent: true, supportsMemoryReferences: true, supportsArgsCanBeInterpretedByShell: true, supportsStartDebuggingRequest: true });
  return dap;
}

async function main() {
  const pkg = JSON.parse(readFileSync(join(EXT, "package.json"), "utf8"));
  const require = createRequire(import.meta.url);
  const Module = require("node:module");
  const load = Module._load;
  Module._load = function (request, ...rest) {
    if (request === "vscode") return vscode;
    return load.call(this, request, ...rest);
  };
  const ext = require(join(EXT, "src/extension.js"));
  const storage = mkdtempSync(join(tmpdir(), "pocket-jsdebug-"));
  const context = {
    subscriptions: [],
    extensionPath: EXT,
    extensionUri: Uri.file(EXT),
    storagePath: storage,
    storageUri: Uri.file(storage),
    globalStoragePath: storage,
    globalStorageUri: Uri.file(storage),
    logPath: storage,
    logUri: Uri.file(storage),
    globalState: memento(),
    workspaceState: memento(),
    secrets: { get: async () => undefined, store: async () => {}, onDidChange: noEvent },
    environmentVariableCollection: { replace() {}, append() {}, prepend() {}, get() {}, forEach() {}, delete() {}, clear() {}, persistent: false, description: "", getScoped: () => ({ replace() {}, clear() {} }) },
    extension: { id: "ms-vscode.js-debug", packageJSON: pkg, extensionPath: EXT, extensionUri: Uri.file(EXT) },
    extensionMode: 1,
    asAbsolutePath: (p) => join(EXT, p),
  };
  ext.activate(context);
  check(`js-debug ${pkg.version} from VS Code activates and registers its adapter factory`, factory && descriptors.includes("pwa-node"), descriptors);

  // editors/vscode/launch.json's configuration, resolved as VS Code resolves it.
  const launch = JSON.parse(readFileSync(join(root, "editors/vscode/launch.json"), "utf8").replace(/^\s*\/\/.*$/gm, ""));
  let config = JSON.parse(JSON.stringify(launch.configurations[0]).replaceAll("${workspaceFolder}", root));
  config.port = PORT;
  for (const p of providers.get(config.type) ?? []) {
    if (p.resolveDebugConfiguration) config = (await p.resolveDebugConfiguration(folder, config)) ?? config;
  }
  for (const p of providers.get(config.type) ?? []) {
    if (p.resolveDebugConfigurationWithSubstitutedVariables) config = (await p.resolveDebugConfigurationWithSubstitutedVariables(folder, config)) ?? config;
  }
  log("resolved", config);

  const game = spawn(EXE, ["--port", String(PORT), "--wait"], { stdio: ["ignore", "pipe", "pipe"] });
  const gameErr = [];
  game.stderr.on("data", (d) => gameErr.push(String(d)));
  const lines = createInterface({ input: game.stdout });
  await new Promise((ok) => lines.once("line", ok));
  try {
    const rootDap = await session("root", config);
    await rootDap.request("attach", config);
    await rootDap.event("initialized");
    await rootDap.request("configurationDone");
    // js-debug starts a child session for the target it found.
    const deadline = Date.now() + 15000;
    while (!childLaunches.length && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
    check("the root session finds the pocket-game target and starts a child session", childLaunches.length > 0, childLaunches[0]?.__pendingTargetId);
    let dap = rootDap;
    if (childLaunches.length) {
      dap = await session("child", childLaunches[0]);
      dap.request("attach", childLaunches[0]).catch((e) => log("child attach", String(e)));
      await dap.event("initialized");
    }
    const bpLine = line1("const speed = Math.abs(b.speed[r]);");
    const set = await dap.request("setBreakpoints", { source: { path: RULES, name: "rules.ts" }, breakpoints: [{ line: bpLine }], lines: [bpLine], sourceModified: false });
    log("setBreakpoints", set);
    await dap.request("configurationDone");
    const verified = set.breakpoints?.[0]?.verified || (await dap.event("breakpoint", (b) => b.breakpoint?.verified, 15000).then(() => true, () => false));
    check("the breakpoint set by the TypeScript file's path verifies", verified, set.breakpoints);
    const stopped = await dap.event("stopped", () => true, 30000);
    check("stopped on the breakpoint", stopped.reason === "breakpoint", stopped);
    const st = await dap.request("stackTrace", { threadId: stopped.threadId, startFrame: 0, levels: 20 });
    const top = st.stackFrames[0];
    check("the top frame is rules.ts on disk at the breakpoint's TypeScript line",
      top.source?.path === RULES && top.line === bpLine, { path: top.source?.path, line: top.line, name: top.name });
    const run = st.stackFrames.find((f) => f.name.includes("run"));
    check("a caller frame is the system's run in rules.ts at `boats.each`",
      run && run.source?.path === RULES && run.line === line1("boats.each((r, e) => {"), run && { name: run.name, line: run.line });
    const scopes = await dap.request("scopes", { frameId: top.id });
    const localScope = scopes.scopes.find((s) => /local/i.test(s.name));
    const vars = (await dap.request("variables", { variablesReference: localScope.variablesReference })).variables;
    const v = Object.fromEntries(vars.map((x) => [x.name, x.value]));
    check("Variables: Local shows r = 0 and e", v.r === "0" && /^\d+$/.test(v.e ?? ""), v);
    const closureScope = scopes.scopes.find((s) => /closure/i.test(s.name));
    const cvars = closureScope ? (await dap.request("variables", { variablesReference: closureScope.variablesReference })).variables.map((x) => x.name) : [];
    check("Variables: Closure shows b, l and ctx", ["b", "l", "ctx"].every((n) => cvars.includes(n)), cvars);
    const ctxVar = closureScope && (await dap.request("variables", { variablesReference: closureScope.variablesReference })).variables.find((x) => x.name === "ctx");
    const ctxKids = ctxVar?.variablesReference ? (await dap.request("variables", { variablesReference: ctxVar.variablesReference })).variables.map((x) => x.name) : [];
    check("Variables: ctx expands to its fields", ["tick", "dt", "system"].every((n) => ctxKids.includes(n)), ctxKids);
    const ev = await dap.request("evaluate", { expression: "ctx.system + ' r=' + r", frameId: top.id, context: "repl" });
    check("Debug Console evaluation in the frame", ev.result === "'log r=0'" || ev.result === "log r=0", ev.result);
    const hover = await dap.request("evaluate", { expression: "b.speed", frameId: top.id, context: "hover" });
    check("hover over a column shows a Float64Array", /Float64Array/.test(hover.result), hover.result);
    await dap.request("next", { threadId: stopped.threadId });
    const stepped = await dap.event("stopped", () => true, 15000);
    const st2 = await dap.request("stackTrace", { threadId: stepped.threadId, startFrame: 0, levels: 1 });
    check("step over: the next TypeScript line", stepped.reason === "step" && st2.stackFrames[0].line === line1("l.distance[r] = l.distance[r]"), { reason: stepped.reason, line: st2.stackFrames[0].line });
    await dap.request("setBreakpoints", { source: { path: RULES, name: "rules.ts" }, breakpoints: [], lines: [] });
    await dap.request("continue", { threadId: stepped.threadId });
    await dap.event("continued", () => true, 5000).catch(() => null);
    await new Promise((r) => setTimeout(r, 500));
    await dap.request("disconnect", { restart: false }).catch(() => null);
    await rootDap.request("disconnect", { restart: false }).catch(() => null);
    const status = [];
    lines.on("line", (l) => status.push(JSON.parse(l)));
    await new Promise((r) => setTimeout(r, 2200));
    check("after disconnect the game runs on, not paused", status.length && !status.at(-1).paused_in_debugger, status.at(-1));
  } finally {
    game.kill();
    if (LOG) writeFileSync(LOG, transcript.join("\n") + "\n");
    if (GAME_LOG) writeFileSync(GAME_LOG, gameErr.join(""));
  }
  const failed = checks.filter(([, ok]) => !ok);
  console.log(`${checks.length - failed.length}/${checks.length} checks passed`);
  process.exit(failed.length ? 1 : 0);
}

main().catch((e) => {
  console.error(e);
  if (LOG) writeFileSync(LOG, transcript.join("\n") + "\n");
  process.exit(2);
});
