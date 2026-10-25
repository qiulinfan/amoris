#!/usr/bin/env python3
"""Times TAA and GTAO natively per backend and adapter (docs/bench/taa-gtao.md), and TAA's
upscaling at render scales below 1 (docs/bench/taau.md).

Workloads, offscreen, each frame submitted and waited for, timestamped passes:
- `check`: aa_eval's bench (`--settle 5` after each configuration's renderer is created): the
  check scene (`demo::aa_scene`, light geometry, shading-bound) at `--sizes`, every anti-aliasing
  mode with GTAO off and on (normals from the depth), and the normal target for msaa and taa;
  with `--scales`, TAA and MSAA with TAA at each scale below 1;
- `cubes`: many_cubes' sphere of 1.6M cubes (`--headless-bench`, geometry-bound) at 1600x900,
  every anti-aliasing mode with GTAO off and on, through AMORIS_AA and AMORIS_GTAO;
- `dense`: many_cubes' dense grid (`--dense --headless-bench --settle 5`, the UE5 comparison's
  scene) at `--sizes`, with occlusion culling `--occlusion` (off, auto), MSAA at the output's
  size and TAA at each of `--scales` (AMORIS_RENDER_SCALE), GTAO off; `--prepass on` pins the
  depth prepass, whose auto mode may otherwise decide differently from one configuration to the
  next.

`--conditions PREFIX` records tools/machine_conditions.py before and after the runs
(PREFIX-start.json, PREFIX-end.json). `--configs` keeps the check scene's configurations it names
(aa_eval's `--bench-configs`: `aa:gtao`, and `aa:gtao:scale` below a scale of 1). Rounds interleave every configuration so that
other work on the machine spreads over all of them. Build first: `cargo build --release -p
amoris-render --example aa_eval --example many_cubes`. Then, from the repository root:

    python tools/render/aa_bench.py --adapters nvidia,780m --backends vulkan,dx12 --rounds 2 \\
        --out out/bench-runs/aa/bench-native.json

    python tools/render/aa_bench.py --adapters nvidia --backends dx12 --workloads check,dense \\
        --scales 1,0.77,0.67,0.5 --sizes 1600x900,2560x1440 --prepass on \\
        --out out/taau/bench-nvidia-dx12.json

The summary keeps every run; the table printed at the end is the median over rounds of each
configuration's 10th-percentile pass times (the check scene: the least disturbed frames; many_cubes:
the mean pass times) and of its median total.
"""
import argparse
import json
import os
import statistics
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
AA = ["off", "msaa", "taa", "msaa+taa"]


def example(name):
    return str(ROOT / "target" / "release" / "examples" / f"{name}{EXE}")


def aa_eval(env, sizes, frames, scratch, scales, configs):
    # --settle 5: each configuration's new renderer compiles its pipelines on every core, which
    # lowers R1's GPU clocks for a few seconds (AGENTS.md).
    argv = [example("aa_eval"), str(scratch), "--only", "bench", "--bench-sizes", sizes,
            "--frames", str(frames), "--settle", "5"]
    if scales:
        argv += ["--scale", scales]
    if configs:
        argv += ["--bench-configs", configs]
    p = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
    rows = []
    for line in p.stderr.splitlines():
        if line.startswith("bench "):
            j = json.loads(line[6:])
            rows.append({"workload": "check scene", "size": j["size"], "aa": j["aa"],
                         "gtao": j["gtao"], "scale": round(j.get("render_scale", 1.0), 3),
                         "gpu_p50": j["profiled_gpu_p50_ms"],
                         "gpu_p10": j["profiled_gpu_p10_ms"], "passes_p10": j["passes_p10_ms"],
                         "passes_p50": j["passes_p50_ms"], "wall_p50": j["wall_p50_ms"]})
        elif "validation" in line or "panicked" in line:
            rows.append({"error": line[:300]})
    return rows


