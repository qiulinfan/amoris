#!/usr/bin/env python3
"""Actual Game + publication + visual replica + draw/wait water matrix.

Generated projects, tick rows and measurements remain in ignored out/. No performance threshold
is a correctness gate. The fixtures use 0.25-m cells, 2-m/1-m wet dams or 1-m flat sleeping lakes.
"""
import argparse
import copy
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess

REPO=Path(__file__).resolve().parents[1]

def project(destination, side, regions, state, solver, probes, distinct=False):
    source=REPO/'tests/fixtures/scenes/water-resident'
    destination.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(source/'project.toml',destination/'project.toml')
    shutil.copytree(source/'scripts',destination/'scripts',dirs_exist_ok=True)
    scene=json.loads((source/'scene.json').read_text())
    templates={e['name']:e for e in scene['entities']}
    entities=[copy.deepcopy(templates['Bed'])]
    entities[0]['components']['Voxels']['chunks']=[]
    entities[0]['components']['Voxels']['cell_m']=.25
    across=math.ceil(math.sqrt(regions));rows=math.ceil(regions/across)
    span=side*.25;pitch=span+4
    width=across*pitch-4;depth=rows*pitch-4
    for region in range(regions):
        running=state=='active' or (state=='mixed' and region==0)
        water=copy.deepcopy(templates['Lake'])
        water['name']=f'Lake{region}'
        origin=[(region%across)*pitch,0,(region//across)*pitch]
        water['components']['Transform']={'position':origin}
        water['components']['WaterRegion']={
            'solver':solver,'cell_m':.25,'size':[side,side],
            'bed':{'Voxels':{'volume':1,'per':1,'y_lo':0,'y_hi':0}},
            'dry_below':0,'substeps':2,'friction':.02,
            'tiles':[{'key':[x,z],'bed':[0]*256,
                      'depth':[2+(region*.0625 if distinct else 0) if running and x*16+n%16<side//2 else 1 for n in range(256)],
                      'u':[0]*256,'w':[0]*256}
                     for z in range(side//16)for x in range(side//16)]}
        entities.append(water)
    if solver=='gpu_resident':
        for region in range(regions):
            for n in range(probes):
                entities.append({'name':f'Probe{region}_{n}','components':{'WaterProbe':{
                    'region':region+2,'at':[(region%across)*pitch+((n%8)*side/8+.5)*.25,
                                           (region//across)*pitch+((n//8)*side/8+.5)*.25]}}})
    floor=copy.deepcopy(templates['Floor']);floor['components']['Transform']['position']=[width/2,-.25,depth/2]
    floor['components']['Model']['scale']=[width,.5,depth];entities.append(floor)
    entities.extend(copy.deepcopy(templates[n])for n in ['Sun','Sky'])
    camera=copy.deepcopy(templates['View']);scale=max(width,depth)/32
    # The authored view looks from (40,24,42) to (16,0,16); preserve its rotation and proportions.
    camera['components']['Transform']['position']=[width/2+24*scale,24*scale,depth/2+26*scale]
    entities.append(camera);scene['entities']=entities
    (destination/'scene.json').write_text(json.dumps(scene,separators=(',',':'))+'\n')

def stats(values):
    values=sorted(values)
    if not values:return None
    return {'n':len(values),'median_ms':values[(len(values)-1)//2],
            'p95_ms':values[int((len(values)-1)*.95)],'max_ms':values[-1]}

def summarize(rows,warmup,wake):
    phases={}
    for phase in ['warmup','ordinary','checkpoint','resource_update','resource_only','asleep','wake','after_wake']:
        selected=[]
        for r in rows:
            tick=r['tick']
            match=(phase=='warmup'and tick<=warmup)or(phase=='checkpoint'and tick>warmup and r['checkpoints']>0)
            if phase=='ordinary':match=tick>warmup and not r['checkpoints'] and not r.get('snapshot_checkpoint_changed',False) and (not wake or tick<wake)
            if phase=='resource_update':match=tick>warmup and r.get('snapshot_checkpoint_changed',False)
            if phase=='resource_only':match=tick>warmup and r.get('snapshot_checkpoint_changed',False) and not r['checkpoints']
            if phase=='asleep':match=tick>warmup and (not wake or tick<wake)
            if phase=='wake':match=tick==wake
            if phase=='after_wake':match=bool(wake and tick>wake)
            if match:selected.append(r)
        # 'asleep' is only retained by the caller for fixtures whose resident still counters prove it.
        phases[phase]={key:stats([r[key]for r in selected])for key in ['water_ms','step_ms','extract_ms','encode_ms','publish_ms','complete_ms']}
        phases[phase]['snapshot_ms']=stats([r.get('snapshot_ms',0)for r in selected])
        phases[phase]['decode_ms']=stats([r.get('decode_ms',0)for r in selected])
        for i,key in enumerate(['apply_ms','render_encode_ms','gpu_wait_ms']):
            phases[phase][key]=stats([r['draw_phases'][i]for r in selected if r['draw_phases']is not None])
    return phases

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--build',action='append',required=True,metavar='NAME=EXE')
    p.add_argument('--case',action='append',metavar='SIDExREGIONS-STATE')
    p.add_argument('--solver',choices=['cpu','gpu','gpu_resident'],default='gpu_resident')
    p.add_argument('--ticks',type=int,default=256);p.add_argument('--warmup',type=int,default=64)
    p.add_argument('--repeats',type=int,default=3);p.add_argument('--probes',type=int,choices=[0,64],default=0)
    p.add_argument('--snapshots',action='store_true',help='include the runtime SnapshotCache after extraction')
    p.add_argument('--decode-frame',action='store_true',help='render decoded feed bytes, including fresh payload validation; excludes network')
    p.add_argument('--distinct',action='store_true',help='different upstream depth in each active region')
    p.add_argument('--out',type=Path,required=True);p.add_argument('--lod',choices=['off','on'],default='on')
    args=p.parse_args()
    if args.ticks<128 or args.warmup<64 or args.repeats<1:p.error('at least 128 measured ticks, 64 warmup ticks and one repeat')
    builds={}
    for text in args.build:
        name,sep,exe=text.partition('=')
        if not sep or not re.fullmatch(r'[A-Za-z0-9_-]+',name)or name in builds:p.error('unique NAME=EXE required')
        exe=Path(exe).resolve()
        if not exe.is_file():p.error(f'missing executable {exe}')
        builds[name]=exe
    cases=args.case or [f'{n}x{r}-{s}'for n in [128,256,512]for r,s in [(1,'active'),(1,'sleep'),(4,'active'),(4,'sleep'),(4,'mixed')]]
    if len(set(cases))!=len(cases):p.error('duplicate cases')
    args.out=args.out.resolve();args.out.mkdir(parents=True,exist_ok=False)
    env={**os.environ,'AMORIS_AA':'msaa','AMORIS_GTAO':'off','AMORIS_OCCLUSION':'off','AMORIS_PREPASS':'off','AMORIS_WATER_CULL':'on','AMORIS_SPLASHES':'on'}
    env.pop('AMORIS_TRACE',None)
    results=[]
    for case in cases:
        match=re.fullmatch(r'(128|256|512)x([1-9]|1[0-7])-(active|sleep|mixed|wake)',case)
        if not match:p.error('cases are 128/256/512 x 1..17 - active/sleep/mixed/wake')
        side,regions,state=int(match[1]),int(match[2]),match[3]
        if state=='mixed'and regions==1:p.error('mixed needs multiple regions')
        destination=args.out/case/'project';project(destination,side,regions,state,args.solver,args.probes,args.distinct)
        wake=args.warmup+128 if state=='wake'else None
        if wake and wake>=args.warmup+args.ticks:p.error('wake fixture needs more than 128 measured ticks')
        references={};physical=None
        for repeat in range(args.repeats):
            order=list(builds.items())
            if len(order)>2:
                # Three repeats put each of three builds in each position once.
                offset=repeat%len(order);order=order[offset:]+order[:offset]
            elif repeat%2:
                order=order[::-1]
            for name,exe in order:
                key=f'{case}-{name}-{repeat}';rows_file=args.out/(key+'-rows.json')
                command=[str(exe),'--bench-project',str(destination),'--ticks',str(args.ticks),'--warmup',str(args.warmup),'--rows',str(rows_file),'--draw-lod',args.lod]
                if wake:command+=['--wake',str(wake)]
                if args.snapshots:command+=['--snapshots']
                if args.decode_frame:command+=['--decode-frame']
                print(key,flush=True)
                done=subprocess.run(command,cwd=REPO,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=300)
                (args.out/(key+'.log')).write_text(done.stderr+done.stdout)
                if done.returncode:raise RuntimeError(f'{key} failed: {done.stderr[-3000:]}')
                data=json.loads(next(line for line in done.stdout.splitlines()if line.startswith('{')))
                rows=json.loads(rows_file.read_text());assert len(rows)==args.ticks+args.warmup
                if args.snapshots:
                    assert data.get('snapshots')is True and rows[0].get('snapshot_ms',0)>0,'snapshot instrumentation was not enabled'
                if args.decode_frame:
                    assert data.get('decode_frame')is True and rows[0].get('decode_ms',0)>0,'feed decoding was not enabled'
                if name in references:assert references[name]==data['world_hash'],f'{key} repeat diverged'
                references[name]=data['world_hash']
                # Execution identity can change with host allocation code; compare physical receipts separately.
                observed=[data['water_state'],data['resident_state']]
                if physical is not None:assert physical==observed,f'{key} physical state/digest diverged'
                physical=observed
                if args.solver=='gpu_resident':
                    states=data['resident_state'];assert len(states)==regions
                    if state=='sleep':assert all(r['WaterResident.still']==60 for r in states)
                    if state=='mixed':assert sum(r['WaterResident.still']==60 for r in states)==regions-1
                    if state=='active':assert all(r['WaterResident.still']<60 for r in states)
                    a=rows[args.warmup-1]['resident'];b=rows[-1]['resident']
                    if state=='sleep':assert a==b,'sleeping service kept uploading/reading/rebuilding'
                    if state=='wake':
                        pre=rows[wake-2]['resident'];sleep=rows[args.warmup-1]['resident'];assert sleep==pre
                        assert rows[wake-1]['resident']['small_readbacks']>pre['small_readbacks'],'wake did not reach GPU'
                phases=summarize(rows,args.warmup,wake)
                if state not in ['sleep','wake']:phases.pop('asleep')
                result={'case':case,'build':name,'repeat':repeat,'solver':args.solver,'snapshots':args.snapshots,'decode_frame':args.decode_frame,'distinct':args.distinct,'warmup':args.warmup,'ticks':args.ticks,'probes_per_region':args.probes,'lod':args.lod,'setup_ms':data['setup_ms'],'first_iteration':rows[0],'phases':phases,'world_hash':data['world_hash'],'execution':data.get('execution'),'resident_service':data['resident_service'],'batch_groups':data.get('batch_groups'),'resident_state':data['resident_state'],'water_state':data['water_state'],'rendering':data['rendering'],'feed_mean_bytes':data['feed_mean_bytes'],'feed_max_bytes':data['feed_max_bytes']}
                results.append(result);(args.out/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
                print(' ',data['complete_tick_publish_draw_wait'],data['resident_service'],flush=True)
    print(args.out/'summary.json',flush=True)
if __name__=='__main__':main()
