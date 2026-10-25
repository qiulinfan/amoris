#!/usr/bin/env python3
"""Continuous collider-bed version of the measured Sea flume; geometry and wet initial state agree."""
import json
import math
from pathlib import Path
root=Path(__file__).resolve().parents[4]
out=root/'tests/fixtures/scenes/water-slope';(out/'scripts').mkdir(parents=True,exist_ok=True)
(out/'project.toml').write_text('name="water-slope"\nrate=60\nseed=1\n')
(out/'scripts/main.ts').write_text('import { game } from "amoris";\n\n// The fixed slope supplies ray-sampled bed heights; Sea drives the resident field.\nexport default game({ components: [], systems: [] });\n')
s=json.loads((root/'tests/fixtures/scenes/water-shore/scene.json').read_text());m=.03;angle=math.atan(m);q=[0,0,math.sin(angle/2),math.cos(angle/2)]
s['entities'][0]['components']={'Transform':{'position':[16,m*16-.25/math.cos(angle),4],'rotation':q},'RigidBody':{'kind':'Fixed'},'Collider':{'shape':{'Cuboid':{'half_extents':[16,.25,4]}}},'Model':{'mesh':'cube','scale':[32,.5,8],'color':[.58,.43,.23,1],'roughness':.9}}
r=s['entities'][1]['components']['WaterRegion'];r['bed']={'Colliders':{'top':2,'bottom':-1}}
for tile in r['tiles']:
    tile['bed']=[m*(tile['key'][0]*16+n%16+.5)*.25 for n in range(256)]
    tile['depth']=[max(.5-b,0) for b in tile['bed']]
xs=[1,3,6,10,13,15.5,16.125,16.625,17.125,17.625,18.125,20]
probes=[e for e in s['entities'] if 'WaterProbe' in e['components']]
for e,x in zip(probes,xs):e['components']['WaterProbe']['at']=[x,4]
(out/'scene.json').write_text(json.dumps(s,separators=(',',':'))+'\n')
