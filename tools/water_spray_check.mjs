// Real GameWorker waterfall, GPU-driven spray, pause/reset and pixel ablation on baseline WebGPU.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const web=resolve('web'),out=resolve(process.env.WATER_SPRAY_OUT||'out/water-spray/browser');mkdirSync(out,{recursive:true});
const expression=`(async()=>{
 const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');
 const a=await import('/amoris.js'),{module}=await a.loadModule(),pkg=await(await fetch('/projects/samples-water-fall/package.json')).arrayBuffer(),p=window.amorisViewport.raw;
 p.set_camera_look(23,10,19,11.5,2.8,6,0,1,0,60,.1,0);p.set_water_culling(false);p.set_splashes('on');
 const g=a.start(module,new Uint8Array(pkg),{pacing:'stepped',seed:1});let frames=0;g.onRender(bytes=>{frames++;if(!window.amorisViewport.pushFrame(bytes))throw Error('fall frame refused');});
 const raf=()=>new Promise(r=>requestAnimationFrame(r));const settle=async()=>{for(let n=0;n<3;n++)await raf();};
 const stats=async()=>JSON.parse(await p.water_spray_stats());
 const shot=name=>new Promise((resolve,reject)=>{window.__amorisStageShotDone=(done,error)=>{if(done===name){if(error)reject(Error(error));else resolve();}};window.__amorisStageShot(name);});
 document.getElementById('hud').style.display='none';
 let end;
 const advance=async count=>{for(let n=0;n<count;n++){end=await g.command('step',{ticks:1});if(end.errors.length)throw Error(JSON.stringify(end.errors));await raf();}};
 try{
  await g.ready;await advance(180);await settle();const before=await stats(),hash=end.world_hash;await shot("on");
  if(before.length!==1||before[0].stats.ring_head<500||before[0].stats.active_upper_bound<50||before[0].stats.sources===0)throw Error('fall did not generate current field spray '+JSON.stringify(before));
  await settle();const paused=await stats();await shot("paused");
  if(before[0].stats.ring_head!==paused[0].stats.ring_head||before[0].stats.frame!==paused[0].stats.frame)throw Error('paused field spray kept moving '+JSON.stringify({before,paused}));
  p.set_splashes('off');await settle();const offStats=await stats();await shot("off");if(offStats.length)throw Error('spray ablation is empty or failed '+JSON.stringify({offStats}));
  p.set_splashes('on');await settle();const reopened=await stats();if(reopened[0].stats.ring_head!==0||reopened[0].stats.draw_instances!==0)throw Error('paused toggle spawned particles');
  await advance(30);await settle();const preRestore=await stats();if(preRestore[0].stats.ring_head<300)throw Error('restore control has no populated history');
  const restore=await g.command('snapshots.restore',{tick:120});await settle();const restored=await stats();if(restore.restored!==120||restored[0].stats.frame>=preRestore[0].stats.frame||restored[0].stats.ring_head>=preRestore[0].stats.ring_head)throw Error('restore retained old particle history '+JSON.stringify({restore,preRestore,restored}));
  await advance(60);await settle();const replayed=await stats();if(end.world_hash!==hash||replayed[0].stats.ring_head===0)throw Error('world replay diverged or new timeline did not emit');
  if(g.problems().length||JSON.parse(g.presenter.problems()).length)throw Error('waterfall host/presenter errors');
  document.getElementById('hud').style.display='none';return{passed:true,scope:'actual current-field presentation spray; no mass/momentum back-coupling claim; explicit counter/image diagnostics',frames,before,paused,preRestore,restored,replayed,world_hash:hash};
 }finally{g.close();}
})()`;
const server=createServer(async(req,res)=>{try{const p=decodeURIComponent(new URL(req.url,'http://localhost').pathname),file=resolve(web,'.'+p+(p.endsWith('/')?'index.html':''));if(!file.startsWith(web+'/'))throw Error('not served');const ext=file.slice(file.lastIndexOf('.'));res.setHeader('Content-Type',({'.wasm':'application/wasm','.js':'text/javascript','.html':'text/html','.json':'application/json','.css':'text/css'})[ext]||'application/octet-stream');res.setHeader('Cache-Control','no-store');res.end(await readFile(file));}catch{res.statusCode=404;res.end('not found');}});
await new Promise((ok,fail)=>{server.once('error',fail);server.listen(0,'127.0.0.1',ok);});
try{const url='http://127.0.0.1:'+server.address().port+'/viewport/?aa=msaa&gtao=off&lod=off&gpu_minimal=features,limits,timestamps&assets=/projects/samples-water-fall/';const {stdout:log}=await promisify(execFile)(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:180000,maxBuffer:16*1024*1024,env:{...process.env,EVAL:expression,LOGS:'1',VSYNC:'1',UNSAFE_WEBGPU:'0',VIEWPORT:'1280x720',STAGE_SHOTS:out}});writeFileSync(resolve(out,'browser.log'),log);const line=log.split('\n').find(l=>l.startsWith('eval: ')),result=line?JSON.parse(line.slice(6)):null;if(!result?.passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(log))throw Error('real waterfall spray failed; inspect '+out);const compare=async name=>JSON.parse((await promisify(execFile)(process.env.WATER_LAB||'target/release/examples/water_lab',['--compare',resolve(out,'on.png'),resolve(out,name+'.png')],{encoding:'utf8'})).stdout);result.pause_image=await compare('paused');result.effect_image=await compare('off');if(result.pause_image.max_abs_rgb!==0||result.effect_image.changed_gt8<10||result.effect_image.max_abs_rgb<4)throw Error('spray screenshot ablation/pause failed '+JSON.stringify({pause:result.pause_image,effect:result.effect_image}));writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));}finally{await new Promise(ok=>server.close(ok));}
