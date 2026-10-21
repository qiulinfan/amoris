#!/usr/bin/env python3
"""Plot cyclic three-build resident-slot and ordered-input replay measurements."""
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
p.add_argument('--final', action='store_true', help='plot the final paired production matrix')
a = p.parse_args()
font_manager.fontManager.addfont(str(a.font))
plt.rcParams.update({'font.family': font_manager.FontProperties(fname=a.font).get_name(),
                     'axes.unicode_minus': False, 'font.size': 10, 'svg.fonttype': 'path'})
data = json.loads((a.root / ('final-matched' if a.final else 'matched') / 'summary.json').read_text())
cases = ['128x16-active', '128x17-active', '512x4-active', '512x5-active']
labels = ['128²×16', '128²×17', '512²×4', '512²×5']
variants = ([('before', '原实现', '#718096'), ('final', '最终版本', '#07899b')] if a.final else
            [('before', '原实现', '#718096'), ('recycle', '槽位复用', '#07899b'),
             ('recorded', '整段回放候选', '#d28b38')])
fig, axes = plt.subplots(1, 3, figsize=(15.5, 5.7), gridspec_kw={'width_ratios': [1, 1, .85]})
for ax, metric, selected, names, quantile, title in [
    (axes[0], 'water_ms', cases, labels, 'median_ms', '普通帧：水系统'),
    (axes[1], 'complete_ms', cases, labels, 'median_ms', '普通帧：完整循环'),
    (axes[2], 'complete_ms', [cases[1], cases[3]], [labels[1], labels[3]],
     'p95_ms', '预算外：完整循环 p95'),
]:
    for j, (build, label, color) in enumerate(variants):
        groups = [[r['phases']['ordinary'][metric][quantile] for r in data
                   if r['case'] == case and r['build'] == build] for case in selected]
        assert all(len(group) == 3 for group in groups), 'exactly three runs per case/build'
        values = [median(group) for group in groups]
        positions = [i + (j-(len(variants)-1)/2)*.28 for i in range(len(selected))]
        ax.bar(positions, values, .23, color=color, label=label,
               yerr=[[v-min(g) for v, g in zip(values, groups)],
                     [max(g)-v for v, g in zip(values, groups)]], capsize=3)
        for i, group in enumerate(groups):
            ax.scatter([positions[i]+.025*(k-1) for k in range(3)], group,
                       s=12, color='#263746', zorder=3)
    ax.set(title=title, ylabel='墙钟时间（ms）', xticks=range(len(selected)), xticklabels=names)
    ax.tick_params(axis='x', labelrotation=25)
    ax.spines[['right', 'top']].set_visible(False)
    ax.grid(axis='y', alpha=.18)
    ax.set_axisbelow(True)
    ax.set_ylim(0, ax.get_ylim()[1]*1.12)
axes[0].legend(frameon=False, fontsize=9)
fig.suptitle('GPU 水：逐出槽位资源复用（最终版本）' if a.final else
             'GPU 水：资源复用与历史回放候选研究', fontsize=16, y=.98)
fig.text(.5, .055, '每项三轮进程统计的中位数及范围，黑点为各轮；范围不是置信区间。全部原始帧及异常值保留。',
         ha='center', fontsize=9)
fig.text(.5, .018, 'Apple M5 / Metal · Game、快照、编码、真实解码、绘制和等待；不含网络。预算与完整检查点频率保持一致。',
         ha='center', fontsize=9)
fig.tight_layout(rect=(0, .085, 1, .92))
for suffix in ['png', 'svg']:
    fig.savefig(a.root / (('final-comparison.' if a.final else 'comparison.') + suffix), dpi=180)
