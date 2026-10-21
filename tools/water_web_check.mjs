// Actual browser WebGPU checks on the viewport's resident compute/sparse-readback probe.
// Start `python3 -m http.server -d web 8097`, then `node tools/water_web_check.mjs`.
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:8097/viewport/?demo=water&case=rapids&t=3&aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const out = resolve(process.env.WATER_WEB_OUT || "out/water-web-check");
mkdirSync(out, { recursive: true });
const expression = `(async () => {
  const p = window.amorisViewport.raw;
  if (${process.env.WATER_WEB_CONTROLLED === "1"}) {
    p.set_water_optics(false,false,false);
    p.set_camera_look(12,0.5,12,20,0.6,20,0,1,0,60,0.1,0);
  }
  const results = {};
  for (const c of ["lake", "dam"]) {
    p.water_probe_begin(c);
    const batches = [];
    for (let i = 0; i < 100 && batches.length < 3; i++) {
      await new Promise(r => setTimeout(r, 20));
      batches.push(...JSON.parse(p.water_probe_take()));
    }
    batches.sort((a,b) => a.tick - b.tick);
    if (JSON.stringify(batches.map(b=>b.tick)) !== "[10,20,60]") throw new Error(c+": missing stamped samples");
    if (JSON.stringify(batches.map(b=>b.request_id)) !== "[0,1,2]") throw new Error(c+": request identities were mixed");
    for (const b of batches) {
      if (b.error || b.epoch !== 1 || b.columns.length !== 6 ||
          !b.columns.flat().every(Number.isFinite)) throw new Error(c+": invalid result");
      if (JSON.stringify(b.columns[0]) !== JSON.stringify(b.columns[5])) throw new Error("query order/duplicates");
      if (c === "lake") {
        const depths = [1.5, 1.53125, 2, 1.75, 2, 1.5];
        b.columns.forEach((v,i) => {
          if (Math.abs(v[0]-2)>1e-5 || Math.abs(v[1]-depths[i])>1e-5 || Math.abs(v[2])+Math.abs(v[3])>1e-5)
            throw new Error("lake at rest, interpolation or boundary clamp failed");
        });
      }
    }
    results[c] = batches;
  }
  if (!(results.dam[2].columns[1][1] > 0.5 && results.dam[0].columns[1][1] < 0.01))
    throw new Error("dam did not cross the dry front over its sampled ticks");
  document.getElementById("hud").style.display = "none";
  return {passed:true,results};
})()`;
const log = execFileSync(process.execPath, ["tools/web_bench.mjs", url, "5"], {
  encoding: "utf8", timeout: 30000, env: {
    ...process.env, VIEWPORT: "1280x720", VSYNC: "1", UNSAFE_WEBGPU: "0", LOGS: "1",
    EVAL: expression, SHOT: resolve(out,"water.png"),
  },
});
writeFileSync(resolve(out,"browser.log"),log);
const line = log.split("\n").find(l=>l.startsWith("eval: "));
if (!line || !JSON.parse(line.slice(6)).passed || /\[(exception|error)\]|\[amoris ERROR\]/i.test(log)) {
  throw new Error(`water WebGPU check failed; inspect ${out}/browser.log`);
}
if (process.env.WATER_WEB_NATIVE) {
  if (process.env.WATER_WEB_CONTROLLED !== "1") throw new Error("native comparison requires controlled optics/camera");
  const comparison = JSON.parse(execFileSync(process.env.WATER_LAB || "target/release/examples/water_lab",
    ["--compare",resolve(process.env.WATER_WEB_NATIVE),resolve(out,"water.png")], {encoding:"utf8"}));
  writeFileSync(resolve(out,"native-comparison.json"),JSON.stringify(comparison,null,2)+"\n");
  if (comparison.max_abs_rgb > 2) throw new Error("controlled native/browser medium image differs by more than two color steps");
}
console.log(`PASS: actual browser water rendering, resident solver and stamped sparse queries; ${out}`);
