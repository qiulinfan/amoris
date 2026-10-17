#!/usr/bin/env python3
"""The script cost baseline (docs/bench/script.md): the rule workload of bench/rules natively, under
Node and in headless Chrome's Web Worker, beside its Rust twin and two probes, in interleaved
rounds.

    python tools/rules_bench.py build
        builds rules_bench (release), rules_web (wasm32, web profile) and, when wasm-opt is on PATH,
        its -O3 copy (the flags of tools/build_web.sh)
    python tools/rules_bench.py run OUT [--rounds 3] [--targets native,node,chrome]
                                        [--warmup 120] [--ticks 600] [--lock LOCK.py]
        verifies the twin and the wasm digests, then runs the rounds: in each, every target in an
        order that rotates by round, each target running every variant once (rotating too) and,
        on the web, both modules in an order that alternates by round. `--lock` wraps every timed
        command in `python LOCK.py run --label rules --` (the shared machine's bench lock); machine
        conditions are recorded before and after the run and before each round, and every result
        file records when its command started and ended (`wall`), lock waits included
    python tools/rules_bench.py table OUT
        prints the result tables in the form docs/bench/script.md keeps them

OUT receives one JSON per target, build and round, the compiled sets and the conditions. Needs
Node and Chrome (webcheck.py's search) for the web targets.
"""
import argparse
import json
import os
import shutil
import socket
import statistics
import subprocess
import sys
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
NATIVE = TARGET / "release" / "examples" / ("rules_bench.exe" if os.name == "nt" else "rules_bench")
WASM = TARGET / "wasm32-unknown-unknown" / "web" / "examples" / "rules_web.wasm"
WASM_OPT = TARGET / "wasm32-unknown-unknown" / "web" / "examples" / "rules_web.opt.wasm"
VARIANTS = ["rules", "twin", "idle", "scan"]


def sh(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run([str(c) for c in cmd], check=True, **kw)


def build(_args):
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "6"))
    sh(["cargo", "build", "--release", "-p", "pocket-script", "--features", "transpile", "--example", "rules_bench"], cwd=REPO, env=env)
    sh(["cargo", "build", "-p", "pocket-script", "--example", "rules_web", "--target", "wasm32-unknown-unknown", "--profile", "web"], cwd=REPO, env=env)
    if shutil.which("wasm-opt"):
        sh(["wasm-opt", "-O3", "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "--enable-sign-ext",
            "--enable-mutable-globals", WASM, "-o", WASM_OPT])
    else:
        print("wasm-opt is not on PATH: only the web-profile module is measured")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def locked(args, cmd):
    if args.lock:
        return [sys.executable, args.lock, "run", "--label", "rules", "--max-hold-min", "20", "--", *cmd]
    return cmd


def wasm_builds(r=0):
    """The web modules, in an order that alternates by round `r`."""
    builds = [("web", WASM)]
    if WASM_OPT.is_file():
        builds.append(("web-opt", WASM_OPT))
    return builds[::-1] if r % 2 else builds


def now():
    return datetime.now().isoformat(timespec="seconds")


def stamp(path, start):
    """Records in a result file when its command started and ended (lock waits included)."""
    d = json.loads(path.read_text())
    d["wall"] = {"start": start, "end": now()}
    path.write_text(json.dumps(d, indent=1))


def run(args):
    out = Path(args.out).resolve()
    sets = out / "sets"
    web = out / "web"
    sets.mkdir(parents=True, exist_ok=True)
    web.mkdir(parents=True, exist_ok=True)
    for f in (NATIVE, WASM):
        if not f.is_file():
            sys.exit(f"{f} is missing: python tools/rules_bench.py build")
    targets = [t for t in args.targets.split(",") if t]
    # The compiled sets and the native verification (the script world against the twin; it fails
    # when they differ, and the native timings below then skip it with `--no-verify`).
    sh([NATIVE, "--emit", sets, "--warmup", args.warmup, "--ticks", args.ticks])
    for f in ("index.html", "worker.js", "bench.mjs"):
        shutil.copy(REPO / "bench/rules/web" / f, web / f)
    for f in sets.glob("*.json"):
        shutil.copy(f, web / f.name)
    for name, path in wasm_builds():
        shutil.copy(path, web / f"{name}.wasm")
    conditions = [sys.executable, REPO / "tools/machine_conditions.py"]
    sh([*conditions, out / "conditions-start.json", "--label", "rules-start"])
    common = ["--warmup", args.warmup, "--ticks", args.ticks, "--rounds", "1"]
    for r in range(args.rounds):
        sh([*conditions, out / f"conditions-r{r + 1}.json", "--label", f"rules-r{r + 1}", "--cpu-seconds", "3"])
        order = targets[r % len(targets):] + targets[: r % len(targets)]
        for t in order:
            if t == "native":
                start, f = now(), out / f"native-r{r + 1}.json"
                sh(locked(args, [NATIVE, *common, "--offset", r, "--no-verify", "--json", f]))
                stamp(f, start)
            elif t == "node":
                for name, path in wasm_builds(r):
                    start, f = now(), out / f"node-{name}-r{r + 1}.json"
                    sh(locked(args, ["node", REPO / "bench/rules/web/node.mjs", path, sets, *common, "--offset", r,
                                     "--json", f, "--raw"]))
                    stamp(f, start)
            elif t == "chrome":
                for name, _ in wasm_builds(r):
                    start = now()
                    port = free_port()
                    url = (f"http://127.0.0.1:{port}/index.html?wasm={name}.wasm&rounds=1&offset={r}"
                           f"&warmup={args.warmup}&ticks={args.ticks}")
                    p = subprocess.run([str(c) for c in locked(args, [sys.executable, REPO / "tools/webcheck.py", url, "--serve", web,
                                                                      "--port", port, "--timeout", "600"])],
                                       capture_output=True, text=True)
                    line = next((l for l in p.stdout.splitlines() if l.startswith(("DONE ", "FAIL "))), None)
                    if line is None or not line.startswith("DONE "):
                        print(p.stdout[-4000:], p.stderr[-2000:])
                        sys.exit(f"chrome round {r + 1} ({name}) failed")
                    f = out / f"chrome-{name}-r{r + 1}.json"
                    f.write_text(line[5:])
                    stamp(f, start)
                    print(f"chrome {name} round {r + 1}: done", flush=True)
            else:
                sys.exit(f"unknown target {t}")
    sh([*conditions, out / "conditions-end.json", "--label", "rules-end"])
    table(argparse.Namespace(out=str(out)))


