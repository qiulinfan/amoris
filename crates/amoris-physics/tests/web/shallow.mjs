// Times the shallow-water spike's substeps in WebAssembly (examples/shallow_web.rs;
// docs/spikes/shallow-water.md) and prints the same JSON lines and hash chains as
// `shallow_bench` natively, for each scheme.
//
//   cargo build -p amoris-physics --example shallow_web --target wasm32-unknown-unknown --release
//   node crates/amoris-physics/tests/web/shallow.mjs <the .wasm> [N ...]
import { readFile } from "node:fs/promises";

const [wasmPath, ...rest] = process.argv.slice(2);
const sizes = rest.length ? rest.map(Number) : [128, 256, 512, 1024];
const module = await WebAssembly.compile(await readFile(wasmPath));
if (WebAssembly.Module.imports(module).length > 0) {
  console.error("the module imports", WebAssembly.Module.imports(module));
  process.exit(1);
}
const { exports: x } = await WebAssembly.instantiate(module, {});
const hex = () =>
  (x.hash(1) >>> 0).toString(16).padStart(8, "0") + (x.hash(0) >>> 0).toString(16).padStart(8, "0");
for (const [pipes, scheme] of [[0, "advect"], [1, "pipes"]]) {
  for (const n of sizes) {
    x.setup(n, pipes);
    x.run(10);
    const rounds = Math.min(200, Math.max(5, Math.floor(30_000_000 / (n * n))));
    const per = [];
    for (let r = 0; r < rounds; r++) {
      const t = performance.now();
      x.run(10);
      per.push((performance.now() - t) / 10);
    }
    per.sort((a, b) => a - b);
    const ms = per[Math.floor(per.length / 2)];
    console.log(JSON.stringify({ target: "wasm (node " + process.version + ")", scheme, n,
      columns: n * n, ms_per_substep: Number(ms.toFixed(4)),
      ns_per_column: Number(((ms * 1e6) / (n * n)).toFixed(3)) }));
  }
  x.setup(128, pipes);
  const chain = [];
  for (let i = 0; i < 10; i++) {
    x.run(60);
    chain.push(hex());
  }
  console.log(JSON.stringify({ scheme, chain }));
}
