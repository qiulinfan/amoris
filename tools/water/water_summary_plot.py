#!/usr/bin/env python3
"""Matched summary-pass and selected-group protection costs; retain every run and tail."""
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
main = json.loads((a.root / 'matched/summary.json').read_text())
edges = json.loads((a.root / 'edges/summary.json').read_text())
cases = ['128x1-active', '512x1-active', '512x4-active', '128x16-active']
colors = {'before': '#718096', 'summary': '#07899b', 'final': '#d28b38'}
fig, axes = plt.subplots(1, 3, figsize=(15.5, 5.7), gridspec_kw={'width_ratios': [1.1, 1.1, 1]})

def bars(ax, data, selected, metric, builds):
    width = .23 if len(builds) == 3 else .32
    for build, shift, label in builds:
        rows = [[r['phases']['ordinary'][metric] for r in data
                 if r['case'] == case and r['build'] == build] for case in selected]
        assert all(len(group) == 3 for group in rows)
        groups = [[r['median_ms'] for r in group] for group in rows]
        values = [median(group) for group in groups]
        positions = [i + shift for i in range(len(selected))]
        ax.bar(positions, values, width, color=colors[build], label=label,
               yerr=[[v-min(g) for v, g in zip(values, groups)],
                     [max(g)-v for v, g in zip(values, groups)]], capsize=3)
        for i, group in enumerate(groups):
            ax.scatter([positions[i]+.025*(j-1) for j in range(3)], group,
                       s=10, color='#263746', zorder=3)
        if data is edges:
            for i, group in enumerate(rows):
                ax.scatter([positions[i]+.025*(j-1) for j in range(3)],
                           [r['p95_ms'] for r in group], marker='D', s=20,
                           color=colors[build], zorder=3,
                           label='各轮 p95' if i == 0 and build == 'before' else None)

variants = [('before', -.25, '原实现'), ('summary', 0, '摘要合并'), ('final', .25, '加整组保护')]
bars(axes[0], main, cases, 'water_ms', variants)
bars(axes[1], main, cases, 'complete_ms', variants)
bars(axes[2], edges, ['128x17-active', '512x5-active'], 'complete_ms',
     [('before', -.18, '原实现'), ('final', .18, '最终版本')])
for ax, title, ylabel in zip(axes[:2], ['普通帧：水系统', '普通帧：完整循环'],
                            ['墙钟时间（ms）', '墙钟时间（ms）']):
    ax.set(title=title, ylabel=ylabel, xticks=range(4),
           xticklabels=['128²×1', '512²×1', '512²×4', '128²×16'])
    ax.tick_params(axis='x', labelrotation=25)
axes[2].set(title='缓存预算之外：回放成本仍然存在', ylabel='完整普通帧（ms）', xticks=range(2),
            xticklabels=['128²×17', '512²×5'])
axes[0].legend(frameon=False, fontsize=9)
axes[2].legend(frameon=False, fontsize=8, loc='upper right')
for ax in axes:
    ax.spines[['right', 'top']].set_visible(False)
    ax.grid(axis='y', alpha=.18)
    ax.set_axisbelow(True)
    ax.set_ylim(0, ax.get_ylim()[1] * 1.10)
fig.suptitle('GPU 水：有序摘要归约与执行组缓存保护', fontsize=16, y=.98)
fig.text(.5, .055, '每项三轮进程中位数及范围，黑点为各轮；右图菱形为各轮 p95。范围不是置信区间。',
         ha='center', fontsize=9)
fig.text(.5, .018, 'Apple M5 / Metal · 包含 Game、快照、编码、真实解码、绘制和等待，不含网络。预算和完整检查点频率保持一致。',
         ha='center', fontsize=9)
fig.tight_layout(rect=(0, .085, 1, .92))
for suffix in ['png', 'svg']:
    fig.savefig(a.root / ('comparison.' + suffix), dpi=180)
