#!/usr/bin/env python3
"""Fork lookahead for the skipper and the cost of a fork (docs/bench/player.md, section 5): runs the
`lookahead_bench` example of pocket-runtime and prints its tables.

    python tools/lookahead_bench.py build
        builds the example (release)
    python tools/lookahead_bench.py run OUT [--fork-runs 3] [--seeds 0-30] [--chunk 8]
                                            [--models greedy,lookahead,nofork]
                                            [--lock LOCK.py] [--no-fork] [--no-course]
        runs `fork` once per fork run (OUT/fork-rN.json) and `course` over the seeds in chunks of
        `--chunk` seeds (OUT/course-A-B.json); each is one `step`, wrapped in
        `python LOCK.py run --label lookahead --` when `--lock` names the shared machine's bench
        lock, which records the machine's conditions (tools/machine_conditions.py,
        OUT/conditions-NAME.json) and then runs the example, inside the same hold
    python tools/lookahead_bench.py step OUT NAME -- ARGS...
        one measurement: the conditions, then the example with ARGS
    python tools/lookahead_bench.py table OUT
        prints every table and figure docs/bench/player.md 5 keeps, from OUT's files: the course
        comparison of the models (exact but for wall time), the paired differences, the crate
        weight's effect, the cost of a weighed decision, the land warning's charges, the omniscient
        condition's effect, and the fork costs as the median of the fork runs' medians (minimum to
        maximum), with the CPU load each timed command started under
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
EXE = TARGET / "release" / "examples" / ("lookahead_bench.exe" if os.name == "nt" else "lookahead_bench")
CRATE_TICKS = 1000.0
MAX_TICKS = 36000
CONDITIONS = ["perception", "omniscient"]
MODELS = ["greedy", "lookahead", "nofork"]
# The pairs the tables compare: what the lookahead gains over the reference skipper, what the same
# model gains without forks, and what forking adds to it (fork on minus fork off).
PAIRS = [("lookahead", "greedy"), ("nofork", "greedy"), ("lookahead", "nofork")]
# An episode with more decisions than this livelocked aground (spec/player.md 10).
LIVELOCK_DECISIONS = 300
SWITCH_M = 15.0


def sh(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def locked(args, cmd):
    if args.lock:
        return [sys.executable, args.lock, "run", "--label", "lookahead", "--max-hold-min", "20",
                "--", *cmd]
    return cmd


def build(_args):
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "5"))
    sh(["cargo", "build", "--release", "-p", "pocket-runtime", "--features", "transpile",
        "--example", "lookahead_bench"], cwd=REPO, env=env)


def step(args):
    out = Path(args.out).resolve()
    sh([sys.executable, REPO / "tools" / "machine_conditions.py",
        out / f"conditions-{args.name}.json", "--label", args.name])
    rest = args.rest[1:] if args.rest[:1] == ["--"] else args.rest
    sh([EXE, *rest])


def run(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    started = datetime.now().isoformat(timespec="seconds")
    me = [sys.executable, Path(__file__).resolve(), "step", out]
    if not args.no_fork:
        for r in range(1, args.fork_runs + 1):
            sh(locked(args, [*me, f"fork-r{r}", "--", "fork", out / f"fork-r{r}.json"]))
    if not args.no_course:
        a, b = (int(x) for x in args.seeds.split("-"))
        for lo in range(a, b + 1, args.chunk):
            hi = min(b, lo + args.chunk - 1)
            sh(locked(args, [*me, f"course-{lo}-{hi}", "--", "course",
                             out / f"course-{lo}-{hi}.json", "--seeds", f"{lo}-{hi}",
                             "--models", args.models]))
    record = {"started": started, "ended": datetime.now().isoformat(timespec="seconds"),
              "args": {k: v for k, v in vars(args).items() if k != "f"}}
    (out / "run.json").write_text(json.dumps(record, indent=1) + "\n")


def mean(v):
    return sum(v) / len(v) if v else float("nan")


def median(v):
    return statistics.median(v) if v else float("nan")


def score(r, w=CRATE_TICKS):
    """Ticks to finish (the limit if unfinished) less `w` ticks per crate aboard: lower is better."""
    ticks = r["ticks"] if r["finished"] else MAX_TICKS
    return ticks - w * r["taken"]


def counts(ds):
    """How many differences are below, at and above zero."""
    return sum(d < 0 for d in ds), sum(d == 0 for d in ds), sum(d > 0 for d in ds)


def plan_name(p):
    return p if isinstance(p, str) else next(iter(p))


def choose(values, switch_m=SWITCH_M):
    """The lookahead's choice over branch values (lookahead.rs `choose`, nothing refused)."""
    best = 0
    for i in range(1, len(values)):
        if values[i] > values[best] + (switch_m if best == 0 else 0.0):
            best = i
    return best


