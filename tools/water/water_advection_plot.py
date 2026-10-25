#!/usr/bin/env python3
"""Plot water_advection's measured profile with Matplotlib; outputs remain beside the raw run."""
import csv
import json
import pathlib
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

root = pathlib.Path(sys.argv[1])
summary = json.loads((root / "summary.json").read_text())
with (root / "shear.csv").open() as source:
    rows = [{key: float(value) for key, value in row.items()} for row in csv.DictReader(source)]
rows = [row for row in rows if row["z"] < 56]
first, mac = summary["first_order"], summary["maccormack"]
fig, axes = plt.subplots(1, 2, figsize=(11, 4.2), gridspec_kw={"width_ratios": [2, 1]})
colors = ["#2878b5", "#d66a20"]
axes[0].plot([r["z"] for r in rows], [r["reference"] for r in rows], "k--", label="Reference", linewidth=1.7)
for name, label, color in zip(["first_order", "maccormack"], ["First order", "Bounded MacCormack"], colors):
    axes[0].plot([r["z"] for r in rows], [r[name] for r in rows], ".-", label=label, color=color)
axes[0].set(xlabel="z (m)", ylabel="u (m/s)")
axes[0].legend(frameon=False, fontsize=9, ncol=3, loc="lower center", bbox_to_anchor=(0.5, 1.01))
values = [first["fitted_amplitude"] / first["expected_amplitude"] * 100,
          mac["fitted_amplitude"] / mac["expected_amplitude"] * 100]
axes[1].bar(["First order", "MacCormack"], values, color=colors, width=0.6)
axes[1].axhline(100, linestyle="--", color="black", linewidth=1)
axes[1].set(ylabel="Fitted amplitude / reference (%)", ylim=(0, 110), title="Retained flow detail")
for i, value in enumerate(values):
    axes[1].text(i, value + 2, f"{value:.1f}%", ha="center")
for ax in axes:
    ax.spines[["right", "top"]].set_visible(False)
    ax.grid(axis="y", alpha=0.18)
    ax.set_axisbelow(True)
fig.suptitle("Translating shear after 1.2 s · 128², cell = 1 m · 120 steps at dt = 0.01 s", fontsize=13)
fig.text(0.5, 0.015,
         f"Relative L2: {first['relative_l2']:.1%} → {mac['relative_l2']:.1%}    |    "
         f"Phase error: {first['phase_error_rad']:.3f} → {mac['phase_error_rad']:.3f} rad", ha="center", fontsize=10)
fig.tight_layout(rect=(0, 0.06, 1, 0.93))
fig.savefig(root / "shear.png", dpi=160)
fig.savefig(root / "shear.svg")
