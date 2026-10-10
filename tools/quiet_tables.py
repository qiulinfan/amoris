#!/usr/bin/env python3
"""Tables of the quiet-machine re-measurement (docs/bench/quiet-2026-10-09.md) from the raw JSON in
out/bench-runs/quiet/: per configuration the minimum, the median and the spread over the rounds
((max - min) / median), never a single chosen run.

    python tools/quiet_tables.py dx12 out/bench-runs/quiet/dx12/bench-headless.json [more.json ...]
    python tools/quiet_tables.py hiz out/bench-runs/quiet/hiz/bench-1280x720.json
    python tools/quiet_tables.py splats out/bench-runs/quiet/splats/ab-*.json
    python tools/quiet_tables.py splatm out/bench-runs/quiet/splats/matrix-*.json
    python tools/quiet_tables.py lod out/bench-runs/quiet/lod/bench-t0.json
    python tools/quiet_tables.py aa out/bench-runs/quiet/aa/bench-native.json
    python tools/quiet_tables.py rt out/bench-runs/quiet/rt/timing-5060.json
    python tools/quiet_tables.py rts out/bench-runs/quiet/rt/shadows-runs.json
    python tools/quiet_tables.py neural out/bench-runs/quiet/neural/*.json
    python tools/quiet_tables.py web out/bench-runs/quiet/web/*.json

Standard library only.
"""
import json
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def stats(values):
    v = sorted(x for x in values if x is not None)
    if not v:
        return None
    med = statistics.median(v)
    return {"n": len(v), "min": v[0], "median": med, "max": v[-1],
            "spread_pct": 100.0 * (v[-1] - v[0]) / med if med else 0.0}


def fmt(s, digits=2):
    if not s:
        return "-"
    return f"{s['min']:.{digits}f} / {s['median']:.{digits}f} ({s['spread_pct']:.0f}%)"


def load(paths):
    return [json.loads(Path(p).read_text(encoding="utf-8")) for p in paths]


