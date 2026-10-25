#!/usr/bin/env python3
"""Checked checkpoint measurements. Ranges are three run medians, not confidence intervals."""
import argparse,json
from pathlib import Path
from statistics import median,mean
import matplotlib
matplotlib.use('Agg')
from matplotlib import font_manager
import matplotlib.pyplot as plt
p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);p.add_argument('--font',type=Path,required=True);a=p.parse_args()
font_manager.fontManager.addfont(str(a.font));plt.rcParams.update({'font.family':font_manager.FontProperties(fname=a.font).get_name(),'axes.unicode_minus':False,'font.size':10,'svg.fonttype':'path'})
matched=json.loads((a.root/'matched/summary.json').read_text());snapshots=json.loads((a.root/'snapshots/summary.json').read_text())
fig,axes=plt.subplots(1,3,figsize=(14,4.9));colors={'before':'#718096','checked':'#07899b'}
def bars(ax,data):
 for build,shift,label in [('before',-.18,'改进前'),('checked',.18,'只读验证对象')]:
  groups=[[r['phases']['checkpoint']['complete_ms']['median_ms']for r in data if r['case']==f'{side}x4-active'and r['build']==build]for side in [128,256,512]]
  assert all(len(g)==3 for g in groups)
  m=[median(g)for g in groups];ax.bar([i+shift for i in range(3)],m,.32,color=colors[build],label=label,yerr=[[v-min(g)for v,g in zip(m,groups)],[max(g)-v for v,g in zip(m,groups)]],capsize=3,error_kw={'lw':1})
  for i,v in enumerate(m):ax.annotate(f'{v:.1f}',(i+shift,max(groups[i])),xytext=(0,4),textcoords='offset points',ha='center',fontsize=9)
 ax.set(xticks=[0,1,2],xticklabels=['128²','256²','512²'],ylabel='检查点帧完整环路（ms）');ax.legend(frameon=False,fontsize=9,loc='upper left')
bars(axes[0],matched);axes[0].set_title('四个活动区域：原测量环路')
bars(axes[1],snapshots);axes[1].set_title('含缓存快照、不同初始水位')
phase_colors=['#536778','#e9b050','#b47545','#4a9b9a','#b0c6c9'];phase_names=['游戏步进（含求解等待）','提取','编码','接收/准备绘制','绘制等待']
for x,build in enumerate(['before','checked']):
 rows=[]
 for log in (a.root/'matched').glob(f'512x4-active-{build}-*-rows.json'):
  rows.extend(r for r in json.loads(log.read_text())if r['tick']>64 and r['checkpoints'])
 assert len(rows)==12,len(rows)
 groups=[mean(r['step_ms']for r in rows),mean(r['extract_ms']for r in rows),mean(r['encode_ms']for r in rows),mean(sum(r['draw_phases'][:2])for r in rows),mean(r['draw_phases'][2]for r in rows)]
 bottom=0
 for color,label,v in zip(phase_colors,phase_names,groups):axes[2].bar(x,v,.5,bottom=bottom,color=color,label=label if x==0 else None);bottom+=v
 axes[2].annotate(f'{bottom:.1f}',(x,bottom),xytext=(0,4),textcoords='offset points',ha='center',fontsize=9)
axes[2].set(xticks=[0,1],xticklabels=['改进前','改进后'],ylabel='墙钟时间平均分项（ms）',title='512²×4：时间花在哪里')
axes[2].legend(frameon=False,fontsize=8,loc='upper left',bbox_to_anchor=(.95,1))
for ax in axes:ax.spines[['right','top']].set_visible(False);ax.grid(axis='y',alpha=.18);ax.set_axisbelow(True);ax.set_ylim(0,ax.get_ylim()[1]*1.13)
fig.suptitle('GPU 水检查点：每份新数据完整检查，后续复用只读身份（Apple M5 / Metal）',fontsize=15,y=.98)
fig.text(.48,.025,'左/中：三轮中位数及范围；右：12 个检查点帧的平均分项；回读次数不变；不含网络与解码。',ha='center',fontsize=9)
fig.tight_layout(rect=(0,.06,1,.92))
for suffix in ['png','svg']:fig.savefig(a.root/('comparison.'+suffix),dpi=180)
