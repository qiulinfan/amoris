// The rule workload's WebAssembly module (crates/pocket-script/examples/rules_web.rs) timed per
// tick, as the native bench (rules_bench) times Sim::step: performance.now() around each tick()
// call. Shared by node.mjs (Node) and worker.js (a browser's Web Worker); docs/bench/script.md.

const PROBES = { rules: 0, idle: 1, scan: 2, twin: 3 };
export const VARIANTS = Object.keys(PROBES);

async function instantiate(module) {
  const instance = await WebAssembly.instantiate(module, { bench: { now: () => performance.now() } });
  return instance.exports;
}

function put(ex, bytes) {
  const p = ex.alloc(bytes.length);
  new Uint8Array(ex.memory.buffer, p, bytes.length).set(bytes);
}

/** Throws the module's last error when `len` (an export's result: 0, or the error's length) is not 0. */
function check(ex, len) {
  if (len !== 0) {
    throw new Error(new TextDecoder().decode(new Uint8Array(ex.memory.buffer, ex.error_ptr(), ex.error_len())));
  }
}

const hex = (x) => BigInt.asUintN(64, x).toString(16).padStart(16, "0");
const r1 = (x) => Math.round(x * 10) / 10;

function percentile(sorted, p) {
  const n = sorted.length;
  return sorted[Math.min(Math.max(Math.ceil(p * n), 1), n) - 1];
}

function stats(values) {
  const s = Float64Array.from(values).sort();
  const mean = s.reduce((a, b) => a + b, 0) / Math.max(s.length, 1);
  return { median: r1(percentile(s, 0.5)), min: r1(s[0]), p99: r1(percentile(s, 0.99)), max: r1(s[s.length - 1]), mean: r1(mean) };
}

/** One variant's round: a fresh instance and world, the warm-up, the measured ticks. */
export async function round(module, sets, variant, warmup, ticks, keepRaw) {
  const ex = await instantiate(module);
  if (variant !== "twin") put(ex, sets[variant === "rules" ? "rules" : variant]);
  check(ex, ex.init(PROBES[variant]));
  for (let i = 0; i < warmup; i++) {
    check(ex, ex.tick());
    ex.sample();
  }
  const wall = new Float64Array(ticks);
  const cols = Array.from({ length: 7 }, () => new Float64Array(ticks));
  for (let i = 0; i < ticks; i++) {
    const t0 = performance.now();
    const r = ex.tick();
    const t1 = performance.now();
    check(ex, r);
    wall[i] = (t1 - t0) * 1000;
    const s = new Float64Array(ex.memory.buffer, ex.sample(), 7);
    for (let k = 0; k < 6; k++) cols[k][i] = s[k] * 1000;
    cols[6][i] = s[6];
  }
  const out = {
    variant, ticks,
    tick_us: stats(wall),
    update_us: stats(cols[0]),
    system_us: stats(cols[1]),
    phases_us: { prepare: stats(cols[2]), run: stats(cols[3]), diff: stats(cols[4]), apply: stats(cols[5]) },
    steps: stats(cols[6]),
    digest: hex(ex.digest()),
  };
  if (keepRaw) out.raw_tick_us = Array.from(wall, r1);
  return out;
}

/** The script world and the twin side by side in the module, digests compared at every tick. */
export async function verify(module, rulesSet, ticks) {
  const ex = await instantiate(module);
  put(ex, rulesSet);
  const t = ex.verify(ticks);
  if (t === -1) check(ex, -1); // u32::MAX: an error, not a tick
  return { ticks, identical: t === 0, first_divergent_tick: t || null, final: hex(ex.verified_final()), chain: hex(ex.verified_chain()) };
}

/** Rounds of every variant, in an order that rotates by round from `offset`. */
export async function run({ module, sets, variants = VARIANTS, rounds = 1, offset = 0, warmup = 120, ticks = 600, keepRaw = false, log = () => {} }) {
  const results = [];
  for (let r = 0; r < rounds; r++) {
    for (let i = 0; i < variants.length; i++) {
      const v = variants[(i + r + offset) % variants.length];
      const res = await round(module, sets, v, warmup, ticks, keepRaw);
      res.round = r + 1;
      log(`round ${r + 1} ${v.padEnd(5)} tick median ${res.tick_us.median} us min ${res.tick_us.min} p99 ${res.tick_us.p99} | system ${res.system_us.median} | steps ${res.steps.median}`);
      results.push(res);
    }
  }
  return results;
}