def dx12(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for run in doc["runs"]:
            if "headline" not in run:
                print(f"error: {run['case']} {run['adapter']} {run['variant']}: {run.get('error', '')[:200]}")
                continue
            g = groups[(run["case"], run["adapter"], run["variant"])]
            g["frame"].append(run["headline"][0])
            g["gpu"].append(run["headline"][1])
            r = run["result"]
            if "renderer_new_ms" in r:
                g["renderer_new"].append(r["renderer_new_ms"])
                g["cpu"].append(r["cpu_encode_ms"]["p50"])
                g["device"].append(r["device_ms"])
                g["first_frame"].append(r["first_frame_ms"])
            elif "cpu_ms_mean" in r:
                g["cpu"].append(r["cpu_ms_mean"])
            passes = r.get("passes_ms") or (r.get("draw_on") or {}).get("passes_ms") or {}
            for p, ms in passes.items():
                g["pass:" + p].append(ms)
    print("| case | adapter | variant | n | frame p50 ms min / median (spread) | GPU ms min / median (spread) | CPU encode ms median | Renderer::new ms min / median |")
    print("|---|---|---|---|---|---|---|---|")
    for (case, adapter, variant), g in groups.items():
        rn = stats(g["renderer_new"])
        cpu = stats(g["cpu"])
        print(f"| {case} | {adapter} | {variant} | {len(g['gpu'])} | {fmt(stats(g['frame']))} | "
              f"{fmt(stats(g['gpu']))} | {cpu['median']:.2f} | "
              f"{(f'{rn['min']:.0f} / {rn['median']:.0f}' if rn else '-')} |" if cpu else
              f"| {case} | {adapter} | {variant} | {len(g['gpu'])} | {fmt(stats(g['frame']))} | "
              f"{fmt(stats(g['gpu']))} | - | - |")
    print("\nPasses (median ms):")
    for (case, adapter, variant), g in groups.items():
        ps = {k[5:]: statistics.median(v) for k, v in g.items() if k.startswith("pass:")}
        print(f"  {case} {adapter} {variant}: " + ", ".join(f"{k} {v:.2f}" for k, v in ps.items() if v >= 0.01))


def hiz(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for r in doc["runs"]:
            if "error" in r:
                print("error:", r.get("config"), r.get("layout"), r.get("mode"), r["error"][:200])
                continue
            g = groups[(r["config"], r["count"], r["layout"], r["mode"])]
            g["gpu"].append(r["gpu_ms"]["mean"])
            g["wall"].append(r["submit_to_idle_ms"]["mean"])
            on = sum(n for m, n in r["occlusion_frames"].items() if m in ("on", "auto-on"))
            g["on"].append(on)
            g["frames"].append(sum(r["occlusion_frames"].values()))
            for p, ms in r["passes_ms"].items():
                g["pass:" + p].append(ms)
            last = r.get("occlusion_last") or {}
            if last:
                g["occluded_tri"].append(last.get("occluded_triangles"))
                g["frustum"].append(last.get("frustum"))
    print("| config | cubes | layout | mode | n | GPU ms min / median (spread) | wall ms min / median (spread) | frames two-phase |")
    print("|---|---|---|---|---|---|---|---|")
    for (config, count, layout, mode), g in sorted(groups.items()):
        on = "/".join(str(x) for x in g["on"])
        print(f"| {config} | {count:,} | {layout} | {mode} | {len(g['gpu'])} | {fmt(stats(g['gpu']))} | "
              f"{fmt(stats(g['wall']))} | {on} of {g['frames'][0]} |")
    print("\nPasses (median ms), mode on:")
    for (config, count, layout, mode), g in sorted(groups.items()):
        if mode != "on" and count == 1_600_000:
            continue
        ps = {k[5:]: statistics.median(v) for k, v in g.items() if k.startswith("pass:")}
        extra = ""
        if g["occluded_tri"]:
            extra = f" | occluded triangles {statistics.median(g['occluded_tri']):,.0f}, frustum instances {statistics.median(g['frustum']):,.0f}"
        print(f"  {config} {count} {layout} {mode}: " + ", ".join(f"{k} {v:.2f}" for k, v in ps.items() if v >= 0.01) + extra)
    # Check 1 of occlusion.md 7: on the sphere (nothing occluded) the second phase's whole cost is
    # on - off, converted to triangles at the configuration's opaque rate with occlusion off, and
    # compared with the auto mode's 1.5x probe bar. The proxy sums the second phase's own passes
    # (late culling, Hi-Z, late opaque pass).
    sphere = 2_250_132
    bar = 1.5 * (200_000 + 0.5 * 187_511)
    rows = [(c, g) for (c, n, layout, m), g in groups.items() if layout == "sphere" and m == "off"]
    if rows:
        print(f"\nCheck 1 (sphere): triangles = cost / opaque off ms x {sphere:,}; bar {bar:,.0f}")
    for config, off in sorted(rows):
        on = groups.get((config, 1_600_000, "sphere", "on"))
        if not on:
            continue
        opaque = statistics.median(off["pass:opaque+sky"])
        cost = statistics.median(on["gpu"]) - statistics.median(off["gpu"])
        proxy = sum(statistics.median(on["pass:" + p]) for p in ("cull late", "hi-z", "opaque+sky"))
        pairs = [b - a for a, b in zip(off["gpu"], on["gpu"])]
        tri = lambda ms: ms / opaque * sphere
        runs = ", ".join(f"{x:.2f}" for x in off["pass:opaque+sky"])
        print(f"  {config}: off {statistics.median(off['gpu']):.3f}, "
              f"on {statistics.median(on['gpu']):.3f}, opaque off {opaque:.3f} (runs {runs}); "
              f"on - off {cost:.3f} ms = {tri(cost):,.0f} ({tri(cost) / bar:.2f} of the bar); "
              f"per repeat {', '.join(f'{tri(x):,.0f}' for x in pairs)}; "
              f"second-phase passes {proxy:.3f} ms = {tri(proxy):,.0f}")


def splats(paths):
    print("| file | scene | n quad / tile | quads total ms min / median (spread) | tiles total ms min / median (spread) | tiles / quads (medians) | per-round ratios |")
    print("|---|---|---|---|---|---|---|")
    for path, doc in zip(paths, load(paths)):
        for scene, best in doc.items():
            q = [r["splat_total"] for r in best["quad"]["runs"]]
            t = [r["splat_total"] for r in best["tile"]["runs"]]
            sq, st = stats(q), stats(t)
            ratios = ", ".join(f"{b / a:.2f}" for a, b in zip(q, t))
            print(f"| {Path(path).stem} | `{scene}` | {len(q)} / {len(t)} | {fmt(sq)} | {fmt(st)} | "
                  f"{st['median'] / sq['median']:.2f} | {ratios} |")
        for scene, best in doc.items():
            for raster in ("quad", "tile"):
                keys = sorted(set().union(*(r["passes"].keys() for r in best[raster]["runs"])))
                med = {k[6:]: statistics.median(r["passes"].get(k, 0.0) for r in best[raster]["runs"])
                       for k in keys}
                print(f"  {Path(path).stem} {scene} {raster}: " + ", ".join(f"{k} {v:.2f}" for k, v in med.items()))


def splatm(paths):
    """tools/splats/matrix.py: per adapter, backend, scene (and build), both rasterizers over the
    rounds; with one rasterizer, its time per build."""
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for r in doc["runs"]:
            if not r.get("passes"):
                print("no passes:", r.get("requested_adapter"), r.get("requested_backend"),
                      r.get("scene"), r.get("raster"), r.get("build", ""))
                continue
            scene = r["scene"] + (f" [{r['build']}]" if r.get("build") else "")
            g = groups[(r["requested_adapter"], r["requested_backend"], scene)]
            g[r["raster"]].append(r["splat_total"])
            g[r["raster"] + ":gpu"].append(r["gpu_total"])
            g[r["raster"] + ":round"].append(r["round"])
            for p, ms in r["passes"].items():
                g[r["raster"] + ":pass:" + p].append(ms)
            if r["raster"] == "tile":
                g["pairs"].append(r.get("pairs"))
            else:
                g["quad_mpixels"].append(r.get("quad_mpixels"))
                g["visible"].append(r.get("visible"))
    print("| adapter | backend | scene | n | quads ms min / median (spread) | tiles ms min / median (spread) "
          "| tiles / quads (medians) | per-round ratios | frame GPU quads / tiles (medians) |")
    print("|---|---|---|---|---|---|---|---|---|")
    for (adapter, backend, scene), g in groups.items():
        sq, st = stats(g["quad"]), stats(g["tile"])
        by_round = lambda r: dict(zip(g[r + ":round"], g[r]))
        q, t = by_round("quad"), by_round("tile")
        ratios = ", ".join(f"{t[k] / q[k]:.2f}" for k in sorted(q) if k in t) or "-"
        ratio = f"{st['median'] / sq['median']:.2f}" if sq and st else "-"
        gq, gt = stats(g["quad:gpu"]), stats(g["tile:gpu"])
        med = lambda s: f"{s['median']:.2f}" if s else "-"
        print(f"| {adapter} | {backend} | {scene} | {max(len(g['quad']), len(g['tile']))} | {fmt(sq)} | "
              f"{fmt(st)} | {ratio} | {ratios} | {med(gq)} / {med(gt)} |")
    print("\nPasses (median ms):")
    for (adapter, backend, scene), g in groups.items():
        for raster in ("quad", "tile"):
            pre = raster + ":pass:"
            med = {k[len(pre) + 6:]: statistics.median(v) for k, v in g.items() if k.startswith(pre)}
            if not med:
                continue
            extra = (f"; pairs {statistics.median(x for x in g['pairs'] if x):,.0f}" if raster == "tile"
                     and any(g["pairs"]) else
                     f"; visible {statistics.median(g['visible']):,.0f}, quad pixels "
                     f"{statistics.median(g['quad_mpixels']):.1f} M" if raster == "quad" and
                     any(g["visible"]) else "")
            print(f"  {adapter} {backend} {scene} {raster}: "
                  + ", ".join(f"{k} {v:.2f}" for k, v in med.items()) + extra)


def lod(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for path, doc in zip(paths, load(paths)):
        t = doc["scene"]["t"]
        occ = doc["scene"]["occlusion"]
        for r in doc["runs"]:
            if "error" in r:
                print("error:", r.get("requested"), r["error"][:200])
                continue
            g = groups[(t, occ, r["adapter"], r["backend"], r["lod"])]
            g["gpu"].append(r["gpu_ms"]["p50"])
            g["wall"].append(r["wall_ms"]["p50"])
            g["cpu"].append(r["cpu_ms"]["p50"] if isinstance(r["cpu_ms"], dict) else r["cpu_ms"])
            g["camera"].append(r["camera_triangles"])
            g["lod_build"].append(sum(r["lod_build_ms"]))
    print("| t | occlusion | adapter | backend | levels | n | GPU p50 ms min / median (spread) | wall p50 ms min / median | camera triangles | level build ms (sum) min / median |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for (t, occ, adapter, backend, lv), g in sorted(groups.items()):
        lb = stats(g["lod_build"])
        print(f"| {t} | {occ} | {adapter} | {backend} | {lv} | {len(g['gpu'])} | {fmt(stats(g['gpu']))} | "
              f"{fmt(stats(g['wall']))} | {statistics.median(g['camera']):,.0f} | {lb['min']:.0f} / {lb['median']:.0f} |")


def aa(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for r in doc["runs"]:
            if "error" in r:
                print("error:", r)
                continue
            g = groups[(r["workload"], r["adapter"], r["backend"], r["size"], r["aa"], r["gtao"])]
            g["gpu"].append(r["gpu_p50"])
            passes = r.get("passes_p10") or r.get("passes_mean") or {}
            for p, ms in passes.items():
                g["pass:" + p].append(ms)
    print("| workload | adapter | backend | size | aa | gtao | n | GPU p50 ms min / median (spread) | gtao pass median | taa pass median | opaque median |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for key, g in sorted(groups.items()):
        med = lambda p: statistics.median(g["pass:" + p]) if g.get("pass:" + p) else None
        cell = lambda v: f"{v:.3f}" if v is not None else "-"
        print("| " + " | ".join(key) + f" | {len(g['gpu'])} | {fmt(stats(g['gpu']), 3)} | "
              f"{cell(med('gtao'))} | {cell(med('taa'))} | {cell(med('opaque+sky') or med('opaque'))} |")


def rt(paths):
    for doc in load(paths):
        groups = defaultdict(lambda: defaultdict(list))
        for r in doc["runs"]:
            g = groups[(r["name"], r["requested_backend"], r.get("adapter"))]
            g["gpu"].append(r["gpu_total_ms"])
            g["wall"].append(r["trace_wall_ms"])
            if "gpu_trace_ms" in r:
                g["trace"].append(r["gpu_trace_ms"])
                g["train"].append(r["gpu_training_ms"])
        print("| configuration | backend | adapter | n | GPU ms/frame min / median (spread) | trace ms / training ms (medians) | batch wall ms median |")
        print("|---|---|---|---|---|---|---|")
        for (name, backend, adapter), g in groups.items():
            tt = (f"{statistics.median(g['trace']):.3f} / {statistics.median(g['train']):.3f}"
                  if g["trace"] else "-")
            print(f"| {name} | {backend} | {adapter} | {len(g['gpu'])} | {fmt(stats(g['gpu']), 3)} | {tt} | "
                  f"{statistics.median(g['wall']):.1f} |")


def rts(paths):
    for doc in load(paths):
        groups = defaultdict(lambda: defaultdict(list))
        # rt_shadows_bench.py --all-rounds wraps the runs; earlier summaries are the bare runs.
        for name, r in (doc["runs"] if "tool" in doc else doc).items():
            g = groups[re.sub(r"-r\d+$", "", name)]
            for path in ("cascaded", "ray_traced"):
                g[path].append(r[path]["gpu_ms"]["p50"])
                g[path + "_wall"].append(r[path].get("submit_to_idle_ms", {}).get("p50"))
        print("| configuration | n | cascaded GPU p50 ms min / median (spread) | ray traced min / median (spread) | change (medians) | per-round change |")
        print("|---|---|---|---|---|---|")
        for base, g in sorted(groups.items()):
            c, t = stats(g["cascaded"]), stats(g["ray_traced"])
            per = ", ".join(f"{100 * (b / a - 1):+.0f}%" for a, b in zip(g["cascaded"], g["ray_traced"]))
            print(f"| {base} | {c['n']} | {fmt(c, 3)} | {fmt(t, 3)} | {100 * (t['median'] / c['median'] - 1):+.0f}% | {per} |")


def neural(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for run in doc.get("runs", [doc]):
            for res in run["results"]:
                size = "x".join(str(x) for x in res["size"])
                for variant, v in res["variants"].items():
                    groups[(run["adapter"], run["backend"], run["view"], size)][variant].append(
                        v["opaque_ms"]["p50"])
    print("| adapter | backend | view | size | n | inline | textured | neural f16 | neural f32 | decode f16 (median f16 - median textured) | decode f32 |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for key, g in sorted(groups.items()):
        s = {k: stats(v) for k, v in g.items()}
        dec = lambda k: (f"{s[k]['median'] - s['textured']['median']:.2f}" if k in s and "textured" in s else "-")
        print("| " + " | ".join(str(x) for x in key) + f" | {len(g['textured'])} | "
              + " | ".join(fmt(s.get(k)) for k in ("inline", "textured", "neural_f16", "neural_f32"))
              + f" | {dec('neural_f16')} | {dec('neural_f32')} |")


def web(paths):
    groups = defaultdict(lambda: defaultdict(list))
    for doc in load(paths):
        for r in doc["runs"]:
            if not r.get("frames"):
                print("no frames:", r.get("label"), r.get("error", "")[:200])
                continue
            g = groups[(r["gpu"], r["page"], r["path"])]
            g["frame"].append(r["frame_ms_p50"])
            g["frame_mean"].append(r["frame_ms_mean"])
            # Frames whose timestamps arrived; a run without any gives no GPU figure.
            g["gpu"].append(r["gpu_ms_timed_mean"] if r.get("gpu_timed_frames") else None)
            g["timed"].append(r.get("gpu_timed_frames"))
            g["frames"].append(r["frames"])
            g["path_drawn"].append(r.get("draw_path"))
    print("| GPU | page | path | n | frame p50 ms min / median (spread) | GPU ms (timed frames) min / median (spread) | timed frames of 300 per run | drawn path |")
    print("|---|---|---|---|---|---|---|---|")
    for key, g in sorted(groups.items()):
        print("| " + " | ".join(key) + f" | {len(g['frame'])} | {fmt(stats(g['frame']))} | {fmt(stats(g['gpu']))} | "
              + ",".join(str(x) for x in g["timed"]) + f" | {','.join(sorted(set(map(str, g['path_drawn']))))} |")


def conditions(paths):
    print("| record | time | AC | CPU % (mean) | RTX 5060 C, SM MHz, W, state | GPU engine users > 0.5% |")
    print("|---|---|---|---|---|---|")
    for path, d in zip(paths, load(paths)):
        q = d["nvidia"]["query"]
        users = d["gpu_engine_users"] or []
        users = users if isinstance(users, list) else [users]
        u = ", ".join(f"{x['process']} {x['pct']}" for x in users)
        print(f"| {Path(path).stem} | {d['time']} | {d['on_ac_power']} | {d['cpu_load']['mean_pct']} | "
              f"{q['temperature.gpu']}, {q['clocks.sm']}, {q['power.draw']}, {q['pstate']} | {u} |")


if __name__ == "__main__":
    {"conditions": conditions, "dx12": dx12, "hiz": hiz, "splats": splats, "splatm": splatm, "lod": lod, "aa": aa, "rt": rt, "rts": rts,
     "neural": neural, "web": web}[sys.argv[1]](sys.argv[2:])
