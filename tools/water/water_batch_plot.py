#!/usr/bin/env python3
"""Plot matched resident-cohort costs and the separate one-pair cache-budget controls."""
import argparse
import json
from pathlib import Path
from statistics import median

import matplotlib
matplotlib.use('Agg')
from matplotlib import font_manager
import matplotlib.pyplot as plt

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('root', type=Path)
p.add_argument('--font', type=Path, required=True)
a = p.parse_args()
font_manager.fontManager.addfont(str(a.font))
plt.rcParams.update({'font.family': font_manager.FontProperties(fname=a.font).get_name(),
                     'axes.unicode_minus': False, 'font.size': 10, 'svg.fonttype': 'path'})
matched = json.loads((a.root / 'matched/summary.json').read_text())
edges = json.loads((a.root / 'edges/summary.json').read_text())
cases = ['128x4-active', '256x4-active', '512x4-active', '128x5-active', '128x16-active']
labels = ['128²×4', '256²×4', '512²×4', '128²×5', '128²×16']
colors = {'before': '#718096', 'batch': '#07899b'}
fig, axes = plt.subplots(1, 3, figsize=(15.5, 5.6), gridspec_kw={'width_ratios': [1.2, 1.2, 1]})
for build, shift, label in [('before', -.18, '改进前'), ('batch', .18, '批处理')]:
    for ax, metric in zip(axes[:2], ['water_ms', 'complete_ms']):
        groups = [[r['phases']['ordinary'][metric]['median_ms'] for r in matched
                   if r['case'] == case and r['build'] == build] for case in cases]
        assert all(len(g) == 3 for g in groups)
        values = [median(g) for g in groups]
        ax.bar([i + shift for i in range(len(cases))], values, .32, color=colors[build],
               label=label, yerr=[[v-min(g) for v, g in zip(values, groups)],
                                  [max(g)-v for v, g in zip(values, groups)]], capsize=3)
        for i, group in enumerate(groups):
            ax.scatter([i+shift-.035, i+shift, i+shift+.035], group,
                       s=12, color='#263746', zorder=3)
    rows = [[r for r in edges if r['case'] == case and r['build'] == build]
            for case in ['128x17-active', '512x5-active']]
    assert all(len(g) == 1 for g in rows)
    values = [g[0]['phases']['ordinary']['complete_ms']['median_ms'] for g in rows]
    tails = [g[0]['phases']['ordinary']['complete_ms']['p95_ms'] for g in rows]
    axes[2].bar([i+shift for i in range(2)], values, .32, color=colors[build], label=label)
    axes[2].scatter([i+shift for i in range(2)], tails, marker='D', s=25,
                    color=colors[build], label='p95' if build == 'before' else None)
    for i, v in enumerate(values):
        axes[2].annotate(f'{v:.0f}', (i+shift, v), xytext=(0, 4), textcoords='offset points',
                         ha='center', fontsize=9)
axes[0].set(title='普通帧：水系统', ylabel='墙钟时间（ms）', xticks=range(5), xticklabels=labels)
axes[1].set(title='普通帧：完整循环', ylabel='墙钟时间（ms）', xticks=range(5), xticklabels=labels)
axes[2].set(title='超预算：单轮探索，仍有严重抖动', ylabel='完整普通帧（ms）', xticks=range(2),
            xticklabels=['128²×17\n超过 16 区域', '512²×5\n超过列数预算'])
axes[0].legend(frameon=False)
axes[2].legend(frameon=False, fontsize=8, loc='upper right')
for ax in axes:
    ax.spines[['right', 'top']].set_visible(False)
    ax.grid(axis='y', alpha=.18)
    ax.set_axisbelow(True)
    ax.set_ylim(0, ax.get_ylim()[1] * 1.12)
for ax in axes[:2]:
    ax.tick_params(axis='x', labelrotation=25)
fig.suptitle('GPU 水多区域批处理：完整路径收益与缓存边界', fontsize=16, y=.98)
fig.text(.5, .055, '左、中：三轮中位数及范围，黑点为每轮；范围不是置信区间。右：单轮中位数与 p95。',
         ha='center', fontsize=9)
fig.text(.5, .018, 'Apple M5 / Metal · 包含 Game、快照、编码、真实解码、绘制和等待；不含网络。数值解法与逐区域回读量保持一致。',
         ha='center', fontsize=9)
fig.tight_layout(rect=(0, .085, 1, .92))
for suffix in ['png', 'svg']:
    fig.savefig(a.root / ('comparison.' + suffix), dpi=180)
