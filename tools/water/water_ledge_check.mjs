// Actual baseline WebGPU: shared raster/medium helpers, independent film oracle, real worker.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {mkdirSync, writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const fixture=resolve(process.env.WATER_LEDGE_FIXTURE||'out/water-ledge/fixtures');
const out=resolve(process.env.WATER_LEDGE_OUT||'out/water-ledge/browser'),web=resolve('web');
mkdirSync(out,{recursive:true});
await Promise.all(['forward.wgsl','classify.wgsl','medium.wgsl','film.json'].map(name=>readFile(resolve(fixture,name))));
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});
 if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');
 const device=await adapter.requestDevice({requiredFeatures:[]}),errors=[];let lost=null;
 device.addEventListener('uncapturederror',e=>errors.push(e.error.message));
 device.lost.then(e=>{if(e.reason!=='destroyed')lost=e.message;});device.pushErrorScope('validation');
 const fetchText=async f=>(await fetch('/ledge-fixture/'+f)).text();
 const fetchJson=async f=>(await fetch('/ledge-fixture/'+f)).json();
 const sources=await Promise.all(['forward.wgsl','classify.wgsl','medium.wgsl'].map(fetchText));
 const modules=sources.map(code=>device.createShaderModule({code}));
 for(const m of modules){const info=await m.getCompilationInfo();if(info.messages.some(m=>m.type==='error'))throw Error(JSON.stringify(info.messages.map(m=>({type:m.type,message:m.message,line:m.lineNum,column:m.linePos}))));}
 const S=GPUShaderStage.COMPUTE,U=GPUBufferUsage;
 const bufferEntry=(binding,type)=>({binding,visibility:S,buffer:{type}});
 const layout=entries=>device.createBindGroupLayout({entries});
 const fieldLayout=layout([bufferEntry(1,'uniform'),bufferEntry(2,'read-only-storage'),bufferEntry(10,'uniform'),{binding:11,visibility:S,texture:{sampleType:'uint'}}]);
 const classFieldLayout=layout([bufferEntry(1,'uniform'),bufferEntry(2,'read-only-storage')]);
 const classLocal=layout([bufferEntry(1,'storage'),bufferEntry(2,'storage'),{binding:3,visibility:S,storageTexture:{access:'write-only',format:'r32uint'}}]);
 const vertexLocal=layout([bufferEntry(0,'uniform'),bufferEntry(7,'read-only-storage'),bufferEntry(8,'storage')]);
 const mediumLocal=layout([bufferEntry(8,'read-only-storage'),bufferEntry(9,'read-only-storage'),bufferEntry(10,'storage')]);
 const empty=layout([]);
 const pipeline=(module,entryPoint,local,field)=>device.createComputePipelineAsync({layout:device.createPipelineLayout({bindGroupLayouts:field?[local,empty,empty,field]:[local]}),compute:{module,entryPoint}});
 const classify=await pipeline(modules[1],'classify',classLocal,classFieldLayout);
 const raster=await pipeline(modules[0],'probe_water_ledges',vertexLocal,fieldLayout);
 const medium=await pipeline(modules[2],'probe_ledge_medium',mediumLocal,fieldLayout);
 const film=await pipeline(modules[0],'probe_water_films',vertexLocal,null);
 const buffers=[];const textures=[];
 const buffer=(size,usage,data)=>{const b=device.createBuffer({size,usage:usage|U.COPY_DST});if(data)device.queue.writeBuffer(b,0,data);buffers.push(b);return b;};
 const bind=(layout,entries)=>device.createBindGroup({layout,entries:entries.map(([binding,resource])=>({binding,resource:resource instanceof GPUBuffer?{buffer:resource}:resource}))});
 const run=async(p,local,field,count,output)=>{
  const stage=buffer(output.size,U.MAP_READ),e=device.createCommandEncoder(),pass=e.beginComputePass();
  pass.setPipeline(p);pass.setBindGroup(0,local);if(field)pass.setBindGroup(3,field);pass.dispatchWorkgroups(Math.ceil(count/64));pass.end();
  e.copyBufferToBuffer(output,0,stage,0,output.size);device.queue.submit([e.finish()]);await stage.mapAsync(GPUMapMode.READ);
  const data=new Float32Array(stage.getMappedRange()).slice();stage.unmap();return data;
 };
 const viewData=new Float32Array(256);for(let matrix=0;matrix<4;matrix++)for(let i=0;i<4;i++)viewData[matrix*16+i*5]=1;
 const view=buffer(1024,U.UNIFORM,viewData),geometry=[];
 for(const axis of [0,1])for(const reverse of [false,true]){
  const columns=await fetchJson('field-'+axis+'-'+reverse+'.json'),rays=await fetchJson('rays-'+axis+'-'+reverse+'.json');
  const words=new Uint32Array(28);words[2]=0x3f800000;words[4]=words[5]=2;words[7]=1;
  const params=buffer(112,U.UNIFORM,words),field=buffer(64,U.STORAGE,new Float32Array(columns.flat()));
  const ids=buffer(16,U.STORAGE|U.COPY_SRC),args=buffer(16,U.STORAGE|U.UNIFORM|U.COPY_SRC);
  const texture=device.createTexture({size:[1,1],format:'r32uint',usage:GPUTextureUsage.TEXTURE_BINDING|GPUTextureUsage.STORAGE_BINDING});textures.push(texture);const mask=texture.createView();
  const cgroup=bind(classLocal,[[1,ids],[2,args],[3,mask]]),cf=bind(classFieldLayout,[[1,params],[2,field]]);
  const count=await run(classify,cgroup,cf,1,args);if(new Uint32Array(count.buffer)[1]!==1)throw Error('straight cut missing');
  const f=bind(fieldLayout,[[1,params],[2,field],[10,args],[11,mask]]);
  const indices=buffer(72*4,U.STORAGE,Uint32Array.from({length:72},(_,i)=>i)),vertices=buffer(72*32,U.STORAGE|U.COPY_SRC);
  const vg=bind(vertexLocal,[[0,view],[7,indices],[8,vertices]]),v=await run(raster,vg,f,72,vertices);
  let walls=0;for(let n=0;n<72;n++){
   if(!Array.from(v.slice(n*8,n*8+8)).every(Number.isFinite))throw Error('nonfinite vertex');
   const kind=v[n*8+7],coordinate=v[n*8+axis*2];
   if(kind===2){walls++;if(Math.abs(coordinate-1)>1e-6||v[n*8+4+axis*2]*(reverse?-1:1)<.99)throw Error('wall plane/normal');}
   if(kind===3&&Math.abs(coordinate-1)>1e-6){const high=(coordinate<1)!==reverse;if(Math.abs(v[n*8+1]-(high?6.4:.6))>2e-6)throw Error('cap height');}
  }if(walls!==12)throw Error('wall topology');
  for(const scale of [0,-1]){
   const stopped=columns.map(c=>c.map((v,k)=>k===axis+2?v*scale:v));device.queue.writeBuffer(field,0,new Float32Array(stopped.flat()));
   const actual=await run(raster,vg,f,72,vertices);let sides=0;
   for(let n=0;n<72;n++){if(actual[n*8+7]===4)sides++;if(actual[n*8+7]===2&&actual[n*8+3]!==0)throw Error('no-flow/uphill film');}
   if(sides!==12)throw Error('no-flow sidewall missing');
  }device.queue.writeBuffer(field,0,new Float32Array(columns.flat()));
  const bounds=buffer(32,U.STORAGE,new Float32Array([0,0,0,0,0,6,.6,6.4]));
  const query=buffer(rays.length*16,U.STORAGE,new Float32Array(rays.flat())),answer=buffer(160,U.STORAGE|U.COPY_SRC);
  const local=bind(mediumLocal,[[8,bounds],[9,query],[10,answer]]);
  const actual=await run(medium,local,f,10,answer);let error=0;
  [0,1,1,0,0].forEach((want,i)=>{if(actual[i*4]!==want)throw Error('inside air/bed classification');});
  [0,.5,.5,.6,.4].forEach((want,i)=>{error=Math.max(error,Math.abs(actual[(i+5)*4+1]-want));});
  if(error>3e-5)throw Error('known medium path '+error);
  device.queue.writeBuffer(args,0,new Uint32Array([72,0,0,0]));
  const legacy=await run(medium,local,f,10,answer);if(legacy[0]!==1||legacy[21]<.05)throw Error('air-wedge control is insensitive');
  geometry.push({axis,reverse,wall_vertices:walls,max_path_error_m:error,legacy_air_wedge_m:legacy[21]});
 }
 const controls=[];
 const guards=[...Array.from({length:2},(_,axis)=>({name:'resolved-ramp-'+axis,size:[4,4],columns:Array.from({length:16},(_,n)=>[.4+2*(axis===0?n%4:Math.floor(n/4)),.4,1,0])})),
  {name:'unreceived',size:[2,2],columns:[[6.4,.4,1,0],[0,-1,0,0],[6.4,.4,1,0],[0,-1,0,0]]},
  {name:'dry-step',size:[2,2],columns:[[6,0,1,0],[0,0,0,0],[6,0,1,0],[0,0,0,0]]},
  {name:'flat-surface-over-bed-step',size:[2,2],columns:[[6.4,.4,1,0],[6.4,6.4,0,0],[6.4,.4,1,0],[6.4,6.4,0,0]]}];
 for(const c of guards){
  const words=new Uint32Array(28);words[2]=0x3f800000;words.set(c.size,4);words[7]=1;
  const params=buffer(112,U.UNIFORM,words),field=buffer(c.columns.length*16,U.STORAGE,new Float32Array(c.columns.flat()));
  const n=(c.size[0]-1)*(c.size[1]-1),ids=buffer(Math.max(n*4,16),U.STORAGE),args=buffer(16,U.STORAGE|U.COPY_SRC);
  const texture=device.createTexture({size:c.size.map(n=>n-1),format:'r32uint',usage:GPUTextureUsage.STORAGE_BINDING});textures.push(texture);
  const actual=await run(classify,bind(classLocal,[[1,ids],[2,args],[3,texture.createView()]]),bind(classFieldLayout,[[1,params],[2,field]]),n,args);
  if(new Uint32Array(actual.buffer)[1]!==0)throw Error('false ledge '+c.name);controls.push(c.name);
 }
 const oracle=await fetchJson('film.json'),input=new Float32Array(oracle.cases.flat());
 const params=buffer(input.byteLength,U.STORAGE,input),output=buffer(oracle.cases.length*32,U.STORAGE|U.COPY_SRC);
 const values=await run(film,bind(vertexLocal,[[0,view],[7,params],[8,output]]),null,oracle.cases.length,output);
 let filmError=0;oracle.expected.forEach((pair,i)=>pair.forEach((rgb,j)=>rgb.forEach((want,k)=>{filmError=Math.max(filmError,Math.abs(want-values[i*8+j*4+k]));})));
 if(filmError>6e-6)throw Error('independent internal-bounce oracle '+filmError);
 const validation=await device.popErrorScope();if(validation||errors.length||lost)throw Error(JSON.stringify({validation:validation?.message,errors,lost}));
 for(const b of buffers)b.destroy();for(const t of textures)t.destroy();device.destroy();
 // Exercise the rebuilt viewport against the authoritative resident worker, not fixture data.
 const firstErrors=[],originalLog=console.log;console.log=(...args)=>{const text=args.join(' ');if(text.includes('[amoris ERROR]')&&firstErrors.length<8)firstErrors.push(text);originalLog(...args);};
 const a=await import('/amoris.js'),{module}=await a.loadModule(),pkg=await(await fetch('/projects/samples-water-fall/package.json')).arrayBuffer(),p=window.amorisViewport.raw;
 p.set_camera_look(23,10,19,11.5,2.8,6,0,1,0,60,.1,0);p.set_splashes('off');p.set_water_culling(true);
 const game=a.start(module,new Uint8Array(pkg),{pacing:'stepped',seed:1});let end;
 game.onRender(bytes=>{if(!window.amorisViewport.pushFrame(bytes))throw Error('real field frame refused');});
 const raf=()=>new Promise(r=>requestAnimationFrame(r));
 const shot=name=>new Promise((resolve,reject)=>{window.__amorisStageShotDone=(done,error)=>{if(done===name){if(error)reject(Error(error));else resolve();}};window.__amorisStageShot(name);});
 try {
  await game.ready;for(let n=0;n<120;n++){end=await game.command('step',{ticks:1});if(end.errors.length)throw Error(JSON.stringify(end.errors));await raf();}
  for(let n=0;n<3;n++)await raf();document.getElementById('hud').style.display='none';
  const before=JSON.parse(await p.water_ledge_stats());if(before.length!==1||before[0].stats.active_quads!==47||before[0].stats.submitted_triangles!==1128)throw Error('real waterfall classification '+JSON.stringify({before,firstErrors}));
  await shot('fall');for(let n=0;n<3;n++)await raf();await shot('paused');
  const pending=p.water_ledge_stats();p.demo_water_mesh(64,'full','full',0);
  for(let n=0;n<3;n++)await raf();
  const replaced=JSON.parse(await p.water_ledge_stats()),captured=JSON.parse(await pending);
  if(replaced.length!==1||replaced[0].stats.active_quads!==0||JSON.stringify(before)!==JSON.stringify(captured))throw Error('renderer reset changed copied diagnostics '+JSON.stringify({before,captured,replaced}));
  p.set_camera_look(23,10,19,11.5,2.8,6,0,1,0,60,.1,0);
  const reset=await game.command('snapshots.restore',{tick:60});
  if(reset.restored!==60)throw Error('missing saved tick 60');
  for(let n=0;n<60;n++){const replay=await game.command('step',{ticks:1});if(replay.errors.length)throw Error(JSON.stringify(replay.errors));if(n===59&&replay.world_hash!==end.world_hash)throw Error('real world replay diverged');await raf();}
  if(firstErrors.length)throw Error(JSON.stringify(firstErrors));
  if(game.problems().length||JSON.parse(game.presenter.problems()).length)throw Error('worker/presenter problems');
  return {passed:true,geometry,classification_controls:controls,film_max_error:filmError,real_field:before,world_hash:end.world_hash,scope:'render-only ledge partition and thin-film interface; no dense 3D liquid or mass feedback claim'};
 } finally {game.close();console.log=originalLog;}
})()`;
const allowed=new Set(['forward.wgsl','classify.wgsl','medium.wgsl','film.json',...Array.from({length:2},(_,axis)=>[false,true].flatMap(reverse=>['field','rays'].map(prefix=>`${prefix}-${axis}-${reverse}.json`))).flat()]);
const server=createServer(async(req,res)=>{try{
 const p=decodeURIComponent(new URL(req.url,'http://localhost').pathname);let file;
 if(p.startsWith('/ledge-fixture/')){const name=p.slice('/ledge-fixture/'.length);if(!allowed.has(name))throw Error('not served');file=resolve(fixture,name);}
 else {file=resolve(web,'.'+p+(p.endsWith('/')?'index.html':''));if(!file.startsWith(web+'/'))throw Error('not served');}
 const ext=file.slice(file.lastIndexOf('.'));res.setHeader('Content-Type',({'.wasm':'application/wasm','.js':'text/javascript','.html':'text/html','.json':'application/json','.css':'text/css','.wgsl':'text/plain'})[ext]||'application/octet-stream');res.setHeader('Cache-Control','no-store');res.end(await readFile(file));
 }catch{res.statusCode=404;res.end('not found');}});
await new Promise((ok,fail)=>{server.once('error',fail);server.listen(0,'127.0.0.1',ok);});
try {
 const url='http://127.0.0.1:'+server.address().port+'/viewport/?aa=msaa&gtao=off&lod=off&gpu_minimal=features,limits,timestamps&assets=/projects/samples-water-fall/';
 const {stdout:log}=await promisify(execFile)(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:180000,maxBuffer:16*1024*1024,env:{...process.env,EVAL:expression,LOGS:'1',VSYNC:'1',UNSAFE_WEBGPU:'0',VIEWPORT:'1280x720',STAGE_SHOTS:out}});
 writeFileSync(resolve(out,'browser.log'),log);const line=log.split('\n').find(l=>l.startsWith('eval: ')),result=line?JSON.parse(line.slice(6)):null;
 if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error('actual WebGPU ledge failed; inspect '+out);
 const compare=await promisify(execFile)(process.env.WATER_LAB||'target/release/examples/water_lab',['--compare',resolve(out,'fall.png'),resolve(out,'paused.png')],{encoding:'utf8'});
 result.pause_image=JSON.parse(compare.stdout);if(result.pause_image.max_abs_rgb!==0)throw Error('paused ledge image changed');
 writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));
} catch(error) {
 if(error.stdout)writeFileSync(resolve(out,'failed-browser.log'),error.stdout);
 throw error;
} finally {await new Promise(ok=>server.close(ok));}
