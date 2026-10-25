#!/usr/bin/env python3
"""Inspect prescribed-field camera motion; figures stay beside the ignored run."""
import json
import pathlib
import sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
root=pathlib.Path(sys.argv[1])
rows=json.loads((root/"motion.json").read_text())
frames=sorted({r["frame"] for r in rows})
selected=[frames[0],frames[len(frames)//3],frames[len(frames)*2//3],frames[-1]]
fig,axes=plt.subplots(2,4,figsize=(14,5.4))
for line,on in enumerate([False,True]):
    for col,frame in enumerate(selected):
        axes[line,col].imshow(plt.imread(root/f"motion-{str(on).lower()}-{frame:03}.png"))
        axes[line,col].set_title(f"LOD {'on' if on else 'off'} · frame {frame}",fontsize=10)
        axes[line,col].axis("off")
fig.suptitle("Same fine water field · moving camera · full-resolution material attributes",fontsize=13)
has_morph=any("transition_patches" in r["mesh"][0][1] for r in rows)
caption="GPU geomorph with an accuracy guard" if has_morph else "Discrete LOD with hysteresis"
fig.text(0.5,0.01,caption+". These are prescribed rendering inputs, not simulated-flow evidence.",ha="center",fontsize=9)
fig.tight_layout(rect=(0,0.03,1,0.94))
fig.subplots_adjust(hspace=0.25)
fig.savefig(root/"contact.png",dpi=160)
on=[r for r in rows if r["lod"]]
fig,axes=plt.subplots(2,1,figsize=(9,5.5),sharex=True)
for level in range(4):
    axes[0].plot([r["frame"] for r in on],[r["mesh"][0][1]["levels"][level] for r in on],label=f"Level {level}")
if has_morph:
    axes[0].plot([r["frame"] for r in on],[r["mesh"][0][1]["transition_patches"] for r in on],label="Morphing",color="#444444",linestyle=":")
axes[0].legend(ncol=4,frameon=False,fontsize=9);axes[0].set_ylabel("Visible patches")
axes[1].plot([r["frame"] for r in on],[r["mesh"][0][1]["max_error_pixels"] for r in on],color="#2879a7")
axes[1].axhline(1,color="#b76d3c",linestyle="--",label="1 output-pixel bound")
axes[1].set(xlabel="Camera frame",ylabel="Conservative error (px)",ylim=(0,1.1));axes[1].legend(frameon=False,fontsize=9)
for axis in axes:axis.spines[["right","top"]].set_visible(False);axis.grid(alpha=0.2)
fig.tight_layout();fig.savefig(root/"levels.png",dpi=160)
