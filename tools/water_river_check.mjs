// Sustained boundary flux in the actual worker, with world accounting and exact restored hashes.
// Build game/viewport plus samples/water-river and serve web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_RIVER_OUT||"out/water-flux/browser-river");mkdirSync(out,{recursive:true});
const native=process.env.WATER_RIVER_NATIVE?JSON.parse(readFileSync(process.env.WATER_RIVER_NATIVE,"utf8")).water_state[0]["WaterResident.boundary"]:null;
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps&assets=/projects/samples-water-river/";
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const pkg=await(await fetch("/projects/samples-water-river/package.json")).json();
 const bytes=new TextEncoder().encode(JSON.stringify(pkg)),runs=[];let previous;
 const check=b=>{
  const actual=b.initial_volume+b.configuration_total+b.entered_total-b.left_total+b.edited_total+b.numerical_residual;
  if(!Number.isFinite(actual)||Math.abs(actual-b.current_volume)>1e-7)throw Error("river ledger does not close: "+JSON.stringify(b));
 };
 for(let run=0;run<2;run++){
  const g=a.start(module,bytes,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
  g.onTicks(items=>hashes.push(...items));g.onRender(b=>{frames++;if(!window.amorisViewport.pushFrame(b))throw Error("river frame refused");});
  try{
   await g.ready;const done=await g.command("step",{ticks:190});if(done.errors.length)throw Error(JSON.stringify(done.errors));
   if(previous&&JSON.stringify(previous)!==JSON.stringify(hashes))throw Error("river hashes do not repeat");previous=hashes.map(h=>[...h]);
   const state=(await g.command("world.get",{entity:"River",fields:["WaterRegion","WaterResident"]})).fields;
   const water=state.WaterResident,budget=water.boundary;check(budget);
   if(water.completed_tick!==190||water.recipe.steps.length!==62||state.WaterRegion.tiles.length||budget.tick!==190||
      Math.abs(budget.entered_total-190/60*8)>1e-4||budget.left_total<10||budget.entered<=0||budget.left<=0||
      Math.abs(budget.numerical_residual)>0.01||Math.abs(water.stats.volume_cells*0.0625-budget.current_volume)>1e-9)
    throw Error("river field/flux is not current or accounting drifts: "+JSON.stringify(water));
   const probe=(await g.command("world.get",{entity:"Gauge",fields:["WaterProbe"]})).fields.WaterProbe;
   if(!probe.ready||probe.tick!==190||probe.depth<=0||probe.velocity[0]<=0||JSON.stringify(probe.state_digest)!==JSON.stringify(water.recipe.digest))throw Error("river probe is stale");
   const native=${JSON.stringify(native)};let nativeError=null;
   if(native){nativeError=0;for(const key of ["entered_total","left_total","current_volume"]){
    nativeError=Math.max(nativeError,Math.abs(budget[key]-native[key]));
    if(Math.abs(budget[key]-native[key])>2e-5*Math.max(1,Math.abs(native[key])))throw Error("native/browser river quantity differs: "+key);
   }}
   const again=await g.command("snapshots.restore",{tick:120});if(again.restored!==120||(await g.command("step",{ticks:70})).world_hash!==done.world_hash)throw Error("river restore diverged");
   const edit=edges=>g.command("world.edit",{label:"valve",ops:[{set:{entity:"River",component:"WaterRegion",value:{edges}}}]});
   await edit(["Wall","Wall","Wall","Wall"]);await g.command("step",{ticks:1});
   const closed=(await g.command("world.get",{entity:"River",fields:["WaterResident"]})).fields.WaterResident.boundary;check(closed);
   if(closed.entered!==0||closed.left!==0)throw Error("closed river reused a stale receipt");
   await edit([{Inflow:{m3s_per_m:0.25}},"Outflow","Wall","Wall"]);await g.command("step",{ticks:1});
   const reopened=(await g.command("world.get",{entity:"River",fields:["WaterResident"]})).fields.WaterResident.boundary;check(reopened);
   if(Math.abs(reopened.entered-0.25*8/60)>1e-6)throw Error("new inlet control did not take effect");
   await g.command("world.edit",{label:"receiving level",ops:[{set:{entity:"River",component:"WaterOutletStage",value:{levels:[null,0.8,null,null]}}}]});await g.command("step",{ticks:1});
   const changed=(await g.command("world.get",{entity:"River",fields:["WaterResident"]})).fields.WaterResident;
   check(changed.boundary);if(changed.controls.outlet_levels[1]!==0.8||changed.recipe.steps.at(-1).boundaries[1].Stage.surface_relative_m!==0.8)throw Error("receiving stage change was not recorded");
   await new Promise(r=>setTimeout(r,150));if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error("river host/presenter problem");
   runs.push({run,hash:done.world_hash,frames,budget,probe,native_max_abs_quantity_error:nativeError,closed:closed.tick,reopened:reopened.tick,stage_changed:changed.completed_tick});
  }finally{g.close();}
 }
 // The first actual open job returns one additional 16-byte boundary receipt, without a full
 // field readback. Two 27-point floaters plus one probe, metadata, receipt and initial
 // verification total 1024 bytes, rather than a complete grid.
 const bindings=await import("/pkg/amoris_web.js");const raw=new bindings.GameWorker(bytes,JSON.stringify({pacing:"stepped",seed:1}));const host=new bindings.BrowserWater();let jobs=0,last;
 const flush=()=>{for(const m of JSON.parse(raw.messages())){if(m.t==="snap"){raw.snap_bytes();raw.ack(m.version);}if(m.t==="reply"&&m.error)throw Error(JSON.stringify(m.error));}raw.render_frame();};
 try{flush();raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:1})}));
  for(let guard=0;guard<16;guard++){const next=JSON.parse(raw.run()).next;flush();if(next==="command")break;if(next==="external"){const result=await host.advance(raw.take_water_job());jobs++;last=JSON.parse(host.diagnostics()).resident;if(!raw.complete_water_job(result))throw Error("river host response rejected");}}
  if(jobs!==1||last.field_readbacks!==0||last.small_readbacks!==2||last.uploads!==1||last.readback_bytes!==1904)throw Error("river transfer scope differs: "+JSON.stringify({jobs,last}));
 }finally{raw.close();raw.free();host.free();}
 document.getElementById("hud").style.display="none";return{passed:true,runs,transfer:{jobs,last}};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:180000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"river.png")}});
writeFileSync(resolve(out,"browser.log"),log);const line=log.split("\n").find(l=>l.startsWith("eval: ")),result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("river check failed; inspect "+out);
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
