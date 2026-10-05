// Runs the web tests of pocket-persist in WebAssembly (docs/spec/persistence.md 12, P4):
//
//   cargo build -p pocket-persist --example web_vectors --target wasm32-unknown-unknown --release
//   node crates/pocket-persist/tests/web/run.mjs <target>/wasm32-unknown-unknown/release/examples/web_vectors.wasm [expected.txt]
//
// Prints the module's report (one `ok`/`FAIL` line per web test, the pinned vectors);
// exits 1 if a test failed, if the module needs imports, or if the report differs from the
// expected file (the native build's report, written by the `web_report` test).
import { readFile } from "node:fs/promises";

const [wasmPath, expectedPath] = process.argv.slice(2);
const module = await WebAssembly.compile(await readFile(wasmPath));
const imports = WebAssembly.Module.imports(module);
if (imports.length > 0) {
  console.error("the module imports", imports);
  process.exit(1);
}
const { exports } = await WebAssembly.instantiate(module, {});
const len = exports.run();
const ptr = exports.report_ptr();
const report = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, ptr, len));
process.stdout.write(report);
let failed = report.split("\n").some((l) => l.startsWith("FAIL"));
if (expectedPath) {
  const expected = (await readFile(expectedPath, "utf8")).replace(/\r\n/g, "\n");
  if (expected !== report) {
    console.error("the WebAssembly report differs from the native one");
    failed = true;
  } else {
    console.error("identical to the native report");
  }
}
process.exit(failed ? 1 : 0);
