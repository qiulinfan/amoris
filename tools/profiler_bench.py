"""The live profiler's measurements (docs/bench/profiler.md), one re-runnable protocol.

1. The machine's conditions (tools/machine_conditions.py) before and after.
2. The profiler's overhead and per-system timings: pocket-runtime's `profile_bench` example on
   each project, `--runs` times, back to back (3,000 ticks with and 3,000 without the profiler,
   alternating blocks of 250) and paced like the real-time game thread (`--paced-ticks` each, a
   tick every `--pace-ms`, blocks of 100), the runs of each build taking turns.
3. With `--capture`: `pocket serve` on a copy of samples/sailing, headless Chrome on the editor it
   serves (editor/tools/profiler-capture.ts: Play, the Profiler panel, screenshots, the panel's
   text, the viewport's stats, and while the editor is still connected in the same Play,
   `profile.frame` and `pocket profile` over the panel's window and over 600 ticks); then a second
   Play with no editor connected, read the same way (`pocket-profile-no-editor.txt`).

Build first (release; the editor's dist for --capture):

    cargo build --release -p pocket-runtime --features transpile --example profile_bench
    cargo build --release -p pocket-app
    cd editor && bun run build

Run (on the shared laptop inside the bench lock; alone on a quiet machine):

    python tools/profiler_bench.py --out out/liveprof/quiet --runs 3 --capture

Raw JSON, screenshots and text go to --out (ignored); summary.md holds the tables.
"""

import argparse
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WINDOWS = os.name == "nt"
EXE = ".exe" if WINDOWS else ""


def conditions(out: Path, label: str) -> None:
    script = REPO / "tools/machine_conditions.py"
    if WINDOWS and script.exists():
        subprocess.run([sys.executable, str(script), str(out / f"conditions-{label}.json"),
                        "--label", f"profiler-{label}"], check=False)


def bench(out: Path, a) -> dict:
    """`runs` runs of each build and pacing, the builds taking turns: {(label, paced): [report]}."""
    exe = REPO / f"target/release/examples/profile_bench{EXE}"
    if not exe.exists():
        sys.exit(f"{exe} is not built (see the docstring)")
    builds = [("this", exe)] + [(x.split("=", 1)[0], Path(x.split("=", 1)[1])) for x in a.also]
    modes = [("back to back", ["--ticks", str(a.ticks)])]
    if a.pace_ms > 0:
        modes.append((f"paced {a.pace_ms:g} ms",
                      ["--ticks", str(a.paced_ticks), "--block", "100", "--pace-ms", str(a.pace_ms)]))
    reports = {}
    for i in range(a.runs):
        for label, path in builds:
            for mode, args in modes:
                tag = "paced-" if mode.startswith("paced") else ""
                report = out / f"profile-bench-{label}-{tag}{i + 1}.json"
                subprocess.run([str(path), *args, "--out", str(report),
                                *[str(REPO / p) for p in a.projects]],
                               check=True, stdout=subprocess.DEVNULL)
                reports.setdefault((label, mode), []).append(json.loads(report.read_text()))
    return reports


def by_project(reports: list) -> dict:
    rows = {}
    for r in reports:
        for run in r["runs"]:
            rows.setdefault(run["project"], []).append(run)
    return rows


