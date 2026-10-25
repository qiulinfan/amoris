#!/usr/bin/env python3
"""Plot matched complete-loop measurements; whiskers are run-median ranges, never confidence bars."""
import argparse
import json
from pathlib import Path
from statistics import median
import matplotlib
matplotlib.use('Agg')
from matplotlib import font_manager
import matplotlib.pyplot as plt

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('root',type=Path)
p.add_argument('--font',type=Path,required=True,help='installed Chinese font; the font file is not copied')
p.add_argument('--out',type=Path)
a=p.parse_args();font_manager.fontManager.addfont(str(a.font))
plt.rcParams.update({'font.family':font_manager.FontProperties(fname=a.font).get_name(),'font.size':10,'axes.unicode_minus':False,'svg.fonttype':'path'})
base=json.loads((a.root/'matrix/summary.json').read_text())
work=json.loads((a.root/'working-set-wake/summary.json').read_text())
def values(data,case,build,phase):
    rows=[r for r in data if r['case']==case and r['build']==build]
    assert len(rows)==3,(case,build,len(rows))
    return [r['phases'][phase]['complete_ms']['median_ms']for r in rows]
def point(v):return median(v),median(v)-min(v),max(v)-median(v)
def bars(ax,x,groups,color,label,width=.32):
    stats=[point(v)for v in groups]
    ax.bar(x,[s[0]for s in stats],width,color=color,label=label,yerr=[[s[1]for s in stats],[s[2]for s in stats]],capsize=3,error_kw={'lw':1})
    for at,s in zip(x,stats):ax.annotate(f'{s[0]:.1f}',(at,s[0]+s[2]),xytext=(0,4),textcoords='offset points',ha='center',fontsize=9)
fig,axes=plt.subplots(1,3,figsize=(14,4.8))
old,new,check='#718096','#07899b','#bd701b'
for build,color,shift,label in [('before',old,-.18,'改进前'),('after',new,.18,'改进后')]:
    bars(axes[0],[shift,1+shift],[values(work,'128x5-active',build,phase)for phase in ['ordinary','checkpoint']],color,label)
axes[0].set(xticks=[0,1],xticklabels=['普通帧','检查点帧'],ylabel='完整测量环路（ms）',title='五个 128²：避免循环逐出',ylim=(0,132))
axes[0].legend(frameon=False,loc='upper left')
for phase,color,shift,label in [('ordinary',new,-.18,'普通帧'),('checkpoint',check,.18,'检查点帧')]:
    bars(axes[1],[i+shift for i in range(3)],[values(base,f'{side}x4-active','after',phase)for side in [128,256,512]],color,label)
axes[1].set(xticks=[0,1,2],xticklabels=['128²','256²','512²'],title='四个活动区域：大场景停顿仍在',ylabel='改进后完整环路（ms）',ylim=(0,86))
axes[1].legend(frameon=False,loc='upper left')
for count,color,label in [(1,new,'一个区域'),(4,old,'四个区域')]:
    group=[point(values(base,f'{side}x{count}-sleep','after','ordinary'))for side in [128,256,512]]
    axes[2].errorbar([0,1,2],[s[0]for s in group],yerr=[[s[1]for s in group],[s[2]for s in group]],fmt='o-',color=color,label=label,capsize=3)
    for i,s in enumerate(group):axes[2].annotate(f'{s[0]:.1f}',(i,s[0]),xytext=(0,6),textcoords='offset points',ha='center',fontsize=9)
axes[2].set(xticks=[0,1,2],xticklabels=['128²','256²','512²'],title='休眠停止求解，水面仍要绘制',ylabel='改进后完整环路（ms）',ylim=(0,12))
axes[2].legend(frameon=False,loc='upper left')
for ax in axes:
    ax.spines[['right','top']].set_visible(False);ax.grid(axis='y',alpha=.18);ax.set_axisbelow(True)
fig.suptitle('GPU 水系统：完整测量环路对照（Apple M5 / Metal）',fontsize=16,y=.98)
fig.text(.5,.025,'1280×720 MSAA、LOD；每组 3 轮；线段为各轮中位数范围；含编码和 GPU 等待，不含网络/解码。',ha='center',fontsize=10)
fig.tight_layout(rect=(0,.06,1,.92));destination=a.out or a.root/'comparison';destination.parent.mkdir(parents=True,exist_ok=True)
for suffix in ['png','svg']:fig.savefig(destination.with_suffix('.'+suffix),dpi=180)
