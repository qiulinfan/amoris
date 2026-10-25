#!/usr/bin/env python3
"""Actual game probe/volume evidence and independently fitted shallow-wave propagation."""
import json
import math
from pathlib import Path
import sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(sys.argv[1]);omega=math.sqrt(9.81*2*math.pi/16);speed=math.sqrt(9.81*.125)
fig,axes=plt.subplots(2,2,figsize=(11,7));derived=[]
for name in ['tank-050','tank-025']:
    d=json.loads((root/(name+'.json')).read_text());rs=[r for r in d['rows'] if r['time']>=8];t=np.array([r['time'] for r in rs]);a=np.stack([np.sin(omega*t),np.cos(omega*t),np.ones_like(t)],axis=1);fits=[]
    for i in range(3):
        y=np.array([r['probes'][i]['WaterProbe']['surface'] for r in rs]);v=np.linalg.lstsq(a,y,rcond=None)[0];amp=np.hypot(*v[:2]);phase=np.arctan2(v[1],-v[0]);rms=np.sqrt(np.mean((a@v-y)**2));fits.append(dict(amplitude=float(amp),phase=float(phase),rms=float(rms)))
        if name=='tank-025' and i<2:axes[0,1].plot(t,(y-np.mean(y))*1000,label=f"x={rs[0]['probes'][i]['WaterProbe']['at'][0]:g} m")
    measured=[omega*2/((fits[i+1]['phase']-fits[i]['phase'])%(2*np.pi)) for i in range(2)];decay=[fits[i+1]['amplitude']/fits[i]['amplitude'] for i in range(2)];axes[0,0].plot([d['cell_m']]*2,measured,'o',color='#158c91')
    defect=[c['actual_volume']-c['boundary']['initial_volume']-c['boundary']['entered_total']+c['boundary']['left_total'] for c in d['checkpoints']];axes[1,1].plot([c['tick']/60 for c in d['checkpoints']],np.array(defect)*1e6,label=name)
    derived.append(dict(case=name,cell_m=d['cell_m'],fits=fits,speeds=measured,reference_speed=speed,amplitude_ratios=decay,expected_ratio=math.exp(-.2/speed),max_volume_defect=max(abs(v) for v in defect)))
axes[0,0].axhline(speed,color='gray',linestyle='--',label='sqrt(gH)');axes[0,0].set(xlabel='Cell width (m)',ylabel='Measured phase speed (m/s)',title='Actual Game: same frequency, shallow propagation');axes[0,0].legend(frameon=False)
axes[0,1].set(xlabel='Simulation time (s)',ylabel='Surface anomaly (mm)',title='Current script probes, dx=0.25 m');axes[0,1].legend(frameon=False)
d=json.loads((root/'shore.json').read_text());t=[r['time'] for r in d['rows']]
for i in [6,7,8,9]:axes[1,0].plot(t,[r['probes'][i]['WaterProbe']['depth']*1000 for r in d['rows']],label=f"x={d['rows'][0]['probes'][i]['WaterProbe']['at'][0]:g} m")
axes[1,0].set(xlabel='Simulation time (s)',ylabel='Water on initially dry terrace (mm)',title='Stepped flume wetting; residual films remain');axes[1,0].legend(frameon=False,ncol=2,fontsize=8)
defect=[c['actual_volume']-c['boundary']['initial_volume']-c['boundary']['entered_total']+c['boundary']['left_total'] for c in d['checkpoints']];axes[1,1].plot([c['tick']/60 for c in d['checkpoints']],np.array(defect)*1e6,label='stepped shore');axes[1,1].set(xlabel='Simulation time (s)',ylabel='Independent volume defect (mL)',title='Complete checkpoints versus actual boundary transfers');axes[1,1].legend(frameon=False)
derived.append(dict(case='stepped shore',max_volume_defect=max(abs(v) for v in defect),scope='wetting/film-depth response; continuous-beach runup/drainback remains open'))
for axis in axes.flat:axis.grid(alpha=.2);axis.spines[['right','top']].set_visible(False)
fig.suptitle('Sea-driven resident GPU water · real game ticks and physical probe oracle');fig.tight_layout(rect=(0,0,1,.96));fig.savefig(root/'sea-quality.png',dpi=160);fig.savefig(root/'sea-quality.svg');(root/'derived.json').write_text(json.dumps(derived,indent=2)+'\n')
