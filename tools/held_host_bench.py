#!/usr/bin/env python3
"""What an agent pays at a host the script debugger holds (docs/bench/agent-debug.md).

Starts `pocket serve` on a copy of samples/sailing, sets a breakpoint in the log system and, ten
times, steps into it and measures the step that returns at the breakpoint, `status` and a field read
while held, and a `scripts apply` the host refuses with `debug.paused`: through the CLI (its process
start included), through `POST /api/call` on a new connection per call (urllib), and through
`POST /api/call` on one kept-alive connection (no connection set-up). Also the sizes of the reads the
debug evaluation found too big: `world get Sloop` whole, as text and JSON, and two fields alone;
`debug state` as full JSON, `{brief: true}` and the CLI's text. Medians, and every call's time
under `*_ms_all`.

Standard library only; macOS, Linux and Windows. Usage:
  python3 tools/held_host_bench.py [--pocket target/release/pocket] [--out FILE.json]
"""

import argparse
import http.client
import json
import os
import shutil
import signal
import statistics
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WINDOWS = os.name == "nt"
EXE = ".exe" if WINDOWS else ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pocket", default=str(REPO / f"target/release/pocket{EXE}"))
    ap.add_argument("--out", default=None)
    ap.add_argument("--runs", type=int, default=10)
    a = ap.parse_args()
    pocket = str(Path(a.pocket).resolve())
    tmp = Path(tempfile.mkdtemp(prefix="pocket-held-"))
    proj = tmp / "sailing"
    shutil.copytree(REPO / "samples/sailing", proj)
    err = open(tmp / "serve.err", "w")
    group = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if WINDOWS else {}
    server = subprocess.Popen([pocket, "serve", str(proj), "--port", "0"],
                              stdout=subprocess.PIPE, stderr=err, text=True, **group)
    result = {"date": time.strftime("%Y-%m-%d"), "runs": a.runs}

    def cli(*args):
        t = time.perf_counter()
        r = subprocess.run([pocket, *args], cwd=proj, capture_output=True, text=True, timeout=90)
        return r, (time.perf_counter() - t) * 1000

    def api(base, method, params=None):
        body = json.dumps({"id": 1, "method": method, "params": params or {}}).encode()
        req = urllib.request.Request(base + "/api/call", data=body, method="POST")
        req.add_header("content-type", "application/json")
        t = time.perf_counter()
        with urllib.request.urlopen(req, timeout=60) as r:
            text = r.read().decode()
        return json.loads(text), (time.perf_counter() - t) * 1000, len(text)

    conn = None

    def kept(method, params=None):
        body = json.dumps({"id": 1, "method": method, "params": params or {}})
        t = time.perf_counter()
        conn.request("POST", "/api/call", body=body, headers={"content-type": "application/json"})
        text = conn.getresponse().read().decode()
        return json.loads(text), (time.perf_counter() - t) * 1000

    try:
        base = json.loads(server.stdout.readline())["url"]
        host, _, port = base.split("//", 1)[1].rstrip("/").partition(":")
        conn = http.client.HTTPConnection(host, int(port), timeout=60)
        rules = (proj / "scripts/rules.ts").read_text().splitlines()
        line = 1 + next(i for i, l in enumerate(rules) if "const speed = Math.abs" in l)
        r, _ = cli("world", "get", "Sloop")
        rj, _ = cli("world", "get", "Sloop", "--json")
        rf, _ = cli("world", "get", "Sloop", "Boat.heading_deg,Boat.rudder")
        result["world_get_sloop_bytes"] = {"text": len(r.stdout), "json": len(rj.stdout),
                                           "two_fields_text": len(rf.stdout)}
        cli("debug", "break", f"rules.ts:{line}")
        names = ["step_to_breakpoint", "status_held", "world_get_held", "apply_refused"]
        by_cli = {n: [] for n in names}
        by_http = {n: [] for n in names}
        by_kept = {n: [] for n in names}
        for i in range(a.runs):
            r, ms = cli("step", "60")
            assert "stopped at" in r.stdout, r.stdout + r.stderr
            by_cli["step_to_breakpoint"].append(ms)
            r, ms = cli("status")
            assert "HELD" in r.stdout, r.stdout
            by_cli["status_held"].append(ms)
            r, ms = cli("world", "get", "Sloop", "Boat.speed")
            by_cli["world_get_held"].append(ms)
            r, ms = cli("scripts", "apply")
            assert "debug.paused" in r.stderr, r.stderr
            by_cli["apply_refused"].append(ms)
            if i == 0:
                _, _, full = api(base, "debug.state")
                _, _, brief = api(base, "debug.state", {"brief": True})
                t, _ = cli("debug", "state")
                result["debug_state_bytes"] = {"json_full": full, "json_brief": brief,
                                               "cli_text": len(t.stdout)}
            cli("debug", "continue")
            v, ms, _ = api(base, "time.step", {"ticks": 60})
            assert v["result"]["paused"] is True, v
            by_http["step_to_breakpoint"].append(ms)
            _, ms, _ = api(base, "status")
            by_http["status_held"].append(ms)
            _, ms, _ = api(base, "world.get", {"entity": "Sloop", "fields": ["Boat.speed"]})
            by_http["world_get_held"].append(ms)
            v, ms, _ = api(base, "scripts.apply")
            assert v["error"]["code"] == "debug.paused", v
            by_http["apply_refused"].append(ms)
            api(base, "debug.continue")
            v, ms = kept("time.step", {"ticks": 60})
            assert v["result"]["paused"] is True, v
            by_kept["step_to_breakpoint"].append(ms)
            _, ms = kept("status")
            by_kept["status_held"].append(ms)
            _, ms = kept("world.get", {"entity": "Sloop", "fields": ["Boat.speed"]})
            by_kept["world_get_held"].append(ms)
            v, ms = kept("scripts.apply")
            assert v["error"]["code"] == "debug.paused", v
            by_kept["apply_refused"].append(ms)
            kept("debug.continue")
        for label, by in (("cli", by_cli), ("http", by_http), ("http_keepalive", by_kept)):
            result[f"{label}_ms_median"] = {n: round(statistics.median(x), 2) for n, x in by.items()}
            result[f"{label}_ms_all"] = {n: [round(v, 2) for v in x] for n, x in by.items()}
        _, ms = cli("status")
        result["cli_ms_status_running"] = round(ms, 1)
    finally:
        if conn:
            conn.close()
        server.send_signal(signal.CTRL_BREAK_EVENT if WINDOWS else signal.SIGTERM)
        try:
            server.wait(10)
        except subprocess.TimeoutExpired:
            server.kill()
        err.close()
        shutil.rmtree(tmp, ignore_errors=True)
    text = json.dumps(result, indent=2) + "\n"
    if a.out:
        Path(a.out).write_text(text)
    print(text, end="")


if __name__ == "__main__":
    main()
