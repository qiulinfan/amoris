#!/usr/bin/env python3
"""The transparent stage's cost on one adapter and backend (docs/bench/transparency.md 2).

Runs the `transparency` example's `bench` for each kind (`hidden`, `opaque`, `blend`, `glass`,
`double`: the same 1,000 spheres invisible, opaque, alpha-blended, transmissive and double-sided
blended) in interleaved rounds, each in its own process with `--settle 5`, and records the
machine's conditions before the first round (tools/machine_conditions.py). Run it inside the
bench lock; one adapter and backend per invocation keeps the hold short:

    python ../../../out/locks/bench_lock.py run --label transparency -- python tools/render/transparency_bench.py \\
        --adapter 5060 --backend dx12 --out ../../../out/transparency/bench

Build first: `cargo build --release -p amoris-render --example transparency`. Each run's JSON is
kept under `--out`; `summary-ADAPTER-BACKEND.json` holds the medians over the rounds of each kind's
GPU time (timestamped passes), its opaque pass (which the stage draws in, or with GTAO on the
`transparent` pass after GTAO), GTAO, the shadow cascades, its CPU encoding time and the stage's
own CPU time, and the differences from `hidden`. `--extra '--orbit'` turns the camera (the order and
the list change every frame), `--env AMORIS_GTAO=off` sets the renderer's options through its
environment (both name the summary), and `--exe` runs another build's example (an A/B).
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
KINDS = ["hidden", "opaque", "blend", "glass", "double"]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--backend", required=True)
    ap.add_argument("--out", default="out/transparency/bench")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--count", type=int, default=1000)
    ap.add_argument("--frames", type=int, default=300)
    ap.add_argument("--kinds", default=",".join(KINDS))
    ap.add_argument("--extra", default="", help="more bench options, e.g. '--orbit'")
    ap.add_argument("--env", action="append", default=[], metavar="KEY=VALUE",
                    help="an environment variable for the runs, e.g. AMORIS_GTAO=off (repeatable)")
    ap.add_argument("--exe", help="the transparency example to run (default: this checkout's)")
    ap.add_argument("--ghost", action="store_true",
                    help="instead: TAA's ghosting (the example's `ghost`) for each kind and motion")
    args = ap.parse_args()
    out = (ROOT / args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f"{args.adapter}-{args.backend}{'-' + args.extra.strip('-').replace(' ', '-') if args.extra else ''}"
    name += "".join(f"-{e.split('=', 1)[0].removeprefix('AMORIS_').lower()}-{e.split('=', 1)[1]}" for e in args.env)
    subprocess.run([sys.executable, str(ROOT / "tools" / "machine_conditions.py"),
                    str(out / f"conditions-{name}.json"), "--label", f"transparency {name}"], check=False)
    env = dict(os.environ, AMORIS_ADAPTER=args.adapter, AMORIS_BACKEND=args.backend)
    env.update(e.split("=", 1) for e in args.env)
    exe = args.exe or str(ROOT / "target" / "release" / "examples" / f"transparency{EXE}")
    if args.ghost:
        return ghost(exe, env, out, name)
    kinds = args.kinds.split(",")
    runs = {k: [] for k in kinds}
    for r in range(args.rounds):
        for kind in kinds:
            p = subprocess.run([exe, "bench", "--kind", kind, "--count", str(args.count), "--frames",
                                str(args.frames), "--settle", "5", *args.extra.split()],
                               cwd=ROOT, env=env, capture_output=True, text=True, timeout=600)
            if p.returncode:
                print(f"{kind} round {r}: failed\n{p.stderr[-1500:]}", file=sys.stderr)
                return 1
            j = json.loads(p.stdout[p.stdout.index("{"):])
            (out / f"{name}-{kind}-{r}.json").write_text(json.dumps(j, indent=1) + "\n", newline="\n")
            runs[kind].append(j)
    med = lambda xs: round(statistics.median(xs), 3)  # noqa: E731
    table = {}
    for kind, js in runs.items():
        table[kind] = {
            "gpu_ms": med([j["gpu_ms"]["p50"] for j in js]),
            "opaque_pass_ms": med([j["passes_ms"].get("opaque+sky", 0.0) for j in js]),
            # The pass the stage draws in after GTAO (with GTAO on), and GTAO.
            "transparent_pass_ms": med([j["passes_ms"].get("transparent", 0.0) for j in js]),
            "gtao_ms": med([j["passes_ms"].get("gtao", 0.0) for j in js]),
            "shadows_ms": med([j["passes_ms"].get("shadows", 0.0) for j in js]),
            "cpu_encode_ms": med([j["cpu_encode_ms"]["p50"] for j in js]),
            "transparent_ms": med([j["transparent_ms"]["p50"] for j in js]),
            "submit_to_idle_ms": med([j["submit_to_idle_ms"]["p50"] for j in js]),
            "transparent_draws": js[0]["transparent_draws"],
            "gpu_ms_rounds": [j["gpu_ms"]["p50"] for j in js],
        }
    if "hidden" in table:
        base = table["hidden"]
        for kind, row in table.items():
            row["gpu_over_hidden_ms"] = round(row["gpu_ms"] - base["gpu_ms"], 3)
            row["cpu_over_hidden_ms"] = round(row["cpu_encode_ms"] - base["cpu_encode_ms"], 3)
    first = runs[kinds[0]][0]
    summary = {"tool": "tools/render/transparency_bench.py", "adapter": first["adapter"], "backend": first["backend"],
               "driver": first["driver"], "size": first["size"], "antialiasing": first["antialiasing"],
               "gtao": first["gtao"], "count": args.count, "frames": args.frames, "rounds": args.rounds,
               "extra": args.extra, "env": args.env, "exe": exe, "kinds": table}
    (out / f"summary-{name}.json").write_text(json.dumps(summary, indent=1) + "\n", newline="\n")
    print(f"{first['adapter']} {first['backend']} {first['antialiasing']} gtao {first['gtao']}")
    print(f"{'kind':8} {'gpu':>7} {'+hidden':>8} {'opaque':>7} {'transp':>7} {'gtao':>6} {'shadow':>7} "
          f"{'cpu':>7} {'+hidden':>8} {'sort':>6} {'draws':>6}")
    for kind, row in table.items():
        print(f"{kind:8} {row['gpu_ms']:7.3f} {row.get('gpu_over_hidden_ms', 0):8.3f} "
              f"{row['opaque_pass_ms']:7.3f} {row['transparent_pass_ms']:7.3f} {row['gtao_ms']:6.3f} "
              f"{row['shadows_ms']:7.3f} {row['cpu_encode_ms']:7.3f} "
              f"{row.get('cpu_over_hidden_ms', 0):8.3f} {row['transparent_ms']:6.3f} {row['transparent_draws']:6}")
    return 0


def ghost(exe, env, out, name):
    """TAA against 4x MSAA, each against a still jittered reference, by kind and motion."""
    rows = []
    for kind in ["opaque", "blend", "glass"]:
        for motion in ["object", "pan"]:
            p = subprocess.run([exe, "ghost", "--kind", kind, "--motion", motion, "--save", str(out)],
                               cwd=ROOT, env=env, capture_output=True, text=True, timeout=900)
            if p.returncode:
                print(f"ghost {kind} {motion}: failed\n{p.stderr[-1500:]}", file=sys.stderr)
                return 1
            rows.append(json.loads(p.stdout[p.stdout.index("{"):]))
    (out / f"ghost-{name}.json").write_text(json.dumps(rows, indent=1) + "\n", newline="\n")
    print(f"{rows[0]['adapter']} {rows[0]['backend']}: mean |luminance - reference|, TAA (4x MSAA)")
    print(f"{'kind':8} {'motion':7} {'sphere':>17} {'trail':>17} {'image':>17}")
    for r in rows:
        cell = lambda k: f"{r['taa'][k]:.4f} ({r['msaa'][k]:.4f})"  # noqa: E731
        print(f"{r['kind']:8} {r['motion']:7} {cell('sphere'):>17} {cell('trail'):>17} {cell('image'):>17}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
