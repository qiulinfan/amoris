#!/usr/bin/env python3
"""QuickJS-ng's native speed (docs/bench/script.md, section 7): builds of the script host compared
in interleaved rounds on the rule workload (`rules_bench`) and on microloops (`qjs_micro`).

    python tools/qjs_bench.py build NAME [--rev REV] [--out DIR]
        builds rules_bench and qjs_micro (release, --features transpile) from this checkout, or from
        the git revision REV exported (`git archive`, every file dated now) into DIR/src/NAME and
        built with the shared target directory DIR/target-rev, and copies both into DIR/bin/NAME/
        (DIR: out/qjs)
    python tools/qjs_bench.py run DIR --builds base,p12,... [--rounds 5] [--bins BIN] [--lock LOCK.py]
        takes each build's binaries from BIN/NAME (BIN: DIR/../bin, where `build` puts them), verifies each build once (its script world against the Rust twin at every one of 720 ticks;
        all builds must end on one digest), then runs the rounds: in each, every build in an order
        that rotates by round, each running rules_bench once (rules, twin, idle, scan) and
        qjs_micro once. `--lock` wraps every timed command in `python LOCK.py run --label qjs --`;
        the machine conditions are recorded before the run, before each round and after it
    python tools/qjs_bench.py table DIR [--builds base,p12,...]
        prints the tables of docs/bench/script.md, section 7 (the first build is the reference)

Windows or any host with the release examples; timings depend on the machine and its load.
"""
import argparse
import io
import json
import os
import shutil
import statistics
import subprocess
import sys
import tarfile
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
EXAMPLES = ["rules_bench", "qjs_micro"]
PROBES = ["loop", "int", "call", "f64", "prop", "sqrt", "sin", "cos", "atan2", "pow", "hypot"]
CALLS = ["sqrt", "sin", "cos", "atan2", "pow", "hypot"]


def sh(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def now():
    return datetime.now().isoformat(timespec="seconds")


def build(args):
    out = Path(args.out).resolve()
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "5"))
    cmd = ["cargo", "build", "--release", "-p", "amoris-script", "--features", "transpile"]
    for e in EXAMPLES:
        cmd += ["--example", e]
    if args.rev:
        src = out / "src" / args.name
        if src.exists():
            shutil.rmtree(src)
        src.mkdir(parents=True)
        tar = subprocess.run(["git", "archive", "--format=tar", args.rev], cwd=REPO, check=True,
                             capture_output=True).stdout
        with tarfile.open(fileobj=io.BytesIO(tar)) as t:
            t.extractall(src, filter="data")
        # A revision from before the 2026-10-16 rename uses the old names: rename its tree as this
        # checkout was renamed (a no-op on a later revision).
        subprocess.run([sys.executable, str(REPO / "tools" / "amoris_rename.py"), "--tree", str(src)],
                       check=True)
        # git archive dates every file at its commit, earlier than the last build into the shared
        # target directory: cargo would take another revision's sources as unchanged and keep the
        # previous build's binaries. Every exported file is dated now instead.
        for f in src.rglob("*"):
            if f.is_file():
                os.utime(f)
        target = out / "target-rev"
        sh(cmd + ["--target-dir", target], cwd=src, env=env)
        rev = subprocess.run(["git", "rev-parse", args.rev], cwd=REPO, check=True,
                             capture_output=True, text=True).stdout.strip()
    else:
        target = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
        sh(cmd, cwd=REPO, env=env)
        rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, check=True,
                             capture_output=True, text=True).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=REPO, check=True,
                               capture_output=True, text=True).stdout.strip()
        if dirty:
            rev += "+dirty"
    dest = out / "bin" / args.name
    dest.mkdir(parents=True, exist_ok=True)
    for e in EXAMPLES:
        shutil.copy2(target / "release" / "examples" / f"{e}{EXE}", dest / f"{e}{EXE}")
    (dest / "build.json").write_text(json.dumps({"name": args.name, "rev": rev, "built": now()}))
    print(f"{args.name}: {rev} -> {dest}")


def locked(args, cmd):
    if args.lock:
        return [sys.executable, args.lock, "run", "--label", "qjs", "--max-hold-min", "20", "--", *cmd]
    return cmd


