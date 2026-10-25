// Actual WebGPU viewport conversion of nonzero outside faces, with a sensitive CPU reference.
// Export fixtures with WATER_FACE_FIXTURE and the renderer's complete_outside_faces test.
// Build the water_lab example for its CPU-only PNG comparison command.
import {execFileSync} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_FACES_OUT||"out/water-boundaries/browser-faces");mkdirSync(out,{recursive:true});
const frames=process.argv[3]||"http://127.0.0.1:8097/face-fixture";
const url=process.argv[2]||"http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
// WebGPU's canvas bitmap may already be cleared after presentation. Capture through the
// compositor, as the existing browser image checks do, rather than comparing empty bitmap copies.
for(const fixture of ["complete","centred","missing"]){
 const expression=`(async()=>{
  const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
  if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
  window.amorisViewport.raw.set_camera_look(34,4,34,30.5,1,30.5,0,1,0,60,0.1,0);
  const response=await fetch(${JSON.stringify(frames+"/"+fixture+".bin")});if(!response.ok)throw Error("face fixture missing");
  if(!window.amorisViewport.pushFrame(new Uint8Array(await response.arrayBuffer())))throw Error("complete MAC frame refused");
  document.getElementById("hud").style.display="none";await new Promise(r=>setTimeout(r,250));
  return{passed:true,fixture:${JSON.stringify(fixture)}};
 })()`;
 const log=execFileSync(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:60000,maxBuffer:8*1024*1024,
  env:{...process.env,EVAL:expression,LOGS:"1",UNSAFE_WEBGPU:"0",VSYNC:"1",VIEWPORT:"1280x720",SHOT:resolve(out,fixture+".png")}});
 writeFileSync(resolve(out,fixture+".log"),log);
 const line=log.split("\n").find(l=>l.startsWith("eval: ")),result=line?JSON.parse(line.slice(6)):null;
 if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("complete face viewport failed; inspect "+out);
}
const executable=process.env.WATER_IMAGE_COMPARE_EXE||resolve("target/release/examples/water_lab"+(process.platform==="win32"?".exe":""));
const compare=name=>JSON.parse(execFileSync(executable,["--compare",resolve(out,"complete.png"),resolve(out,name+".png")],{encoding:"utf8"}));
const reference=compare("centred"),calibration=compare("missing");
if(reference.max_abs_rgb!==0||calibration.max_abs_rgb===0||calibration.changed_gt8===calibration.width*calibration.height)throw Error("outside face comparison differs, is insensitive, or changes the whole background: "+JSON.stringify({reference,calibration}));
const result={passed:true,reference,calibration};
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
