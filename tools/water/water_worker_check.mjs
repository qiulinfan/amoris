// Actual GPU physics in the shipped Web Worker, including its snapshot and render-feed bridges.
// Build: SKIP_VIEWPORT=1 tools/build_web.sh tests/fixtures/scenes/water-dam
// Serve web/ on 8097; node tools/water/water_worker_check.mjs [viewport URL].
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const url = process.argv[2] || "http://127.0.0.1:8097/viewport/?gpu_minimal=features,limits,timestamps&eye=25,6,24&target=14,1,16&aa=msaa&gtao=off";
const out = resolve(process.env.WATER_WORKER_OUT || "out/water-worker/check");
mkdirSync(out, { recursive: true });
const expression = `(async()=>{
  const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
  if(!adapter || adapter.info?.isFallbackAdapter===true) throw new Error("GPU worker check requires a hardware WebGPU adapter");
  const adapterInfo={vendor:adapter.info?.vendor,architecture:adapter.info?.architecture,isFallbackAdapter:adapter.info?.isFallbackAdapter};
  const a=await import("/amoris.js");
  const {module}=await a.loadModule();
  const pkg=new Uint8Array(await (await fetch("/projects/tests-fixtures-scenes-water-dam/package.json")).arrayBuffer());
  const summaries=[];
  let realTime;
  const stats=values=>{const v=[...values].sort((a,b)=>a-b);return {median:v[Math.floor(v.length/2)],p95:v[Math.floor(v.length*.95)],max:v[v.length-1]};};
  let previous;
  for(let run=0;run<2;run++){
    const g=a.start(module,pkg,{pacing:"stepped",seed:1});
    const hashes=[];let frames=0;
    g.onTicks(h=>hashes.push(...h));
    g.onRender(bytes=>{frames++;window.amorisViewport.pushFrame(bytes);});
    try {
      await g.ready;
      const start=performance.now();
      const step=await g.command("step",{ticks:60});
      const elapsed=performance.now()-start;
      const region=(await g.command("world.get",{entity:"Lake",fields:["WaterRegion"]})).fields.WaterRegion;
      const perf=await g.perf();
      const depth=region.tiles.flatMap(t=>t.depth);
      const volume=depth.reduce((a,b)=>a+b,0)*region.cell_m*region.cell_m;
      let wetRight=0;let velocity=0;
      for(const tile of region.tiles) for(let i=0;i<256;i++){
        if(tile.key[0]*16+i%16>=16 && tile.depth[i]>.05) wetRight++;
        velocity=Math.max(velocity,Math.abs(tile.u[i]),Math.abs(tile.w[i]));
      }
      if(step.tick!==60 || region.solver!=="gpu" || step.errors.length || g.problems().length)
        throw new Error("GPU worker tick, solver or snapshot reconstruction failed");
      if(!depth.every(d=>Number.isFinite(d)&&d>=0) || Math.abs(volume-1024)>.02 || wetRight<32 || velocity<.5)
        throw new Error("GPU dam conservation, wet front or flow failed");
      if(frames<60) throw new Error("completed GPU ticks did not publish their render feed");
      if(hashes.length!==61 || hashes.some((h,i)=>h[0]!==i)) throw new Error("missing tick hashes");
      if(previous && JSON.stringify(hashes)!==previous) throw new Error("same-browser GPU replayed inputs diverged");
      previous=JSON.stringify(hashes);
      summaries.push({run,elapsed_ms:elapsed,tick:step.tick,hash:step.world_hash,volume,wetRight,max_velocity:velocity,
        render_frames:frames,snapshots:g.rebuildMs.length,max_unacked:g.maxUnacked,cold_active_ms:perf.step_ms[0],
        cold_external_ms:perf.external_ms[0],active_ms:stats(perf.step_ms.slice(1)),
        external_ms:stats(perf.external_ms.slice(1)),publish_ms:stats(perf.publish_ms.slice(1)),
        total_tick_ms:stats(perf.step_ms.slice(1).map((v,i)=>v+perf.external_ms[i+1]+perf.publish_ms[i+1]))});
      if(run===1){
        let counting=false;let rendered=0;
        window.amorisViewport.onStats(()=>{if(counting)rendered++;});
        await g.command("time_control",{pacing:{real_time:{speed:1}},pause:false});
        const begin=performance.now();counting=true;
        await new Promise(r=>setTimeout(r,1000));
        await g.command("time_control",{pause:true});
        counting=false;
        const elapsed=performance.now()-begin;
        const advanced=g.latest().tick-60;
        if(advanced===0 || rendered===0) throw new Error("real-time GPU water or viewport made no progress");
        realTime={elapsed_ms:elapsed,ticks_advanced:advanced,viewport_submissions:rendered,viewport_submissions_per_s:rendered*1000/elapsed};
      }
    } finally {g.close();}
  }
  // An actual browser-host cancellation must return a typed failure and the previous boundary,
  // rather than exposing the partially initialized region or throwing a mutable-borrow error.
  const bindings=await import("/pkg/amoris_web.js");
  const raw=new bindings.GameWorker(pkg,JSON.stringify({pacing:"stepped",seed:1}));
  for(const m of JSON.parse(raw.messages())) if(m.t==="snap") raw.snap_bytes();
  raw.ack(1);
  raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:1})}));
  if(JSON.parse(raw.run()).next!=="external") throw new Error("GPU cancellation probe did not suspend");
  const host=new bindings.BrowserWater();
  const pending=host.advance(raw.take_water_job());
  await Promise.resolve();
  host.abort();
  raw.complete_water_job(await pending);
  raw.run();
  const failed=JSON.parse(raw.messages());
  const fault=failed.find(m=>m.t==="reply"&&m.seq===1)?.error;
  const halted=failed.find(m=>m.t==="snap");
  if(fault?.code!=="water.compute_failed" || halted?.header.tick!==0 || !halted.time.halted)
    throw new Error("cancelled GPU work exposed an incomplete boundary");
  raw.close();raw.free();host.free();
  document.getElementById("hud").style.display="none";
  await new Promise(r=>setTimeout(r,80));
  return {passed:true,adapterInfo,summaries,realTime,cancellation:{code:fault.code,snapshot_tick:halted.header.tick}};
})()`;
const log = execFileSync(process.execPath, ["tools/web_bench.mjs", url, "2"], {
  encoding: "utf8", timeout: 90000, maxBuffer: 8*1024*1024,
  env: { ...process.env, EVAL: expression, LOGS: "1", UNSAFE_WEBGPU: "0", VSYNC: process.env.VSYNC || "1",
    VIEWPORT:"1280x720", SHOT:resolve(out,"dam.png") },
});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));
const result=line ? JSON.parse(line.slice(6)) : null;
if(!result?.passed || /\[(exception|error)\]|\[amoris ERROR\]/i.test(log))
  throw new Error(`GPU water worker failed; inspect ${out}/browser.log`);
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2));
console.log(JSON.stringify(result,null,2));
