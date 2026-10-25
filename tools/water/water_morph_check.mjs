// Actual browser frames, GPU-owned transition states and bounded diagnostics.
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_MORPH_OUT||"out/water-morph/browser-morph");
mkdirSync(out,{recursive:true});
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const results=[];
for(const side of [128,512]){
 const expression=`(async()=>{
  const p=window.amorisViewport.raw;
  const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
  if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
  p.set_water_culling(true);p.demo_water_mesh(${side},"full","full",0);p.set_lod("on");
  const wait=()=>new Promise(r=>setTimeout(r,24)),rows=[];
  const check=async()=>{const s=JSON.parse(await p.water_mesh_stats())[0]?.stats;
   if(!s||!Number.isFinite(s.max_error_pixels)||s.max_error_pixels>1.0001||s.active_triangles>s.submitted_triangles||s.submitted_triangles>s.capacity_triangles)throw Error("unbounded GPU morph: "+JSON.stringify(s));return s;};
  await wait();await wait();
  const c=${side}*0.125,e=${side}*0.25;
  for(let n=0;n<54;n++){
   const f=1-n*0.014;p.set_camera_look(c,e*0.9*f,c+e*0.8*f,c,0,c,0,1,0,60,0.05,0);
   await wait();rows.push(await check());
  }
  if(!rows.some(s=>s.transition_patches>0))throw Error("camera sweep never exercised bridge geometry");
  await new Promise(r=>setTimeout(r,800));const held=await check();
  if(held.transition_patches!==0)throw Error("held camera did not finish its transition");
  p.set_camera_look(c,0.5,c,c+e*0.1,0.7,c+e*0.1,0,1,0,60,0.05,0);
  await wait();await wait();const under=await check();
  if(under.transition_patches||under.levels.slice(1).some(n=>n))throw Error("medium exclusion retained a morph");
  return{passed:true,side:${side},rows,held,under};
 })()`;
 const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:90000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,VIEWPORT:"1280x720",VSYNC:"1",UNSAFE_WEBGPU:"0",LOGS:"1"}});
 writeFileSync(resolve(out,`${side}.log`),log);
 const line=log.split("\n").find(l=>l.startsWith("eval: ")),data=line?JSON.parse(line.slice(6)):null;
 if(!data?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("actual browser morph failed: "+side);
 results.push(data);
}
const result={passed:true,results};
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");
console.log(JSON.stringify(result,null,2));
