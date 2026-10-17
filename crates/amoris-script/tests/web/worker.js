// The game thread's form on the web (threads.md 7): a module Web Worker that runs the script host's
// web report from examples/web_workload.rs and posts it to the page (tests/web/index.html).
const module = await WebAssembly.compileStreaming(fetch("web_workload.wasm"));
const imports = WebAssembly.Module.imports(module).length;
const { exports } = await WebAssembly.instantiate(module, {});
const input = new Uint8Array(await (await fetch("set.json")).arrayBuffer());
const ptr = exports.alloc(input.length);
new Uint8Array(exports.memory.buffer, ptr, input.length).set(input);
const started = performance.now();
const len = exports.run();
const ms = performance.now() - started;
const report = new TextDecoder().decode(new Uint8Array(exports.memory.buffer, exports.report_ptr(), len));
postMessage({ report, ms, imports });
