// Actual baseline-WebGPU ownership of dry water rectangles, including the ninth region.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
const run=promisify(execFile),out=resolve(process.env.WATER_OCEAN_MASK_OUT||'out/water-sea/browser-mask'),fixture=resolve(process.env.WATER_OCEAN_MASK_FIXTURE||'out/water-sea/mask-fixture'),web=resolve('web');mkdirSync(out,{recursive:true});
for(const name of ['reference','masked','calibration'])await readFile(resolve(fixture,name+'.bin'));
const server=createServer(async(req,res)=>{try{const p=decodeURIComponent(new URL(req.url,'http://localhost').pathname);let file;if(p.startsWith('/mask-fixture/')){const name=p.slice(14);if(!['reference.bin','masked.bin','calibration.bin'].includes(name))throw Error('not served');file=resolve(fixture,name);}else{file=resolve(web,'.'+p+(p.endsWith('/')?'index.html':''));if(!file.startsWith(web+'/'))throw Error('not served');}const ext=file.slice(file.lastIndexOf('.'));res.setHeader('Content-Type',({'.wasm':'application/wasm','.js':'text/javascript','.html':'text/html','.json':'application/json','.css':'text/css'})[ext]||'application/octet-stream');res.setHeader('Cache-Control','no-store');res.end(await readFile(file));}catch{res.statusCode=404;res.end('not found');}});
await new Promise((ok,fail)=>{server.once('error',fail);server.listen(0,'127.0.0.1',ok);});
try{
 const url='http://127.0.0.1:'+server.address().port+'/viewport/?aa=msaa&gtao=off&gpu_minimal=features,limits,timestamps';
 for(const name of ['reference','masked','calibration']){
  const expression=`(async()=>{const adapter=await navigator.gpu.requestAdapter({powerPreference:'high-performance'});if(!adapter||adapter.info?.isFallbackAdapter===true)throw Error('hardware WebGPU required');window.amorisViewport.raw.set_camera_look(80,10,80,80,0,80,0,0,1,60,.1,16);const response=await fetch('/mask-fixture/${name}.bin');if(!response.ok)throw Error('current native mask frame missing');if(!window.amorisViewport.pushFrame(new Uint8Array(await response.arrayBuffer())))throw Error('mask frame refused');document.getElementById('hud').style.display='none';await new Promise(r=>setTimeout(r,250));return{passed:true,name:${JSON.stringify(name)}};})()`;
  const {stdout}=await run(process.execPath,['tools/web_bench.mjs',url,'2'],{encoding:'utf8',timeout:90000,maxBuffer:8*1024*1024,env:{...process.env,EVAL:expression,LOGS:'1',VSYNC:'1',UNSAFE_WEBGPU:'0',VIEWPORT:'320x180',SHOT:resolve(out,name+'.png')}});writeFileSync(resolve(out,name+'.log'),stdout);const line=stdout.split('\n').find(l=>l.startsWith('eval: '));if(!line||!JSON.parse(line.slice(6)).passed||/\[(exception|error)\]|\[amoris ERROR\]/i.test(stdout))throw Error('actual ocean mask failed '+name);
 }
 const exe=process.env.WATER_IMAGE_COMPARE_EXE||resolve('target/release/examples/water_lab'+(process.platform==='win32'?'.exe':''));
 const compare=async name=>JSON.parse((await run(exe,['--compare',resolve(out,'reference.png'),resolve(out,name+'.png')],{encoding:'utf8'})).stdout);
 const masked=await compare('masked'),calibration=await compare('calibration');if(masked.max_abs_rgb!==0||calibration.max_abs_rgb===0||calibration.changed_gt8===0)throw Error('ocean ownership is incorrect or the reference is insensitive '+JSON.stringify({masked,calibration}));const result={passed:true,scope:'actual WebGPU ninth dry region versus independent no-sea image; ocean-visible calibration; native separate splat test covers unjittered depth',masked,calibration};writeFileSync(resolve(out,'summary.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));
}finally{await new Promise(ok=>server.close(ok));}