def run(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    builds = [b for b in args.builds.split(",") if b]
    bin_dir = Path(args.bins).resolve() if args.bins else out.parent / "bin"
    bins = {b: bin_dir / b for b in builds}
    for b, d in bins.items():
        for e in EXAMPLES:
            if not (d / f"{e}{EXE}").is_file():
                sys.exit(f"{d / (e + EXE)} is missing: python tools/qjs_bench.py build {b}")
    finals = {}
    for b, d in bins.items():
        sets = out / f"sets-{b}"
        sh([d / f"rules_bench{EXE}", "--emit", sets, "--warmup", 120, "--ticks", 600])
        v = json.loads((sets / "verify.json").read_text())
        if not v.get("identical"):
            sys.exit(f"{b}: the script world differs from the twin: {v}")
        finals[b] = v["final"]
    if len(set(finals.values())) != 1:
        sys.exit(f"the builds end on different digests: {finals}")
    print(f"verified: every build ends on {next(iter(finals.values()))}", flush=True)
    conditions = [sys.executable, REPO / "tools/machine_conditions.py"]
    sh([*conditions, out / "conditions-start.json", "--label", "qjs-start"])
    for r in range(args.rounds):
        sh([*conditions, out / f"conditions-r{r + 1}.json", "--label", f"qjs-r{r + 1}"])
        k = len(builds)
        for i in range(k):
            b = builds[(i + r) % k]
            d = bins[b]
            for kind, e, extra in (("rules", "rules_bench", ["--no-verify", "--warmup", 120, "--ticks", 600]),
                                   ("micro", "qjs_micro", [])):
                f = out / f"{kind}-{b}-r{r + 1}.json"
                start = now()
                sh(locked(args, [d / f"{e}{EXE}", "--rounds", 1, "--offset", r, *extra, "--json", f]))
                res = json.loads(f.read_text())
                res["wall"] = {"start": start, "end": now()}
                res["build"] = json.loads((d / "build.json").read_text())
                f.write_text(json.dumps(res, indent=1))
    sh([*conditions, out / "conditions-end.json", "--label", "qjs-end"])
    table(argparse.Namespace(out=str(out), builds=args.builds))


def load(out, kind, build):
    """[result] of every round of `kind` (rules, micro) for `build`, by round."""
    rows = []
    for f in Path(out).glob(f"{kind}-{build}-r*.json"):
        d = json.loads(f.read_text())
        n = int(f.stem.rsplit("-r", 1)[1])
        for res in d["results"]:
            rows.append({**res, "round": n, "qjs_cc": d.get("qjs_cc")})
    return sorted(rows, key=lambda r: r["round"])


def fmt(x):
    if x >= 100:
        return f"{x:,.0f}"
    if x >= 10:
        return f"{x:,.1f}"
    return f"{x:,.2f}"


def pct(a, b):
    return f"{(a / b - 1) * 100:+.1f} %"


def builds_in(out):
    names = {f.stem.rsplit("-r", 1)[0].split("-", 1)[1] for f in Path(out).glob("rules-*-r*.json")}
    return sorted(names)


def table(args):
    out = args.out
    builds = [b for b in (args.builds or "").split(",") if b] or builds_in(out)
    ref = builds[0]
    print(f"Builds: {', '.join(builds)} (reference: {ref})\n")
    print("| Build | Variant | Tick median, us (by round) | Median | Min | p99 (median) | System median | Steps |")
    print("|---|---|---|---|---|---|---|---|")
    for b in builds:
        rows = load(out, "rules", b)
        for v in ("rules", "twin", "idle", "scan"):
            rs = [r for r in rows if r["variant"] == v]
            if not rs:
                continue
            meds = [r["tick_us"]["median"] for r in rs]
            print(f"| {b} | {v} | {' / '.join(fmt(m) for m in meds)} | {fmt(statistics.median(meds))} | "
                  f"{fmt(min(r['tick_us']['min'] for r in rs))} | "
                  f"{fmt(statistics.median(r['tick_us']['p99'] for r in rs))} | "
                  f"{fmt(statistics.median(r['system_us']['median'] for r in rs))} | "
                  f"{statistics.median(r['steps']['median'] for r in rs):,.0f} |")
    print("\nPhases of `damage` (median of the round medians, us):\n")
    print("| Build | prepare | run | diff | apply |")
    print("|---|---|---|---|---|")
    for b in builds:
        rs = [r for r in load(out, "rules", b) if r["variant"] == "rules"]
        if rs:
            ph = [fmt(statistics.median(r["phases_us"][k]["median"] for r in rs))
                  for k in ("prepare", "run", "diff", "apply")]
            print(f"| {b} | {' | '.join(ph)} |")
    print(f"\nAgainst {ref}, paired by round (tick median of `rules`, `run` median, twin tick median):\n")
    print("| Build | rules tick, by round | median | `run`, median of pairs | twin tick, median of pairs |")
    print("|---|---|---|---|---|")
    base = {r["round"]: r for r in load(out, "rules", ref) if r["variant"] == "rules"}
    tbase = {r["round"]: r for r in load(out, "rules", ref) if r["variant"] == "twin"}
    for b in builds[1:]:
        rows = load(out, "rules", b)
        rs = [r for r in rows if r["variant"] == "rules" and r["round"] in base]
        ts = [r for r in rows if r["variant"] == "twin" and r["round"] in tbase]
        if not rs:
            continue
        ratios = [r["tick_us"]["median"] / base[r["round"]]["tick_us"]["median"] for r in rs]
        runs = [r["phases_us"]["run"]["median"] / base[r["round"]]["phases_us"]["run"]["median"] for r in rs]
        twins = [r["tick_us"]["median"] / tbase[r["round"]]["tick_us"]["median"] for r in ts] or [float("nan")]
        print(f"| {b} | {' / '.join(f'{(x - 1) * 100:+.1f}' for x in ratios)} % | "
              f"{(statistics.median(ratios) - 1) * 100:+.1f} % | {(statistics.median(runs) - 1) * 100:+.1f} % | "
              f"{(statistics.median(twins) - 1) * 100:+.1f} % |")
    print("\nMicroloops, ns per iteration (median of the round medians; steps per tick):\n")
    print("| Probe | " + " | ".join(builds) + " | steps |")
    print("|---|" + "---|" * (len(builds) + 1))
    micro = {b: load(out, "micro", b) for b in builds}
    for p in PROBES:
        cells, steps = [], set()
        for b in builds:
            rs = [r for r in micro[b] if r["probe"] == p]
            steps |= {r["steps"] for r in rs}
            cells.append(fmt(statistics.median(r["ns_per_iter"] for r in rs)) if rs else "")
        st = ", ".join(f"{s:,}" for s in sorted(steps))
        print(f"| {p} | {' | '.join(cells)} | {st} |")
    print(f"\nMicroloops against {ref}, paired by round (the median of the rounds' ratios):\n")
    print("| Probe | " + " | ".join(builds[1:]) + " |")
    print("|---|" + "---|" * len(builds[1:]))
    refs = {(r["probe"], r["round"]): r["ns_per_iter"] for r in micro[ref]}
    for p in PROBES:
        cells = []
        for b in builds[1:]:
            rs = [r["ns_per_iter"] / refs[(p, r["round"])] for r in micro[b]
                  if r["probe"] == p and (p, r["round"]) in refs]
            cells.append(f"{(statistics.median(rs) - 1) * 100:+.1f} %" if rs else "")
        print(f"| {p} | {' | '.join(cells)} |")
    print("\nA `Math` call, ns (the probe less `loop`, per round, then the median):\n")
    print("| Call | " + " | ".join(builds) + " |")
    print("|---|" + "---|" * len(builds))
    for p in CALLS:
        cells = []
        for b in builds:
            loops = {r["round"]: r["ns_per_iter"] for r in micro[b] if r["probe"] == "loop"}
            ds = [r["ns_per_iter"] - loops[r["round"]] for r in micro[b] if r["probe"] == p and r["round"] in loops]
            cells.append(fmt(statistics.median(ds)) if ds else "")
        print(f"| `Math.{p}` | {' | '.join(cells)} |")
    ccs = {b: {r["qjs_cc"] for r in load(out, "rules", b)} for b in builds}
    print("\nCompilers: " + "; ".join(f"{b}: {', '.join(sorted(c for c in v if c))}" for b, v in ccs.items()))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("name")
    b.add_argument("--rev")
    b.add_argument("--out", default=str(REPO / "out" / "qjs"))
    r = sub.add_parser("run")
    r.add_argument("out")
    r.add_argument("--builds", required=True)
    r.add_argument("--rounds", type=int, default=5)
    r.add_argument("--bins")
    r.add_argument("--lock")
    t = sub.add_parser("table")
    t.add_argument("out")
    t.add_argument("--builds")
    a = ap.parse_args()
    {"build": build, "run": run, "table": table}[a.cmd](a)


if __name__ == "__main__":
    main()
