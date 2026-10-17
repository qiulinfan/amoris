#!/usr/bin/env python3
"""Times the physics cache on the level-scale scene, natively and in WebAssembly under Node, and on
the sailing scene natively (docs/bench/physics.md, "Static geometry and the cache").

Each level run is one `physics_bench level CELLS` (native) or one `tests/web/level.mjs` (the
`level_web` module) and prints one JSON line: the cache's bytes, its encode and decode, the
stand-in snapshot, restore and fork, a fork's continuation and a cold restore. Each sailing run is
one `physics_bench` with no arguments, whose last line is JSON. Builds, sizes and engines are
interleaved round by round, and the order of the builds and the engines turns by one each round,
so that a disturbance from other work, or a build that always runs after another, spreads over all
of them instead of landing on one.

Build first, in each checkout to compare:

    cargo build -p pocket-physics --example physics_bench --release
    cargo build -p pocket-physics --example level_web --target wasm32-unknown-unknown --release

Then, from the repository root:

    python tools/physcache_bench.py --build layout2=. --rounds 3 --out out/physcache/run

`--build NAME=DIR` (repeatable) names a build: DIR holds `physics_bench(.exe)` and
`level_web.wasm`, or is a checkout whose `target/` has them. `--sailing N` adds N interleaved
sailing runs per build; `--trajectory` runs each build's `physics_bench trajectory` once (on a build
of layout 1 its folds are `probe::layout::LAYOUT1_TRAJECTORIES`). The JSON lines go to
`OUT/runs.jsonl`, the machine's conditions before and after to `OUT/conditions-*.json`, and the
tables printed at the end give each configuration's median over the rounds and the spread of the
timings that the docs' claims rest on.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
# Each run's own statistic: the median of its `--bench-rounds` rounds, except where the label says
# otherwise (one sample per run, or the median of the warm ticks).
FIELDS = [
    ("cache_bytes", "cache MB", 1e-6),
    ("snapshot_bytes", "snapshot MB", 1e-6),
    ("terrain_spec_hash_ms", "spec hash ms", 1),
    ("cache_encode_ms", "encode ms", 1),
    ("cache_decode_ms", "decode ms", 1),
    ("snapshot_ms", "snapshot ms", 1),
    ("restore_ms", "restore ms", 1),
    ("fork_ms", "fork ms", 1),
    ("cold_restore_ms", "cold ms (1 sample)", 1),
    ("tick_ms", "tick ms (warm ticks)", 1),
    ("first_step_ms", "first tick ms (1 sample)", 1),
]
SPREAD = ["cache_encode_ms", "snapshot_ms", "fork_ms", "cold_restore_ms"]
SAILING = [
    ("tick_us", "tick µs"),
    ("snapshot_hash_us", "snapshot and hash µs"),
    ("fork_us", "fork µs"),
    ("cache_bytes", "cache bytes"),
]


def binaries(directory):
    d = Path(directory).resolve()
    native = d / f"physics_bench{EXE}"
    wasm = d / "level_web.wasm"
    if not native.exists():
        native = d / "target" / "release" / "examples" / f"physics_bench{EXE}"
        wasm = d / "target" / "wasm32-unknown-unknown" / "release" / "examples" / "level_web.wasm"
    return native, wasm


def run(cmd):
    out = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if out.returncode != 0:
        sys.exit(f"{' '.join(map(str, cmd))} failed:\n{out.stdout}\n{out.stderr}")
    return json.loads(out.stdout.strip().splitlines()[-1])


def turned(items, r):
    k = r % len(items)
    return items[k:] + items[:k]


def fmt(x):
    return f"{x:.4g}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", action="append", required=True, help="NAME=DIR")
    ap.add_argument("--cells", default="100,707")
    ap.add_argument("--engines", default="native,wasm")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--side", type=int, default=10)
    ap.add_argument("--warm", type=int, default=150)
    ap.add_argument("--bench-rounds", type=int, default=5, help="each run's own rounds")
    ap.add_argument("--check", type=int, default=30)
    ap.add_argument("--sailing", type=int, default=0, help="sailing runs per build")
    ap.add_argument("--trajectory", action="store_true")
    ap.add_argument("--node", default="node")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    builds = []
    for b in args.build:
        name, _, directory = b.partition("=")
        builds.append((name, *binaries(directory)))
    conditions = [sys.executable, str(ROOT / "tools" / "machine_conditions.py")]
    subprocess.run([*conditions, str(out / "conditions-start.json"), "--label", "physcache-start"], check=False)

    common = ["--side", str(args.side), "--warm", str(args.warm), "--rounds", str(args.bench_rounds),
              "--check", str(args.check)]
    engines = args.engines.split(",") if args.engines else []
    results, sailing, folds = [], [], []
    with open(out / "runs.jsonl", "a", encoding="utf-8") as log:
        def record(res, **tags):
            res.update(tags)
            log.write(json.dumps(res) + "\n")
            log.flush()

        if args.trajectory:
            for name, native, _ in builds:
                res = run([str(native), "trajectory"])
                record(res, build=name, kind="trajectory")
                folds.append((name, res))
                print(f"trajectory {name}: {res['folds']}", flush=True)
        for r in range(args.rounds if engines else 0):
            for cells in args.cells.split(","):
                for name, native, wasm in turned(builds, r):
                    for engine in turned(engines, r):
                        if engine == "native":
                            cmd = [str(native), "level", cells, *common]
                        else:
                            cmd = [args.node, str(ROOT / "crates/pocket-physics/tests/web/level.mjs"), str(wasm),
                                   cells, *common]
                        res = run(cmd)
                        record(res, build=name, engine_kind=engine, round=r, kind="level")
                        results.append(res)
                        print(f"round {r} {name} {engine} {cells}: cache {res['cache_bytes']} B, "
                              f"fork {res['fork_ms']:.1f} ms, continues {res['fork_continues']}", flush=True)
        for r in range(args.sailing):
            for name, native, _ in turned(builds, r):
                res = run([str(native)])
                record(res, build=name, round=r, kind="sailing")
                sailing.append(res)
                print(f"sailing {r} {name}: snapshot {res['snapshot_hash_us']:.1f} µs, "
                      f"fork {res['fork_us']:.1f} µs", flush=True)
    subprocess.run([*conditions, str(out / "conditions-end.json"), "--label", "physcache-end"], check=False)

    if results:
        print("\n| Build | Engine | Triangles | " + " | ".join(f[1] for f in FIELDS) + " | fork continues | hash |")
        print("|---" * (len(FIELDS) + 5) + "|")
        keys = sorted({(x["build"], x["engine_kind"], x["triangles"]) for x in results},
                      key=lambda k: (k[2], k[0], k[1]))
        for build, engine, tris in keys:
            rows = [x for x in results if (x["build"], x["engine_kind"], x["triangles"]) == (build, engine, tris)]
            cells = [fmt(statistics.median(x[f] for x in rows) * scale) for f, _, scale in FIELDS]
            ok = all(x["fork_continues"] and x["cold_restore_same"] for x in rows)
            hashes = sorted({x["hash_at_warm"] for x in rows})
            print(f"| {build} | {engine} | {tris:,} | " + " | ".join(cells) + f" | {ok} | {','.join(hashes)} |")
        print("\nSpread over the rounds (fastest–slowest of the runs' own statistics):\n")
        print("| Build | Engine | Triangles | " + " | ".join(SPREAD) + " |")
        print("|---" * (len(SPREAD) + 3) + "|")
        for build, engine, tris in keys:
            rows = [x for x in results if (x["build"], x["engine_kind"], x["triangles"]) == (build, engine, tris)]
            cells = [f"{fmt(min(x[f] for x in rows))}–{fmt(max(x[f] for x in rows))}" for f in SPREAD]
            print(f"| {build} | {engine} | {tris:,} | " + " | ".join(cells) + " |")
    if sailing:
        print(f"\nSailing scene at tick 300, {args.sailing} runs per build (median, fastest–slowest):\n")
        print("| Build | " + " | ".join(f[1] for f in SAILING) + " |")
        print("|---" * (len(SAILING) + 1) + "|")
        for name, _, _ in builds:
            rows = [x for x in sailing if x["build"] == name]
            cells = [f"{fmt(statistics.median(x[f] for x in rows))} "
                     f"({fmt(min(x[f] for x in rows))}–{fmt(max(x[f] for x in rows))})" for f, _ in SAILING]
            print(f"| {name} | " + " | ".join(cells) + " |")
    for name, res in folds:
        print(f"\ntrajectory folds, {name} ({res['identity']}): {json.dumps(res['folds'])}")


if __name__ == "__main__":
    main()
