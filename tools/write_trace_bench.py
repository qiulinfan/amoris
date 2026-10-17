#!/usr/bin/env python3
"""The write trace's cost per tick (docs/bench/write-trace.md): the `write_trace_bench` example on
the sailing sample (a light script load) and on the rule workload of bench/rules (a heavy one),
each run in its own process with the machine's conditions recorded around it.

    python tools/write_trace_bench.py build
        builds the example (release)
    python tools/write_trace_bench.py run OUT [--runs 3] [--sets sailing,rules] [--lock LOCK.py]
        for each run and set (the sets' order alternating by run): one process, under the bench
        lock when `--lock` names it (`python LOCK.py run --label writes -- python THIS one ...`),
        so the conditions before and after are sampled while the lock is held
    python tools/write_trace_bench.py table OUT
        prints the result tables in the form docs/bench/write-trace.md keeps them

OUT receives `<set>-<run>.json` (the example's report, with `conditions` and `wall`) and the
conditions files.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
EXE = TARGET / "release" / "examples" / (
    "write_trace_bench.exe" if os.name == "nt" else "write_trace_bench")

# The sets: the project, the fields traced and the block sizes. In the rule workload the entity
# with id 2 is one of the fighters `populate` spawns; `damage` writes its `Health.hp` when it is
# hit and as it regenerates, and its `Spot.x` every tick through a helper's result (`x[i] +=
# driftX(...)`).
SETS = {
    "sailing": {
        "project": "samples/sailing",
        "fields": ["Sloop.Boat.rudder", "Sloop.Log.distance", "Sloop.Model"],
        "args": ["--ticks", "600", "--rounds", "20", "--warmup", "120"],
    },
    "rules": {
        "project": "bench/rules",
        "fields": ["2.Health.hp", "2.Spot.x"],
        "args": ["--ticks", "60", "--rounds", "6", "--warmup", "30"],
    },
}


def now():
    return datetime.now().isoformat(timespec="seconds")


def sh(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def build(_args):
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "5"))
    sh(["cargo", "build", "--release", "-p", "pocket-runtime", "--features", "transpile",
        "--example", "write_trace_bench"], cwd=REPO, env=env)


def conditions(path, label):
    sh([sys.executable, REPO / "tools" / "machine_conditions.py", path, "--label", label])
    return json.loads(Path(path).read_text(encoding="utf-8"))


def one(args):
    """One process of one set, with the conditions before and after (inside the lock)."""
    out = Path(args.out)
    spec = SETS[args.set]
    stem = out.with_suffix("")
    started = now()
    before = conditions(f"{stem}-conditions-start.json", f"writes-{args.set}-start")
    cmd = [EXE, *spec["args"], "--out", out]
    for f in spec["fields"]:
        cmd += ["--field", f]
    sh([*cmd, REPO / spec["project"]], cwd=REPO)
    after = conditions(f"{stem}-conditions-end.json", f"writes-{args.set}-end")
    report = json.loads(out.read_text(encoding="utf-8"))
    report["wall"] = {"started": started, "ended": now()}
    report["conditions"] = {"start": summary(before), "end": summary(after)}
    out.write_text(json.dumps(report, indent=2), encoding="utf-8")


def summary(c):
    """The conditions the bench doc quotes: when, the mean CPU load and the power source."""
    return {"time": c.get("time"), "cpu_pct": (c.get("cpu_load") or {}).get("mean_pct"),
            "on_ac": c.get("on_ac_power")}


def run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if not EXE.is_file():
        sys.exit(f"{EXE} is missing: python tools/write_trace_bench.py build")
    sets = args.sets.split(",")
    for r in range(1, args.runs + 1):
        order = sets if r % 2 else sets[::-1]
        for name in order:
            cmd = [sys.executable, Path(__file__).resolve(), "one", name, out / f"{name}-{r}.json"]
            if args.lock:
                cmd = [sys.executable, args.lock, "run", "--label", "writes", "--max-hold-min",
                       "20", "--", *cmd]
            sh(cmd, cwd=REPO)


def table(args):
    out = Path(args.out)
    for name in SETS:
        files = sorted(out.glob(f"{name}-[0-9]*.json"), key=lambda p: p.stem)
        files = [f for f in files if "conditions" not in f.stem]
        if not files:
            continue
        reports = [json.loads(f.read_text(encoding="utf-8")) for f in files]
        stmts = [r["statements_per_tick"] for r in reports]
        print(f"\n{name}: {len(reports)} runs, {statistics.median(stmts):.0f} script statements a tick")
        for r, f in zip(reports, files):
            c = r.get("conditions", {})
            print(f"  {f.name}: {r.get('wall', {})} {c}")
        header = ["Mode"] + [f"Run {i + 1}" for i in range(len(reports))] + [
            "Against off", "First tick (median of runs)"]
        print("| " + " | ".join(header) + " |")
        print("|" + "---|" * len(header))
        for i, m in enumerate(reports[0]["modes"]):
            meds = [r["modes"][i]["median_us"] for r in reports]
            ratios = [r["modes"][i]["vs_off"] for r in reports]
            first = statistics.median(r["modes"][i]["first_us"] for r in reports)
            cells = [f"`{m['mode']}`"] + [f"{x:.1f} us" for x in meds]
            cells.append(", ".join(f"{(x - 1) * 100:+.1f} %" for x in ratios) if i else "1")
            cells.append(f"{first:.0f} us")
            print("| " + " | ".join(cells) + " |")
        for i, m in enumerate(reports[-1]["modes"]):
            if m.get("last_row"):
                print(f"  last row of {m['mode']}: {json.dumps(m['last_row'])[:300]}")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build").set_defaults(fn=build)
    r = sub.add_parser("run")
    r.add_argument("out")
    r.add_argument("--runs", type=int, default=3)
    r.add_argument("--sets", default="sailing,rules")
    r.add_argument("--lock")
    r.set_defaults(fn=run)
    o = sub.add_parser("one")
    o.add_argument("set", choices=sorted(SETS))
    o.add_argument("out")
    o.set_defaults(fn=one)
    t = sub.add_parser("table")
    t.add_argument("out")
    t.set_defaults(fn=table)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
