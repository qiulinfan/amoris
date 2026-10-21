#!/usr/bin/env python3
"""Plot the native resident_water open-channel history; raw plots stay beside the run."""
import json
import pathlib
import sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
source = pathlib.Path(sys.argv[1])
result = json.loads(next(line for line in source.read_text().splitlines() if line.startswith("{")))
history = result["water_history"]
dt = result["water_state"][0]["WaterResident.controls"]["dt"]
time = [row["tick"] * dt for row in history]
ledger = [row["state"][0]["WaterResident.boundary"] for row in history]
volume = [b["current_volume"] for b in ledger]
expected = [b["initial_volume"] + b["configuration_total"] + b["entered_total"] - b["left_total"] + b["edited_total"] for b in ledger]
fig, axes = plt.subplots(3, 1, figsize=(10, 7), sharex=True)
axes[0].plot(time, volume, label="GPU volume estimate", color="#2375a5")
axes[0].plot(time, expected, "--", label="Initial + measured net transfer", color="#dc7635")
axes[0].set_ylabel("Volume (m³)");axes[0].legend(frameon=False, fontsize=9)
axes[1].plot(time, [b["entered"] / dt for b in ledger], label="Inlet", color="#2375a5")
axes[1].plot(time, [b["left"] / dt for b in ledger], label="Outlet", color="#dc7635")
axes[1].set_ylabel("Flow (m³/s)");axes[1].legend(frameon=False, fontsize=9)
axes[2].plot(time, [(a-b)*1e6 for a,b in zip(volume, expected)], color="#47795a")
axes[2].set(xlabel="Simulation time (s)", ylabel="Residual (mL)")
for ax in axes:
    ax.spines[["right", "top"]].set_visible(False)
    ax.grid(alpha=0.18)
fig.suptitle("GPU river · fixed receiving stage · actual boundary transfer", fontsize=13)
fig.tight_layout(rect=(0, 0, 1, 0.96))
fig.savefig(source.with_suffix(".png"), dpi=160)
fig.savefig(source.with_suffix(".svg"))
