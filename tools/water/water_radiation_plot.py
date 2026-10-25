#!/usr/bin/env python3
"""Inspect wave-exit evidence; standalone scientific figures stay under ignored out/."""
import json
import pathlib
import sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

root=pathlib.Path(sys.argv[1])
data=json.loads((root/"pulse.json").read_text())
rows=data["rows"]
key=lambda r: "Radiation" if isinstance(r["boundary"],dict) and "Radiation" in r["boundary"] else "Stage" if isinstance(r["boundary"],dict) else r["boundary"]
colors={"Wall":"#777777","Outflow":"#b16b31","Stage":"#9467bd","Radiation":"#158c91"}
fig,axes=plt.subplots(2,2,figsize=(11,7))
fine=[r for r in rows if r["cells"]==128 and r["cell_m"]==.25]
control=next(r for r in rows if r["cells"]==256 and r["cell_m"]==.25)
for case in fine:
    name=key(case);values=case["rows"]
    axes[0,0].plot([r[0] for r in values],[r[1]*1000 for r in values],label=name,color=colors[name])
    returned=[(a[3]-b[3])*1000 for a,b in zip(values,control["rows"])]
    axes[0,1].plot([r[0] for r in values],returned,label=name,color=colors[name])
axes[0,0].set(xlabel="Time (s)",ylabel="Probe surface anomaly (mm)",title="Actual solver: incident and returned packet")
axes[0,0].legend(frameon=False,ncol=2)
axes[0,1].set(xlim=(8,15),xlabel="Time (s)",ylabel="Returned characteristic − long domain (mm)",title="Boundary contribution, dx=0.25 m")
for i,name in enumerate(["Outflow","Stage","Radiation"]):
    axes[1,0].plot([32/r["cells"] for r in data["comparisons"]],[r["wall_relative_amplitude"][i+1] for r in data["comparisons"]],"o-",label=name,color=colors[name])
axes[1,0].axhline(1,color=colors["Wall"],linestyle="--",label="Wall calibration")
axes[1,0].set(xlabel="Cell width (m)",ylabel="Returned L2 amplitude / wall",title="Same solver and doubled-domain subtraction",ylim=(0,1.2))
axes[1,0].legend(frameon=False,fontsize=8)
rad=[r for r in rows if key(r)=="Radiation"]
axes[1,1].plot([r["cell_m"] for r in rad],[r["max_mass_error_m3"]*1e6 for r in rad],"o-",color=colors["Radiation"])
axes[1,1].set(xlabel="Cell width (m)",ylabel="Maximum independent volume defect (mL)",title="Complete GPU field versus actual boundary flux")
for axis in axes.flat:
    axis.grid(alpha=.2);axis.spines[["right","top"]].set_visible(False)
fig.suptitle("Normal shallow-water packet · characteristic reservoir · damped MAC solver")
fig.tight_layout(rect=(0,0,1,.96));fig.savefig(root/"wave-exit.png",dpi=160);fig.savefig(root/"wave-exit.svg")
