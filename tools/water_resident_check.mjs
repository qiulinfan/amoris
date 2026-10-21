// Inspect native published resident recipes on a real browser's WebGPU baseline viewport.
// First: cargo run --release -p amoris-app --example resident_water -- --capture out/water-resident-native
// Serve the repository root on 8097, then: node tools/water_resident_check.mjs
// This checks the visual replica, not resident gameplay in the browser worker.
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
const url = process.argv[2] || "http://127.0.0.1:8097/web/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const frames = process.argv[3] || "http://127.0.0.1:8097/out/water-resident-native";
const out = resolve(process.env.WATER_RESIDENT_OUT || "out/water-resident-browser");
mkdirSync(out, {recursive:true});
const expression = `(async()=>{
  const adapter = await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
  if (!adapter || adapter.info?.isFallbackAdapter===true) throw new Error("hardware WebGPU required");
  const observed=[];
  if (${process.env.WATER_RESIDENT_VIEW === "under"}) {
    // Wet side of the authored dam, looking horizontally through its GPU visual replica.
    window.amorisViewport.raw.set_camera_look(8,0.25,8,8,0.3,16,0,1,0,60,0.1,0);
  }
  for (const tick of [1,27,63,64,120]) {
    const response=await fetch(${JSON.stringify(frames)}+"/tick-"+tick+".bin");
    if(!response.ok) throw new Error("missing published frame "+tick);
    const bytes=new Uint8Array(await response.arrayBuffer());
    if (!window.amorisViewport.pushFrame(bytes)) throw new Error("viewport rejected resident feed at "+tick);
    await new Promise(r=>setTimeout(r,250));
    if (window.amorisStats.tick!==tick) throw new Error("resident frame not drawn at "+tick);
    observed.push({tick,bytes:bytes.length,backend:window.amorisStats.backend});
  }
  document.getElementById("hud").style.display="none";
  return {passed:true,observed};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"3"],{
  encoding:"utf8",timeout:45000, env:{...process.env,VIEWPORT:"1280x720",VSYNC:"1",UNSAFE_WEBGPU:"0",
    LOGS:"1",EVAL:expression,SHOT:resolve(out,"water.png")},
});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));
if(!line || !JSON.parse(line.slice(6)).passed || /\[(exception|error)\]|\[amoris ERROR\]/i.test(log)) {
  throw new Error(`resident viewport failed; inspect ${out}/browser.log`);
}
console.log(`PASS: real WebGPU viewport reconstructs native resident recipes; ${out}`);
