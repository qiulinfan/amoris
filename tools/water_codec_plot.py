#!/usr/bin/env python3
"""Matched checkpoint transport costs. Run-median ranges are not confidence intervals."""
import argparse
import json
from pathlib import Path
from statistics import mean, median

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
data = json.loads((a.root / 'matched/summary.json').read_text())
cpu = json.loads((a.root / 'cpu.json').read_text())
fig, axes = plt.subplots(1, 3, figsize=(15.5, 5.2))
colors = {'before': '#718096', 'after': '#07899b'}
for build, shift, label in [('before', -.18, '改进前'), ('after', .18, '改进后')]:
    groups = [[r['phases']['checkpoint']['complete_ms']['median_ms'] for r in data
               if r['case'] == f'{side}x4-active' and r['build'] == build]
              for side in [128, 256, 512]]
    assert all(len(g) == 3 for g in groups)
    values = [median(g) for g in groups]
    axes[0].bar([i + shift for i in range(3)], values, .32, color=colors[build], label=label,
                yerr=[[v-min(g) for v, g in zip(values, groups)],
                      [max(g)-v for v, g in zip(values, groups)]], capsize=3)
    for i, v in enumerate(values):
        axes[0].annotate(f'{v:.1f}', (i+shift, max(groups[i])), xytext=(0, 4),
                         textcoords='offset points', ha='center', fontsize=9)
axes[0].set(xticks=[0, 1, 2], xticklabels=['128²', '256²', '512²'],
            ylabel='完整检查点帧（ms）', title='四个不同水场：快照到实际接收绘制')
axes[0].legend(frameon=False)
names = ['步进', '快照', '提取', '编码', '解码', '绘制准备', 'GPU 等待']
phase_colors = ['#536778', '#926da6', '#d5c098', '#d49742', '#bc6e65', '#4a9b9a', '#afc9cc']
for x, build in enumerate(['before', 'after']):
    rows = []
    for path in (a.root / 'matched').glob(f'512x4-active-{build}-*-rows.json'):
        rows.extend(r for r in json.loads(path.read_text()) if r['tick'] > 64 and r['checkpoints'])
    assert len(rows) == 12
    values = [mean(r[k] for r in rows) for k in
              ['step_ms', 'snapshot_ms', 'extract_ms', 'encode_ms', 'decode_ms']]
    values += [mean(sum(r['draw_phases'][:2]) for r in rows),
               mean(r['draw_phases'][2] for r in rows)]
    bottom = 0
    for color, name, value in zip(phase_colors, names, values):
        axes[1].bar(x, value, .5, bottom=bottom, color=color, label=name if x == 0 else None)
        bottom += value
    axes[1].annotate(f'{bottom:.1f}', (x, bottom), xytext=(0, 4), textcoords='offset points', ha='center')
axes[1].set(xticks=[0, 1], xticklabels=['改进前', '改进后'], ylabel='墙钟时间平均分项（ms）',
            title='512² × 4：12 个检查点帧的平均分项')
axes[1].legend(frameon=False, ncol=2, fontsize=8, loc='upper right')
for build, shift, label in [('before', -.18, '改进前'), ('after', .18, '改进后')]:
    values = [median(r['bytes'] for r in cpu if r['kind'] == kind and r['build'] == build) / 1e6
              for kind in ['dam', 'flat', 'dense']]
    axes[2].bar([i+shift for i in range(3)], values, .32, color=colors[build], label=label)
    for i, v in enumerate(values):
        axes[2].annotate(f'{v:.3f}' if v < 1 else f'{v:.2f}', (i+shift, v), xytext=(0, 4),
                         textcoords='offset points', ha='center', fontsize=9)
axes[2].set(xticks=[0, 1, 2], xticklabels=['合成坝流', '静水', '高熵合法值'],
            ylabel='消息体积（MB，十进制）', title='CPU 固定输入：四个 512² 检查点')
for ax in axes:
    ax.spines[['right', 'top']].set_visible(False)
    ax.grid(axis='y', alpha=.18)
    ax.set_axisbelow(True)
    ax.set_ylim(0, ax.get_ylim()[1] * 1.12)
fig.suptitle('GPU 水检查点：存档编码与无损消息传输', fontsize=16, y=.98)
fig.text(.5, .025, '左：三轮中位数及范围；中：平均墙钟分项；含快照、解码和绘制等待，不含网络。Apple M5 / Metal。',
         ha='center', fontsize=9)
fig.tight_layout(rect=(0, .06, 1, .92))
for suffix in ['png', 'svg']:
    fig.savefig(a.root / ('comparison.' + suffix), dpi=180)
