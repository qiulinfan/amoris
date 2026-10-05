// Runs the web tests of pocket-physics in WebAssembly (docs/spec/numeric.md 3.1 and 5; checks.md
// 7.2): the sailing scene's hash chain against the golden one, forks at every tick, parry's BVH
// log2, then the scene's hash at every tick.
//
//   cargo build -p pocket-physics --example web_physics --target wasm32-unknown-unknown --release
//   node crates/pocket-physics/tests/web/run.mjs <target>/wasm32-unknown-unknown/release/examples/web_physics.wasm [expected.txt]
//
// Prints the module's report (one `ok`/`FAIL` line per web test, then `tick hash` lines); exits 1
// if a test failed, if the module needs imports, or if the report differs from the expected file
// (the native build's report, written by the `web_report` test).
import { readFile } from "node:fs/promises";

const [wasmPath, expectedPath] = process.argv.slice(2);
const module = await WebAssembly.compile(await readFile(wasmPath));
const imports = WebAssembly.Module.imports(module);
if (imports.length > 0) {
  console.error("the module imports", imports);
  process.exit(1);
}
const { exports } = await WebAssembly.instantiate(module, {});
const started = performance.now();
const len = exports.run();
const ms = performance.now() - started;
const ptr = exports.report_ptr();
const report = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, ptr, len));
const lines = report.split("\n");
process.stdout.write(lines.filter((l) => !/^\d+ /.test(l)).join("\n") + "\n");
console.error(`${lines.length - 1} lines in ${ms.toFixed(0)} ms; last hash: ${lines[lines.length - 2]}`);
let failed = lines.some((l) => l.startsWith("FAIL"));
if (expectedPath) {
  const expected = (await readFile(expectedPath, "utf8")).replace(/\r\n/g, "\n");
  if (expected !== report) {
    const a = expected.split("\n");
    const i = a.findIndex((l, k) => l !== lines[k]);
    console.error(`the WebAssembly report differs from the native one at line ${i + 1}: ${a[i]} / ${lines[i]}`);
    failed = true;
  } else {
    console.error("identical to the native report");
  }
}
process.exit(failed ? 1 : 0);
