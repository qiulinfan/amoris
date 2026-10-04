// Runs the Rust `wasm32-unknown-unknown` module that calls Jolt through joltc
// (bench/physics/jolt-ffi, src/lib.rs `web`) under Node (V8), as aipocket2's browser build would
// load it: no WASI runtime. The module's only imports are the few WASI functions wasi-libc and
// libc++ still name; they get stubs here, and the script reports which were called.
//   node run_jolt_wasm.mjs <jolt_ffi_proto.wasm> [--steps N] [--height H] [--rays]
// Prints one JSON line in jolt-ffi-proto's format; the step is timed from JavaScript.
import { readFile } from "node:fs/promises";
import { argv } from "node:process";

const [wasmPath, ...rest] = argv.slice(2);
let steps = 500, height = 15, rays = false;
for (let i = 0; i < rest.length; i++) {
  if (rest[i] === "--steps") steps = Number(rest[++i]);
  else if (rest[i] === "--height") height = Number(rest[++i]);
  else if (rest[i] === "--rays") rays = true;
  else throw new Error(`unknown argument ${rest[i]}`);
}

const module = await WebAssembly.compile(await readFile(wasmPath));
let memory;
const called = {};
const ENOSYS = 52;
const stubs = {
  // Jolt's Trace and asserts print through stdio: keep the text (stderr) so failures are visible.
  fd_write(fd, iovs, iovsLen, nwritten) {
    const view = new DataView(memory.buffer);
    let n = 0;
    for (let i = 0; i < iovsLen; i++) {
      const ptr = view.getUint32(iovs + 8 * i, true), len = view.getUint32(iovs + 8 * i + 4, true);
      process.stderr.write(new Uint8Array(memory.buffer, ptr, len));
      n += len;
    }
    view.setUint32(nwritten, n, true);
    return 0;
  },
  clock_time_get(_id, _precision, out) {
    new DataView(memory.buffer).setBigUint64(out, process.hrtime.bigint(), true);
    return 0;
  },
  sched_yield() { return 0; },
};
const imports = {};
for (const imp of WebAssembly.Module.imports(module)) {
  if (imp.kind !== "function") throw new Error(`unexpected import ${imp.module}.${imp.name}`);
  imports[imp.module] ??= {};
  const name = `${imp.module}.${imp.name}`;
  const f = stubs[imp.name] ?? (() => ENOSYS);
  imports[imp.module][imp.name] = (...a) => { called[name] = (called[name] ?? 0) + 1; return f(...a); };
}
const instance = await WebAssembly.instantiate(module, imports);
const x = instance.exports;
memory = x.memory;

const t0 = performance.now();
const bodies = x.pyramid_new(height);
const setupMs = performance.now() - t0;
const stepMs = [], rayMs = [];
let hits = 0;
for (let i = 0; i < steps; i++) {
  const a = performance.now();
  x.pyramid_step();
  stepMs.push(performance.now() - a);
  if (rays) {
    const b = performance.now();
    hits += x.pyramid_rays();
    rayMs.push(performance.now() - b);
  }
}
const hash = BigInt.asUintN(64, x.pyramid_hash());
x.pyramid_drop();

const summary = (ms) => {
  const s = [...ms].sort((a, b) => a - b);
  const total = ms.reduce((a, b) => a + b, 0);
  return { mean: total / ms.length, p95: s[Math.min(s.length - 1, Math.round(0.95 * (s.length - 1)))], total };
};
const s = summary(stepMs), warm = summary(stepMs.slice(1));
const out = {
  engine: "jolt-via-joltc-wasm32-unknown-unknown", deterministic: true, scene: height === 15 ? "Pyramid" : `Pyramid${height}`,
  threads: 1, steps, bodies, setup_ms: +setupMs.toFixed(1), mean_ms: +s.mean.toFixed(4), p95_ms: +s.p95.toFixed(4),
  total_ms: +s.total.toFixed(2), mean_ms_after_first: +warm.mean.toFixed(4), node: process.version,
  wasi_imports: WebAssembly.Module.imports(module).map((i) => i.name), wasi_calls: called,
};
if (rays) {
  const r = summary(rayMs);
  Object.assign(out, { ray_mean_ms: +r.mean.toFixed(4), ray_p95_ms: +r.p95.toFixed(4), ray_hits: hits });
}
out.hash = `0x${hash.toString(16)}`;
console.log(JSON.stringify(out));
