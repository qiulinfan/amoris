#!/usr/bin/env python3
"""Continuous collider-bed flume: actual probe extrema and independent mass accounting."""
import json
from pathlib import Path
import sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root = Path(sys.argv[1])
d = json.loads((root / 'slope.json').read_text())
rows = d['rows']
t = [r['time'] for r in rows]
x = np.array([p['WaterProbe']['at'][0] for p in rows[0]['probes']])
h = np.array([[p['WaterProbe']['depth'] for p in r['probes']] for r in rows])
surface = np.array([[p['WaterProbe']['surface'] for p in r['probes']] for r in rows])
bed = surface[0] - h[0]
fig, axes = plt.subplots(1, 3, figsize=(12, 3.6))
axes[0].plot(x, bed, color='#987246', label='Sampled physical bed')
wet = h.max(axis=0) > 1e-6
axes[0].plot(x, np.where(wet, bed + h.max(axis=0), np.nan), 'o-', color='#168a9e', label='Maximum wet probe surface')
axes[0].scatter(x[~wet], bed[~wet], marker='x', color='gray', label='Never-wet probe')
axes[0].axhline(.5, color='gray', linestyle='--', label='Mean Sea level')
axes[0].set(xlim=(13, 20), ylim=(.38, .62), xlabel='x (m)', ylabel='World height (m)', title='Continuous slope; dry samples stay on bed')
for i in [6, 7, 8, 9]:
    axes[1].plot(t, h[:, i] * 1000, label=f'x={x[i]:g} m')
axes[1].set(xlabel='Simulation time (s)', ylabel='Water depth (mm)', title='Thin films and never-wet probes remain')
defects = []
for c in d['checkpoints']:
    b = c['boundary']
    defects.append(c['actual_volume'] - b['initial_volume'] - b['entered_total'] + b['left_total'] - b['edited_total'] - b['configuration_total'])
axes[2].plot([c['tick'] / 60 for c in d['checkpoints']], np.array(defects) * 1e6, color='#168a9e')
axes[2].set(xlabel='Simulation time (s)', ylabel='Independent volume defect (mL)', title='Complete field versus boundary ledger')
for a in axes:
    a.grid(alpha=.2)
    a.spines[['right', 'top']].set_visible(False)
axes[0].legend(frameon=False, fontsize=7)
axes[1].legend(frameon=False, fontsize=8)
fig.suptitle('Actual resident GPU Game · collider-bed geometry, not certified shoreline accuracy')
fig.tight_layout()
for suffix in ['png', 'svg']:
    fig.savefig(root / ('slope-quality.' + suffix), dpi=160)
result = {'scope': 'actual native resident GPU flume; not a timing or runup-accuracy claim', 'max_volume_defect': max(abs(v) for v in defects), 'probes': [{'x': float(at), 'min_depth': float(h[:, i].min()), 'max_depth': float(h[:, i].max())} for i, at in enumerate(x)]}
(root / 'slope-derived.json').write_text(json.dumps(result, indent=2) + '\n')
