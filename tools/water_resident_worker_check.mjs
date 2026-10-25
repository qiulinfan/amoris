// Actual resident GPU game ticks, restoration, sparse transfers and cancellation in Chrome.
// SKIP_VIEWPORT=1 tools/build_web.sh samples/water-resident samples/water-dam
// Serve web/ on 8097, then node tools/water_resident_worker_check.mjs [viewport URL].
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const out=resolve(process.env.WATER_RESIDENT_WORKER_OUT||"out/water-resident-worker");mkdirSync(out,{recursive:true});
const expression=`(async()=>{
  const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
  if(!adapter || adapter.info?.isFallbackAdapter===true)throw new Error("hardware WebGPU required");
  const a=await import("/amoris.js");const {module}=await a.loadModule();
  const packageObject=await(await fetch("/projects/samples-water-resident/package.json")).json();
  const pkg=new TextEncoder().encode(JSON.stringify(packageObject));
  const summaries=[];let previous;
  const stats=values=>{const v=[...values].sort((a,b)=>a-b);return {median:v[Math.floor(v.length/2)],p95:v[Math.floor(v.length*.95)],max:v[v.length-1]};};
  for(let run=0;run<2;run++){
    const g=a.start(module,pkg,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
    g.onTicks((items,epochs)=>{items.forEach((h,i)=>hashes.push([epochs[i],...h]));});
    g.onRender(bytes=>{frames++;if(!window.amorisViewport.pushFrame(bytes))throw new Error("resident render feed refused");});
    try{
      await g.ready;const start=performance.now();
      const forward=await g.command("step",{ticks:130});
      const elapsed=performance.now()-start;
      const state=(await g.command("world.get",{entity:"Lake",fields:["WaterRegion","WaterResident"]})).fields;
      const probe=(await g.command("world.get",{entity:"Front",fields:["WaterProbe"]})).fields.WaterProbe;
      const water=state.WaterResident;
      if(state.WaterRegion.tiles.length!==0||water.recipe.steps.length!==2||!water.recipe.environment||
         water.completed_tick!==130||!probe.ready||probe.tick!==130||probe.depth<0.05||
         JSON.stringify(probe.state_digest)!==JSON.stringify(water.recipe.digest)||Math.abs(water.stats.volume_cells-1024)>0.03)
        throw new Error("resident world, checkpoint or current-tick observation invalid");
      if(previous && JSON.stringify(hashes)!==JSON.stringify(previous))throw new Error("resident worker hashes did not repeat");
      previous=hashes.map(h=>[...h]);
      const list=await g.command("snapshots.list");
      if(JSON.stringify(list.snapshots.map(s=>s.tick))!=="[0,60,120]")throw new Error("resident snapshots not retained");
      const restored=await g.command("snapshots.restore",{tick:60});
      if(restored.restored!==60||restored.epoch!==1)throw new Error("resident restore refused");
      const continued=await g.command("step",{ticks:70});
      if(continued.world_hash!==forward.world_hash)throw new Error("resident restored GPU field diverged");
      // The old tail and the restored timeline are all delivered with explicit epoch stamps.
      await new Promise(r=>setTimeout(r,50));
      const first=hashes.filter(h=>h[0]===0),again=hashes.filter(h=>h[0]===1);
      if(first.length!==131||again.length!==70||again.some(h=>h[2]!==first[h[1]][2]))throw new Error("restored hash stream lost or mixed epochs");
      if(JSON.parse(g.presenter.problems()).length)throw new Error("snapshot presenter rejected resident restoration");
      let sourceVolume;
      if(run===0){
        await g.command("world.edit",{label:"resident source",ops:[{spawn:{name:"Spring",components:{WaterSource:{region:2,at:[0.5,0.5],rate_m3s:6}}}}]});
        await g.command("step",{ticks:10});
        sourceVolume=(await g.command("world.get",{entity:"Lake",fields:["WaterResident"]})).fields.WaterResident.stats.volume_cells;
        if(Math.abs(sourceVolume-1025)>0.03)throw new Error("resident source did not apply once per tick");
      }
      summaries.push({run,elapsed_ms:elapsed,hash:forward.world_hash,volume:water.stats.volume_cells,
        probe_depth:probe.depth,frames,restored:restored.restored,source_volume:sourceVolume,snapshots:g.presenter.received(),max_unacked:g.maxUnacked});
    }finally{g.close();}
  }
  const bindings=await import("/pkg/amoris_web.js");
  // Resize the same authored problem without another compiled script or native fixture importer.
  const large=structuredClone(packageObject);large.scene.entities=large.scene.entities.slice(0,2);
  const r=large.scene.entities[1].components.WaterRegion;r.size=[512,512];r.tiles=[];
  for(let z=0;z<32;z++)for(let x=0;x<32;x++)r.tiles.push({key:[x,z],bed:Array(256).fill(0),
    depth:Array(256).fill(x<16?2:0),u:Array(256).fill(0),w:Array(256).fill(0)});
  for(let n=0;n<64;n++)large.scene.entities.push({name:"Probe"+n,components:{WaterProbe:{region:2,at:[(n%8)*64+0.5,Math.floor(n/8)*64+0.5]}}});
  const raw=new bindings.GameWorker(new TextEncoder().encode(JSON.stringify(large)),JSON.stringify({pacing:"stepped",seed:1}));
  const host=new bindings.BrowserWater();let snapshotBytes=0,snapshots=0,renderBytes=0,jobs=0,last;const sizes=[];
  const flush=()=>{for(const m of JSON.parse(raw.messages())){
    if(m.t==="snap"){const b=raw.snap_bytes();snapshotBytes+=b.length;sizes.push(b.length);snapshots++;raw.ack(m.version);}
    if(m.t==="reply"&&m.error)throw new Error(JSON.stringify(m.error));
  }const frame=raw.render_frame();if(frame)renderBytes+=frame.length;};
  flush();raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:1,name:"step",params:JSON.stringify({ticks:64})}));
  const start=performance.now();
  for(let guard=0;guard<512;guard++){
    const next=JSON.parse(raw.run()).next;flush();
    if(next==="command")break;
    if(next==="external"){
      const job=raw.take_water_job();if(!job||job.kind!=="resident"||job.region_count!==1)throw new Error("resident queue changed mode or region count");
      const result=await host.advance(job);last=JSON.parse(host.diagnostics()).resident;
      if(jobs<63 && last?.field_readbacks!==0)throw new Error("normal resident tick read back a full field");
      jobs++;if(!raw.complete_water_job(result))throw new Error("resident response rejected");
    }
  }
  if(jobs!==64||last.uploads!==1||last.field_readbacks!==1||last.small_readbacks!==65||last.readback_bytes!==4333632)
    throw new Error("resident transfer scope differs: "+JSON.stringify({jobs,last}));
  if(last.sync_groups!==1||last.step_groups!==64||last.checkpoint_groups!==1||last.max_group_regions!==1)
    throw new Error("single-region batch phase counts differ: "+JSON.stringify(last));
  const largeResult={side:512,probes:64,elapsed_ms:performance.now()-start,jobs,transfers:last,snapshot_bytes:snapshotBytes,snapshots,render_bytes:renderBytes,snapshot_size:stats(sizes)};
  // Cancel after the service is warm, when the next GPU map may race a completed callback.
  raw.push(JSON.stringify({t:"cmd",source:{developer:0},seq:2,name:"step",params:JSON.stringify({ticks:1})}));
  if(JSON.parse(raw.run()).next!=="external")throw new Error("resident cancellation did not suspend");
  const pending=host.advance(raw.take_water_job());await Promise.resolve();host.abort();
  raw.complete_water_job(await pending);raw.run();
  const failed=JSON.parse(raw.messages());const fault=failed.find(m=>m.t==="reply"&&m.seq===2)?.error;
  const halted=failed.find(m=>m.t==="snap");
  if(fault?.code!=="water.compute_failed"||halted?.header.tick!==64||!halted.time.halted)
    throw new Error("resident cancellation published an incomplete field: "+JSON.stringify(failed));
  raw.close();raw.free();host.free();
  const live=a.start(module,new TextEncoder().encode(JSON.stringify(large)),{pacing:"stepped",seed:1});
  let liveFrames=0;live.onRender(bytes=>{liveFrames++;window.amorisViewport.pushFrame(bytes);});
  window.amorisViewport.raw.set_camera_look(520,260,540,256,0,256,0,1,0,50,0.1,0);
  let interactive;
  try{
    await live.ready;const start=performance.now();const done=await live.command("step",{ticks:64});
    const elapsed=performance.now()-start;const perf=await live.perf();
    const state=(await live.command("world.get",{entity:"Lake",fields:["WaterResident"]})).fields.WaterResident;
    if(done.tick!==64||done.errors.length||Math.abs(state.stats.volume_cells-262144)>1||live.problems().length)
      throw new Error("512 resident worker/presenter failed");
    let rendered=0,counting=false;window.amorisViewport.onStats(()=>{if(counting)rendered++;});
    await live.command("time_control",{pacing:{real_time:{speed:1}},pause:false});
    const begin=performance.now();counting=true;await new Promise(r=>setTimeout(r,3000));
    await live.command("time_control",{pause:true});counting=false;
    const advanced=live.latest().tick-64;const span=performance.now()-begin;
    if(advanced<=0||rendered<=0)throw new Error("512 resident game/view made no progress");
    interactive={side:512,probes:64,elapsed_ms:elapsed,snapshot_bytes:live.snapshotBytes,
      frames:liveFrames,snapshots:live.presenter.received(),cold_active_ms:perf.step_ms[0],cold_external_ms:perf.external_ms[0],
      active_ms:stats(perf.step_ms.slice(1)),external_ms:stats(perf.external_ms.slice(1)),publish_ms:stats(perf.publish_ms.slice(1)),
      presenter_ms:stats(live.rebuildMs),real_time:{elapsed_ms:span,ticks_advanced:advanced,viewport_submissions:rendered}};
  }finally{live.close();}
  document.getElementById("hud").style.display="none";
  return {passed:true,summaries,large:largeResult,interactive,cancellation:{code:fault.code,snapshot_tick:halted.header.tick}};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:120000,maxBuffer:8*1024*1024,
  env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"resident.png")}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: "));const result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed || /\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw new Error("resident worker failed; inspect "+out+"/browser.log");
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
