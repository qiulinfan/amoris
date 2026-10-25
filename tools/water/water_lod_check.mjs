// Actual baseline-WebGPU LOD, current diagnostics and asynchronous reset safety.
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_LOD_OUT||"out/water-lod/browser");mkdirSync(out,{recursive:true});
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const cases=[[512,"full","full"],[512,"full","near"],[128,"full","under"],[128,"sparse","full"],[128,"cliff","full"],[128,"blocky","full"]],results=[];
for(const [side,field,camera] of cases){
 const runs=[];const images=[];
 for(const on of [false,true]){
  const key=`${side}-${field}-${camera}-${on}`,shot=resolve(out,key+".png");images.push(shot);
  const expression=`(async()=>{
   const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
   if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
   const p=window.amorisViewport.raw;p.set_water_culling(true);p.demo_water_mesh(${side},${JSON.stringify(field)},${JSON.stringify(camera)},0);p.set_lod(${JSON.stringify(on?"on":"off")});
   await new Promise(r=>setTimeout(r,200));const regions=JSON.parse(await p.water_mesh_stats());const s=regions[0]?.stats;
   if(!s||s.levels.reduce((a,b)=>a+b,0)!==s.visible||!Number.isFinite(s.max_error_pixels)||s.max_error_pixels>1.0001||s.active_triangles>s.submitted_triangles||s.submitted_triangles>s.capacity_triangles)throw Error("invalid actual LOD counters: "+JSON.stringify(regions));
   const coarse=s.levels.slice(1).reduce((a,b)=>a+b,0);
   if(${on}&&${JSON.stringify(field)}==="full"&&${JSON.stringify(camera)}!=="under"&&coarse===0)throw Error("LOD did not run");
   if((!${on}||${JSON.stringify(camera)}==="under"||${JSON.stringify(field)}==="blocky")&&coarse!==0)throw Error("reference/underwater/Blocky detail changed");
   // The returned Promise must own its readback, so a live renderer remains usable before await.
   const pending=p.water_mesh_stats();p.demo_water_mesh(64,"full","full",0);const copied=JSON.parse(await pending);
   if(copied[0].stats.input_triangles!==s.input_triangles)throw Error("reset changed an already-copied diagnostic snapshot");
   // Restore the captured view after the reset-safety check.
   p.demo_water_mesh(${side},${JSON.stringify(field)},${JSON.stringify(camera)},0);p.set_lod(${JSON.stringify(on?"on":"off")});
   document.getElementById("hud").style.display="none";await new Promise(r=>setTimeout(r,200));
   return{passed:true,regions};
  })()`;
  const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:90000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,SHOT:shot,VIEWPORT:"1280x720",VSYNC:"1",UNSAFE_WEBGPU:"0",LOGS:"1"}});
  writeFileSync(resolve(out,key+".log"),log);const line=log.split("\n").find(l=>l.startsWith("eval: ")),data=line?JSON.parse(line.slice(6)):null;
  if(!data?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("actual browser LOD failed: "+key);
  runs.push({lod:on,...data});
 }
 const imageError=JSON.parse(execFileSync(process.env.WATER_IMAGE_COMPARE_EXE||"target/release/examples/water_lab",["--compare",...images],{encoding:"utf8"}));
 // Approximation error is established independently from actual GPU triangles in water_lod.rs.
 // These images record material/optics changes; exact cases retain exact comparison.
 if((camera==="under"||field==="blocky")&&imageError.max_abs_rgb!==0)throw Error("full-detail LOD exclusion changed browser pixels");
 results.push({side,field,camera,runs,image_error:imageError});
}
const result={passed:true,results};writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
