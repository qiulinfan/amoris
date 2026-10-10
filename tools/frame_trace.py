#!/usr/bin/env python3
"""Summarizes and compares per-frame traces (crates/pocket-render/src/trace.rs; docs/bench/dx12.md,
"Per-frame profiling").

    python tools/frame_trace.py stats out/profiler/trace-direct3d12.json
    python tools/frame_trace.py diff out/profiler/trace-vulkan.json out/profiler/trace-direct3d12.json
    python tools/frame_trace.py stats A.json --json out/profiler/a-stats.json

A trace is Chrome Trace Event JSON: CPU spans on process 1 (the render thread), GPU passes and
frames on process 2, every event with its frame in `args.frame`. `stats` gives, over the recorded
frames, per GPU pass (summed per frame when a pass repeats, as the shadow cascades do) and per
CPU span: median, p95, minimum, maximum and spread (max - min) / median, in ms; the frame
interval (one frame's CPU start to the next's) with its spikes (over 1.5 times the median); the
GPU's busy time (sum of passes), its frame span (first pass start to last pass end), the idle gap
between consecutive GPU frames, and the latency from a frame's submit to its GPU start. `diff`
puts two traces side by side per row with B - A and B / A of the medians.
"""
import argparse
import json
import statistics
import sys
from collections import defaultdict


def pct(values, p):
    v = sorted(values)
    if not v:
        return None
    return v[min(len(v) - 1, int(round(p / 100 * (len(v) - 1))))]


def summary(values):
    v = [x for x in values if x is not None]
    if not v:
        return None
    med = statistics.median(v)
    return {"n": len(v), "median": round(med, 4), "p95": round(pct(v, 95), 4),
            "min": round(min(v), 4), "max": round(max(v), 4),
            "spread": round((max(v) - min(v)) / med, 4) if med else None}


def load(path):
    with open(path, encoding="utf-8") as f:
        trace = json.load(f)
    if "traceEvents" not in trace:
        sys.exit(f"{path}: not a Chrome trace (no traceEvents)")
    return trace


def analyze(trace):
    """Per-frame values (ms) from the events: {row: [value per frame]}, grouped by kind."""
    cpu = defaultdict(lambda: defaultdict(float))
    gpu = defaultdict(lambda: defaultdict(float))
    frames = {}
    gpu_frames = {}
    submits = {}
    for e in trace["traceEvents"]:
        if e.get("ph") != "X":
            continue
        f = e.get("args", {}).get("frame")
        if f is None:
            continue
        dur_ms = e["dur"] / 1000.0
        if e["pid"] == 1:
            if e["name"] == "frame":
                frames[f] = (e["ts"], e["ts"] + e["dur"])
            else:
                cpu[e["name"]][f] += dur_ms
                if e["name"] == "submit":
                    submits[f] = e["ts"] + e["dur"]
        elif e["pid"] == 2:
            if e["name"] == "GPU frame":
                gpu_frames[f] = (e["ts"], e["ts"] + e["dur"], e["args"].get("busy_ms"))
            else:
                gpu[e["name"]][f] += dur_ms
    order = sorted(frames)
    rows = {"frame": {}, "cpu": {}, "gpu": {}}
    starts = [frames[f][0] for f in order]
    intervals = [(b - a) / 1000.0 for a, b in zip(starts, starts[1:])]
    rows["frame"]["frame interval"] = intervals
    rows["frame"]["CPU frame"] = [(frames[f][1] - frames[f][0]) / 1000.0 for f in order]
    g = sorted(gpu_frames)
    rows["frame"]["GPU busy"] = [gpu_frames[f][2] for f in g]
    rows["frame"]["GPU frame span"] = [(gpu_frames[f][1] - gpu_frames[f][0]) / 1000.0 for f in g]
    rows["frame"]["GPU idle before frame"] = [
        (gpu_frames[b][0] - gpu_frames[a][1]) / 1000.0 for a, b in zip(g, g[1:]) if b == a + 1]
    rows["frame"]["submit to GPU start"] = [
        (gpu_frames[f][0] - submits[f]) / 1000.0 for f in g if f in submits]
    for name, per in cpu.items():
        rows["cpu"][name] = [per.get(f, 0.0) for f in order]
    for name, per in gpu.items():
        rows["gpu"][name] = [per.get(f, 0.0) for f in g]
    med = statistics.median(intervals) if intervals else 0
    spikes = [{"frame": order[i + 1], "ms": round(v, 3)} for i, v in enumerate(intervals)
              if med and v > 1.5 * med]
    return rows, spikes


