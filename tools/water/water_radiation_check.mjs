// Experimental WGSL on an actual baseline WebGPU device. No production solver integration.
import {execFile} from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {readFile} from "node:fs/promises";
import {createServer} from "node:http";
import {promisify} from "node:util";
import {resolve} from "node:path";
const out=resolve(process.env.WATER_RADIATION_OUT||"out/water-radiation/browser");mkdirSync(out,{recursive:true});
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:"high-performance"});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error("hardware WebGPU required");
 const device=await adapter.requestDevice({requiredFeatures:[]}),errors=[];let lost=null;
 device.lost.then(x=>{if(x.reason!=="destroyed")lost=x.message;});device.addEventListener("uncapturederror",e=>errors.push(e.error.message));device.pushErrorScope("validation");
 const data=await(await fetch("/radiation-fixture/fixture.json")).json();
 if(data.uniform_words!==152||data.fixtures.length!==16||data.fixtures.filter(f=>f.analytic!==null).length!==8)throw Error("regenerate all current radiation fixtures");
 const layout=n=>device.createBindGroupLayout({entries:Array.from({length:n},(_,i)=>({binding:i,visibility:GPUShaderStage.COMPUTE,buffer:{type:i===0?"uniform":i===1||i===3?"read-only-storage":"storage"}}))});
 const baseLayout=layout(7),macLayout=layout(9),boundaryLayout=layout(8);
 const pipelines=async(name,entries,binding)=>{
  const module=device.createShaderModule({code:await(await fetch("/radiation-fixture/"+name+".wgsl")).text()});
  const info=await module.getCompilationInfo();if(info.messages.some(m=>m.type==="error"))throw Error(JSON.stringify(info.messages));
  const p=device.createPipelineLayout({bindGroupLayouts:[binding]});return Promise.all(entries.map(entryPoint=>device.createComputePipelineAsync({layout:p,compute:{module,entryPoint}})));
 };
 const base=await pipelines("base",["donors","transport"],baseLayout),mac=await pipelines("mac",["mac_forward","mac_reverse","mac_faces"],macLayout),boundary=await pipelines("boundary",["boundary_predict","boundary_limit","boundary_reduce"],boundaryLayout);
 const make=(values,usage)=>{const b=device.createBuffer({size:Math.max(16,values.byteLength),usage:usage|GPUBufferUsage.COPY_DST});device.queue.writeBuffer(b,0,values);return b;};
 const results=[];
 for(const f of data.fixtures){
  const [nx,nz]=f.initial.size,count=nx*nz,faces=2*(nx+nz),initial=new Uint32Array([...f.initial.columns.flat(),...f.initial.far_faces]);
  if(initial.length!==count*4+nx+nz||f.parameters.some(p=>p.length!==20))throw Error("fixture buffer contract mismatch");
  const state=[make(initial,GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_SRC),make(initial,GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_SRC)];
  const params=make(new Uint32Array(data.uniform_words),GPUBufferUsage.UNIFORM),awake=make(new Uint32Array(count).fill(1),GPUBufferUsage.STORAGE),velocity=make(new Float32Array(count*2),GPUBufferUsage.STORAGE),scale=make(new Float32Array(count),GPUBufferUsage.STORAGE),flux=make(new Float32Array(faces*4),GPUBufferUsage.STORAGE),report=make(new Float32Array(8),GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_SRC),predicted=make(new Float32Array(count*2),GPUBufferUsage.STORAGE),reversed=make(new Float32Array(count*2),GPUBufferUsage.STORAGE);
  const group=(binding,buffers)=>device.createBindGroup({layout:binding,entries:buffers.map((b,i)=>({binding:i,resource:{buffer:b}}))});
  const common=state.map((b,i)=>[params,b,state[1-i],awake,velocity,scale,flux]),groups=common.map(b=>({base:group(baseLayout,b),mac:group(macLayout,[...b,predicted,reversed]),boundary:group(boundaryLayout,[...b,report])}));
  const stage=device.createBuffer({size:16+initial.byteLength,usage:GPUBufferUsage.MAP_READ|GPUBufferUsage.COPY_DST});let current=0,entered=0,left=0,actual;
  const v0=f.initial.columns.reduce((s,c)=>s+new Float32Array(new Uint32Array([c[1]]).buffer)[0],0);
  let massError=0;
  for(const p of f.parameters){
   device.queue.writeBuffer(params,0,new Uint32Array(p));const encoder=device.createCommandEncoder();encoder.clearBuffer(flux);encoder.clearBuffer(report);
   const dispatch=(pipeline,bind,x,y=1)=>{const pass=encoder.beginComputePass();pass.setPipeline(pipeline);pass.setBindGroup(0,bind);pass.dispatchWorkgroups(x,y);pass.end();};
   for(const pipeline of mac)dispatch(pipeline,groups[current].mac,Math.ceil(nx/8),Math.ceil(nz/8));
   dispatch(boundary[0],groups[current].boundary,Math.ceil(faces/64));dispatch(base[0],groups[current].base,Math.ceil(nx/8),Math.ceil(nz/8));dispatch(boundary[1],groups[current].boundary,Math.ceil(faces/256));dispatch(base[1],groups[current].base,Math.ceil(nx/8),Math.ceil(nz/8));current=1-current;dispatch(boundary[2],groups[current].boundary,1);
   encoder.copyBufferToBuffer(report,0,stage,0,16);encoder.copyBufferToBuffer(state[current],0,stage,16,initial.byteLength);device.queue.submit([encoder.finish()]);
   await stage.mapAsync(GPUMapMode.READ);const values=new Float32Array(stage.getMappedRange()).slice();stage.unmap();actual=values.slice(4);
   if([...values].some(x=>!Number.isFinite(x))||values[0]<0||values[1]<0||values[2]!==0||values[3]!==0)throw Error("invalid radiation flux/field");entered+=values[0];left+=values[1];
   let volume=0;const words=new Uint32Array(actual.buffer);for(let n=0;n<count;n++){if(words[n*4]!==initial[n*4])throw Error("boundary changed physical bed");if(actual[n*4+1]<0)throw Error("boundary emptied a cell");volume+=actual[n*4+1];}
   massError=Math.max(massError,Math.abs(volume-v0-entered+left));if(massError>0.0005)throw Error("independent boundary mass budget "+massError);
  }
  const expected=new Float32Array(new Uint32Array([...f.expected.columns.flat(),...f.expected.far_faces]).buffer);let depthError=0,velocityError=0;
  for(let n=0;n<expected.length;n++){const error=Math.abs(actual[n]-expected[n]);if(n<count*4&&n%4===1)depthError=Math.max(depthError,error);else velocityError=Math.max(velocityError,error);}
  if(depthError>2e-6||velocityError>5e-6||Math.abs(entered-f.expected_entered)>2e-5||Math.abs(left-f.expected_left)>2e-5)throw Error("native/WebGPU field comparison "+JSON.stringify({name:f.name,depthError,velocityError,entered,left}));
  let analyticError=0;if(f.analytic!==null){analyticError=Math.abs((f.analytic[0]?entered:left)-f.analytic[1]);if(analyticError>2e-8)throw Error("independent sonic rarefaction "+analyticError);}
  results.push({name:f.name,steps:f.parameters.length,depth_error:depthError,velocity_error:velocityError,analytic_flux_error:analyticError,mass_error:massError,entered,left});
 }
 const visual=[];
 window.amorisViewport.raw.set_camera_look(32,12,32,16,1,16,0,1,0,60,0.1,0);
 for(const name of ["tick-1","tick-27","tick-63","tick-64","tick-70","reset-27","reset-63"]){
  const response=await fetch("/radiation-fixture/"+name+".bin");if(!response.ok)throw Error("regenerate native visual fixture "+name);
  if(!window.amorisViewport.pushFrame(new Uint8Array(await response.arrayBuffer())))throw Error("radiation program feed refused "+name);
  await new Promise(r=>setTimeout(r,150));const regions=JSON.parse(await window.amorisViewport.raw.water_mesh_stats());
  if(regions.length!==1||regions[0].stats.active_triangles===0||regions[0].stats.visible===0)throw Error("radiation program did not draw its actual GPU mesh");
  const tick=name.startsWith("reset-")?1:Number(name.slice(5));if(window.amorisStats.tick!==tick)throw Error("visual program tick not drawn "+name);
  visual.push({name,tick,active_triangles:regions[0].stats.active_triangles});
 }
 const validation=await device.popErrorScope();if(validation||errors.length||lost)throw Error(JSON.stringify({validation:validation?.message,errors,lost}));
 return{passed:true,scope:data.scope,results,visual};
})()`;
// Default invocation serves only web/ and the explicitly generated numerical and visual fixture files.
// Async child execution keeps the loopback fixture server responsive during Chrome execution.
let server=null,url=process.argv[2];
if(!url){
 const web=resolve("web"),fixture=resolve(process.env.WATER_RADIATION_FIXTURE||"out/water-radiation/quality");
 await Promise.all([readFile(resolve(fixture,"fixture.json")),readFile(resolve(fixture,"base.wgsl")),readFile(resolve(fixture,"mac.wgsl")),readFile(resolve(fixture,"boundary.wgsl"))]);
 server=createServer(async(req,res)=>{try{
  const path=decodeURIComponent(new URL(req.url,"http://localhost").pathname);let file;
  if(path.startsWith("/radiation-fixture/")){
   const name=path.slice("/radiation-fixture/".length);if(!["fixture.json","base.wgsl","mac.wgsl","boundary.wgsl","tick-1.bin","tick-27.bin","tick-63.bin","tick-64.bin","tick-70.bin","reset-27.bin","reset-63.bin"].includes(name))throw Error("not served");file=resolve(fixture,name);
  }else{file=resolve(web,"."+path+(path.endsWith("/")?"index.html":""));if(!file.startsWith(web+"/"))throw Error("not served");}
  const ext=file.slice(file.lastIndexOf("."));res.setHeader("Content-Type",({".wasm":"application/wasm",".js":"text/javascript",".html":"text/html",".json":"application/json",".css":"text/css",".wgsl":"text/plain"})[ext]||"application/octet-stream");
  res.setHeader("Cache-Control","no-store");res.end(await readFile(file));
 }catch{res.statusCode=404;res.end("not found");}});
 await new Promise((ok,fail)=>{server.once("error",fail);server.listen(0,"127.0.0.1",ok);});
 url="http://127.0.0.1:"+server.address().port+"/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps";
}
try{
const {stdout:log}=await promisify(execFile)(process.execPath,["tools/web_bench.mjs",url,"2"],{encoding:"utf8",timeout:90000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,UNSAFE_WEBGPU:"0",VSYNC:"1",LOGS:"1"}});
writeFileSync(resolve(out,"browser.log"),log);
const line=log.split("\n").find(l=>l.startsWith("eval: ")),result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error("actual browser radiation failed: "+log);
writeFileSync(resolve(out,"summary.json"),JSON.stringify(result,null,2)+"\n");console.log(JSON.stringify(result,null,2));
}finally{if(server)await new Promise(ok=>server.close(ok));}
