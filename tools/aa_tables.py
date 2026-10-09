#!/usr/bin/env python3
"""Markdown tables from tools/aa_bench.py's summary (docs/bench/taa-gtao.md 3): per configuration
the better of the rounds (the lesser total GPU time p50, and each pass's lesser time).

    python tools/aa_tables.py docs/evidence/aa/bench-native.json
"""
import json
import sys
from collections import defaultdict

runs = json.load(open(sys.argv[1], encoding="utf-8"))["runs"]
best = defaultdict(lambda: {"gpu": float("inf"), "passes": {}})
for r in runs:
    if "error" in r:
        continue
    key = (r["workload"], r["adapter"], r["backend"], r["size"], r["aa"], r["gtao"])
    b = best[key]
    b["gpu"] = min(b["gpu"], r["gpu_p50"])
    passes = r.get("passes_p10") or r.get("passes_mean") or {}
    for p, ms in passes.items():
        b["passes"][p] = min(b["passes"].get(p, float("inf")), ms)

AA = ["off", "msaa", "taa", "msaa+taa"]
GPU = {"nvidia": "RTX 5060", "780m": "Radeon 780M"}
API = {"vulkan": "Vulkan", "dx12": "D3D12"}


def cell(v):
    return f"{v:.2f}" if v != float("inf") else ""


for workload in ["check scene", "many_cubes 1.6M"]:
    print(f"\n{workload}: total GPU ms (GTAO off), and the passes GTAO and TAA add\n")
    print("| GPU, API, size | off | msaa | taa | msaa+taa | GTAO pass | TAA pass | normal target in the opaque pass (msaa) |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|")
    rows = sorted({(k[1], k[2], k[3]) for k in best if k[0] == workload},
                  key=lambda r: (r[0] != "nvidia", r[1] != "vulkan", r[2]))
    for adapter, backend, size in rows:
        g = lambda aa, gtao="off": best.get((workload, adapter, backend, size, aa, gtao))
        totals = [cell(g(aa)["gpu"]) if g(aa) else "" for aa in AA]
        gt = g("msaa", "depth")
        ta = g("taa")
        tgt = g("msaa", "target")
        normal = (cell(tgt["passes"].get("opaque+sky", 0) - gt["passes"].get("opaque+sky", 0))
                  if tgt and gt else "")
        print(f"| {GPU[adapter]}, {API[backend]}, {size} | " + " | ".join(totals) +
              f" | {cell(gt['passes'].get('gtao', float('inf'))) if gt else ''}"
              f" | {cell(ta['passes'].get('taa', float('inf'))) if ta else ''} | {normal} |")
