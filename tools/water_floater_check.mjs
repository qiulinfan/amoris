// Real browser worker + Rapier + resident GPU field, with four 27-point floating crates.
// Build web/viewport and `tools/build_web.sh samples/water-floaters`, serve web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const out=resolve(process.env.WATER_FLOATER_OUT||"out/water-floaters-browser");mkdirSync(out,{recursive:true});
const native=process.env.WATER_FLOATER_NATIVE ? JSON.parse(readFileSync(process.env.WATER_FLOATER_NATIVE,"utf8")).find(row=>row.tick===120) : null;
if(process.env.WATER_FLOATER_NATIVE && !native)throw Error("native body capture must include tick 120");
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const packageObject=await(await fetch("/projects/samples-water-floaters/package.json")).json();
 if (${JSON.stringify(process.env.WATER_FLOATER_OPTICS || "all")} === "none") window.amorisViewport.raw.set_water_optics(false,false,false);
 if (${JSON.stringify(process.env.WATER_FLOATER_OPTICS || "all")} === "no-reflection") window.amorisViewport.raw.set_water_optics(true,false,true);
 if (${JSON.stringify(process.env.WATER_FLOATER_OPTICS || "all")} === "no-refraction") window.amorisViewport.raw.set_water_optics(false,true,true);
 const pkg=new TextEncoder().encode(JSON.stringify(packageObject));
 let previous;const runs=[];
 for(let run=0;run<2;run++){
  const g=a.start(module,pkg,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
  g.onTicks(items=>hashes.push(...items));
  g.onRender(bytes=>{frames++;if(!window.amorisViewport.pushFrame(bytes))throw Error("floater feed rejected");});
  try{
   await g.ready;const start=performance.now();const done=await g.command("step",{ticks:120});
   const elapsed=performance.now()-start;const bodies=[];
   for(let n=0;n<4;n++){
    const f=(await g.command("world.get",{entity:"Float"+n,fields:["Transform","Velocity","Floater"]})).fields;
    const p=f.Transform.position,v=f.Velocity.linear;
    if(![...p,...v,...f.Transform.rotation,...f.Velocity.angular].every(Number.isFinite)||
       f.Floater.submerged<0.1||f.Floater.submerged>0.9||p[1]<0.3||p[1]>2.5)
      throw Error("body must float with real integrated motion: "+JSON.stringify(f));
    bodies.push({position:p,velocity:v,angular:f.Velocity.angular,rotation:f.Transform.rotation,submerged:f.Floater.submerged});
   }
   const state=(await g.command("world.get",{entity:"Lake",fields:["WaterRegion","WaterResident"]})).fields;
   let nativeMaxError=null;const native=${JSON.stringify(native)};
   if(native){
    if(native.bodies.length!==bodies.length)throw Error("native/browser body count differs");
    nativeMaxError=0;
    bodies.forEach((b,n)=>{for(const key of ["position","velocity","angular","rotation"]){
     b[key].forEach((x,i)=>nativeMaxError=Math.max(nativeMaxError,Math.abs(x-native.bodies[n][key][i])));
    }nativeMaxError=Math.max(nativeMaxError,Math.abs(b.submerged-native.bodies[n].submerged));});
    if(nativeMaxError>1e-4)throw Error("native/browser floater trajectories differ: "+nativeMaxError);
   }
   if(done.errors.length||state.WaterRegion.tiles.length||Math.abs(state.WaterResident.stats.volume_cells-1024)>0.03||
      bodies[0].position[0]>=5.6||bodies[3].position[0]<=11.7||g.problems().length)
    throw Error("current propagation, volume or resident body execution failed");
   if(hashes.length!==121)throw Error("body tick stream lost boundaries");
   if(previous&&JSON.stringify(hashes)!==JSON.stringify(previous))throw Error("GPU body trajectories did not repeat");
   previous=hashes.map(h=>[...h]);
   await g.command("snapshots.restore",{tick:60});
   const replay=await g.command("step",{ticks:60});
   if(replay.world_hash!==done.world_hash||replay.errors.length)throw Error("body/field restore diverged");
   const edit={label:"move floater",ops:[
    {set:{entity:"Float0",component:"Transform",value:{position:[24.5,10,8.5]}}},
    {set:{entity:"Float0",component:"Velocity",value:{linear:[0,0,0],angular:[0,0,0]}}}
   ]};
   await g.command("world.edit",edit);await g.command("step",{ticks:1});
   const moved=(await g.command("world.get",{entity:"Float0",fields:["Floater","Velocity"]})).fields;
   if(moved.Floater.submerged!==0||moved.Velocity.linear[1]>=-0.15)throw Error("old-pose samples lifted the teleported body");
   // Leave the viewport showing the completed physical trajectory rather than the edit probe.
   await g.command("snapshots.restore",{tick:60});await g.command("step",{ticks:60});
   await new Promise(r=>setTimeout(r,150));
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error("floater presenter rejected restored state");
   runs.push({run,ticks:120,elapsed_ms:elapsed,hash:done.world_hash,volume:state.WaterResident.stats.volume_cells,frames,bodies,native_max_abs_error:nativeMaxError});
  }finally{g.close();}
 }
 // Exercise the expanded exact capacity in the same real browser compute host. Only one tick:
 // the above-water fixed sample poses avoid treating this capacity fixture as a force benchmark.
 const bindings=await import("/pkg/amoris_web.js");const stress=structuredClone(packageObject);
 for(const e of stress.scene.entities)if(e.components.Floater){
  e.components.Floater.points=Array.from({length:1024},()=>structuredClone(e.components.Floater.points[0]));
  e.components.RigidBody={kind:"Fixed"};
 }
 const raw=new bindings.GameWorker(new TextEncoder().encode(JSON.stringify(stress)),JSON.stringify({pacing:"stepped",seed:1}));
 const host=new bindings.BrowserWater();let jobs=0,transfers,reply;
 const flush=()=>{for(const m of JSON.parse(raw.messages())){
  if(m.t==="snap"){raw.snap_bytes();raw.ack(m.version);}
  if(m.t==="reply"){if(m.error)throw Error(JSON.stringify(m.error));reply=m;}
 }raw.render_frame();};
 try{
  flush();raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:1})}));
  for(let guard=0;guard<20;guard++){
   const next=JSON.parse(raw.run()).next;flush();if(next==="command")break;
   if(next==="external"){
    const job=raw.take_water_job();if(job.kind!=="resident")throw Error("body capacity changed solver mode");
    const output=await host.advance(job);transfers=JSON.parse(host.diagnostics()).resident;jobs++;
    if(!raw.complete_water_job(output))throw Error("body sample completion refused");
   }
  }
  if(jobs!==1||!reply||transfers.field_readbacks!==0||transfers.small_readbacks!==2||transfers.readback_bytes!==131200)
   throw Error("4096-point readback scope changed: "+JSON.stringify({jobs,transfers,reply}));
 }finally{raw.close();raw.free();host.free();}
 document.getElementById("hud").style.display="none";
 return{passed:true,runs,capacity:{points:4096,jobs,transfers}};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:120000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"floaters.png")}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));const result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("GPU floater check failed; inspect "+out+"/browser.log");
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