def overhead_table(reports: list) -> list:
    """One row per project. Overheads are per run (mean of all blocks, and median block) and their
    median over the runs; the us/tick columns are the run whose mean overhead is that median, so
    they give its overhead."""
    lines = ["| Project | Runs | Run with the median overhead: without / with (us/tick) | "
             "Overhead (mean of blocks): median of runs | per run | Overhead (median block): "
             "median of runs | per run | With - without (us/tick), median of runs | "
             "Clock reading (ns) | Readings per tick |",
             "|---|---|---|---|---|---|---|---|---|---|"]
    for name, runs in by_project(reports).items():
        ordered = sorted(runs, key=lambda x: x["overhead_pct"])
        mid = ordered[(len(ordered) - 1) // 2]
        over = statistics.median(x["overhead_pct"] for x in runs)
        block = statistics.median(x["median_overhead_pct"] for x in runs)
        diff = statistics.median(x["hooked_us_per_tick"] - x["plain_us_per_tick"] for x in runs)
        per = ", ".join(f"{x['overhead_pct']:+.2f}" for x in runs)
        per_block = ", ".join(f"{x['median_overhead_pct']:+.2f}" for x in runs)
        clock = statistics.median(x.get("clock_ns", 0) for x in runs)
        reads = runs[-1].get("readings_per_tick", 0)
        lines.append(f"| {name} | {len(runs)} | {mid['plain_us_per_tick']:.1f} / "
                     f"{mid['hooked_us_per_tick']:.1f} ({mid['overhead_pct']:+.2f} %) | "
                     f"{over:+.2f} % | {per} | {block:+.2f} % | {per_block} | {diff:+.2f} | "
                     f"{clock:.1f} | {reads:.1f} |")
    return lines


def profile_table(reports: list, title: str) -> list:
    lines = []
    for name, runs in by_project(reports).items():
        p = runs[-1]["profile"]
        lines += ["", f"{name}, {title}, last run ({p['ticks']} profiled ticks): ms per tick",
                  "", "| Row | ms | max ms |", "|---|---|---|"]
        for s in p["systems"]:
            lines.append(f"| {s['name']} | {s['ms']:.4f} | {s['max_ms']:.4f} |")
        lines.append(f"| tick | {p['tick_ms']:.4f} | {p['tick_max_ms']:.4f} |")
        phases = ", ".join(f"{s['name']} {s['ms']:.4f}" for s in p["phases"])
        lines += ["", f"Phases (ms per tick): {phases}"]
    return lines


def pacing_table(fast: list, paced: list) -> list:
    """What pacing alone does: the plain tick and each system, paced against back to back (the
    medians over runs of the plain tick; systems from the last runs' profiles)."""
    lines = ["| Project | Back to back: plain tick (us) | Paced: plain tick (us) | Ratio |",
             "|---|---|---|---|"]
    a, b = by_project(fast), by_project(paced)
    for name in a:
        if name not in b:
            continue
        x = statistics.median(r["plain_us_per_tick"] for r in a[name])
        y = statistics.median(r["plain_us_per_tick"] for r in b[name])
        lines.append(f"| {name} | {x:.1f} | {y:.1f} | {y / x:.2f}x |")
    for name in a:
        if name not in b:
            continue
        pa = {s["name"]: s["ms"] for s in a[name][-1]["profile"]["systems"]}
        pb = {s["name"]: s["ms"] for s in b[name][-1]["profile"]["systems"]}
        lines += ["", f"{name}: paced / back to back, per row (profiled ticks)", "",
                  "| Row | Back to back (ms) | Paced (ms) | Ratio |", "|---|---|---|---|"]
        for k, v in pa.items():
            if k in pb and v > 0:
                lines.append(f"| {k} | {v:.4f} | {pb[k]:.4f} | {pb[k] / v:.2f}x |")
    return lines


def summarize(reports: dict) -> str:
    out = []
    for (label, mode), runs in reports.items():
        out += [f"## {label}, {mode}", "", *overhead_table(runs), *profile_table(runs, mode), ""]
    for label in {k[0] for k in reports}:
        fast = reports.get((label, "back to back"))
        paced = next((v for (lab, m), v in reports.items() if lab == label and m != "back to back"),
                     None)
        if fast and paced:
            out += [f"## {label}: what pacing alone does", "", *pacing_table(fast, paced), ""]
    return "\n".join(out)


def post(base: str, method: str, params=None) -> dict:
    """`method` through the host's `/api/call`: its result."""
    req = urllib.request.Request(base + "api/call", method="POST",
                                 data=json.dumps({"method": method,
                                                  "params": params or {}}).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as r:
        reply = json.loads(r.read())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def capture(out: Path, seconds: int) -> None:
    pocket = REPO / f"target/release/pocket{EXE}"
    dist = REPO / "editor/dist"
    if not pocket.exists() or not (dist / "index.html").exists():
        sys.exit("--capture needs target/release/pocket and editor/dist (see the docstring)")
    bun = shutil.which("bun")
    if not bun:
        sys.exit("--capture needs bun on PATH")
    tmp = Path(tempfile.mkdtemp(prefix="pocket-profiler-"))
    proj = tmp / "sailing"
    shutil.copytree(REPO / "samples/sailing", proj)
    env = dict(os.environ, POCKET_EDITOR_DIST=str(dist),
               POCKET_WEB_VIEWPORT=str(REPO / "web/viewport"))
    group = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if WINDOWS else {}
    errlog = open(out / "serve.err", "w")
    server = subprocess.Popen([str(pocket), "serve", str(proj), "--port", "0"], env=env,
                              stdout=subprocess.PIPE, stderr=errlog, text=True, **group)
    try:
        info = json.loads(server.stdout.readline())
        base = info["url"].rstrip("/") + "/"
        print(f"host at {base}")
        shot = out / "capture"
        subprocess.run([bun, "tools/profiler-capture.ts", "--url", base, "--out", str(shot),
                        "--seconds", str(seconds), "--pocket", str(pocket), "--project",
                        str(proj)], cwd=REPO / "editor", check=True, env=env)
        # The editor is gone (its Chrome closed): a second Play with nothing connected but the
        # CLI, read the same way.
        post(base, "play.start")
        time.sleep(3)
        early = post(base, "profile.frame", {"ticks": 600})
        (out / "play-early-no-editor.json").write_text(json.dumps(early, indent=2) + "\n")
        time.sleep(max(0, seconds - 3))
        r = subprocess.run([str(pocket), "profile", "--ticks", "600"], cwd=proj,
                           capture_output=True, text=True, timeout=60)
        (out / "pocket-profile-no-editor.txt").write_text(r.stdout + r.stderr)
        print(r.stdout)
        post(base, "play.stop")
    finally:
        server.send_signal(signal.CTRL_BREAK_EVENT if WINDOWS else signal.SIGTERM)
        try:
            server.wait(10)
        except subprocess.TimeoutExpired:
            server.kill()
        errlog.close()
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", default="out/liveprof/run")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--ticks", type=int, default=3000)
    ap.add_argument("--pace-ms", type=float, default=1000 / 60,
                    help="the paced runs' tick interval (0: no paced runs)")
    ap.add_argument("--paced-ticks", type=int, default=1800)
    ap.add_argument("--projects", nargs="+", default=["samples/sailing", "samples/sailing-course"])
    ap.add_argument("--no-bench", action="store_true", help="only the capture")
    ap.add_argument("--capture", action="store_true")
    ap.add_argument("--seconds", type=int, default=15, help="Play time before a capture")
    ap.add_argument("--also", nargs="*", default=[], metavar="LABEL=EXE",
                    help="other builds of profile_bench, run in turn with this one")
    a = ap.parse_args()
    out = (REPO / a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    conditions(out, "start")
    if not a.no_bench:
        reports = bench(out, a)
        table = summarize(reports)
        (out / "summary.md").write_text(table + "\n")
        print(table)
    if a.capture:
        capture(out, a.seconds)
    conditions(out, "end")


if __name__ == "__main__":
    main()
