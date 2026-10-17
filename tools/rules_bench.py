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
    python tools/rules_bench.py split OUT [--rounds 5] [--warmup 120] [--ticks 600]
                                          [--profile-interval 100] [--lock LOCK.py]
        natively only, the split of `run` (docs/bench/script.md, section 8): per round `rules`,
        `natives` (the same rule with the host's natives timed on the time-stamp counter) and
        `profiled` (under the step profiler) in an order that rotates by round, the machine
        conditions before each round; then `split-table`
    python tools/rules_bench.py split-table OUT
        prints the split: `run` inside the natives and the rest, interpretation, against the
        untimed `rules` of the same round, as bounds: the natives raw (their windows hold part of
        a clock reading) and less one back-to-back reading a call (which over-subtracts when a
        reading costs less in place)

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
    sh(["cargo", "build", "--release", "-p", "amoris-script", "--features", "transpile", "--example", "rules_bench"], cwd=REPO, env=env)
    sh(["cargo", "build", "-p", "amoris-script", "--example", "rules_web", "--target", "wasm32-unknown-unknown", "--profile", "web"], cwd=REPO, env=env)
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


def split(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if not NATIVE.is_file():
        sys.exit(f"{NATIVE} is missing: python tools/rules_bench.py build")
    conditions = [sys.executable, REPO / "tools/machine_conditions.py"]
    sh([*conditions, out / "conditions-start.json", "--label", "split-start"])
    for r in range(args.rounds):
        sh([*conditions, out / f"conditions-r{r + 1}.json", "--label", f"split-r{r + 1}", "--cpu-seconds", "3"])
        start, f = now(), out / f"split-r{r + 1}.json"
        # The first round also verifies the script world against the twin (the timings skip it).
        verify = [] if r == 0 else ["--no-verify"]
        sh(locked(args, [NATIVE, "--variants", "rules,natives,profiled", "--profile-interval", args.profile_interval,
                         "--rounds", 1, "--offset", r, "--warmup", args.warmup, "--ticks", args.ticks, *verify,
                         "--json", f]))
        stamp(f, start)
    sh([*conditions, out / "conditions-end.json", "--label", "split-end"])
    split_table(argparse.Namespace(out=str(out)))


def split_table(args):
    out = Path(args.out)
    files = sorted(out.glob("split-r*.json"), key=lambda f: int(f.stem.rsplit("-r", 1)[1]))
    rows = []
    for f in files:
        d = json.loads(f.read_text())
        res = {r["variant"]: r for r in d["results"]}
        a, b = res.get("rules"), res.get("natives")
        if not a or not b:
            continue
        if a["digest"] != b["digest"]:
            sys.exit(f"{f.name}: the timed natives changed the world ({a['digest']} against {b['digest']})")
        n = b["natives"]
        run_plain = a["phases_us"]["run"]["median"]
        run_timed = b["phases_us"]["run"]["median"]
        raw = n["native_us"]["median"]
        calls = n["calls"]["median"]
        reading = n["clock_reading_ns"]["median"]
        native = raw - calls * reading / 1000
        rows.append({
            "round": int(f.stem.rsplit("-r", 1)[1]),
            "first": d["results"][0]["variant"],
            "tick": a["tick_us"]["median"],
            "tick_timed": b["tick_us"]["median"],
            "run": run_plain,
            "run_timed": run_timed,
            "raw": raw,
            "calls": calls,
            "reading": reading,
            "native": native,
            "interp": run_plain - native,
            "interp_raw": run_plain - raw,
            "steps": a["steps"]["median"],
        })
    if not rows:
        sys.exit(f"no split rounds in {out}")
    print("Per round, medians of the 600 ticks, us (`rules` untimed; `natives` its timed twin). "
          "Interpretation is `run` less the natives, between the natives raw and less one reading a call:\n")
    print("| Round | First | rules tick | rules `run` | natives `run` | native, raw | calls | reading, ns "
          "| native, less readings | interpretation | of `run` | of the tick |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        print(f"| {r['round']} | {r['first']} | {fmt(r['tick'])} | {fmt(r['run'])} | {fmt(r['run_timed'])} | "
              f"{fmt(r['raw'])} | {r['calls']:,.0f} | {r['reading']:.1f} | {fmt(r['native'])} | "
              f"{fmt(r['interp_raw'])} to {fmt(r['interp'])} | "
              f"{r['interp_raw'] / r['run'] * 100:.1f} to {r['interp'] / r['run'] * 100:.1f} % | "
              f"{r['interp_raw'] / r['tick'] * 100:.1f} to {r['interp'] / r['tick'] * 100:.1f} % |")
    med = lambda k: statistics.median(r[k] for r in rows)
    # Shares are the medians of the rounds' own shares; absolute parts are of the rounds' medians, the
    # interpretation the median `run` less the median natives, so the parts add up to `run`.
    share = lambda k, of: statistics.median(r[k] / r[of] * 100 for r in rows)
    lo, hi = med("run") - med("raw"), med("run") - med("native")
    print(f"\nMedians of the rounds: rules `run` {fmt(med('run'))} us of a {fmt(med('tick'))} us tick; inside the "
          f"natives {fmt(med('native'))} to {fmt(med('raw'))} us (less one {med('reading'):.1f} ns reading a call, "
          f"and raw; {med('calls'):,.0f} calls); interpretation the rest of `run`, {fmt(lo)} to {fmt(hi)} us. "
          f"Medians of the rounds' shares: interpretation {share('interp_raw', 'run'):.1f} to "
          f"{share('interp', 'run'):.1f} % of `run`, {share('interp_raw', 'tick'):.1f} to "
          f"{share('interp', 'tick'):.1f} % of the tick. Removing it would leave {fmt(med('tick') - hi)} to "
          f"{fmt(med('tick') - lo)} us of the tick ({med('tick') / (med('tick') - lo):.2f} to "
          f"{med('tick') / (med('tick') - hi):.2f} times faster). "
          f"Timing the natives made `run` {(med('run_timed') - med('run')):+,.0f} us "
          f"({(med('run_timed') / med('run') - 1) * 100:+.1f} %; per round "
          f"{', '.join(f'{r['run_timed'] - r['run']:+,.0f}' for r in rows)}), which is "
          f"{(med('run_timed') - med('run')) * 1000 / (2 * med('calls')):.1f} ns per reading in place, two a call. "
          f"Steps {med('steps'):,.0f} a tick.")
    kinds = ["math", "events", "rng", "world", "other"]
    print("\nInside the natives by kind, per round: us a tick less one reading a call, and raw (calls a tick):\n")
    print("| Round | " + " | ".join(kinds) + " |")
    print("|---|" + "---|" * len(kinds))
    for f in files:
        d = json.loads(f.read_text())
        b = {r["variant"]: r for r in d["results"]}.get("natives")
        if not b or "by_kind" not in b["natives"]:
            continue
        reading = b["natives"]["clock_reading_ns"]["median"]
        cells = []
        for k in kinds:
            v = b["natives"]["by_kind"][k]
            calls = v["calls"]["median"]
            us = v["us"]["median"]
            lo, hi = fmt(us - calls * reading / 1000), fmt(us)
            cells.append(f"{lo if lo == hi else f'{lo} to {hi}'} ({calls:,.0f})")
        print(f"| {f.stem.rsplit('-r', 1)[1]} | " + " | ".join(cells) + " |")
    profiled = []
    for f in files:
        d = json.loads(f.read_text())
        res = {r["variant"]: r for r in d["results"]}
        a, c = res.get("rules"), res.get("profiled")
        if not a or not c or not c.get("profile"):
            continue
        if a["digest"] != c["digest"]:
            sys.exit(f"{f.name}: the profiler changed the world ({a['digest']} against {c['digest']})")
        p = c["profile"]
        ticks = c["ticks"] + d["warmup"]
        profiled.append({"round": int(f.stem.rsplit("-r", 1)[1]), "tick": a["tick_us"]["median"],
                         "tick_profiled": c["tick_us"]["median"], "samples": p["samples"] / ticks,
                         "interval": p["interval"], "profile": p})
    if not profiled:
        return
    print("\nThe step profiler on the same rule (`profiled`; tick medians, us):\n")
    print("| Round | rules tick | profiled tick | samples a tick | cost a sample, us |")
    print("|---|---|---|---|---|")
    for r in profiled:
        cost = (r["tick_profiled"] - r["tick"]) / r["samples"]
        print(f"| {r['round']} | {fmt(r['tick'])} | {fmt(r['tick_profiled'])} | {r['samples']:,.0f} | {cost:.2f} |")
    p = profiled[0]["profile"]
    same = all(json.dumps(r["profile"], sort_keys=True) == json.dumps(p, sort_keys=True) for r in profiled)
    print(f"\nThe profile (sampled every {p['interval']} steps on average, so its shares are estimates; the same "
          f"in every round: {same}), `damage`'s top lines by self samples of {p['damage']['samples']:,}:\n")
    print("| Line | Function | Self samples | Share |")
    print("|---|---|---|---|")
    for at, fn, n in p["damage"]["lines"]:
        print(f"| {at} | {fn} | {n:,} | {n / p['damage']['samples'] * 100:.1f} % |")
    print("\n| Function | File | Self | Total |")
    print("|---|---|---|---|")
    for fn, file, own, total in p["damage"]["functions"]:
        print(f"| {fn} | {file} | {own / p['damage']['samples'] * 100:.1f} % | {total / p['damage']['samples'] * 100:.1f} % |")


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
    sp = sub.add_parser("split")
    sp.add_argument("out")
    sp.add_argument("--rounds", type=int, default=5)
    sp.add_argument("--warmup", type=int, default=120)
    sp.add_argument("--ticks", type=int, default=600)
    sp.add_argument("--profile-interval", type=int, default=100)
    sp.add_argument("--lock")
    st = sub.add_parser("split-table")
    st.add_argument("out")
    a = ap.parse_args()
    {"build": build, "run": run, "table": table, "split": split, "split-table": split_table}[a.cmd](a)


if __name__ == "__main__":
    main()
