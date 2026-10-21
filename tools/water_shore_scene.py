#!/usr/bin/env python3
"""Author the reproducible stepped-shore wave flume from its actual voxel bed."""
import base64
import json
import math
from pathlib import Path

root=Path(__file__).resolve().parents[1]
out=root/'samples/water-shore'
out.mkdir(exist_ok=True)
(out/'scripts').mkdir(exist_ok=True)
(out/'scripts/main.ts').write_text('import { game } from "amoris";\n\n// Scene Sea waves drive the actual resident GPU field.\nexport default game({ components: [], systems: [] });\n')
(out/'project.toml').write_text('name = "water-shore"\nrate = 60\nseed = 1\n')
nx,nz,cell=128,32,.25
step=lambda x: 0 if x<48 else 1 if x<80 else 2 if x<112 else 3
chunks=[]
for z in range(nz//16):
    for x in range(nx//16):
        cells=bytearray(4096)
        for j in range(16):
            for i in range(16):
                for y in range(step(x*16+i)+1):cells[i+16*(j+16*y)]=1
        chunks.append({'key':[x,0,z],'cells':base64.b64encode(cells).decode()})
chunks.sort(key=lambda c:c["key"])
tiles=[]
for z in range(nz//16):
    for x in range(nx//16):
        bed=[step(x*16+n%16)*cell for n in range(256)]
        tiles.append({'key':[x,z],'bed':bed,'depth':[max(.5-b,0) for b in bed],'u':[0]*256,'w':[0]*256})
entities=[
 {'name':'Terrain','components':{'Transform':{'position':[0,-.25,0]},'Voxels':{'cell_m':cell,'palette':[{'name':'air'},{'name':'sand','solid':True,'faces':['materials.gltf#sand']*3}],'chunks':chunks}}},
 {'name':'Shore','components':{'WaterRegion':{'solver':'gpu_resident','cell_m':cell,'size':[nx,nz],'bed':{'Voxels':{'volume':1,'per':1,'y_lo':0,'y_hi':4}},'tiles':tiles,'edges':['Sea','Wall','Wall','Wall'],'substeps':2,'dry_below':0,'friction':.02}}},
 {'name':'Sea','components':{'Sea':{'level':.5,'density':1000,'waves':[{'length':64,'height':.08,'toward_deg':90,'phase':0},{'length':32,'height':.02,'toward_deg':112,'phase':1.1},{'length':8,'height':.01,'toward_deg':65,'phase':2.0}]}}}
]
# Reuse this repository's authored floater parameters; all sample geometry/materials are local.
floater=next(e for e in json.loads((root/'samples/water-river/scene.json').read_text())['entities'] if e['name']=='Float0')
floater['name']='Float';floater['components']['Transform']['position']=[4,.59,4]
floater['components']['Velocity']={'linear':[0,0,0],'angular':[0,0,0]}
entities.append(floater)
for z in [-.125,8.125]:
    entities.append({'name':'FlumeSouth' if z<0 else 'FlumeNorth','components':{'Transform':{'position':[16,.35,z]},'Model':{'mesh':'cube','scale':[32,1.2,.25],'color':[.45,.48,.5,1]},'RigidBody':{'kind':'Fixed'},'Collider':{'shape':{'Cuboid':{'half_extents':[16,.6,.125]}}}}})
lighting=json.loads((root/'samples/water-river/scene.json').read_text())['entities']
entities.extend(e for e in lighting if e['name'] in ['Sun','Sky'])
eye=[22,9,15];target=[14,.5,4]
f=[b-a for a,b in zip(eye,target)];norm=math.sqrt(sum(v*v for v in f));f=[v/norm for v in f]
r=[-f[2],0,f[0]];norm=math.sqrt(sum(v*v for v in r));r=[v/norm for v in r]
u=[r[1]*f[2]-r[2]*f[1],r[2]*f[0]-r[0]*f[2],r[0]*f[1]-r[1]*f[0]]
m=[[r[i],u[i],-f[i]] for i in range(3)]
w=math.sqrt(1+m[0][0]+m[1][1]+m[2][2])/2
q=[(m[2][1]-m[1][2])/(4*w),(m[0][2]-m[2][0])/(4*w),(m[1][0]-m[0][1])/(4*w),w]
entities.append({'name':'View','components':{'Transform':{'position':eye,'rotation':q},'Camera':{'active':True}}})
for i,x in enumerate([1,3,6,10,14,18,20.125,21,22,24,28,31]):
    entities.append({'name':f'Gauge{i}','components':{'WaterProbe':{'region':2,'at':[x,4]}}})
(out/'scene.json').write_text(json.dumps({'format':'amoris-scene','version':1,'entities':entities},separators=(',',':'))+'\n')
(out/'materials.gltf').write_text(json.dumps({'asset':{'version':'2.0'},'materials':[{'name':'sand','pbrMetallicRoughness':{'baseColorFactor':[.58,.43,.23,1],'metallicFactor':0,'roughnessFactor':.9}}],'scenes':[{'nodes':[]}],'scene':0},indent=2)+'\n')
