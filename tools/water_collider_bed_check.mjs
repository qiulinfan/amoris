// Current fixed-collider rays in actual native/browser Games, plus resident sloping-shore replay.
// First run collider_bed_probe and tools/build_web.sh tests/fixtures/scenes/water-slope.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const out=resolve(process.env.WATER_COLLIDER_OUT||'out/water-collider-bed/browser'),fixture=resolve(process.env.WATER_COLLIDER_FIXTURE||'out/water-collider-bed/rays.json'),web=resolve('web');mkdirSync(out,{recursive:true});
await readFile(fixture);
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');
 const a=await import('/amoris.js'),{module}=await a.loadModule(),pkg=await(await fetch('/projects/tests-fixtures-scenes-water-slope/package.json')).json(),data=await(await fetch('/collider-fixture.json')).json();
 const get=async(g,entity,fields)=>(await g.command('world.get',{entity,fields})).fields;
 const step=async(g,ticks)=>{const done=await g.command('step',{ticks});if(done.errors.length)throw Error(JSON.stringify(done.errors));return done;};
 const rays=[];
 if(data.cases.length!==4)throw Error('missing native fixtures');
 for(const c of data.cases){
  const p=structuredClone(pkg);p.scene=c.scene;const g=a.start(module,new TextEncoder().encode(JSON.stringify(p)),{pacing:'stepped',seed:1});
  try{
   await g.ready;await step(g,1);const fields=await get(g,'Shore',['WaterRegion','WaterColliderBed']),bed=fields.WaterRegion.tiles.flatMap(t=>t.bed);
   if(bed.length!==256||bed.some((v,i)=>!Number.isFinite(v)||!Object.is(v,c.expected_bed[i])))throw Error('native/Chrome ray bits differ '+c.name);
   if(JSON.stringify(fields.WaterColliderBed)!==JSON.stringify(c.expected_source))throw Error('native/Chrome geometry identity differs '+c.name);
   const hit=bed.filter(v=>v>-1).length;if(c.name==='no-hit'?hit!==0:hit<64)throw Error('ray fixture is empty '+c.name);
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error('ray game errors');
   rays.push({name:c.name,columns:bed.length,hit_columns:hit,exact:true});
  }finally{g.close();}
 }
 const bytes=new TextEncoder().encode(JSON.stringify(pkg)),runs=[];let previous;
 for(let run=0;run<2;run++){
  const g=a.start(module,bytes,{pacing:'stepped',seed:1}),hashes=[];let frames=0;g.onTicks(items=>hashes.push(...items));g.onRender(frame=>{frames++;if(!window.amorisViewport.pushFrame(frame))throw Error('slope render feed refused');});
  try{
   await g.ready;const trajectory=[];let done;
   for(let n=0;n<80;n++){done=await step(g,30);if(run===0)trajectory.push({tick:(n+1)*30,probes:await g.command('world.query',{with:['WaterProbe'],limit:4294967295,fields:['WaterProbe']})});}
   const state=await get(g,'Shore',['WaterResident','WaterRegion','WaterColliderBed']),b=state.WaterResident.boundary;
   if(state.WaterRegion.tiles.length||state.WaterColliderBed.source.length!==32||state.WaterResident.completed_tick!==2400||b.entered_total<=.1||b.left_total<=.1||Math.abs(b.numerical_residual)>.003)throw Error('resident slope did not execute');
   if(previous&&JSON.stringify(hashes)!==JSON.stringify(previous))throw Error('slope hashes did not repeat');previous=hashes.map(h=>[...h]);
   const restored=await g.command('snapshots.restore',{tick:2336});if(!Number.isSafeInteger(restored.restored)||restored.restored<=0||restored.restored>2336)throw Error('invalid kept snapshot');
   const replay=await step(g,2400-restored.restored);if(replay.world_hash!==done.world_hash)throw Error('slope replay diverged');
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error('slope host/presenter errors');
   runs.push({run,hash:done.world_hash,frames,restored_tick:restored.restored,boundary:b,trajectory});
  }finally{g.close();}
 }
 // A live geometry edit must fail the suspended game, before any stale bed can advance.
 const g=a.start(module,bytes,{pacing:'stepped',seed:1});let refusal;
 try{
  await g.ready;await step(g,1);const t=await get(g,'Terrain',['Transform']),position=[...t.Transform.position];position[1]+=.1;
  await g.command('world.edit',{label:'raise floor',ops:[{set:{entity:'Terrain',component:'Transform',value:{position}}}]});
  try{const result=await g.command('step',{ticks:1});refusal=JSON.stringify(result);}catch(e){refusal=JSON.stringify(e);}
  if(!refusal?.includes('collider-bed inputs changed'))throw Error('resident geometry edit used a stale bed '+refusal);
 }finally{g.close();}
 document.getElementById('hud').style.display='none';return{passed:true,scope:'actual Game current fixed-collider rays and resident continuous-slope shore; does not certify wet/dry accuracy or dynamic GPU rebedding',rays,runs,refusal};
})()`;
const server=createServer(async(req,res)=>{try{const p=decodeURIComponent(new URL(req.url,'http://localhost').pathname);let file;if(p==='/collider-fixture.json')file=fixture;else{file=resolve(web,'.'+p+(p.endsWith('/')?'index.html':''));if(!file.startsWith(web+'/'))throw Error('not served');}const ext=file.slice(file.lastIndexOf('.'));res.setHeader('Content-Type',({'.wasm':'application/wasm','.js':'text/javascript','.html':'text/html','.json':'application/json','.css':'text/css'})[ext]||'application/octet-stream');res.setHeader('Cache-Control','no-store');res.end(await readFile(file));}catch{res.statusCode=404;res.end('not found');}});
await new Promise((ok,fail)=>{server.once('error',fail);server.listen(0,'127.0.0.1',ok);});
try{
 const url='http://127.0.0.1:'+server.address().port+'/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps&assets=/projects/tests-fixtures-scenes-water-slope/';
 const {stdout:log}=await promisify(execFile)(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:240000,maxBuffer:16*1024*1024,env:{...process.env,EVAL:expression,VSYNC:'1',UNSAFE_WEBGPU:'0',LOGS:'1',VIEWPORT:'1280x720',SHOT:resolve(out,'slope.png')}});
 writeFileSync(resolve(out,'browser.log'),log);const line=log.split('\n').find(l=>l.startsWith('eval: ')),result=line?JSON.parse(line.slice(6)):null;
 // The last game intentionally faults; require the exact expected fault and reject other errors.
 if(!result?.passed)throw Error('collider browser check failed; inspect '+out);
 const errors=log.split('\n').filter(l=>/\[(exception|error)\]|\[amoris ERROR\]/i.test(l)&&!l.includes('collider-bed inputs changed'));
 if(errors.length)throw Error('unexpected browser errors '+errors.join('\n'));
 writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify({passed:true,rays:result.rays,runs:result.runs.map(r=>({run:r.run,hash:r.hash,frames:r.frames,restored_tick:r.restored_tick,boundary:r.boundary})),refusal:result.refusal},null,2));
}finally{await new Promise(ok=>server.close(ok));}
