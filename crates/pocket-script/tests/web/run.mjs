// Runs the script host's web report in WebAssembly (crates/pocket-script/tests/web.rs):
//
//   cargo build -p pocket-script --example web_workload --target wasm32-unknown-unknown --release
//   node crates/pocket-script/tests/web/run.mjs <web_workload.wasm> <set.json> [native report]
//
// Prints the report; exits 1 if a line failed, if the module needs imports, or if the report
// differs from the native one (written by the `web_report` test to <target>/tmp/).
import { readFile } from "node:fs/promises";

const [wasmPath, setPath, expectedPath] = process.argv.slice(2);
const module = await WebAssembly.compile(await readFile(wasmPath));
const imports = WebAssembly.Module.imports(module);
if (imports.length > 0) {
  console.error("the module imports", imports);
  process.exit(1);
}
const { exports } = await WebAssembly.instantiate(module, {});
const input = await readFile(setPath);
const ptr = exports.alloc(input.length);
new Uint8Array(exports.memory.buffer, ptr, input.length).set(input);
const started = performance.now();
const len = exports.run();
const ms = performance.now() - started;
const report = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, exports.report_ptr(), len));
process.stdout.write(report);
console.error(`report in ${ms.toFixed(0)} ms`);
let failed = report.split("\n").some((l) => l.startsWith("FAIL"));
if (expectedPath) {
  const expected = (await readFile(expectedPath, "utf8")).replace(/\r\n/g, "\n");
  if (expected !== report) {
    const a = expected.split("\n");
    const b = report.split("\n");
    const i = a.findIndex((line, k) => line !== b[k]);
    console.error(`the WebAssembly report differs from the native one at line ${i + 1}:`);
    console.error(`  native: ${a[i]}`);
    console.error(`  wasm:   ${b[i]}`);
    failed = true;
  } else {
    console.error("identical to the native report");
  }
}
process.exit(failed ? 1 : 0);
