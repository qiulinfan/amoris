// Exact actual-WebGPU culling comparison. Serve rebuilt web/ on 8097.
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_MESH_OUT||"out/water-mesh/browser");mkdirSync(out,{recursive:true});
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
const cases=[["full","full"],["full","local"],["full","under"],["sparse","near"],["cliff","near"],["blocky","near"],["dry","full"]];
const results=[];
for(const [field,camera] of cases){
 const images=[];
 for(const on of [false,true]){
  const key=`${field}-${camera}-${on}`,shot=resolve(out,key+".png");images.push(shot);
  const expression=`(async()=>{
   const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
   if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
   const p=window.amorisViewport.raw;p.set_water_culling(${on});p.demo_water_mesh(128,${JSON.stringify(field)},${JSON.stringify(camera)},0);
   document.getElementById("hud").style.display="none";await new Promise(r=>setTimeout(r,250));
   return{passed:true,field:${JSON.stringify(field)},camera:${JSON.stringify(camera)},culling:${on}};
  })()`;
  const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:60000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,SHOT:shot,VIEWPORT:"1280x720",VSYNC:"1",UNSAFE_WEBGPU:"0",LOGS:"1"}});
  writeFileSync(resolve(out,key+".log"),log);
  const line=log.split("\n").find(l=>l.startsWith("eval: "));
  if(!line||!JSON.parse(line.slice(6)).passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("water mesh browser execution failed: "+key);
 }
 const compare=JSON.parse(execFileSync(process.env.WATER_IMAGE_COMPARE_EXE||"target/release/examples/water_lab",["--compare",...images],{encoding:"utf8"}));
 if(compare.max_abs_rgb!==0)throw Error("water mesh culling changes browser pixels: "+JSON.stringify({field,camera,compare}));
 results.push({field,camera,compare});
}
const calibration=JSON.parse(execFileSync(process.env.WATER_IMAGE_COMPARE_EXE||"target/release/examples/water_lab",["--compare",resolve(out,"full-full-true.png"),resolve(out,"dry-full-true.png")],{encoding:"utf8"}));
if(calibration.changed_gt8<1000)throw Error("water mesh comparison did not see a water surface");
const result={passed:true,results,calibration};writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
