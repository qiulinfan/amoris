// Actual worker/Rapier/WebGPU reaction loop, restored input journal and isolated transfer count.
// Build the game/viewport for samples/water-feedback and serve web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_FEEDBACK_OUT||"out/water-feedback/browser");mkdirSync(out,{recursive:true});
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const native=process.env.WATER_FEEDBACK_NATIVE?JSON.parse(readFileSync(process.env.WATER_FEEDBACK_NATIVE,"utf8")).find(r=>r.tick===120):null;
if(process.env.WATER_FEEDBACK_NATIVE&&!native)throw Error("native capture must include tick 120");
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const packageObject=await(await fetch("/projects/samples-water-feedback/package.json")).json();
 const pkg=new TextEncoder().encode(JSON.stringify(packageObject));let previous;const runs=[];
 const pending=(f,axis)=>f.pending.reduce((s,p)=>s+p.impulse[axis],0);
 const check=f=>{for(let axis=0;axis<2;axis++){
  const expected=f.emitted_total[axis],actual=f.applied_total[axis]+f.rejected_total[axis]+f.residual_total[axis]+pending(f,axis);
  if(!Number.isFinite(actual)||Math.abs(expected-actual)>1e-6+Math.abs(expected)*1e-6)throw Error("reaction ledger failed "+JSON.stringify(f));
 }};
 for(let run=0;run<2;run++){
  const g=a.start(module,pkg,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
  g.onTicks(items=>hashes.push(...items));g.onRender(bytes=>{frames++;if(!window.amorisViewport.pushFrame(bytes))throw Error("reaction frame refused");});
  try{
   await g.ready;const done=await g.command("step",{ticks:120});
   const state=(await g.command("world.get",{entity:"Lake",fields:["WaterRegion","WaterResident","WaterFeedback"]})).fields;
   const f=state.WaterFeedback;check(f);
   if(done.errors.length||state.WaterRegion.tiles.length||f.source_tick!==119||f.queued_tick!==120||
      f.applied_total[0]<100||Math.abs(f.rejected_total[0])>1e-6||Math.abs(state.WaterResident.stats.volume_cells*0.0625-2048)>0.03||
      state.WaterResident.stats.max_speed<1e-4||!state.WaterResident.recipe.environment.includes("kernels ")||g.problems().length)
    throw Error("current field reaction, mass, source tick or environment failed: "+JSON.stringify(state));
   const bodies=[];
   for(let n=0;n<4;n++){
    const body=(await g.command("world.get",{entity:"Float"+n,fields:["Transform","Velocity","Floater"]})).fields;
    bodies.push({position:body.Transform.position,rotation:body.Transform.rotation,velocity:body.Velocity.linear,angular:body.Velocity.angular,submerged:body.Floater.submerged});
   }
   const native=${JSON.stringify(native)};let nativeError=null;
   if(native){nativeError=0;bodies.forEach((b,n)=>{
    for(const key of ["position","rotation","velocity","angular"]){b[key].forEach((x,i)=>nativeError=Math.max(nativeError,Math.abs(x-native.bodies[n][key][i])));}
    nativeError=Math.max(nativeError,Math.abs(b.submerged-native.bodies[n].submerged));
   });if(nativeError>1e-4)throw Error("native/browser reaction bodies diverged: "+nativeError);}
   if(hashes.length!==121)throw Error("reaction hash stream lost boundaries");
   if(previous&&JSON.stringify(hashes)!==JSON.stringify(previous))throw Error("reaction trajectories did not repeat");
   previous=hashes.map(h=>[...h]);
   await g.command("snapshots.restore",{tick:60});const replay=await g.command("step",{ticks:60});
   if(replay.world_hash!==done.world_hash||replay.errors.length)throw Error("reaction packets/journal did not restore");
   const again=(await g.command("world.get",{entity:"Lake",fields:["WaterFeedback"]})).fields.WaterFeedback;check(again);
   if(JSON.stringify(again)!==JSON.stringify(f))throw Error("restored reaction ledger differs");
   const forged=await g.command("world.edit",{label:"forge feedback",ops:[{set:{entity:"Lake",component:"WaterFeedback",value:{pending:[]}}}]}).then(()=>null,e=>e);
   if(!forged||!JSON.stringify(forged).includes("water.feedback_readonly"))throw Error("reaction outputs became writable");
   await new Promise(r=>setTimeout(r,150));
   if(JSON.parse(g.presenter.problems()).length)throw Error("presenter rejected reaction restoration");
   runs.push({run,hash:done.world_hash,frames,bodies,native_max_abs_error:nativeError,
    volume_m3:state.WaterResident.stats.volume_cells*0.0625,max_speed:state.WaterResident.stats.max_speed,
    applied_total:f.applied_total,residual_total:f.residual_total,pending:[pending(f,0),pending(f,1)],emitted_total:f.emitted_total});
  }finally{g.close();}
 }
 // A matched control does not receive any physical reaction in the water field.
 const plain=structuredClone(packageObject);delete plain.scene.entities[1].components.WaterFeedback;
 const control=a.start(module,new TextEncoder().encode(JSON.stringify(plain)),{pacing:"stepped",seed:1});let controlSpeed;
 try{await control.ready;await control.command("step",{ticks:120});
  const water=(await control.command("world.get",{entity:"Lake",fields:["WaterResident"]})).fields.WaterResident;
  controlSpeed=water.stats.max_speed;if(controlSpeed!==0)throw Error("one-way control unexpectedly changes the fluid");
 }finally{control.close();}
 // Inspect the first two real host jobs directly: tick one has no old packet, tick two adds
 // exactly 32 bytes of coupled impulse budget to the existing metadata + 108 point samples.
 const bindings=await import("/pkg/amoris_web.js");const raw=new bindings.GameWorker(pkg,JSON.stringify({pacing:"stepped",seed:1}));
 const host=new bindings.BrowserWater();let jobs=0,transfers;const bytes=[];
 const flush=()=>{for(const m of JSON.parse(raw.messages())){if(m.t==="snap"){raw.snap_bytes();raw.ack(m.version);}if(m.t==="reply"&&m.error)throw Error(JSON.stringify(m.error));}raw.render_frame();};
 try{flush();raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:2})}));
  for(let guard=0;guard<20;guard++){
   const next=JSON.parse(raw.run()).next;flush();if(next==="command")break;
   if(next==="external"){
    const job=raw.take_water_job();const result=await host.advance(job);transfers=JSON.parse(host.diagnostics()).resident;
    bytes.push(transfers.readback_bytes);jobs++;if(!raw.complete_water_job(result))throw Error("reaction response rejected");
   }
  }
  if(jobs!==2||transfers.field_readbacks!==0||JSON.stringify(bytes)!=="[1856,3680]")throw Error("reaction readback scope changed: "+JSON.stringify({jobs,bytes,transfers}));
 }finally{raw.close();raw.free();host.free();}
 document.getElementById("hud").style.display="none";
 return{passed:true,runs,control_max_speed:controlSpeed,transfer:{jobs,bytes,transfers}};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:120000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"feedback.png")}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));const result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("GPU feedback check failed; inspect "+out+"/browser.log");
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
