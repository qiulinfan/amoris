// The rule workload's WebAssembly module in a browser's module Web Worker, the game thread's form
// on the web (threads.md 7): runs bench.mjs with the page's parameters and posts the results.
import { run, verify } from "./bench.mjs";

onmessage = async ({ data: p }) => {
  try {
    const module = await WebAssembly.compileStreaming(fetch(p.wasm));
    const sets = {};
    for (const v of ["rules", "idle", "scan"]) sets[v] = new Uint8Array(await (await fetch(`${v}.json`)).arrayBuffer());
    const native = await (await fetch("verify.json")).json();
    const verified = await verify(module, sets.rules, native.ticks);
    verified.native_final = native.final;
    verified.same_as_native = verified.identical && verified.final === native.final && verified.chain === native.chain;
    const results = await run({ module, sets, variants: p.variants, rounds: p.rounds, offset: p.offset, warmup: p.warmup, ticks: p.ticks });
    postMessage({ verify: verified, results, crossOriginIsolated: self.crossOriginIsolated });
  } catch (e) {
    postMessage({ error: String(e && e.stack || e) });
  }
};
