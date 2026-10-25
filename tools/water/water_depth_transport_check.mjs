// Independent f64 flux graph against production WGSL on actual baseline WebGPU.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const fixture=resolve(process.env.WATER_DEPTH_FIXTURE||'out/water-depth-transport/fixtures'),out=resolve(process.env.WATER_DEPTH_OUT||'out/water-depth-transport/browser'),web=resolve('web');mkdirSync(out,{recursive:true});
await Promise.all(['fixture.json','depth.wgsl'].map(f=>readFile(resolve(fixture,f))));
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');
 const device=await adapter.requestDevice({requiredFeatures:[]}),errors=[];let lost=null;device.addEventListener('uncapturederror',e=>errors.push(e.error.message));device.lost.then(e=>{if(e.reason!=='destroyed')lost=e.message;});device.pushErrorScope('validation');
 const module=device.createShaderModule({code:await(await fetch('/depth-fixture/depth.wgsl')).text()}),info=await module.getCompilationInfo();if(info.messages.some(m=>m.type==='error'))throw Error(JSON.stringify(info.messages));
 const layout=device.createBindGroupLayout({entries:Array.from({length:7},(_,i)=>({binding:i,visibility:GPUShaderStage.COMPUTE,buffer:{type:i===0?'uniform':[1,3].includes(i)?'read-only-storage':'storage'}}))}),pl=device.createPipelineLayout({bindGroupLayouts:[layout]});
 const pipelines=await Promise.all(['donors','transport'].map(entryPoint=>device.createComputePipelineAsync({layout:pl,compute:{module,entryPoint}})));
 const data=await(await fetch('/depth-fixture/fixture.json')).json(),results=[];
 if(data.cases.length!==6)throw Error('missing flux cases');
 for(const c of data.cases){
  const n=c.size[0]*c.size[1],buffers=[];
  const buffer=(bytes,usage,typed)=>{const b=device.createBuffer({size:bytes,usage:usage|GPUBufferUsage.COPY_DST});if(typed)device.queue.writeBuffer(b,0,typed);buffers.push(b);return b;};
  const words=new Uint32Array(152);words.set(c.size);words[2]=new Uint32Array(new Float32Array([c.dx]).buffer)[0];words[3]=new Uint32Array(new Float32Array([c.dt]).buffer)[0];
  const params=buffer(608,GPUBufferUsage.UNIFORM,words),src=buffer(n*16+4096,GPUBufferUsage.STORAGE,new Uint32Array(c.initial.flat())),dst=buffer(n*16+4096,GPUBufferUsage.STORAGE|GPUBufferUsage.COPY_SRC),awake=buffer(n*4,GPUBufferUsage.STORAGE,new Uint32Array(n).fill(1)),velocity=buffer(n*8,GPUBufferUsage.STORAGE,new Float32Array(c.velocity.flat())),scale=buffer(n*4,GPUBufferUsage.STORAGE),edges=buffer((c.size[0]+c.size[1])*32,GPUBufferUsage.STORAGE),stage=buffer(n*16,GPUBufferUsage.MAP_READ);
  const group=device.createBindGroup({layout,entries:[params,src,dst,awake,velocity,scale,edges].map((b,i)=>({binding:i,resource:{buffer:b}}))});
  const e=device.createCommandEncoder();for(const p of pipelines){const pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,group);pass.dispatchWorkgroups(Math.ceil(c.size[0]/8),Math.ceil(c.size[1]/8));pass.end();}e.copyBufferToBuffer(dst,0,stage,0,n*16);device.queue.submit([e.finish()]);
  await stage.mapAsync(GPUMapMode.READ);const actual=new Uint32Array(stage.getMappedRange()).slice();stage.unmap();const values=new Float32Array(actual.buffer);let maxError=0,initial=0,volume=0;
  const velocityBits=new Uint32Array(new Float32Array(c.velocity.flat()).buffer);
  for(let i=0;i<n;i++){if(actual[i*4]!==c.initial[i][0]||actual[i*4+2]!==velocityBits[i*2]||actual[i*4+3]!==velocityBits[i*2+1])throw Error('transport changed bed/face words');const h=values[i*4+1];if(!Number.isFinite(h)||h<0)throw Error('invalid depth');maxError=Math.max(maxError,Math.abs(h-c.expected_depth[i]));initial+=new Float32Array(new Uint32Array([c.initial[i][1]]).buffer)[0];volume+=h;}
  const massError=(volume-initial)*c.dx*c.dx;if(maxError>3e-6||Math.abs(massError)>5e-6)throw Error('independent flux oracle '+JSON.stringify({case:c.case,maxError,massError}));
  results.push({case:c.case,max_error:maxError,mass_error_m3:massError});for(const b of buffers)b.destroy();
 }
 const validation=await device.popErrorScope();if(validation||errors.length||lost)throw Error(JSON.stringify({validation:validation?.message,errors,lost}));device.destroy();
 return{passed:true,scope:'isolated production limited-linear depth flux; independent native f64 face graph; world/replay checks are separate',results};
})()`;
const server=createServer(async(req,res)=>{try{const p=decodeURIComponent(new URL(req.url,'http://localhost').pathname);let file;if(p.startsWith('/depth-fixture/')){const name=p.slice('/depth-fixture/'.length);if(!['fixture.json','depth.wgsl'].includes(name))throw Error('not served');file=resolve(fixture,name);}else{file=resolve(web,'.'+p+(p.endsWith('/')?'index.html':''));if(!file.startsWith(web+'/'))throw Error('not served');}const ext=file.slice(file.lastIndexOf('.'));res.setHeader('Content-Type',({'.wasm':'application/wasm','.js':'text/javascript','.html':'text/html','.json':'application/json','.css':'text/css','.wgsl':'text/plain'})[ext]||'application/octet-stream');res.setHeader('Cache-Control','no-store');res.end(await readFile(file));}catch{res.statusCode=404;res.end('not found');}});
await new Promise((ok,fail)=>{server.once('error',fail);server.listen(0,'127.0.0.1',ok);});
try{const url='http://127.0.0.1:'+server.address().port+'/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps';const {stdout:log}=await promisify(execFile)(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:90000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,LOGS:'1',VSYNC:'1',UNSAFE_WEBGPU:'0'}});writeFileSync(resolve(out,'browser.log'),log);const line=log.split('\n').find(l=>l.startsWith('eval: ')),result=line?JSON.parse(line.slice(6)):null;if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error('WebGPU flux check failed '+log);writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));}finally{await new Promise(ok=>server.close(ok));}
