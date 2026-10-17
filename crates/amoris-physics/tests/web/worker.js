// Runs web_physics.wasm (no imports) and compares its report with expected.txt, the native one.
const module = await WebAssembly.compileStreaming(fetch("web_physics.wasm"));
const imports = WebAssembly.Module.imports(module);
const { exports } = await WebAssembly.instantiate(module, {});
const started = performance.now();
const len = exports.run();
const ms = Math.round(performance.now() - started);
const report = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, exports.report_ptr(), len));
const expected = (await (await fetch("expected.txt")).text()).replace(/\r\n/g, "\n");
const lines = report.split("\n");
const tests = lines.filter((l) => /^(ok|FAIL) /.test(l));
const want = expected.split("\n");
const first = want.findIndex((l, i) => l !== lines[i]);
const ok = imports.length === 0 && !tests.some((l) => l.startsWith("FAIL")) && report === expected;
postMessage({
  ok,
  tests,
  imports: imports.length,
  lines: lines.length - 1,
  identical: report === expected,
  first_difference: first < 0 ? null : first + 1,
  last: lines[lines.length - 2],
  ms,
});
