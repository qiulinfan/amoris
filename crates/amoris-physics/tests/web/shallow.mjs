// Times the shallow-water spike's substeps in WebAssembly under Node (examples/shallow_web.rs;
// docs/spikes/shallow-water.md) and prints the same JSON lines and hash chains as
// `shallow_bench` natively, for each scheme. Chrome runs the same code through shallow.html.
//
//   cargo build -p amoris-physics --example shallow_web --target wasm32-unknown-unknown --release
//   node crates/amoris-physics/tests/web/shallow.mjs <the .wasm> [N ...]
import { readFile } from "node:fs/promises";
import { run } from "./shallow_run.mjs";

const [wasmPath, ...rest] = process.argv.slice(2);
const sizes = rest.length ? rest.map(Number) : [128, 256, 512, 1024];
for (const line of await run(await readFile(wasmPath), sizes, "wasm (node " + process.version + ")")) {
  console.log(JSON.stringify(line));
}
