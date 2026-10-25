#!/usr/bin/env python3
"""Matched first-order/MUSCL moving-shore reference; all data are actual GPU checkpoints."""
import json
from pathlib import Path
import sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(sys.argv[1])
sets=[('Prior depth flux',json.loads((root/'first-order/shoreline.json').read_text()),'#b16b35'),('Limited linear depth flux',json.loads((root/'production/shoreline.json').read_text()),'#168a9e')]
fig,axes=plt.subplots(2,2,figsize=(10,7))
for label,data,color in sets:
    runs=[r for r in data['runs'] if r['shift']>0 and r['dt']==.01]
    axes[0,0].plot([r['dx'] for r in runs],[r['max_l1_depth']*1000 for r in runs],'o-',label=label,color=color)
    fine=runs[-1];t=[r['time'] for r in fine['trajectory']]
    axes[0,1].plot(t,[r['fronts'][1]['actual'][1] for r in fine['trajectory']],label=label,color=color)
    p=next(p for p in fine['profiles'] if p['time']==8)
    axes[1,0].plot(p['x'],[h*1000 for h in p['depth']],label=label,color=color)
    axes[1,1].plot(t,[r['velocity_l1']*1000 for r in fine['trajectory']],label=label,color=color)
axes[0,1].plot(t,[r['fronts'][1]['reference'][1] for r in fine['trajectory']],'--',color='black',label='Analytic 1-mm contour')
axes[1,0].plot(p['x'],[h*1000 for h in p['reference_depth']],'--',color='black',label='Analytic depth')
axes[0,0].set(xlabel='Cell width (m)',ylabel='Maximum domain-mean depth error (mm)',title='Depth improves across tested grids')
axes[0,1].set(xlabel='Time (s)',ylabel='Right 1-mm depth contour x (m)',title='Recession improves; retained films remain')
axes[1,0].set(xlabel='Centred x (m)',ylabel='Depth at 8 s (mm)',title='Resolved profile near the moving shore',xlim=(7,9),ylim=(0,60))
axes[1,1].set(xlabel='Time (s)',ylabel='Reference-depth-weighted velocity error (mm/s)',title='Velocity improves with consistent depth flux')
for a in axes.flat:
    a.grid(alpha=.2);a.spines[['right','top']].set_visible(False);a.legend(frameon=False,fontsize=8)
fig.suptitle('Production GPU moving-shore study · same pressure/advection, changed depth transport')
fig.tight_layout(rect=(0,0,1,.96))
for suffix in ['png','svg']:fig.savefig(root/('comparison.'+suffix),dpi=160)