def cubes(env, aa, gtao, frames, size="1600x900", dense=False, occlusion=None, scale=1.0,
          prepass=None):
    e = dict(env, AMORIS_AA=aa, AMORIS_GTAO=gtao, AMORIS_RENDER_SCALE=str(scale))
    argv = [example("many_cubes"), "--headless-bench", str(frames), "--size", size]
    if dense:
        argv += ["--dense", "--settle", "5"]
    if occlusion:
        argv += ["--occlusion", occlusion]
    if prepass:
        argv += ["--prepass", prepass]
    p = subprocess.run(argv, cwd=ROOT, env=e, capture_output=True, text=True, timeout=900)
    for line in p.stdout.splitlines():
        if line.startswith("{"):
            j = json.loads(line)
            workload = "many_cubes 1.6M"
            if dense:
                workload = f"dense occlusion {j.get('occlusion_mode', occlusion)}"
            return {"workload": workload, "size": size, "aa": j["antialiasing"],
                    "gtao": "depth" if gtao == "on" else gtao,
                    "scale": round(j.get("render_scale", 1.0), 3),
                    "scene_size": j.get("scene_size"), "gpu_p50": j["gpu_ms"]["p50"],
                    # The depth prepass's auto mode: which way the measured frames drew.
                    "prepass_frames": j.get("prepass_frames"),
                    "passes_mean": j["passes_ms"], "wall_p50": j["submit_to_idle_ms"]["p50"],
                    "adapter_name": j["adapter"]}
    return {"error": (p.stdout + p.stderr)[-300:], "aa": aa, "gtao": gtao, "scale": scale}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--adapters", default="nvidia,780m")
    ap.add_argument("--backends", default="vulkan,dx12")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--first-round", type=int, default=0,
                    help="the first round's number (odd rounds run the dense grid in reverse)")
    ap.add_argument("--frames", type=int, default=150)
    ap.add_argument("--sizes", default="1600x900,2560x1440")
    ap.add_argument("--workloads", default="check,cubes")
    ap.add_argument("--scales", default="", help="render scales, e.g. 1,0.77,0.67,0.5")
    ap.add_argument("--occlusion", default="off,auto", help="the dense grid's occlusion modes")
    ap.add_argument("--prepass", help="the dense grid's depth prepass (off, on, auto; default auto)")
    ap.add_argument("--configs", default="", help="the check scene's configurations")
    ap.add_argument("--conditions", help="record machine conditions to PREFIX-start/end.json")
    ap.add_argument("--skip-cubes", action="store_true")
    ap.add_argument("--out")
    a = ap.parse_args()
    workloads = a.workloads.split(",")
    if a.skip_cubes:
        workloads = [w for w in workloads if w != "cubes"]
    scales = [float(s) for s in a.scales.split(",") if s] or [1.0]
    scratch = ROOT / "target" / "aa_bench"
    scratch.mkdir(parents=True, exist_ok=True)
    runs = []

    def conditions(label):
        if a.conditions:
            subprocess.run(["python", str(ROOT / "tools" / "machine_conditions.py"),
                            f"{a.conditions}-{label}.json", "--label", label], cwd=ROOT)

    conditions("start")

    def keep(row, r, adapter, backend):
        row.update({"round": r, "adapter": adapter, "backend": backend})
        runs.append(row)
        print(json.dumps(row), flush=True)

    for r in range(a.first_round, a.first_round + a.rounds):
        for adapter in a.adapters.split(","):
            for backend in a.backends.split(","):
                env = dict(os.environ, AMORIS_BACKEND=backend, AMORIS_ADAPTER=adapter)
                if "check" in workloads:
                    for row in aa_eval(env, a.sizes, a.frames, scratch, a.scales, a.configs):
                        keep(row, r, adapter, backend)
                if "cubes" in workloads:
                    for aa in AA:
                        for gtao in ["off", "on"]:
                            keep(cubes(env, aa, gtao, a.frames), r, adapter, backend)
                if "dense" in workloads:
                    configs = [("msaa", 1.0)] + [("taa", s) for s in scales]
                    # Every other round in the reverse order.
                    if r % 2:
                        configs.reverse()
                    for size in a.sizes.split(","):
                        for occlusion in a.occlusion.split(","):
                            for aa, scale in configs:
                                row = cubes(env, aa, "off", a.frames, size, True, occlusion, scale,
                                            a.prepass)
                                keep(row, r, adapter, backend)
    conditions("end")
    # The median over rounds per configuration.
    table = {}
    for row in runs:
        if "error" in row:
            continue
        key = (row["workload"], row["adapter"], row["backend"], row["size"], row["aa"],
               row["gtao"], round(row.get("scale", 1.0), 3))
        table.setdefault(key, []).append(row)
    summary = []
    for key, rows in sorted(table.items()):
        passes = rows[0].get("passes_p10") or rows[0].get("passes_mean") or {}
        med = {p: round(statistics.median((x.get("passes_p10") or x.get("passes_mean") or {}).get(p, 0.0)
                                          for x in rows), 3) for p in passes}
        summary.append({"workload": key[0], "adapter": key[1], "backend": key[2], "size": key[3],
                        "aa": key[4], "gtao": key[5], "scale": key[6],
                        "gpu_p50": round(statistics.median(x["gpu_p50"] for x in rows), 3),
                        "wall_p50": round(statistics.median(x["wall_p50"] for x in rows), 3),
                        "passes": med, "rounds": len(rows),
                        "prepass_frames": [x.get("prepass_frames") for x in rows]})
    for s in summary:
        p = s["passes"]
        opaque = p.get("opaque+sky", 0) + p.get("opaque early", 0) + p.get("depth prepass", 0)
        print(f'{s["workload"]:22} {s["adapter"]:6} {s["backend"]:6} {s["size"]:9} {s["aa"]:8} '
              f'{s["gtao"]:6} x{s["scale"]:<4} gpu {s["gpu_p50"]:7.3f}  wall {s["wall_p50"]:7.3f}  '
              f'opaque {opaque:6.3f}  hi-z {p.get("hi-z", 0):6.3f}  gtao {p.get("gtao", 0):6.3f}  '
              f'taa {p.get("taa", 0):6.3f}')
    if a.out:
        Path(a.out).parent.mkdir(parents=True, exist_ok=True)
        Path(a.out).write_text(json.dumps({"summary": summary, "runs": runs}, indent=1))


if __name__ == "__main__":
    main()
