// Actual worker restore/repeat and bounded host-cache transfers for independent water regions.
// Build game/viewport plus tests/fixtures/scenes/water-feedback; serve web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_MULTIREGION_OUT||"out/water-multiregion/browser");
mkdirSync(out,{recursive:true});
const native=process.env.WATER_MULTIREGION_NATIVE
  ? JSON.parse(readFileSync(process.env.WATER_MULTIREGION_NATIVE,"utf8"))["4x16-after-1"].reaction : null;
const nativeMotion=process.env.WATER_MULTIREGION_MOTION
  ? JSON.parse(readFileSync(process.env.WATER_MULTIREGION_MOTION,"utf8")).find(r=>r.tick===190) : null;
if(process.env.WATER_MULTIREGION_MOTION&&!nativeMotion)throw Error("native motion must include tick 190");
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const expression=`(async()=>{
 const native=${JSON.stringify(native)};
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const pkg=await(await fetch("/projects/tests-fixtures-scenes-water-feedback/package.json")).json();
 const template=pkg.scene.entities.find(e=>e.components.Floater);
 const lake=pkg.scene.entities.find(e=>e.components.WaterRegion);
 pkg.scene.entities=pkg.scene.entities.filter(e=>!e.components.Floater&&!e.components.WaterRegion);
 const floor=pkg.scene.entities.find(e=>e.name==="Floor").components;
 floor.Transform.position[0]=76;floor.Collider.shape.Cuboid.half_extents[0]=76;floor.Model.scale[0]=152;
 for(let r=0;r<4;r++){
  const water=structuredClone(lake);water.name="Lake"+r;water.components.Transform={position:[r*40,0,0]};
  pkg.scene.entities.push(water);
  for(let n=0;n<16;n++){
   const body=structuredClone(template);body.name="Float"+r+"_"+n;
   body.components.Transform.position=[r*40+3.5+(n%16)*1.5,2.09,3.5+Math.floor(n/16)*2.5];
   pkg.scene.entities.push(body);
  }
 }
 const bytes=new TextEncoder().encode(JSON.stringify(pkg));const runs=[];let previous;
 for(let run=0;run<2;run++){
  const g=a.start(module,bytes,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
  g.onTicks(items=>hashes.push(...items));
  g.onRender(b=>{frames++;if(!window.amorisViewport.pushFrame(b))throw Error("multi-region frame refused");});
  try{
   await g.ready;const done=await g.command("step",{ticks:190});
   if(done.errors.length)throw Error(JSON.stringify(done.errors));
   if(previous&&JSON.stringify(previous)!==JSON.stringify(hashes))throw Error("multi-region hashes did not repeat");
   previous=hashes.map(h=>[...h]);
   const regions=[];let nativeExact=native?true:null,nativeRelativeError=0;
   for(let r=0;r<4;r++){
    const row=(await g.command("world.get",{entity:"Lake"+r,fields:["WaterResident","WaterFeedback"]})).fields;
    const state=row.WaterResident,f=row.WaterFeedback;
    const pending=[0,1].map(axis=>f.pending.reduce((s,p)=>s+p.impulse[axis],0));
    const closure=[0,1].map(axis=>f.emitted_total[axis]-f.applied_total[axis]-f.rejected_total[axis]-f.residual_total[axis]-pending[axis]);
    if(closure.some((v,axis)=>!Number.isFinite(v)||Math.abs(v)>1e-6+Math.abs(f.emitted_total[axis])*1e-6))throw Error("region reaction budget does not close");
    if(state.completed_tick!==190||state.recipe.steps.length!==62||Math.abs(state.stats.volume_cells*0.0625-2048)>0.04||
       state.stats.max_speed<=0||f.applied_total[0]<=0)throw Error("a region did not advance its real field");
    const expected=native;
    if(expected){
     // The contract promises bitwise replay within one execution environment. Across Metal
     // and browser WebGPU, compare vector error on the emitted-momentum scale (8 f32 eps),
     // including near-cancelling components. Keep exact equality as a reported measurement.
     const scale=Math.max(1,Math.hypot(...expected[r]["WaterFeedback.emitted_total"]),Math.hypot(...f.emitted_total));
     for(const key of ["applied_total","emitted_total","rejected_total","residual_total"]){
      const value=expected[r]["WaterFeedback."+key];
      nativeExact=nativeExact&&JSON.stringify(f[key])===JSON.stringify(value);
      const error=Math.hypot(...f[key].map((v,axis)=>v-value[axis]))/scale;
      nativeRelativeError=Math.max(nativeRelativeError,error);
      if(!Number.isFinite(error)||error>8*2**-23)throw Error("native/browser reaction differs "+JSON.stringify({region:r,key,native:value,browser:f[key],relative_error:error}));
     }
    }
    regions.push({region:r,volume_m3:state.stats.volume_cells*0.0625,max_speed:state.stats.max_speed,
      applied:f.applied_total,emitted:f.emitted_total,rejected:f.rejected_total,residual:f.residual_total,pending,closure});
   }
   const motion=${JSON.stringify(nativeMotion)};let bodyError=null;
   if(motion){
    const poses=await g.command("world.query",{with:["Floater","Transform"],limit:4294967295,
     fields:["Transform.position","Transform.rotation","Velocity.linear","Velocity.angular","Floater.submerged"]});
    if(poses.length!==64||motion.bodies.length!==64)throw Error("multi-region body count differs");
    bodyError={position:0,rotation:0,velocity:0,angular:0,submerged:0};
    for(let n=0;n<64;n++){
     const actual=poses[n],expected=motion.bodies[n];if(actual.id!==expected.id)throw Error("native body identity differs");
     for(const [name,field] of Object.entries({position:"Transform.position",rotation:"Transform.rotation",velocity:"Velocity.linear",angular:"Velocity.angular",submerged:"Floater.submerged"})){
      const a=actual[field],b=expected[name];const error=Array.isArray(a)?Math.max(...a.map((v,i)=>Math.abs(v-b[i]))):Math.abs(a-b);
      bodyError[name]=Math.max(bodyError[name],error);
     }
    }
    // The same 1e-4 cross-API physical-state budget used by the existing feedback check.
    if(Object.values(bodyError).some(v=>!Number.isFinite(v)||v>1e-4))throw Error("native/browser body state differs: "+JSON.stringify(bodyError));
   }
   const restore=await g.command("snapshots.restore",{tick:120});
   if(restore.restored!==120||(await g.command("step",{ticks:70})).world_hash!==done.world_hash)
    throw Error("multi-region restore/replay diverged");
   await new Promise(r=>setTimeout(r,80));
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error("multi-region host/presenter problem");
   runs.push({run,hash:done.world_hash,frames,regions,native_exact:nativeExact,
    native_max_relative_momentum_error:native?nativeRelativeError:null,native_body_max_abs_errors:bodyError});
  }finally{g.close();}
 }
 // Compare genuinely different region inputs against isolated actual worker executions.
 // Identical copied lakes alone would not expose a cross-region parameter-buffer alias.
 const variedRun=async(indices)=>{
  const input=structuredClone(pkg);
  input.scene.entities=input.scene.entities.filter(e=>!e.components.Floater&&!e.components.WaterRegion);
  for(const r of indices){
   const water=structuredClone(lake);water.name="Variant"+r;
   water.components.Transform={position:[r*40,0,0]};
   const region=water.components.WaterRegion;const width=32+16*r,height=32;
   region.size=[width,height];region.friction=.01*r;region.substeps=r+1;region.tiles=[];
   for(let z=0;z<height/16;z++)for(let x=0;x<width/16;x++)region.tiles.push({key:[x,z],
    bed:Array(256).fill(0),
    depth:Array.from({length:256},(_,n)=>x*16+n%16<width/2?2+r*.125:1+r*.0625),
    u:Array.from({length:256},(_,n)=>x*16+n%16>0?.03*(r+1):0),
    w:Array.from({length:256},(_,n)=>z*16+Math.floor(n/16)>0?-.02*(r+1):0)});
   input.scene.entities.push(water);
  }
  const g=a.start(module,new TextEncoder().encode(JSON.stringify(input)),{pacing:"stepped",seed:1});
  try{
   await g.ready;const done=await g.command("step",{ticks:70});
   if(done.errors.length)throw Error("varied worker tick failed");
   const values=[];
   for(const r of indices){
    const s=(await g.command("world.get",{entity:"Variant"+r,fields:["WaterResident"]})).fields.WaterResident;
    if(s.completed_tick!==70||s.recipe.steps.length!==6)throw Error("varied worker skipped checkpoint or steps");
    values.push({digest:s.recipe.digest,stats:s.stats,boundary:s.boundary});
   }
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error("varied worker problem");
   return values;
  }finally{g.close();}
 };
 const varied=await variedRun([0,1,2,3]);
 for(let r=0;r<4;r++){
  const single=(await variedRun([r]))[0];
  if(JSON.stringify(varied[r])!==JSON.stringify(single))
   throw Error("batch differs from isolated region "+r+": "+JSON.stringify({batch:varied[r],single}));
 }
 const bindings=await import("/pkg/amoris_web.js");
 const rawTransfer=async(payload,count)=>{
 const raw=new bindings.GameWorker(payload,JSON.stringify({pacing:"stepped",seed:1}));
 const host=new bindings.BrowserWater();let jobs=0,regionJobs=0,maxBatch=0,diagnostics,reply;
 const flush=()=>{for(const m of JSON.parse(raw.messages())){
  if(m.t==="snap"){raw.snap_bytes();raw.ack(m.version);}
  if(m.t==="reply"){if(m.error)throw Error(JSON.stringify(m.error));reply=m;}
 }raw.render_frame();};
 try{
  flush();raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:8})}));
  for(let guard=0;guard<128;guard++){
   const next=JSON.parse(raw.run()).next;flush();if(next==="command")break;
   if(next==="external"){
    const job=raw.take_water_job();if(!job||job.kind!=="resident")throw Error("resident job missing");
    const regions=job.region_count;
    const expected=count===17?(jobs%2===0?16:1):count;
    if(regions!==expected)throw Error("resident cohort was not prepared together: "+JSON.stringify({job:jobs,regions,expected}));
    regionJobs+=regions;maxBatch=Math.max(maxBatch,regions);
    const result=await host.advance(job);jobs++;
    if(!raw.complete_water_job(result))throw Error("multi-region result refused");
   }
  }
  if(reply?.seq!==1||reply.ok?.tick!==8||reply.ok.errors.length)
   throw Error("resident transfer step did not complete eight ticks: "+JSON.stringify(reply));
  const worldHash=reply.ok.world_hash;
  diagnostics=JSON.parse(host.diagnostics()).resident;
  if(count===17){
   if(jobs!==16||regionJobs!==136||maxBatch!==16||diagnostics.uploads!==31||diagnostics.cache_evictions!==15||
      diagnostics.field_readbacks!==0||diagnostics.cached_regions!==16||diagnostics.cached_columns!==16*16384||
      diagnostics.cache_capacity!==16||diagnostics.cache_column_budget!==1048576||diagnostics.small_readbacks!==167||
      diagnostics.sync_groups!==16||diagnostics.step_groups!==16||diagnostics.checkpoint_groups!==0||diagnostics.max_group_regions!==16)
    throw Error("17-region bounded cache or whole-group pinning changed: "+JSON.stringify({jobs,region_jobs:regionJobs,max_batch:maxBatch,diagnostics}));
  }else{
   if(jobs!==8||regionJobs!==count*8||diagnostics.uploads!==count||diagnostics.field_readbacks!==0||diagnostics.cached_regions!==count||
     diagnostics.cache_evictions!==0||diagnostics.cache_capacity!==16||diagnostics.cache_column_budget!==1048576||diagnostics.cached_columns!==count*16384)
    throw Error("alternating regions rebuilt their GPU caches: "+JSON.stringify({jobs,diagnostics}));
   if(diagnostics.small_readbacks!==count*9||diagnostics.sync_groups!==1||diagnostics.step_groups!==8||
     diagnostics.checkpoint_groups!==0||diagnostics.max_group_regions!==count)
    throw Error("resident reads were not grouped: "+JSON.stringify(diagnostics));
  }
  const states=[];
  if(count===17){
   for(let r=0;r<count;r++){
    const seq=r+2;reply=null;
    raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq,name:"world.get",
     params:JSON.stringify({entity:"Transfer17_"+r,fields:["WaterResident"]})}));
    for(let guard=0;guard<8;guard++){
     const next=JSON.parse(raw.run()).next;flush();
     if(reply?.seq===seq)break;
     if(next==="external")throw Error("reading 17-region state unexpectedly requested GPU work");
    }
    const state=reply?.seq===seq?reply.ok?.fields?.WaterResident:null;
    if(!state||state.completed_tick!==8||state.recipe.steps.length!==8||state.stats.max_speed<=0)
     throw Error("17-region state did not reach the current tick: "+JSON.stringify({region:r,reply}));
    states.push({region:r,current_tick:state.completed_tick,digest:state.recipe.digest,stats:state.stats,boundary:state.boundary});
   }
   if(new Set(states.map(s=>JSON.stringify(s.digest))).size!==17)
    throw Error("distinct 17-region inputs lost their independent field digests");
  }
  let cancellation=null;
  if(count===4){
   raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:2,name:"step",params:JSON.stringify({ticks:1})}));
   if(JSON.parse(raw.run()).next!=="external")throw Error("batch cancellation did not suspend");
   const job=raw.take_water_job();if(job.region_count!==count)throw Error("cancelled job lost regions");
   const pending=host.advance(job);await Promise.resolve();host.abort();
   if(!raw.complete_water_job(await pending))throw Error("cancelled active token was lost");
   raw.run();const messages=JSON.parse(raw.messages());
   const fault=messages.find(m=>m.t==="reply"&&m.seq===2)?.error;
   const halted=messages.find(m=>m.t==="snap");
   if(fault?.code!=="water.compute_failed"||halted?.header.tick!==8||!halted.time.halted)
    throw Error("batch cancellation published a partial tick: "+JSON.stringify(messages));
   cancellation={code:fault.code,snapshot_tick:halted.header.tick,regions:count};
  }
 return {jobs,region_jobs:regionJobs,max_batch:maxBatch,diagnostics,cancellation,world_hash:worldHash,states};
 }finally{raw.close();raw.free();host.free();}
 };
 const transfer=await rawTransfer(bytes,4);
 const fifth=structuredClone(pkg);const extra=structuredClone(lake);extra.name="Lake4";extra.components.Transform={position:[160,0,0]};fifth.scene.entities.push(extra);
 const fiveTransfer=await rawTransfer(new TextEncoder().encode(JSON.stringify(fifth)),5);
 const rawTransfer17=async()=>{
  const input=structuredClone(pkg);
  input.scene.entities=input.scene.entities.filter(e=>!e.components.Floater&&!e.components.WaterRegion);
  for(let r=0;r<17;r++){
   const water=structuredClone(lake);water.name="Transfer17_"+r;
   water.components.Transform={position:[r*40,0,0]};delete water.components.WaterFeedback;
   const region=water.components.WaterRegion;
   region.size=[128,128];region.friction=.005*r;region.substeps=1+r%3;region.tiles=[];
   for(let z=0;z<8;z++)for(let x=0;x<8;x++)region.tiles.push({key:[x,z],bed:Array(256).fill(0),
    depth:Array.from({length:256},(_,n)=>x*16+n%16<64?2+r*.0625:1+r*.03125),
    u:Array.from({length:256},(_,n)=>x*16+n%16>0?.025*(r+1):0),
    w:Array.from({length:256},(_,n)=>z*16+Math.floor(n/16)>0?-.015*(r+1):0)});
   input.scene.entities.push(water);
  }
  return rawTransfer(new TextEncoder().encode(JSON.stringify(input)),17);
 };
 const seventeenTransfer=await rawTransfer17();const seventeenRepeat=await rawTransfer17();
 if(seventeenTransfer.diagnostics.environment!==seventeenRepeat.diagnostics.environment||
    seventeenTransfer.world_hash!==seventeenRepeat.world_hash||JSON.stringify(seventeenTransfer.states)!==JSON.stringify(seventeenRepeat.states))
  throw Error("17-region state did not repeat in the same execution environment");
 // Rebuild the long checkpoint history one step at a time on a fresh GPU service.
 // These raw hosts never push frames into the viewport's replica cache. Also reject the
 // experimental replay source arena, which the production resource-reuse path does not use.
 const formerReplayArenaBytes=1024*1024;
 const captureColdRebuild=async(advance)=>{
  const devicePrototype=GPUDevice.prototype,queuePrototype=GPUQueue.prototype;
  const methods=[[devicePrototype,"createBuffer"],[queuePrototype,"writeBuffer"],[queuePrototype,"submit"]];
  const descriptors=methods.map(([prototype,name])=>Object.getOwnPropertyDescriptor(prototype,name));
  if(descriptors.some(d=>typeof d?.value!=="function"||(!d.configurable&&!d.writable)))
   throw Error("WebGPU prototypes cannot record stepwise cold reconstruction");
  const [createBuffer,writeBuffer,submit]=descriptors.map(d=>d.value);
  const sources=new Map(),submissions=new Map(),queueIds=new Map();
  const queueId=queue=>{
   if(!queueIds.has(queue))queueIds.set(queue,queueIds.size);
   return queueIds.get(queue);
  };
  const copyUsage=GPUBufferUsage.COPY_SRC|GPUBufferUsage.COPY_DST;
  try{
   Object.defineProperty(devicePrototype,"createBuffer",{...descriptors[0],value:function(descriptor){
    const buffer=createBuffer.call(this,descriptor);
    const labelled=descriptor.label==="water replay inputs";
    if(labelled||(descriptor.size===formerReplayArenaBytes&&descriptor.usage===copyUsage))
     sources.set(buffer,{identified_by:labelled?"label":"size_usage",size:descriptor.size,usage:descriptor.usage,uploads:[],queue:null});
    return buffer;
   }});
   Object.defineProperty(queuePrototype,"writeBuffer",{...descriptors[1],value:function(buffer,bufferOffset,data,dataOffset,size){
    const result=writeBuffer.apply(this,arguments);
    const source=sources.get(buffer);
    if(source){
     // WebGPU uses typed-array elements for dataOffset/size, and bytes for ArrayBuffer
     // and DataView. wgpu's current browser binding supplies a Uint8Array.
     const elementBytes=ArrayBuffer.isView(data)?(data.BYTES_PER_ELEMENT??1):1;
     const offset=(dataOffset??0)*elementBytes;
     const bytes=size===undefined?data.byteLength-offset:size*elementBytes;
     if(!Number.isSafeInteger(bytes)||bytes<=0||offset<0||offset+bytes>data.byteLength)
      throw Error("invalid experimental replay source upload: "+JSON.stringify({bufferOffset,offset,bytes}));
     if(source.queue&&source.queue!==this)throw Error("replay source upload changed GPU queues");
     source.queue=this;source.queue_id=queueId(this);
     source.uploads.push({bytes,after_submit:submissions.get(this)??0});
    }
    return result;
   }});
   Object.defineProperty(queuePrototype,"submit",{...descriptors[2],value:function(){
    const result=submit.apply(this,arguments);
    queueId(this);submissions.set(this,(submissions.get(this)??0)+1);
    return result;
   }});
   const result=await advance();
   const arenas=[...sources.values()].map(({queue,...source})=>({...source,queue_submissions:submissions.get(queue)??0}));
   const sourceUploadBytes=arenas.reduce((sum,source)=>sum+source.uploads.reduce((n,u)=>n+u.bytes,0),0);
   if(arenas.length||sourceUploadBytes!==0)
    throw Error("stepwise cold reconstruction created an experimental replay source arena: "+JSON.stringify(arenas));
   return {result,evidence:{arenas,source_upload_bytes:sourceUploadBytes,
    queue_submissions:[...submissions.values()].reduce((sum,n)=>sum+n,0),
    queues:[...submissions].map(([queue,count])=>({queue_id:queueId(queue),submissions:count,
     source_arenas:arenas.flatMap((source,n)=>source.queue_id===queueId(queue)?[n]:[])}))}};
  }finally{
   methods.forEach(([prototype,name],n)=>Object.defineProperty(prototype,name,descriptors[n]));
  }
 };
 const coldReplayPackage=structuredClone(pkg);
 coldReplayPackage.scene.entities=coldReplayPackage.scene.entities.filter(e=>!e.components.Floater&&!e.components.WaterRegion);
 const replayBodies=128,replayPoints=template.components.Floater.points.length;
 if(replayPoints!==27||replayBodies*replayPoints>4096)throw Error("cold replay fixture exceeds the real floater query budget");
 for(let r=0;r<4;r++){
  const water=structuredClone(lake);water.name="ReplayLake"+r;
  water.components.Transform={position:[r*40,0,0]};
  if(JSON.stringify(water.components.WaterRegion.size)!=="[128,128]"||!water.components.WaterFeedback)
   throw Error("cold replay fixture needs authored 128-square feedback fields");
  coldReplayPackage.scene.entities.push(water);
  for(let n=0;n<replayBodies;n++){
   const body=structuredClone(template);body.name="ReplayFloat"+r+"_"+n;
   body.components.Transform.position=[r*40+3.5+(n%16)*1.5,2.09,3.5+Math.floor(n/16)*2.5];
   coldReplayPackage.scene.entities.push(body);
  }
 }
 const coldReplayBytes=new TextEncoder().encode(JSON.stringify(coldReplayPackage));
 const rawReplayRun=async(cold)=>{
  const raw=new bindings.GameWorker(coldReplayBytes,JSON.stringify({pacing:"stepped",seed:1}));
  let host=new bindings.BrowserWater(),sequence=0,reply=null,jobs=0,rebuildEvidence=null;
  const flush=()=>{for(const message of JSON.parse(raw.messages())){
   if(message.t==="snap"){raw.snap_bytes();raw.ack(message.version);}
   if(message.t==="reply"){
    if(message.error)throw Error("cold replay command failed: "+JSON.stringify(message.error));
    reply=message;
   }
  }raw.render_frame();};
  const command=async(name,params,recordInputs=false)=>{
   const seq=++sequence;reply=null;
   raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq,name,params:JSON.stringify(params)}));
   for(let guard=0;guard<2048;guard++){
    const next=JSON.parse(raw.run()).next;flush();
    if(reply?.seq===seq){
     if(reply.ok?.errors?.length)throw Error("cold replay tick reported errors: "+JSON.stringify(reply.ok.errors));
     return reply.ok;
    }
    if(next==="external"){
     const job=raw.take_water_job();
     if(!job||job.kind!=="resident"||job.region_count!==4)throw Error("cold replay lost its four-region resident cohort");
     let result;
     if(recordInputs){
      if(rebuildEvidence)throw Error("one cold replay tick unexpectedly requested multiple batches");
      const captured=await captureColdRebuild(()=>host.advance(job));
      result=captured.result;rebuildEvidence=captured.evidence;
     }else result=await host.advance(job);
     jobs++;
     if(!raw.complete_water_job(result))throw Error("cold replay GPU result refused");
    }else if(next==="command")throw Error("cold replay command ended without a reply");
   }
   throw Error("cold replay command exceeded its bounded driver loop");
  };
  const states=async(tick)=>{
   const values=[];
   for(let r=0;r<4;r++){
    const row=(await command("world.get",{entity:"ReplayLake"+r,fields:["WaterResident","WaterFeedback"]})).fields;
    const state=row.WaterResident,feedback=row.WaterFeedback;
    if(state.completed_tick!==tick||state.recipe.steps.length!==tick-64||!state.recipe.environment||
       state.stats.max_speed<=0||feedback.applied_total[0]<=0||!feedback.pending.length)
     throw Error("cold replay fixture lost its checkpoint history or physical feedback: "+JSON.stringify({region:r,tick:state.completed_tick,steps:state.recipe.steps.length}));
    values.push({region:r,completed_tick:state.completed_tick,recipe_steps:state.recipe.steps.length,
     environment:state.recipe.environment,digest:state.recipe.digest,stats:state.stats,boundary:state.boundary,feedback});
   }
   return values;
  };
  try{
   flush();let checkpointBefore=null;
   if(cold){
    const initial=await command("step",{ticks:120});
    if(initial.tick!==120)throw Error("cold replay did not reach its service replacement boundary");
    checkpointBefore=await states(120);
    const before=JSON.parse(host.diagnostics()).resident;
    if(before.field_readbacks!==4||before.cache_evictions!==0)
     throw Error("cold replay did not retain one tick-64 checkpoint per region: "+JSON.stringify(before));
    host.free();host=new bindings.BrowserWater();
   }
   const done=await command("step",{ticks:cold?1:121},cold);
   if(done.tick!==121||jobs!==121)throw Error("cold replay did not complete exactly 121 resident ticks");
   const values=await states(121),diagnostics=JSON.parse(host.diagnostics()).resident;
   if(cold&&(!rebuildEvidence||diagnostics.uploads!==4||diagnostics.small_readbacks!==8||diagnostics.field_readbacks!==0||
      diagnostics.sync_groups!==1||diagnostics.step_groups!==1||diagnostics.checkpoint_groups!==0||diagnostics.max_group_regions!==4))
    throw Error("fresh GPU service did not replay and authenticate all four fields: "+JSON.stringify(diagnostics));
   return {world_hash:done.world_hash,states:values,diagnostics,rebuild_evidence:rebuildEvidence,
    checkpoint_before:checkpointBefore?.map(s=>({region:s.region,completed_tick:s.completed_tick,recipe_steps:s.recipe_steps,digest:s.digest}))};
  }finally{raw.close();raw.free();host.free();}
 };
 const warmReplay=await rawReplayRun(false),coldReplay=await rawReplayRun(true);
 if(warmReplay.world_hash!==coldReplay.world_hash||JSON.stringify(warmReplay.states)!==JSON.stringify(coldReplay.states))
  throw Error("stepwise long-history cold reconstruction differs from uninterrupted GPU execution");
 const stepwiseColdRebuild={world_hash:coldReplay.world_hash,exact:true,regions:4,bodies_per_region:replayBodies,
  query_points_per_region:replayBodies*replayPoints,checkpoint_before:coldReplay.checkpoint_before,
  states:coldReplay.states.map(({feedback,...state})=>({...state,feedback:{
   applied_total:feedback.applied_total,emitted_total:feedback.emitted_total,rejected_total:feedback.rejected_total,
   residual_total:feedback.residual_total,pending_packets:feedback.pending.length}})),
  warm_diagnostics:warmReplay.diagnostics,cold_diagnostics:coldReplay.diagnostics,rebuild_evidence:coldReplay.rebuild_evidence};
 document.getElementById("hud").style.display="none";
 return{passed:true,runs,varied_isolated_exact:varied,transfer,five_transfer:fiveTransfer,
  seventeen_transfer:seventeenTransfer,seventeen_repeat:{world_hash:seventeenRepeat.world_hash,environment:seventeenRepeat.diagnostics.environment,exact:true},
  stepwise_cold_rebuild:stepwiseColdRebuild};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{
 encoding:"utf8",timeout:300000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"regions.png")}
});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));
const result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("multi-region check failed; inspect "+out);
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");
console.log(JSON.stringify(result,null,2));