def load(out):
    """{(target, variant): [result per round]} and the verification records."""
    rows, verify = {}, {}
    files = [f for f in Path(out).glob("*-r*.json") if not f.name.startswith("conditions-")]
    # By target, then by round as a number (r10 after r9). Each file holds one round of `run`,
    # whose number is in the file's name.
    for f in sorted(files, key=lambda f: (f.stem.rsplit("-r", 1)[0], int(f.stem.rsplit("-r", 1)[1]))):
        d = json.loads(f.read_text())
        target, n = f.stem.rsplit("-r", 1)
        if d.get("verify"):
            verify[target] = d["verify"]
        for res in d["results"]:
            rows.setdefault((target, res["variant"]), []).append({**res, "round": int(n)})
    native = Path(out) / "sets" / "verify.json"
    if native.is_file():
        verify["native"] = json.loads(native.read_text())
    return rows, verify


def fmt(x):
    if x >= 100:
        return f"{x:,.0f}"
    return f"{x:,.1f}"


def table(args):
    rows, verify = load(args.out)
    targets = sorted({t for t, _ in rows}, key=lambda t: (t != "native", t))
    print("Verification (script world against the twin at every tick; wasm digests against native):\n")
    for t, v in sorted(verify.items()):
        same = v.get("same_as_native", v.get("identical"))
        print(f"- {t}: identical {v.get('identical')}, final {v.get('final')}, as native {same}")
    print("\n| Target | Variant | Tick median, us (by round) | Tick min | Tick p99 (median) | System median | Steps |")
    print("|---|---|---|---|---|---|---|")
    for t in targets:
        for v in VARIANTS:
            rs = rows.get((t, v))
            if not rs:
                continue
            med = " / ".join(fmt(r["tick_us"]["median"]) for r in rs)
            mn = min(r["tick_us"]["min"] for r in rs)
            p99 = statistics.median(r["tick_us"]["p99"] for r in rs)
            sysm = statistics.median(r["system_us"]["median"] for r in rs)
            steps = statistics.median(r["steps"]["median"] for r in rs)
            print(f"| {t} | {v} | {med} | {fmt(mn)} | {fmt(p99)} | {fmt(sysm)} | {steps:,.0f} |")
    print("\n| Target | rules / twin, tick (median of medians) | by min | `damage` / `twin.damage`, system median |")
    print("|---|---|---|---|")
    for t in targets:
        a, b = rows.get((t, "rules")), rows.get((t, "twin"))
        if not a or not b:
            continue
        med = statistics.median(r["tick_us"]["median"] for r in a) / statistics.median(r["tick_us"]["median"] for r in b)
        mn = min(r["tick_us"]["min"] for r in a) / min(r["tick_us"]["min"] for r in b)
        sy = statistics.median(r["system_us"]["median"] for r in a) / statistics.median(r["system_us"]["median"] for r in b)
        print(f"| {t} | {med:.1f} | {mn:.1f} | {sy:.1f} |")
    print("\n| Host | `web-opt` against `web`, rules tick median, by round (paired) |")
    print("|---|---|")
    for host in ("node", "chrome"):
        a, b = rows.get((f"{host}-web", "rules")), rows.get((f"{host}-web-opt", "rules"))
        if not a or not b:
            continue
        opt = {r["round"]: r["tick_us"]["median"] for r in b}
        pairs = [f"{(opt[r['round']] / r['tick_us']['median'] - 1) * 100:+.1f} %" for r in a if r["round"] in opt]
        print(f"| {host} | {' / '.join(pairs)} |")
    print("\n| Target | Variant | prepare | run | diff | apply (medians, us) |")
    print("|---|---|---|---|---|---|")
    for t in targets:
        for v in ("rules", "scan", "idle"):
            rs = rows.get((t, v))
            if not rs:
                continue
            ph = [fmt(statistics.median(r["phases_us"][k]["median"] for r in rs)) for k in ("prepare", "run", "diff", "apply")]
            print(f"| {t} | {v} | {' | '.join(ph)} |")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build")
    r = sub.add_parser("run")
    r.add_argument("out")
    r.add_argument("--rounds", type=int, default=3)
    r.add_argument("--targets", default="native,node,chrome")
    r.add_argument("--warmup", type=int, default=120)
    r.add_argument("--ticks", type=int, default=600)
    r.add_argument("--lock")
    t = sub.add_parser("table")
    t.add_argument("out")
    a = ap.parse_args()
    {"build": build, "run": run, "table": table}[a.cmd](a)


if __name__ == "__main__":
    main()
