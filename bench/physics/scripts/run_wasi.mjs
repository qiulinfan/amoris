// Runs a wasm32-wasip1 build of the Rapier benchmark under Node's WASI (Node 24):
//   node run_wasi.mjs <module.wasm> [benchmark arguments...]
// The host filesystem is mapped at / so the benchmark's paths work unchanged.
import { readFile } from "node:fs/promises";
import { WASI } from "node:wasi";
import { argv, exit } from "node:process";

const [wasmPath, ...args] = argv.slice(2);
const wasi = new WASI({
  version: "preview1",
  args: ["physics-bench", ...args],
  env: {},
  preopens: { "/": "/" },
  returnOnExit: true,
});
const module = await WebAssembly.compile(await readFile(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
exit(wasi.start(instance));