def stats(trace):
    rows, spikes = analyze(trace)
    other = trace.get("otherData", {})
    return {
        "meta": {k: other.get(k) for k in ("backend", "adapter", "size", "frames", "antialiasing",
                                           "gtao", "occlusion", "prepass", "gpu_frames_missing",
                                           "gpu_frames_dropped", "gpu_placement")},
        "clock_uncertainty_us": (other.get("clock") or {}).get("uncertainty_us"),
        "rows": {kind: {name: summary(v) for name, v in r.items()} for kind, r in rows.items()},
        "spikes": spikes,
    }


def fmt(x, width=8):
    return f"{x:{width}.3f}" if isinstance(x, (int, float)) else f"{'-':>{width}}"


def print_stats(name, s):
    m = s["meta"]
    print(f"{name}: {m['backend']} on {m['adapter']}, {m['size']}, {m['frames']} frames; "
          f"aa {m['antialiasing']}, gtao {m['gtao']}, occlusion {m['occlusion']}, prepass {m['prepass']}; "
          f"GPU frames missing "
          f"{m['gpu_frames_missing']}, dropped {m['gpu_frames_dropped']}; clock +-{s['clock_uncertainty_us']} us")
    for kind in ("frame", "gpu", "cpu"):
        rows = s["rows"][kind]
        if not rows:
            continue
        print(f"  {kind:30} {'median':>8} {'p95':>8} {'min':>8} {'max':>8} {'spread':>7}")
        for row, v in sorted(rows.items(), key=lambda kv: -(kv[1] or {}).get("median", 0)):
            if v:
                print(f"  {row[:30]:30} {fmt(v['median'])} {fmt(v['p95'])} {fmt(v['min'])} "
                      f"{fmt(v['max'])} {fmt(v['spread'], 7)}")
    if s["spikes"]:
        print(f"  frame-interval spikes (> 1.5 x median): {len(s['spikes'])}: "
              + ", ".join(f"#{x['frame']} {x['ms']}" for x in s["spikes"][:10]))


def diff(a, b):
    out = {}
    for kind in ("frame", "gpu", "cpu"):
        names = list(dict.fromkeys([*a["rows"][kind], *b["rows"][kind]]))
        out[kind] = {}
        for n in names:
            va, vb = a["rows"][kind].get(n), b["rows"][kind].get(n)
            ma, mb = (va or {}).get("median"), (vb or {}).get("median")
            out[kind][n] = {"a": ma, "b": mb,
                            "a_p95": (va or {}).get("p95"), "b_p95": (vb or {}).get("p95"),
                            "delta": round(mb - ma, 4) if ma is not None and mb is not None else None,
                            "ratio": round(mb / ma, 4) if ma and mb is not None else None}
    return out


def print_diff(na, nb, d, sa, sb):
    print(f"A = {na} ({sa['meta']['backend']}), B = {nb} ({sb['meta']['backend']}); medians in ms")
    for kind in ("frame", "gpu", "cpu"):
        rows = d[kind]
        if not rows:
            continue
        print(f"  {kind:30} {'A':>8} {'B':>8} {'B-A':>8} {'B/A':>6}   {'A p95':>8} {'B p95':>8}")
        for row, v in sorted(rows.items(), key=lambda kv: -max(kv[1]["a"] or 0, kv[1]["b"] or 0)):
            print(f"  {row[:30]:30} {fmt(v['a'])} {fmt(v['b'])} {fmt(v['delta'])} "
                  f"{fmt(v['ratio'], 6)}   {fmt(v['a_p95'])} {fmt(v['b_p95'])}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("stats", help="per-pass and per-span statistics of one trace")
    s.add_argument("trace")
    s.add_argument("--json", help="also write the statistics here")
    d = sub.add_parser("diff", help="two traces side by side (B against A)")
    d.add_argument("a")
    d.add_argument("b")
    d.add_argument("--json", help="also write the comparison here")
    args = ap.parse_args()
    if args.cmd == "stats":
        result = stats(load(args.trace))
        print_stats(args.trace, result)
    else:
        sa, sb = stats(load(args.a)), stats(load(args.b))
        result = {"a": args.a, "b": args.b, "a_stats": sa, "b_stats": sb, "diff": diff(sa, sb)}
        print_diff(args.a, args.b, result["diff"], sa, sb)
    if args.json:
        with open(args.json, "w", encoding="utf-8", newline="\n") as f:
            json.dump(result, f, indent=1)
            f.write("\n")


if __name__ == "__main__":
    main()
