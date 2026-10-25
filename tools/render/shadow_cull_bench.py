#!/usr/bin/env python3
"""Runs the `shadow_cull` example (crates/amoris-render/examples/shadow_cull.rs) across backends and
adapters and turns its output into the tables of docs/bench/shadows.md.

- `compare`: per configuration, `shadow_cull compare` (every scene drawn with slice culling off and
  on by two renderers on one device; frames compared to the bit; each cascade's draws read back).
- `bench`: per configuration and scene, `shadow_cull bench SCENE` (one renderer, off and on in
  interleaved blocks of frames), after recording the machine's conditions
  (tools/machine_conditions.py).
- `games`: per configuration, the amoris-app example `draw_paths --compare shadow_cull` on game
  projects (each run headless for its ticks, its merged render frame drawn twice in one process at
  one moment, off and on; pixels and the id pass's coverage compared), with occlusion culling and
  the depth prepass off. Needs `cargo build --release -p amoris-app --example draw_paths`.
- `table DIR...`: the Markdown tables from what those wrote.

A configuration is `BACKEND:ADAPTER` (AMORIS_BACKEND, AMORIS_ADAPTER), optionally `+minimal` for
WebGPU's baseline device (AMORIS_GPU_MINIMAL=features,limits,timestamps; `compare` only, the
baseline device has no timestamps). Build first: `cargo build --release -p amoris-render --example
shadow_cull`. Then, from the repository root (timing runs under the machine's bench lock):

    python tools/render/shadow_cull_bench.py compare --configs dx12:nvidia,vulkan:nvidia,dx12:780m
    python tools/render/shadow_cull_bench.py bench --configs dx12:nvidia,vulkan:nvidia
    python tools/render/shadow_cull_bench.py table out/shadowcull/compare out/shadowcull/bench

`bench` defaults to the scenes of docs/bench/shadows.md 4 (`BENCH_SCENES`), 4 rounds of 40-frame
blocks and `--settle 5`.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
EXAMPLE = ROOT / "target" / "release" / "examples" / f"shadow_cull{EXE}"
MINIMAL = "features,limits,timestamps"
# The scenes of docs/bench/shadows.md 4's timing table.
BENCH_SCENES = "lod-off-0,lod-off-0.5,lod-0,lod-0.5,cubes-dense,mixed"


def config_env(config):
    backend, _, rest = config.partition(":")
    adapter, _, extra = rest.partition("+")
    env = dict(os.environ, AMORIS_BACKEND=backend)
    env.pop("AMORIS_GPU_MINIMAL", None)
    if adapter:
        env["AMORIS_ADAPTER"] = adapter
    if extra == "minimal":
        env["AMORIS_GPU_MINIMAL"] = MINIMAL
    return env


def label(config):
    return config.replace(":", "-").replace("+", "-")


def compare(args):
    out = ROOT / args.out
    summary = {"tool": "tools/render/shadow_cull_bench.py compare",
               "date": time.strftime("%Y-%m-%d %H:%M"), "configs": {}}
    for config in args.configs.split(","):
        d = out / label(config)
        cmd = [str(EXAMPLE), "compare", "--out", str(d), "--size", args.size, "--count",
               str(args.count)]
        if args.occlusion:
            cmd += ["--occlusion", args.occlusion]
        cmd += [s for s in args.scenes.split(",") if s]
        t = time.time()
        p = subprocess.run(cmd, cwd=ROOT, env=config_env(config), capture_output=True, text=True)
        rows = [json.loads(line) for line in p.stdout.splitlines() if line.startswith("{")]
        summary["configs"][config] = {"returncode": p.returncode,
                                      "seconds": round(time.time() - t), "scenes": rows,
                                      "stderr": p.stderr[-800:] if p.returncode else ""}
        for r in rows:
            print(f"{config:24} {r['scene']:12} identical {r['identical_frames']}/{r['frames']}"
                  f" max diff {r['max_abs_diff']} shadow tris {r['shadow_triangles_last']}",
                  file=sys.stderr, flush=True)
        if p.returncode:
            print(f"{config}: exit {p.returncode}: {p.stderr[-800:]}", file=sys.stderr)
    out.mkdir(parents=True, exist_ok=True)
    name = f"compare{('-' + args.occlusion) if args.occlusion else ''}.json"
    (out / name).write_text(json.dumps(summary, indent=1) + "\n", newline="\n")


def games(args):
    out = ROOT / args.out
    summary = {"tool": "tools/render/shadow_cull_bench.py games",
               "date": time.strftime("%Y-%m-%d %H:%M"), "configs": {}}
    exe = ROOT / "target" / "release" / "examples" / f"draw_paths{EXE}"
    for config in args.configs.split(","):
        d = out / label(config)
        env = config_env(config)
        env.setdefault("AMORIS_OCCLUSION", "off")
        env.setdefault("AMORIS_PREPASS", "off")
        cmd = [str(exe), "--compare", "shadow_cull", "--out", str(d), "--ticks", str(args.ticks),
               *args.scenes.split(",")]
        p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
        rows = [json.loads(line) for line in p.stdout.splitlines() if line.startswith("{")]
        summary["configs"][config] = {"returncode": p.returncode, "scenes": rows,
                                      "stderr": p.stderr[-800:] if p.returncode else ""}
        for r in rows:
            c = r["compare"]
            culled = [a["instances"] - b["instances"]
                      for a, b in zip(r["off"].get("cascades", []), r["on"].get("cascades", []))]
            print(f"{config:24} {r['scene']:18} max diff {c['max_abs_diff']}, mean "
                  f"{c['mean_abs_diff']}, coverage L1 {c['coverage_l1']}, culled per cascade "
                  f"{culled}", file=sys.stderr, flush=True)
        if p.returncode:
            print(f"{config}: exit {p.returncode}: {p.stderr[-800:]}", file=sys.stderr)
    out.mkdir(parents=True, exist_ok=True)
    (out / "games.json").write_text(json.dumps(summary, indent=1) + "\n", newline="\n")


def bench(args):
    out = ROOT / args.out
    out.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    subprocess.run([sys.executable, str(ROOT / "tools" / "machine_conditions.py"),
                    str(out / f"conditions-{stamp}.json"), "--label",
                    f"shadow_cull {args.configs}"],
                   cwd=ROOT, check=False)
    for config in args.configs.split(","):
        for scene in args.scenes.split(","):
            path = out / f"{label(config)}-{scene}-{stamp}.json"
            cmd = [str(EXAMPLE), "bench", scene, "--size", args.size, "--count", str(args.count),
                   "--rounds", str(args.rounds), "--block", str(args.block), "--skip",
                   str(args.skip), "--json", str(path)]
            if args.settle:
                cmd += ["--settle", str(args.settle)]
            p = subprocess.run(cmd, cwd=ROOT, env=config_env(config), capture_output=True,
                               text=True)
            if p.returncode:
                print(f"{config} {scene}: exit {p.returncode}: {p.stderr[-800:]}", file=sys.stderr)
                continue
            r = json.loads(path.read_text())
            r["conditions"] = f"conditions-{stamp}.json"
            r["config"] = config
            path.write_text(json.dumps(r, indent=1) + "\n", newline="\n")
            print(f"{config:16} {scene:12} shadows {r['off']['shadows_ms']['p50']} -> "
                  f"{r['on']['shadows_ms']['p50']} ms, gpu {r['off']['gpu_ms']['p50']} -> "
                  f"{r['on']['gpu_ms']['p50']} ms", file=sys.stderr, flush=True)


def fmt(v, digits=2):
    return f"{v:.{digits}f}" if v is not None else "-"


def rounds_of(r, mode, key):
    """The per-round medians of `key` for `mode`: (median over rounds, spread)."""
    vals = [row[mode][key]["p50"] for row in r["per_round"] if mode in row]
    if not vals:
        return None, None
    med = statistics.median(vals)
    return med, ((max(vals) - min(vals)) / med if med else 0.0)


def table(args):
    compares, benches = [], []
    for d in args.dirs:
        for f in sorted(Path(d).glob("*.json")):
            j = json.loads(f.read_text())
            if j.get("tool", "").endswith(("compare", "games")):
                compares.append((f, j))
            elif j.get("bench") == "shadow_cull":
                benches.append(j)
    for f, j in [x for x in compares if x[1]["tool"].endswith("games")]:
        print(f"\n{f}:\n")
        print("| Configuration | Scene | Max difference | Coverage L1 | Instances | Cascade "
              "instances off -> on | Cascade triangles off -> on |")
        print("|---|---|---|---|---|---|---|")
        for config, c in j["configs"].items():
            for r in c["scenes"]:
                cmp = r["compare"]
                off, on = r["off"].get("cascades", []), r["on"].get("cascades", [])
                inst = " ".join(f"{a['instances']}->{b['instances']}" for a, b in zip(off, on))
                tri = (f"{sum(a['triangles'] for a in off):,} -> "
                       f"{sum(b['triangles'] for b in on):,}") if off else "-"
                print(f"| {config} | {r['scene']} | {cmp['max_abs_diff']} | {cmp['coverage_l1']} "
                      f"| {r['on']['instances']} | {inst or '-'} | {tri} |")
    compares = [x for x in compares if not x[1]["tool"].endswith("games")]
    for f, j in compares:
        print(f"\n{f}:\n")
        print("| Configuration | Draw path | Scene | Frames identical | Camera draws equal |")
        print("|---|---|---|---|---|")
        for config, c in j["configs"].items():
            for r in c["scenes"]:
                print(f"| {config} | {r.get('draw_path', '-')} | {r['scene']} | "
                      f"{r['identical_frames']} / {r['frames']} | "
                      f"{'yes' if r['camera_draws_same'] else 'NO'} |")
        first = next(iter(j["configs"].values()))
        print("\nPer cascade, last frame (instances / triangles), off -> on:\n")
        print("| Scene | Cascade 0 | Cascade 1 | Cascade 2 | Cascade 3 | Shadow triangles "
              "| Camera triangles |")
        print("|---|---|---|---|---|---|---|")
        for r in first["scenes"]:
            cells = []
            for a, b in zip(r["last_frame"]["off"], r["last_frame"]["on"]):
                cells.append(f"{a['instances']:,} / {a['triangles']:,} -> {b['instances']:,} / "
                             f"{b['triangles']:,}")
            so, sn = r["shadow_triangles_last"]
            ratio = f" ({sn / so:.2f}x)" if so else ""
            print(f"| {r['scene']} | " + " | ".join(cells)
                  + f" | {so:,} -> {sn:,}{ratio} | {r['camera_triangles']:,} |")
        # Do the counts agree across configurations?
        for config, c in j["configs"].items():
            for r, r0 in zip(c["scenes"], first["scenes"]):
                if r["all_frames"] != r0["all_frames"]:
                    print(f"\nnote: {config} {r['scene']} counts differ from the first "
                          f"configuration: {r['all_frames']}")
    if benches:
        print("\nGPU ms, median over rounds of each block's p50 (spread across rounds):\n")
        print("| Adapter, backend | Scene | Shadows off | Shadows on | Ratio | Cull off | Cull on "
              "| Frame GPU off | Frame GPU on | Wall off | Wall on |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for r in sorted(benches, key=lambda r: (r["adapter"], r["backend"], r["scene"],
                                                r["instances"])):
            row = {}
            for mode in ("off", "on"):
                for key in ("shadows_ms", "cull_ms", "gpu_ms", "wall_ms"):
                    row[mode, key] = rounds_of(r, mode, key)
            so, sn = row["off", "shadows_ms"][0], row["on", "shadows_ms"][0]
            cell = lambda m, k: f"{fmt(row[m, k][0])} ({row[m, k][1] * 100:.0f}%)"
            name = "RTX 5060" if "NVIDIA" in r["adapter"] else ("780M" if "780M" in r["adapter"]
                                                               else r["adapter"])
            # `--count` sets the cubes scenes' size; label any other than the default.
            scene = r["scene"]
            if scene.startswith("cubes") and r["instances"] != 200_000:
                scene += f" ({r['instances']:,})"
            print(f"| {name}, {r['backend']} | {scene} | {cell('off', 'shadows_ms')} | "
                  f"{cell('on', 'shadows_ms')} | {sn / so:.2f} | {cell('off', 'cull_ms')} | "
                  f"{cell('on', 'cull_ms')} | {cell('off', 'gpu_ms')} | {cell('on', 'gpu_ms')} | "
                  f"{cell('off', 'wall_ms')} | {cell('on', 'wall_ms')} |")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("compare")
    c.add_argument("--configs", default="dx12:nvidia,vulkan:nvidia")
    c.add_argument("--scenes", default="", help="comma-separated (default: every scene)")
    c.add_argument("--size", default="1280x720")
    c.add_argument("--count", type=int, default=200_000)
    c.add_argument("--occlusion", default="", help="off (default), on or auto")
    c.add_argument("--out", default="out/shadowcull/compare")
    b = sub.add_parser("bench")
    b.add_argument("--configs", default="dx12:nvidia,vulkan:nvidia")
    b.add_argument("--scenes", default=BENCH_SCENES)
    b.add_argument("--size", default="1280x720")
    b.add_argument("--count", type=int, default=200_000)
    b.add_argument("--rounds", type=int, default=4)
    b.add_argument("--block", type=int, default=40)
    b.add_argument("--skip", type=int, default=6)
    b.add_argument("--settle", type=float, default=5.0)
    b.add_argument("--out", default="out/shadowcull/bench")
    g = sub.add_parser("games")
    g.add_argument("--configs", default="dx12:nvidia,vulkan:nvidia")
    g.add_argument("--scenes",
                   default="samples/anim,samples/sailing,tests/fixtures/scenes/gi-room,tests/fixtures/scenes/pt-lab")
    g.add_argument("--ticks", type=int, default=120)
    g.add_argument("--out", default="out/shadowcull/games")
    t = sub.add_parser("table")
    t.add_argument("dirs", nargs="+")
    args = ap.parse_args()
    {"compare": compare, "bench": bench, "games": games, "table": table}[args.cmd](args)


if __name__ == "__main__":
    main()
