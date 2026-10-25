// The deterministic water kernels in Chrome (water.md 66): steps the samples' GPU water in the
// shipped game worker, tick by tick, and compares each region's field digest after every tick with
// crates/amoris-app/tests/golden/water-digests.txt, which the native test records and checks on
// every native backend. node tools/water/water_determinism_check.mjs [--adapter swiftshader]
// [--samples a,b] [--ticks N]. Build first: tools/build_web.sh with the projects the record names
// (samples/pool-party, river-falls, surf-beach, water-valley and water-storm;
// tests/fixtures/scenes/water-resident, water-shore, water-floaters and water-feedback). `--adapter swiftshader` runs WebGPU on SwiftShader, Chrome's CPU implementation:
// another device entirely. Prints one JSON line {ok, adapter, samples: {name: {ticks, regions,
// first_divergence}}, errors}; exit code 0 when every digest matched. Chrome is found as
// tools/web_bench.mjs finds it (CHROME= overrides; CHROME_FLAGS= adds switches).
import { spawn } from "node:child_process";
import { createReadStream, existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromeForTesting } from "../chrome_for_testing.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : fallback;
};
const adapter = opt("--adapter", "default");
const only = opt("--samples", null)?.split(",");
const tickLimit = Number(opt("--ticks", "0"));

// The record: per sample, its regions' digests after each tick.
const record = new Map();
for (const line of readFileSync(join(root, "crates/amoris-app/tests/golden/water-digests.txt"), "utf8").split("\n")) {
  if (!line || line.startsWith("#")) continue;
  const [head, digests] = line.split(":");
  const [sample, region, ticks] = head.split(" ");
  if (only && !only.includes(sample)) continue;
  if (!record.has(sample)) record.set(sample, { ticks: Number(ticks), regions: [] });
  record.get(sample).regions.push({ id: Number(region), digests: digests.trim().split(" ") });
}

const web = join(root, "web");
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json" };
const server = createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
  if (path === "determinism.html") {
    res.writeHead(200, { "content-type": "text/html" });
    res.end("<!doctype html><meta charset=utf-8><title>water determinism</title>");
    return;
  }
  const file = join(web, path);
  if (!file.startsWith(web) || !existsSync(file) || statSync(file).isDirectory()) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const url = `http://127.0.0.1:${server.address().port}/determinism.html`;

const candidates = [...chromeForTesting(), ...({
  darwin: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
  win32: [
    join(process.env.PROGRAMFILES || "C:\\Program Files", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env["PROGRAMFILES(X86)"] || "C:\\Program Files (x86)", "Google\\Chrome\\Application\\chrome.exe"),
    join(process.env.LOCALAPPDATA || "", "Google\\Chrome\\Application\\chrome.exe"),
  ],
  linux: ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"],
}[process.platform] || [])];
const chrome = process.env.CHROME || candidates.find((p) => existsSync(p));
if (!chrome) throw new Error(`no Chrome found (tried ${candidates.join(", ")}); set CHROME=<path>`);
const profile = mkdtempSync(join(tmpdir(), "amoris-determinism-"));
const port = 9300 + Math.floor(Math.random() * 600);
const proc = spawn(chrome, [
  "--headless=new", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  "--enable-unsafe-webgpu", "--no-first-run", "--no-default-browser-check",
  ...(process.platform === "win32" ? [] : ["--enable-features=Vulkan"]),
  // SwiftShader's JIT runs only with the GPU process unsandboxed (Chrome 155 on macOS: without
  // these switches its first dispatch kills the GPU process). A throwaway profile, for this check.
  ...(adapter === "swiftshader"
    ? ["--use-webgpu-adapter=swiftshader", "--enable-unsafe-swiftshader", "--disable-gpu-sandbox"]
    : []),
  ...(process.env.CHROME_FLAGS || "").split(" ").filter(Boolean),
  "about:blank",
], { stdio: process.env.CHROME_LOG ? ["ignore", "ignore", "pipe"] : "ignore" });
if (process.env.CHROME_LOG) {
  const { createWriteStream } = await import("node:fs");
  proc.stderr.pipe(createWriteStream(process.env.CHROME_LOG));
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try {
    target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page");
  } catch {}
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
const errors = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); return; }
  if (m.method === "Runtime.consoleAPICalled") {
    const text = m.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
    if ((m.params.type === "error" || /Validation|panicked/.test(text)) && errors.length < 20) errors.push(text.slice(0, 600));
  }
  if (m.method === "Runtime.exceptionThrown" && errors.length < 20) {
    errors.push("[exception] " + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
  }
};
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text);
  return r.result.result.value;
};
await send("Runtime.enable");
await send("Page.navigate", { url });
await sleep(500);

const result = { ok: true, adapter: null, samples: {}, errors };
// The record's samples are games in samples/ or scenes in tests/fixtures/scenes/; a package's
// slug is its project's path (tools/build_web.sh).
const FIXTURES = new Set(["water-resident", "water-shore", "water-floaters", "water-feedback"]);
const slug = (sample) => (FIXTURES.has(sample) ? "tests-fixtures-scenes-" : "samples-") + sample;
try {
  result.adapter = await evaluate(`(async () => {
    const a = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
    return a ? { vendor: a.info.vendor, architecture: a.info.architecture, fallback: a.info.isFallbackAdapter } : null;
  })()`);
  for (const [sample, { ticks: recorded, regions }] of record) {
    const ticks = tickLimit > 0 ? Math.min(tickLimit, recorded) : recorded;
    const digests = await evaluate(`(async () => {
      const a = await import("/amoris.js");
      const { module } = await a.loadModule();
      const pkg = new Uint8Array(await (await fetch("/projects/${slug(sample)}/package.json")).arrayBuffer());
      const g = a.start(module, pkg, { pacing: "stepped", seed: 1, waterTimeoutMs: ${adapter === "swiftshader" ? 600000 : 10000} });
      try {
        await g.ready;
        const ids = ${JSON.stringify(regions.map((r) => r.id))};
        const out = ids.map(() => []);
        for (let t = 0; t < ${ticks}; t++) {
          const step = await g.command("step", { ticks: 1 });
          if (step.errors && step.errors.length) throw new Error(JSON.stringify(step.errors));
          for (let r = 0; r < ids.length; r++) {
            const v = await g.command("world.get", { entity: ids[r], fields: ["WaterResident"] });
            const d = v.fields.WaterResident?.recipe?.digest;
            out[r].push(d ? d.slice(0, 4).map((b) => b.toString(16).padStart(2, "0")).join("") : "-");
          }
        }
        return out;
      } catch (e) {
        throw new Error(e instanceof Error ? e.message : JSON.stringify(e));
      } finally { g.close(); }
    })()`);
    const firsts = regions.map((r, i) => {
      const at = digests[i].findIndex((d, t) => d !== r.digests[t]);
      return at < 0 ? null : at + 1;
    });
    const first = firsts.filter((t) => t !== null).sort((x, y) => x - y)[0] ?? null;
    result.samples[sample] = { ticks, regions: regions.length, first_divergence: first };
    console.error(`${sample}: ${ticks} ticks, ${first === null ? "every digest matched" : `first differs after tick ${first}`}`);
    if (first !== null) result.ok = false;
  }
} catch (e) {
  result.ok = false;
  errors.push(String(e.message || e).slice(0, 1200));
}
if (errors.length) result.ok = false;
console.log(JSON.stringify(result));
ws.close();
proc.kill();
server.close();
await sleep(300);
try { rmSync(profile, { recursive: true, force: true }); } catch {}
process.exit(result.ok ? 0 : 1);
