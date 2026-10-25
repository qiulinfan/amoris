// Real Sea-driven GameWorker physics, phase/decay oracle, resident transfers and restoration.
// Build the paired modules with tests/fixtures/scenes/water-shore; serve web/ on 8097.
import {execFileSync} from 'node:child_process';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const out=resolve(process.env.WATER_SEA_OUT||'out/water-sea/browser');mkdirSync(out,{recursive:true});
const url=process.argv[2]||'http://127.0.0.1:8097/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps&assets=/projects/tests-fixtures-scenes-water-shore/';
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');
 const a=await import('/amoris.js'),{module}=await a.loadModule(),pkg=await(await fetch('/projects/tests-fixtures-scenes-water-shore/package.json')).json();
 const bytes=new TextEncoder().encode(JSON.stringify(pkg)),shore=[];let previous;
 const query=g=>g.command('world.query',{with:['WaterProbe'],limit:4294967295,fields:['WaterProbe']});
 const inspect=async(g,name)=> (await g.command('world.get',{entity:name,fields:['WaterResident']})).fields.WaterResident;
 for(let run=0;run<2;run++){
  const g=a.start(module,bytes,{pacing:'stepped',seed:1}),hashes=[];let frames=0;g.onTicks(items=>hashes.push(...items));g.onRender(frame=>{frames++;if(!window.amorisViewport.pushFrame(frame))throw Error('sea render feed refused');});
  try{
   await g.ready;const trajectory=[];let done;
   for(let n=0;n<80;n++){done=await g.command('step',{ticks:30});if(done.errors.length)throw Error(JSON.stringify(done.errors));if(run===0){const probes=await query(g);trajectory.push({tick:(n+1)*30,probes});}}
   const state=await inspect(g,'Shore'),b=state.boundary;
   if(state.completed_tick!==2400||state.recipe.steps.length!==32||state.recipe.steps.at(-1).sea.waves.length!==2||b.entered_total<=0.1||b.left_total<=0.1||Math.abs(b.numerical_residual)>0.003)throw Error('sea did not execute its real boundary controls '+JSON.stringify(state));
   if(previous&&JSON.stringify(hashes)!==JSON.stringify(previous))throw Error('sea hashes did not repeat');previous=hashes.map(h=>[...h]);
   const wet=trajectory.map(r=>r.probes[6].WaterProbe.depth);if(run===0&&(Math.min(...wet)>1e-6||Math.max(...wet)<0.01))throw Error('real stepped-shore wetting was not exercised');
   const restore=await g.command('snapshots.restore',{tick:2336});if(!Number.isSafeInteger(restore.restored)||restore.restored<=0||restore.restored>2336)throw Error('invalid kept snapshot boundary '+JSON.stringify(restore));const continued=await g.command('step',{ticks:2400-restore.restored});if(continued.world_hash!==done.world_hash)throw Error('sea checkpoint/phase restore diverged '+JSON.stringify({restore,target:done.world_hash,continued}));
   if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error('sea host/presenter errors');
   shore.push({run,hash:done.world_hash,frames,restored_tick:restore.restored,boundary:b,trajectory});
  }finally{g.close();}
 }
 // Same flat tank as the native propagation test, with actual script-facing current probes.
 const tank=structuredClone(pkg);tank.scene.entities=tank.scene.entities.filter(e=>!e.components.Floater&&!e.components.WaterProbe);
 const terrain=tank.scene.entities[0].components;terrain.Transform.position=[0,0,0];terrain.Voxels.cell_m=.25;terrain.Voxels.chunks=[];
 const r=tank.scene.entities[1].components.WaterRegion;r.cell_m=.25;r.size=[128,16];r.bed={Voxels:{volume:1,per:1,y_lo:0,y_hi:0}};r.edges=['Sea','Sea','Wall','Wall'];r.friction=0;r.tiles=Array.from({length:8},(_,x)=>({key:[x,0],bed:Array(256).fill(0),depth:Array(256).fill(.125),u:Array(256).fill(0),w:Array(256).fill(0)}));
 tank.scene.entities[2].components.Sea={level:.125,density:1000,waves:[{length:16,height:.002,toward_deg:90,phase:0}]};
 for(const [i,x] of [2.125,4.125,6.125].entries())tank.scene.entities.push({name:'Probe'+i,components:{WaterProbe:{region:2,at:[x,2]}}});
 const tankBytes=new TextEncoder().encode(JSON.stringify(tank)),g=a.start(module,tankBytes,{pacing:'stepped',seed:1}),samples=[];
 try{
  await g.ready;for(let tick=5;tick<=1200;tick+=5){const done=await g.command('step',{ticks:5});if(done.errors.length)throw Error(JSON.stringify(done.errors));if(tick>=480){const p=await query(g);if(p.length!==3||p.some(r=>!r.WaterProbe.ready||r.WaterProbe.tick!==tick))throw Error('tank probe is not current');samples.push([tick/60,p.map(r=>r.WaterProbe.surface)]);}}
 }finally{g.close();}
 const omega=Math.sqrt(9.81*2*Math.PI/16),reference=Math.sqrt(9.81*.125);
 const fit=probe=>{
  const m=Array.from({length:3},()=>Array(4).fill(0));for(const [t,y] of samples){const x=[Math.sin(omega*t),Math.cos(omega*t),1];for(let i=0;i<3;i++){for(let j=0;j<3;j++)m[i][j]+=x[i]*x[j];m[i][3]+=x[i]*y[probe];}}
  for(let i=0;i<3;i++){const p=m[i][i];if(Math.abs(p)<1e-8)throw Error('empty harmonic reference');for(let j=i;j<4;j++)m[i][j]/=p;for(let k=0;k<3;k++){if(k===i)continue;const f=m[k][i];for(let j=i;j<4;j++)m[k][j]-=f*m[i][j];}}
  const [s,c,mean]=m.map(r=>r[3]),rms=Math.sqrt(samples.reduce((a,[t,y])=>a+(s*Math.sin(omega*t)+c*Math.cos(omega*t)+mean-y[probe])**2,0)/samples.length);return{amplitude:Math.hypot(s,c),phase:Math.atan2(c,-s),rms};
 };
 const fits=[0,1,2].map(fit),propagation=[];
 for(let i=0;i<2;i++){const phase=((fits[i+1].phase-fits[i].phase)%(2*Math.PI)+2*Math.PI)%(2*Math.PI),speed=omega*2/phase,decay=fits[i+1].amplitude/fits[i].amplitude,expected=Math.exp(-.2/reference);if(Math.abs(speed/reference-1)>.02||Math.abs(decay/expected-1)>.03)throw Error('independent shallow propagation oracle '+JSON.stringify({speed,reference,decay,expected}));propagation.push({speed,reference,amplitude_ratio:decay,expected});}
 if(fits.some(f=>f.amplitude<.0003||f.amplitude>.0012||f.rms>.05*f.amplitude))throw Error('wave mode did not propagate harmonically');
 // Owned host jobs provide actual transfer counts before the first checkpoint.
 const bindings=await import('/pkg/amoris_web.js'),raw=new bindings.GameWorker(bytes,JSON.stringify({pacing:'stepped',seed:1})),host=new bindings.BrowserWater();let jobs=0,transfers;
 const flush=()=>{for(const m of JSON.parse(raw.messages())){if(m.t==='snap'){raw.snap_bytes();raw.ack(m.version);}if(m.t==='reply'&&m.error)throw Error(JSON.stringify(m.error));}raw.render_frame();};
 try{flush();raw.push(JSON.stringify({t:'cmd',source:{developer:0},seq:1,name:'step',params:JSON.stringify({ticks:8})}));for(let guard=0;guard<64;guard++){const next=JSON.parse(raw.run()).next;flush();if(next==='command')break;if(next==='external'){const job=raw.take_water_job();if(!job||job.kind!=='resident')throw Error('sea resident job missing');const result=await host.advance(job);jobs++;if(!raw.complete_water_job(result))throw Error('sea result refused');}}transfers=JSON.parse(host.diagnostics()).resident;if(jobs!==8||transfers.uploads!==1||transfers.field_readbacks!==0)throw Error('sea lost resident transfer bound '+JSON.stringify({jobs,transfers}));}finally{raw.close();raw.free();host.free();}
 document.getElementById('hud').style.display='none';return{passed:true,scope:'actual Sea-driven GameWorker; native-equivalent tank plus independent shallow-speed/decay oracle; stepped-shore wetting, not certified continuous-beach runup',shore,tank:{fits,propagation,samples},transfer:{jobs,transfers}};
})()`;
const log=execFileSync(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:240000,maxBuffer:16*1024*1024,env:{...process.env,EVAL:expression,VSYNC:'1',UNSAFE_WEBGPU:'0',LOGS:'1',VIEWPORT:'1280x720',SHOT:resolve(out,'sea.png')}});
writeFileSync(resolve(out,'browser.log'),log);const line=log.split('\n').find(l=>l.startsWith('eval: ')),result=line?JSON.parse(line.slice(6)):null;
if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error('actual sea worker failed; inspect '+out);
writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify({passed:true,propagation:result.tank.propagation,transfer:result.transfer,shore:result.shore.map(r=>({run:r.run,hash:r.hash,frames:r.frames,boundary:r.boundary}))},null,2));
