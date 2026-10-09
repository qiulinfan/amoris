#!/usr/bin/env python3
"""Times TAA and GTAO natively per backend and adapter (docs/bench/taa-gtao.md).

Two workloads, offscreen, each frame submitted and waited for, timestamped passes:
- aa_eval's bench: the check scene (`demo::aa_scene`, light geometry, shading-bound) at 1600x900
  and 2560x1440, every anti-aliasing mode with GTAO off and on (normals from the depth), and the
  normal target for msaa and taa;
- many_cubes' sphere of 1.6M cubes (`--headless-bench`, geometry-bound) at 1600x900, every
  anti-aliasing mode with GTAO off and on, through POCKET_AA and POCKET_GTAO.

Rounds interleave every configuration so that other work on the machine spreads over all of them.
Build first: `cargo build --release -p pocket-render --examples`. Then, from the repository root:

    python tools/aa_bench.py --adapters nvidia,780m --backends vulkan,dx12 --rounds 2 \
        --out docs/evidence/aa/bench-native.json

The summary keeps every run; the table printed at the end is the median over rounds of each
configuration's 10th-percentile pass times (the least disturbed frames) and of its median total.
"""
import argparse
import json
import os
import statistics
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
AA = ["off", "msaa", "taa", "msaa+taa"]


def example(name):
    return str(ROOT / "target" / "release" / "examples" / f"{name}{EXE}")


def aa_eval(env, sizes, frames, scratch):
    p = subprocess.run([example("aa_eval"), str(scratch), "--only", "bench", "--bench-sizes", sizes,
                        "--frames", str(frames)], cwd=ROOT, env=env, capture_output=True, text=True,
                       timeout=1800)
    rows = []
    for line in p.stderr.splitlines():
        if line.startswith("bench "):
            j = json.loads(line[6:])
            rows.append({"workload": "check scene", "size": j["size"], "aa": j["aa"],
                         "gtao": j["gtao"], "gpu_p50": j["profiled_gpu_p50_ms"],
                         "gpu_p10": j["profiled_gpu_p10_ms"], "passes_p10": j["passes_p10_ms"],
                         "passes_p50": j["passes_p50_ms"], "wall_p50": j["wall_p50_ms"]})
        elif "validation" in line or "panicked" in line:
            rows.append({"error": line[:300]})
    return rows


def cubes(env, aa, gtao, frames):
    e = dict(env, POCKET_AA=aa, POCKET_GTAO=gtao)
    p = subprocess.run([example("many_cubes"), "--headless-bench", str(frames), "--size",
                        "1600x900"], cwd=ROOT, env=e, capture_output=True, text=True, timeout=900)
    for line in p.stdout.splitlines():
        if line.startswith("{"):
            j = json.loads(line)
            return {"workload": "many_cubes 1.6M", "size": "1600x900", "aa": aa,
                    "gtao": "depth" if gtao == "on" else gtao, "gpu_p50": j["gpu_ms"]["p50"],
                    "passes_mean": j["passes_ms"], "wall_p50": j["submit_to_idle_ms"]["p50"],
                    "adapter": j["adapter"]}
    return {"error": (p.stdout + p.stderr)[-300:], "aa": aa, "gtao": gtao}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--adapters", default="nvidia,780m")
    ap.add_argument("--backends", default="vulkan,dx12")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--frames", type=int, default=150)
    ap.add_argument("--sizes", default="1600x900,2560x1440")
    ap.add_argument("--skip-cubes", action="store_true")
    ap.add_argument("--out")
    a = ap.parse_args()
    scratch = ROOT / "target" / "aa_bench"
    scratch.mkdir(parents=True, exist_ok=True)
    runs = []
    for r in range(a.rounds):
        for adapter in a.adapters.split(","):
            for backend in a.backends.split(","):
                env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter)
                for row in aa_eval(env, a.sizes, a.frames, scratch):
                    row.update({"round": r, "adapter": adapter, "backend": backend})
                    runs.append(row)
                    print(json.dumps(row))
                if a.skip_cubes:
                    continue
                for aa in AA:
                    for gtao in ["off", "on"]:
                        row = cubes(env, aa, gtao, a.frames)
                        row.update({"round": r, "adapter": adapter, "backend": backend})
                        runs.append(row)
                        print(json.dumps(row))
    # The median over rounds per configuration.
    table = {}
    for row in runs:
        if "error" in row:
            continue
        key = (row["workload"], row["adapter"], row["backend"], row["size"], row["aa"], row["gtao"])
        table.setdefault(key, []).append(row)
    summary = []
    for key, rows in sorted(table.items()):
        passes = rows[0].get("passes_p10") or rows[0].get("passes_mean") or {}
        med = {p: round(statistics.median((x.get("passes_p10") or x.get("passes_mean") or {}).get(p, 0.0)
                                          for x in rows), 3) for p in passes}
        summary.append({"workload": key[0], "adapter": key[1], "backend": key[2], "size": key[3],
                        "aa": key[4], "gtao": key[5],
                        "gpu_p50": round(statistics.median(x["gpu_p50"] for x in rows), 3),
                        "passes": med, "rounds": len(rows)})
    for s in summary:
        p = s["passes"]
        print(f'{s["workload"]:16} {s["adapter"]:6} {s["backend"]:6} {s["size"]:9} {s["aa"]:8} '
              f'{s["gtao"]:6} gpu {s["gpu_p50"]:6.3f}  opaque {p.get("opaque+sky", 0):6.3f}  '
              f'gtao {p.get("gtao", 0):6.3f}  taa {p.get("taa", 0):6.3f}')
    if a.out:
        Path(a.out).write_text(json.dumps({"summary": summary, "runs": runs}, indent=1))


if __name__ == "__main__":
    main()
