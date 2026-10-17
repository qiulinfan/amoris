// The rule workload's WebAssembly module under Node (docs/bench/script.md):
//
//   node bench/rules/web/node.mjs <rules_web.wasm> <sets dir> [--rounds 1] [--offset 0]
//       [--warmup 120] [--ticks 600] [--variants rules,twin,idle,scan] [--json out.json] [--raw]
//
// <sets dir> holds rules.json, idle.json and scan.json (`rules_bench --emit DIR`) and the native
// verify.json, whose final digest the module's own verification must give.
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { run, verify, VARIANTS } from "./bench.mjs";

const [wasmPath, setsDir, ...rest] = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = rest.indexOf(name);
  return i >= 0 && i + 1 < rest.length ? rest[i + 1] : fallback;
};
const rounds = Number(opt("--rounds", 1));
const offset = Number(opt("--offset", 0));
const warmup = Number(opt("--warmup", 120));
const ticks = Number(opt("--ticks", 600));
const variants = opt("--variants", VARIANTS.join(",")).split(",").filter(Boolean);
const jsonOut = opt("--json", null);

const module = await WebAssembly.compile(await readFile(wasmPath));
const sets = {};
for (const v of ["rules", "idle", "scan"]) sets[v] = await readFile(join(setsDir, `${v}.json`));
const native = JSON.parse(await readFile(join(setsDir, "verify.json"), "utf8"));
const verified = await verify(module, sets.rules, native.ticks);
verified.native_final = native.final;
verified.same_as_native = verified.identical && verified.final === native.final && verified.chain === native.chain;
console.log("verify", JSON.stringify(verified));
const results = await run({ module, sets, variants, rounds, offset, warmup, ticks, keepRaw: rest.includes("--raw"), log: console.log });
const out = { target: "node", node: process.version, wasm: wasmPath, warmup, ticks, rounds, verify: verified, results };
if (jsonOut) {
  await writeFile(jsonOut, JSON.stringify(out, null, 1));
  console.log("wrote", jsonOut);
}
process.exit(verified.same_as_native ? 0 : 1);
