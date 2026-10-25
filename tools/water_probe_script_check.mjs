// Actual QuickJS scripts consuming resident GPU observations in the shipped browser worker.
// Build web with tests/fixtures/scenes/water-probes and serve web/ on 8097 before running this check.
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const out=resolve(process.env.WATER_PROBE_OUT||"out/water-probe-scripts");mkdirSync(out,{recursive:true});
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const pkg=new Uint8Array(await(await fetch("/projects/tests-fixtures-scenes-water-probes/package.json")).arrayBuffer());
 const results=[];let previous;
 for(let run=0;run<2;run++){
  const g=a.start(module,pkg,{pacing:"stepped",seed:1});const hashes=[];
  g.onTicks(items=>hashes.push(...items));g.onRender(bytes=>window.amorisViewport.pushFrame(bytes));
  try{
   await g.ready;const done=await g.command("step",{ticks:120});
   const f=(await g.command("world.get",{entity:"Front",fields:["ProbeReadings","WaterProbe","Transform"]})).fields;
   const r=f.ProbeReadings,p=f.WaterProbe,position=f.Transform.position;
   const state=(await g.command("world.get",{entity:"Lake",fields:["WaterResident"]})).fields.WaterResident;
   if(done.errors.length||!r.ready||r.sample_tick!==119||r.age_ticks!==1||!r.request_changed||!p.ready||p.tick!==120)
     throw Error("script result age/request lifetime invalid: "+JSON.stringify(f));
   const expected=[20.5+Math.sin(119/60*0.8)*3,16.5+Math.sin(119/60*0.4)*7];
   if(Math.abs(r.at.x-expected[0])>1e-10||Math.abs(r.at.y-expected[1])>1e-10||
      Math.abs(position[0]-r.at.x)>1e-10||Math.abs(position[1]-r.surface-0.5)>1e-10||Math.abs(position[2]-r.at.y)>1e-10||
      Math.abs(state.stats.volume_cells-1024)>0.03||JSON.stringify(p.state_digest)!==JSON.stringify(state.recipe.digest))
     throw Error("marker or field identity differs from its observation");
   if(previous&&JSON.stringify(hashes)!==JSON.stringify(previous))throw Error("script-driven GPU hashes did not repeat");
   previous=hashes.map(h=>[...h]);
   // An editor request at the boundary cannot relabel the previous result as ready.
   const edit={label:"retarget",ops:[{set:{entity:"Front",component:"WaterProbe",value:{at:[5.5,5.5]}}}]};
   await g.command("world.edit",edit);
   const invalid=(await g.command("world.get",{entity:"Front",fields:["WaterProbe"]})).fields.WaterProbe;
   if(invalid.ready)throw Error("external request retained stale ready output");
   await g.command("step",{ticks:1});
   const pending=(await g.command("world.get",{entity:"Front",fields:["ProbeReadings"]})).fields.ProbeReadings;
   if(pending.ready)throw Error("script read a result for the old editor target");
   const end=await g.command("step",{ticks:1});
   await g.command("snapshots.restore",{tick:60});
   const restored=await g.command("step",{ticks:60});
   if(restored.world_hash!==done.world_hash)throw Error("restored script trajectory diverged before editing");
   await g.command("world.edit",edit);
   const replay=await g.command("step",{ticks:2});
   if(replay.world_hash!==end.world_hash||replay.errors.length||g.problems().length)throw Error("restored script observations diverged");
   results.push({run,tick:120,hash:done.world_hash,sample_tick:r.sample_tick,age_ticks:r.age_ticks,
     at:r.at,depth:r.depth,marker:position,volume:state.stats.volume_cells,restored_hash:replay.world_hash});
  }finally{g.close();}
 }
 document.getElementById("hud").style.display="none";
 return{passed:true,results};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:90000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"marker.png")}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));const result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("probe SDK check failed; inspect "+out+"/browser.log");
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
