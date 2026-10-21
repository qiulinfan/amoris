#!/usr/bin/env python3
"""Plot paired per-run medians and active/submitted slots, without a game-FPS inference."""
import json
import pathlib
import sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import frame_trace

root = pathlib.Path(sys.argv[1])
cases = [
    ("128-full-full", "128² overview"), ("128-full-near", "128² grazing"),
    ("512-full-full", "512² overview"), ("512-full-near", "512² grazing"),
    ("512-sparse-full", "512² sparse"), ("512-dry-full", "512² dry"),
    ("512-blocky-near", "512² Blocky"), ("512-full-local", "512² local"),
]
data = [json.loads((root / name / "summary.json").read_text()) for name, _ in cases]
for (name, _), rows in zip(cases, data):
    for row in rows:
        row["gpu_frame"] = None
        if row.get("trace_file"):
            path = pathlib.Path(row["trace_file"])
            if not path.is_absolute() and not path.exists():
                path = root / name / path.name
            trace = frame_trace.load(path)
            if trace.get("otherData", {}).get("gpu_timestamps") is False:
                continue
            stats = frame_trace.stats(trace)
            if any(stats["meta"][key] != 0 for key in ("gpu_frames_missing", "gpu_frames_dropped")):
                raise ValueError(f"incomplete GPU frame trace: {path}")
            row["gpu_frame"] = stats["rows"]["frame"]["GPU frame span"]
key_mode="lod" if data[0][0].get("comparison")=="lod" else "culling"
mode_name="LOD" if key_mode=="lod" else "Culling"
for rows in data:
    assert len(rows) == 6 and all(sum(r[key_mode] == on for r in rows) == 3 for on in [False, True])
fig, axes = plt.subplots(1, 3, figsize=(15, 5.2), sharey=True, gridspec_kw={"width_ratios": [1.3, 1.3, 1]})
colors = ["#b76d3c", "#2879a7"]
for mode, color in zip([False, True], colors):
    for index, rows in enumerate(data):
        selected = [r for r in rows if r[key_mode] == mode]
        y = index + (0.12 if mode else -0.12)
        for axis, key, stat in zip(axes[:2], ["wall", "gpu_frame"], ["median_ms", "median"]):
            values = [r[key][stat] for r in selected if r[key] is not None]
            if values:
                axis.plot([min(values), max(values)], [y, y], color=color, alpha=0.5)
                axis.scatter(values, [y] * len(values), s=22, color=color, label=(mode_name+" on" if mode else mode_name+" off") if index == 0 else None)
    for axis in axes[:2]:
        axis.set_xlim(left=0);axis.grid(axis="x", alpha=0.2)
axes[0].set(yticks=range(len(cases)), yticklabels=[label for _, label in cases], xlabel="Render + explicit GPU wait (ms)")
axes[1].set_xlabel("GPU frame span from frame-ID trace (ms)")
if not any(row["gpu_frame"] is not None for rows in data for row in rows):
    axes[1].text(0.5, 0.5, "No frame-ID GPU spans", transform=axes[1].transAxes,
                 ha="center", va="center", fontsize=10)
axes[0].legend(frameon=False, fontsize=9)
for index, rows in enumerate(data):
    counts = next(r["mesh"][0][1] for r in rows if r[key_mode])
    for delta, key, color in [(-0.12, "active_triangles", "#3b8871"), (0.12, "submitted_triangles", "#778495")]:
        value = counts[key] / max(1, counts["input_triangles"])
        axes[2].barh(index + delta, value, height=0.2, color=color, label=key.removesuffix("_triangles").capitalize() if index == 0 else None)
axes[2].set(xlim=(0, 1.04), xlabel="Fraction of input triangle slots")
axes[2].legend(frameon=False, fontsize=9, loc="lower center", bbox_to_anchor=(0.5, 1.01), ncol=2)
axes[2].grid(axis="x", alpha=0.2)
axes[0].invert_yaxis()
for axis in axes:
    axis.spines[["right", "top"]].set_visible(False)
fig.suptitle("GPU water geometry · Apple M5 / Metal · three per-run medians", fontsize=13)
fig.text(0.5, 0.01, "Static rendering inputs, MSAA, 120 measured frames after 30 warmups. Solver, captures and counter readbacks excluded. Asynchronous GPU snapshots omitted.", ha="center", fontsize=8)
fig.tight_layout(rect=(0, 0.035, 1, 0.95))
for extension in ["png", "svg"]:
    fig.savefig(root / ("comparison." + extension), dpi=160)
