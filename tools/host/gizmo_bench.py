#!/usr/bin/env python3
"""Live gizmo drags on the real host, timed in the browser (docs/bench/gizmo.md).

Each run starts a fresh `amoris serve samples/sailing` (the edit world, paused) and runs
`editor/tools/gizmo-drag.ts` in headless Chrome on `built`, the editor's production build served by
the host (as `amoris editor` serves it; built with VITE_AMORIS_EXPOSE=1 so the tool can reach the
editor's stores). `--dev` additionally checks the Vite development server proxying to the host;
the published timing protocol uses the production build. Development viewport remounts and hot
updates have their own check, editor/tools/viewport-loads.ts. Mouse moves go at 60 Hz without
waiting for each (the page coalesces them
per frame, as Chrome does a real mouse's). The tool first times the host's floor without a browser
(edits over /ws and their feed messages). A run's raw output (drag.json and the screenshots) goes
to `<out>/<run>/<mode>/`, the machine's conditions before the first run and after the last (inside
the bench lock when the script runs under it) to `<out>/conditions-{start,end}.json`; `--table`
prints the medians over the valid runs (a page that logged a panic or an exception is left out).

Build first: `cargo build --release -p amoris-app --bin amoris`, then `cd editor && bun install
--frozen-lockfile && VITE_AMORIS_EXPOSE=1 bun run build` (editor/dist is ignored; a normal
`bun run build` afterwards drops the hooks again). With `--dev` the script starts and stops Vite
itself. From the repository root:

    python tools/host/gizmo_bench.py --runs 5 --out out/gizmo/quiet
    python tools/host/gizmo_bench.py --table out/gizmo/quiet
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
MODES = ("built", "dev")


def conditions(out, label):
    """The machine's conditions (tools/machine_conditions.py) into <out>/conditions-<label>.json."""
    r = subprocess.run([sys.executable, str(ROOT / "tools" / "machine_conditions.py"),
                        str(out / f"conditions-{label}.json"), "--label", f"gizmo-{label}"],
                       cwd=ROOT, capture_output=True, text=True)
    print(f"conditions {label}: exit {r.returncode}")


def amoris_version(amoris) -> dict:
    """`amoris version` of the build measured (commit and source hash), kept with the results."""
    r = subprocess.run([str(amoris), "version"], capture_output=True, text=True, timeout=60)
    try:
        return json.loads(r.stdout)
    except ValueError:
        return {"error": (r.stdout + r.stderr).strip()}


def wait_http(url, seconds=30.0, method="GET", body=None):
    end = time.time() + seconds
    while time.time() < end:
        try:
            req = urllib.request.Request(url, data=body, method=method,
                                         headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=2) as r:
                return r.read()
        except OSError:
            time.sleep(0.25)
    raise RuntimeError(f"{url} did not answer in {seconds} s")


def start_host(amoris, port, log, dist):
    env = dict(os.environ, AMORIS_EDITOR_DIST=str(dist))
    p = subprocess.Popen([str(amoris), "serve", "samples/sailing", "--port", str(port)], cwd=ROOT,
                         env=env, stdout=log, stderr=subprocess.STDOUT)
    wait_http(f"http://127.0.0.1:{port}/api/call", method="POST",
              body=json.dumps({"method": "status", "params": {}}).encode())
    return p


def start_vite(host_port, editor_port, log):
    bun = shutil.which("bun")
    if not bun:
        raise RuntimeError("bun is not on PATH")
    env = dict(os.environ, AMORIS_HOST=f"http://127.0.0.1:{host_port}", EDITOR_PORT=str(editor_port))
    p = subprocess.Popen([bun, "run", "dev"], cwd=ROOT / "editor", env=env, stdout=log,
                         stderr=subprocess.STDOUT)
    wait_http(f"http://127.0.0.1:{editor_port}/", seconds=60)
    return p


def stop(p):
    if p and p.poll() is None:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
        else:
            p.terminate()
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()


