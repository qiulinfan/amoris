#!/usr/bin/env python3
"""Damped translating-parabola reference against production GPU depth/front histories."""
import json
from pathlib import Path
import sys
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root = Path(sys.argv[1])
d = json.loads((root / 'shoreline.json').read_text())
runs = [r for r in d['runs'] if r['shift'] > 0 and r['dt'] == .01]
fig, axes = plt.subplots(2, 2, figsize=(10, 7))
for r in runs:
    label = f"dx={r['dx']:g} m"
    t = [p['time'] for p in r['trajectory']]
    axes[0, 1].plot(t, [p['fronts'][1]['actual'][1] for p in r['trajectory']], label=label)
    axes[1, 1].plot(t, [p['dry_reference_volume'] * 1000 for p in r['trajectory']], label=label)
r = runs[-1]
p = next(p for p in r['profiles'] if p['time'] == 8)
axes[0, 0].plot(p['x'], p['reference_depth'], '--', color='black', label='Analytic depth')
axes[0, 0].plot(p['x'], p['depth'], color='#168a9e', label='GPU, dx=0.25 m')
axes[0, 0].set(xlim=(-9, 9), xlabel='Centred x (m)', ylabel='Water depth (m)', title='Point-sampled profile at 8 s')
axes[0, 1].plot(t, [p['fronts'][1]['reference'][1] for p in r['trajectory']], '--', color='black', label='Analytic 1-mm contour')
axes[0, 1].set(xlabel='Time (s)', ylabel='Right 1-mm depth contour x (m)', title='Retained films prevent accurate recession')
for dt in [.01, .005]:
    rs = [r for r in d['runs'] if r['shift'] > 0 and r['dt'] == dt]
    axes[1, 0].plot([r['dx'] for r in rs], [r['max_l1_depth'] * 1000 for r in rs], 'o-', label=f'dt={dt:g} s')
axes[1, 0].set(xlabel='Cell width (m)', ylabel='Maximum domain-mean absolute depth error (mm)', title='Time halving leaves spatial error')
axes[1, 1].set(xlabel='Time (s)', ylabel='Water where reference is dry (L)', title='Sampled support diagnostic, not lost mass')
for a in axes.flat:
    a.grid(alpha=.2)
    a.spines[['right', 'top']].set_visible(False)
    a.legend(frameon=False, fontsize=8)
fig.suptitle('Unchanged production GPU MAC · independently checked damped moving shoreline')
fig.tight_layout(rect=(0, 0, 1, .96))
for suffix in ['png', 'svg']:
    fig.savefig(root / ('shoreline-quality.' + suffix), dpi=160)
