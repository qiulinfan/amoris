// Translating shear in the actual Web Worker/BrowserWater path, including sparse observations.
// Build game/viewport and samples/water-feedback; serve web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,readFileSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_ADVECTION_OUT||"out/water-advection/browser-quality");mkdirSync(out,{recursive:true});
const native=process.env.WATER_ADVECTION_NATIVE
 ? readFileSync(resolve(process.env.WATER_ADVECTION_NATIVE,"shear.csv"),"utf8").trim().split("\n").slice(1).map(l=>Number(l.split(",")[3])) : null;
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const a=await import("/amoris.js");const {module}=await a.loadModule();
 const pkg=await(await fetch("/projects/samples-water-feedback/package.json")).json();
 pkg.rate=50;pkg.scene.entities=pkg.scene.entities.filter(e=>!e.components.Floater);
 pkg.scene.entities[0].components.Voxels.cell_m=2;
 const lake=pkg.scene.entities.find(e=>e.components.WaterRegion);delete lake.components.WaterFeedback;
 const r=lake.components.WaterRegion;r.cell_m=1;r.size=[128,128];r.substeps=2;r.friction=0;r.tiles=[];
 for(let z=0;z<8;z++)for(let x=0;x<8;x++){
  const u=[],w=[];
  for(let n=0;n<256;n++){const i=x*16+n%16,k=z*16+Math.floor(n/16);
   u.push(i===0?0:0.4*Math.sin(Math.PI*2*(k+0.5)/8));w.push(k===0?0:1);
  }
  r.tiles.push({key:[x,z],bed:Array(256).fill(0),depth:Array(256).fill(2),u,w});
 }
 for(let k=32;k<96;k++)pkg.scene.entities.push({name:"Probe"+k,components:{WaterProbe:{region:2,at:[64.5,k+0.5]}}});
 const bytes=new TextEncoder().encode(JSON.stringify(pkg)),results=[];let previous;
 for(let run=0;run<2;run++){
  const g=a.start(module,bytes,{pacing:"stepped",seed:1});const hashes=[];let frames=0;
  g.onTicks(items=>hashes.push(...items));g.onRender(b=>{frames++;if(!window.amorisViewport.pushFrame(b))throw Error("advection frame refused");});
  try{
   await g.ready;const done=await g.command("step",{ticks:60});
   if(done.errors.length)throw Error(JSON.stringify(done.errors));
   if(previous&&JSON.stringify(previous)!==JSON.stringify(hashes))throw Error("advection hashes did not repeat");previous=hashes.map(h=>[...h]);
   const water=(await g.command("world.get",{entity:"Lake",fields:["WaterResident"]})).fields.WaterResident;
   if(water.completed_tick!==60||water.recipe.steps.length!==60||Math.abs(water.stats.volume_cells-32768)>0.04||
      water.recipe.steps.some(s=>s.substeps!==2||Math.abs(s.substep_dt-0.01)>1e-12))throw Error("shear timestep, mass or current field differs");
   let amplitude=0.4,cross=1,distance=0,error=0,norm=0,nativeError=0;const profile=[];
   const dt=Math.fround(0.01);for(let n=0;n<120;n++){distance+=cross*dt;amplitude/=1+0.2*dt;cross/=1+0.1*dt/cross;}
   const native=${JSON.stringify(native)};
   for(let k=32;k<96;k++){
    const p=(await g.command("world.get",{entity:"Probe"+k,fields:["WaterProbe"]})).fields.WaterProbe;
    if(!p.ready||p.tick!==60||Math.abs(p.depth-2)>1e-4||Math.abs(p.velocity[1]-cross)>1e-4||
       JSON.stringify(p.state_digest)!==JSON.stringify(water.recipe.digest))throw Error("sparse shear observation stale or invalid");
    const expected=amplitude*Math.sin(Math.PI*2*(k+0.5-distance)/8);error+=(p.velocity[0]-expected)**2;norm+=expected**2;
    if(native)nativeError=Math.max(nativeError,Math.abs(p.velocity[0]-native[k-32]));
    profile.push([k+0.5,expected,p.velocity[0]]);
   }
   const relativeL2=Math.sqrt(error/norm);
   if(relativeL2>=0.16||native&&nativeError>5e-6)throw Error("browser shear accuracy differs: "+JSON.stringify({relativeL2,nativeError}));
   const restored=await g.command("snapshots.restore",{tick:0});
   if(restored.restored!==0||(await g.command("step",{ticks:60})).world_hash!==done.world_hash)throw Error("shear restore/replay diverged");
   await new Promise(r=>setTimeout(r,80));
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error("shear host/presenter problem");
   results.push({run,hash:done.world_hash,frames,relative_l2:relativeL2,native_max_abs_error:native?nativeError:null,volume_m3:water.stats.volume_cells,profile});
  }finally{g.close();}
 }
 document.getElementById("hud").style.display="none";return{passed:true,results};
})()`;
const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:180000,maxBuffer:8*1024*1024,
 env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,"shear.png")}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: ")),result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("advection worker check failed; inspect "+out);
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