def run(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    amoris = Path(args.amoris) if args.amoris else ROOT / "target" / "release" / f"amoris{EXE}"
    built = Path(args.dist) if args.dist else ROOT / "editor" / "dist"
    dist = built / "_app"
    if not any("amorisEditorViewport" in f.read_text(errors="ignore") for f in dist.glob("*.js")):
        raise SystemExit("editor/dist lacks the hooks: cd editor && VITE_AMORIS_EXPOSE=1 bun run build")
    bun = shutil.which("bun")
    modes = MODES if args.dev else ("built",)
    conditions(out, "start")
    (out / "amoris-version.json").write_text(json.dumps(
        {"amoris": str(amoris), "dist": str(built), "version": amoris_version(amoris)},
        indent=2) + "\n")
    with open(out / "vite.log", "a") as vlog:
        # Vite proxies to whichever host listens on host-port; each run restarts the host there.
        vite = start_vite(args.host_port, args.editor_port, vlog) if args.dev else None
        try:
            for r in range(args.runs):
                for mode in modes:
                    dest = out / f"run{r + 1}" / mode
                    dest.mkdir(parents=True, exist_ok=True)
                    port = args.host_port if mode == "built" else args.editor_port
                    # A page whose viewport fails to start (WebGPU under another track's load) is
                    # tried once more with a fresh host and browser.
                    for attempt in (1, 2):
                        with open(dest / "serve.log", "w") as hlog:
                            host = start_host(amoris, args.host_port, hlog, built)
                            try:
                                cmd = [bun, "tools/gizmo-drag.ts", "--url", f"http://127.0.0.1:{port}/",
                                       "--out", str(dest.resolve()), "--moves", str(args.moves),
                                       "--hz", str(args.hz), "--host", f"127.0.0.1:{args.host_port}"]
                                p = subprocess.run(cmd, cwd=ROOT / "editor", capture_output=True,
                                                   text=True, timeout=300)
                                (dest / f"tool-{attempt}.log").write_text(p.stdout + p.stderr)
                                print(f"run {r + 1} {mode} attempt {attempt}: exit {p.returncode}")
                                print(p.stdout.strip())
                            finally:
                                stop(host)
                        if p.returncode == 0:
                            break
        finally:
            stop(vite)
    conditions(out, "end")


def table(out):
    rows = {m: [] for m in MODES}
    invalid = []
    for d in sorted(Path(out).glob("run*/*/drag.json")):
        r = json.loads(d.read_text())
        r["_run"] = d.parent.parent.name
        if r.get("page_errors"):
            invalid.append(f"{r['_run']}/{d.parent.name} ({r['page_errors']} page errors)")
            continue
        rows.setdefault(d.parent.name, []).append(r)
    med = lambda xs: round(statistics.median(xs), 1) if xs else None
    print("| Editor | Runs | Input to frame, median (p95) ms | Listener | Wait | Send to feed "
          "| Feed to frame | Edit reply | Edits / feeds / moves | History pushes | Frame interval "
          "| Long tasks (ms) | Undo entries added | Escape |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for mode, runs in rows.items():
        if not runs:
            continue
        lat = [r["streamed"]["latency"] for r in runs]
        get = lambda k, q="median": med([l[k][q] for l in lat if k in l and l[k][q] is not None])
        s = [r["streamed"] for r in runs]
        counts = f"{med([x['sends'] for x in s])} / {med([x['feeds'] for x in s])} / " \
                 f"{med([x.get('moves_seen', 0) for x in s])}"
        pushes = med([x["history_pushes"] for x in s if "history_pushes" in x])
        added = sorted({x["undo_after"] - x["undo_before"] for x in s})
        esc = all(r["escape"]["restored"] and r["escape"]["undo_after"] == r["escape"]["undo_before"]
                  and r["escape"]["redo_after"] == 1 for r in runs)
        frame = med([x["frame_interval_ms"]["median"] for x in s if "frame_interval_ms" in x])
        long = f"{med([x.get('long_tasks', 0) for x in s])} ({med([x.get('long_task_ms', 0) for x in s])})"
        print(f"| {mode} | {len(runs)} | {get('total_ms')} ({get('total_ms', 'p95')}) | "
              f"{get('listener_ms')} | {get('wait_ms')} | {get('host_ms')} | {get('draw_ms')} | "
              f"{get('reply_ms')} | {counts} | {pushes} | {frame} | {long} | {added} | "
              f"{'restores; the drag moves to redo' if esc else 'no'} |")
    for mode, runs in rows.items():
        for r in runs:
            s, rel, o = r["streamed"], r["on_release"], r.get("orbit") or {}
            lat = s["latency"]
            fi, rfi = s.get("frame_interval_ms", {}), rel.get("frame_interval_ms", {})
            print(f"{mode} {r['_run']}: orbit distance {round(o['distance'], 2) if 'distance' in o else '?'}, "
                  f"moved {round(s['position_after'][0] - s['position_before'][0], 2)}; input to frame "
                  f"{lat['total_ms']['median']} (from the listener {lat.get('from_listener_ms', lat['total_ms'])['median']}); "
                  f"frame interval streamed {fi.get('median')} (p95 {fi.get('p95')}, {fi.get('n')} frames), "
                  f"on release {rfi.get('median')} (p95 {rfi.get('p95')}, {rfi.get('n')} frames); "
                  f"adapter {((r.get('browser') or {}).get('adapter') or {}).get('architecture')}")
    for i in invalid:
        print(f"left out: {i}")
    floors = {
        "to the host": [r["floor"]["direct"] for runs in rows.values() for r in runs if "floor" in r],
        "through Vite": [r["floor"]["proxied"] for r in rows.get("dev", []) if "floor" in r],
    }
    for where, fs in floors.items():
        if fs:
            print(f"floor, {where}: reply {med([f['reply_ms']['median'] for f in fs])} ms, feed "
                  f"{med([f['feed_ms']['median'] for f in fs])} ms (p95 {med([f['feed_ms']['p95'] for f in fs])})")
    for mode, runs in rows.items():
        rel = [r["on_release"] for r in runs]
        if rel:
            print(f"{mode}, on release: first move to first feed "
                  f"{med([x['first_move_to_first_feed_ms'] for x in rel])} ms, release to feed "
                  f"{med([x['release_to_first_feed_ms'] for x in rel])} ms, sends "
                  f"{med([x['sends'] for x in rel])}, undo entries added {sorted({x['undo_added'] for x in rel})}, "
                  f"frame interval {med([x['frame_interval_ms']['median'] for x in rel if 'frame_interval_ms' in x])} ms "
                  f"(p95 {med([x['frame_interval_ms']['p95'] for x in rel if 'frame_interval_ms' in x])}), long tasks "
                  f"{med([x.get('long_tasks', 0) for x in rel])}")
        s_ = [r["streamed"] for r in runs]
        if s_:
            print(f"{mode}, streamed: frame interval p95 "
                  f"{med([x['frame_interval_ms']['p95'] for x in s_ if 'frame_interval_ms' in x])} ms")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default="out/gizmo/runs")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--amoris", default="")
    ap.add_argument("--dist", default="", help="the editor build the host serves (default editor/dist)")
    ap.add_argument("--host-port", type=int, default=7890)
    ap.add_argument("--editor-port", type=int, default=5190)
    ap.add_argument("--moves", type=int, default=90)
    ap.add_argument("--hz", type=int, default=60)
    ap.add_argument("--dev", action="store_true",
                    help="also time the Vite dev server (its wasm viewport panics; not a protocol step)")
    ap.add_argument("--table", metavar="OUT")
    args = ap.parse_args()
    if args.table:
        table(args.table)
    else:
        run(args)


if __name__ == "__main__":
    sys.exit(main())