def load_course(out):
    runs = []
    for f in sorted(out.glob("course-*.json")):
        runs += json.loads(f.read_text())["runs"]
    by = {}
    for r in runs:
        by.setdefault((r["condition"], r["model"]), {})[r["seed"]] = r
    return runs, by


def course_table(out):
    runs, by = load_course(out)
    if not runs:
        return
    seeds = sorted({r["seed"] for r in runs})
    models = [m for m in MODELS if any((c, m) in by for c in CONDITIONS)]
    print(f"Course: seeds {seeds[0]} to {seeds[-1]} ({len(seeds)} variants), models {', '.join(models)}\n")
    print("| Condition | Model | Finished | Ticks to finish, mean (median) | Crates aboard, mean "
          "| Score, mean | Decisions, mean (median) | Forks, mean | Branch ticks, mean "
          "| Overrides, mean | Wall s, median |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for cond in CONDITIONS:
        for model in models:
            rs = list(by.get((cond, model), {}).values())
            if not rs:
                continue
            fin = [r["ticks"] for r in rs if r["finished"]]
            la = [r["lookahead"] for r in rs if r["lookahead"]]
            dec = [r["decisions"] for r in rs]
            look = (f"| {mean([l['forks'] for l in la]):.1f} | {mean([l['branch_ticks'] for l in la]):,.0f} "
                    f"| {mean([l['overrides'] for l in la]):.1f} " if la else "| | | ")
            print(f"| {cond} | {model} | {len(fin)}/{len(rs)} | {mean(fin):,.0f} ({median(fin):,.0f}) "
                  f"| {mean([r['taken'] for r in rs]):.2f} | {mean([score(r) for r in rs]):,.0f} "
                  f"| {mean(dec):.1f} ({median(dec):.0f}) {look}"
                  f"| {median([r['wall_s'] for r in rs]):.2f} |")

    pairs = [(a, b) for a, b in PAIRS if a in models and b in models]
    print("\nPaired by variant (first model minus second):\n")
    print("| Condition | Pair | Score, mean (median) | Crates, mean | Ticks, mean (median) "
          "| Variants better / equal / worse | More / fewer crates | Faster / slower |")
    print("|---|---|---|---|---|---|---|---|")
    for a, b in pairs:
        for cond in CONDITIONS:
            x, y = by.get((cond, a), {}), by.get((cond, b), {})
            common = sorted(set(x) & set(y))
            if not common:
                continue
            ds = [score(x[s]) - score(y[s]) for s in common]
            dc = [x[s]["taken"] - y[s]["taken"] for s in common]
            dt = [x[s]["ticks"] - y[s]["ticks"] for s in common]
            better, equal, worse = counts(ds)
            more, _, fewer = counts([-d for d in dc])
            faster, _, slower = counts(dt)
            print(f"| {cond} | {a} - {b} | {mean(ds):+,.0f} ({median(ds):+,.0f}) | {mean(dc):+.2f} "
                  f"| {mean(dt):+,.0f} ({median(dt):+,.0f}) | {better} / {equal} / {worse} "
                  f"| {more} / {fewer} | {faster} / {slower} |")

    print("\nThe crate's weight in the score (ticks a crate is worth; the bench and the lookahead's "
          "value use 1000): variants better / equal / worse\n")
    weights = [0, 250, 500, 1000, 2000]
    print("| Condition | Pair | " + " | ".join(f"{w}" for w in weights) + " |")
    print("|---|---|" + "---|" * len(weights))
    for a, b in pairs:
        for cond in CONDITIONS:
            x, y = by.get((cond, a), {}), by.get((cond, b), {})
            common = sorted(set(x) & set(y))
            if not common:
                continue
            cells = []
            for w in weights:
                cells.append("%d / %d / %d" % counts([score(x[s], w) - score(y[s], w) for s in common]))
            print(f"| {cond} | {a} - {b} | " + " | ".join(cells) + " |")

    print(f"\nWithout the variants where an episode livelocked aground (more than "
          f"{LIVELOCK_DECISIONS} decisions):\n")
    print("| Condition | Pair | Variants left (left out) | Score, mean (median) | Ticks, mean (median) |")
    print("|---|---|---|---|---|")
    for cond in CONDITIONS:
        stuck = sorted({r["seed"] for r in runs
                        if r["condition"] == cond and r["decisions"] > LIVELOCK_DECISIONS})
        for a, b in pairs:
            x, y = by.get((cond, a), {}), by.get((cond, b), {})
            common = [s for s in sorted(set(x) & set(y)) if s not in stuck]
            if not common:
                continue
            ds = [score(x[s]) - score(y[s]) for s in common]
            dt = [x[s]["ticks"] - y[s]["ticks"] for s in common]
            print(f"| {cond} | {a} - {b} | {len(common)} ({', '.join(map(str, stuck)) or 'none'}) "
                  f"| {mean(ds):+,.0f} ({median(ds):+,.0f}) | {mean(dt):+,.0f} ({median(dt):+,.0f}) |")

    print("\nCost of a weighed decision (wall time inside the model's decisions over the decisions "
          "that weighed candidates):\n")
    print("| Condition | Model | Weighed decisions | Wall ms a weighed decision | Forks a weighed "
          "decision | Branch ticks a weighed decision | Branch ticks / main ticks |")
    print("|---|---|---|---|---|---|---|")
    for cond in CONDITIONS:
        for model in models:
            rs = [r for r in by.get((cond, model), {}).values() if r["lookahead"]]
            if not rs:
                continue
            looked = sum(r["lookahead"]["looked"] for r in rs)
            forks = sum(r["lookahead"]["forks"] for r in rs)
            bt = sum(r["lookahead"]["branch_ticks"] for r in rs)
            ticks = sum(r["ticks"] for r in rs)
            dec_s = sum(r["decide_s"] for r in rs)
            print(f"| {cond} | {model} | {looked} | {1000 * dec_s / looked:.1f} | {forks / looked:.2f} "
                  f"| {bt / looked:.0f} | {bt / ticks:.2f} |")

    land = {}
    changed = []
    refused = 0
    branches = 0
    for r in runs:
        for d in r.get("weighed") or []:
            bs = d["branches"]
            branches += len(bs)
            refused += sum(1 for b in bs if b["refused"])
            for b in bs:
                if b["land"]:
                    key = (r["model"], r["condition"], plan_name(b["plan"]))
                    land[key] = land.get(key, 0) + 1
            if any(b["land"] for b in bs):
                with_charge = choose([b["value"] for b in bs])
                without = choose([b["value"] + (1000.0 if b["land"] else 0.0) for b in bs])
                if with_charge != without:
                    changed.append(f"{r['model']} {r['condition']} variant {r['seed']} tick {d['tick']}")
    if branches:
        print(f"\nBranches charged for a land warning ({branches} branches weighed, "
              f"{refused} refused their first act):\n")
        print("| Model | Condition | Plan | Branches charged |")
        print("|---|---|---|---|")
        for (model, cond, plan), n in sorted(land.items()):
            print(f"| {model} | {cond} | {plan} | {n} |")
        print(f"\nDecisions whose choice the charge changed: {len(changed)}"
              + (f" ({'; '.join(changed)})" if changed else ""))

    print("\nOmniscient minus perception, by model:\n")
    print("| Model | Score, mean | Crates, mean | Decisions, mean |")
    print("|---|---|---|---|")
    for model in models:
        p, o = by.get(("perception", model), {}), by.get(("omniscient", model), {})
        common = sorted(set(p) & set(o))
        if common:
            print(f"| {model} | {mean([score(o[s]) - score(p[s]) for s in common]):+,.0f} "
                  f"| {mean([o[s]['taken'] - p[s]['taken'] for s in common]):+.2f} "
                  f"| {mean([o[s]['decisions'] - p[s]['decisions'] for s in common]):+.1f} |")


def fork_table(out):
    runs = [json.loads(f.read_text()) for f in sorted(out.glob("fork-r*.json"))]
    if not runs:
        return
    r0 = runs[0]
    print(f"\nFork costs: {r0['world']}, tick {r0['tick']}, {r0['entities']} entities, snapshot "
          f"{r0['snapshot_bytes']:,} bytes; {len(runs)} runs, median of the runs' medians "
          "(minimum to maximum)\n")
    print("| Operation | Wall time, us | Bytes allocated (median call) | Bytes held after the call "
          "(median) |")
    print("|---|---|---|---|")
    for key, name in [("snapshot", "snapshot"), ("restore_into_kept_branch", "restore into a kept branch"),
                      ("fork_into_kept_branch", "fork into a kept branch (`fork_into`)"),
                      ("fork_fresh_branch", "fresh branch (`Game::fork`)")]:
        meds = [r[key]["us"]["median"] for r in runs]
        alloc = [r[key]["allocated_bytes_median"] for r in runs]
        held = [r[key].get("held_bytes_median") for r in runs]
        held_s = f"{median(held):,.0f}" if None not in held else "not measured"
        print(f"| {name} | {median(meds):,.1f} ({min(meds):,.1f} to {max(meds):,.1f}) "
              f"| {median(alloc):,.0f} | {held_s} |")
    if all("drop_fresh_branch_us" in r for r in runs):
        drops = [r["drop_fresh_branch_us"]["median"] for r in runs]
        print(f"| dropping a fresh branch | {median(drops):,.1f} ({min(drops):,.1f} to "
              f"{max(drops):,.1f}) | | |")
    held = [r["fresh_branch_rust_bytes_held"]["median"] for r in runs]
    heap = [r["fresh_branch_script_heap_bytes"]["median"] for r in runs]
    tick = [r["branch_tick_us"] for r in runs]
    print(f"\nA live fresh branch holds {median(held):,.0f} bytes of Rust heap and "
          f"{median(heap):,.0f} bytes of script heap (main's: "
          f"{r0['main_script_heap_bytes']:,}); a branch tick takes {median(tick):.1f} us "
          f"({min(tick):.1f} to {max(tick):.1f}).")


def conditions_table(out):
    loads = {}
    for f in sorted(out.glob("conditions-*.json")):
        c = json.loads(f.read_text())
        kind = "fork" if c["label"].startswith("fork") else "course"
        load = (c.get("cpu_load") or {}).get("mean_pct")
        if load is not None:
            loads.setdefault(kind, []).append(load)
    if loads:
        print("\nCPU load at the start of each timed command (machine_conditions.py, inside the hold): "
              + "; ".join(f"{k} {min(v):.0f} to {max(v):.0f}%" for k, v in sorted(loads.items())))


def table(args):
    out = Path(args.out).resolve()
    course_table(out)
    fork_table(out)
    conditions_table(out)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build").set_defaults(f=build)
    st = sub.add_parser("step")
    st.add_argument("out")
    st.add_argument("name")
    st.add_argument("rest", nargs=argparse.REMAINDER)
    st.set_defaults(f=step)
    r = sub.add_parser("run")
    r.add_argument("out")
    r.add_argument("--fork-runs", type=int, default=3)
    r.add_argument("--seeds", default="0-30")
    r.add_argument("--chunk", type=int, default=8)
    r.add_argument("--models", default=",".join(MODELS))
    r.add_argument("--lock")
    r.add_argument("--no-fork", action="store_true")
    r.add_argument("--no-course", action="store_true")
    r.set_defaults(f=run)
    t = sub.add_parser("table")
    t.add_argument("out")
    t.set_defaults(f=table)
    a = p.parse_args()
    a.f(a)


if __name__ == "__main__":
    main()
