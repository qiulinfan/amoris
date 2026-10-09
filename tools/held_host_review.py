#!/usr/bin/env python3
"""The review of the held host (docs/spec/server.md 3.4), re-run against `pocket serve`.

Two findings of the adversarial review of 2026-10-09, reproduced through `POST /api/call` as the
reviewer did with the CLI, and what the fixes make of them (docs/bench/agent-debug.md):

1. Stop while the debugger holds Play: a breakpoint in the gauge loop of the debugger's test
   project (one gauge and three), Play, a one-second pause, `debug.continue`, then `play.stop`
   (the editor's Stop). Then, on samples/sailing in Play with a breakpoint in the log system,
   60 times `debug.continue` followed at once by `world.edit` (the "continue, then act" race), and
   the way out: `time.control {pause: true}` while held, `debug.continue`, then an edit, a step
   and Stop.
2. Two `time.step` calls queued when the debugger stops the first: a step of 4000 ticks with a
   breakpoint whose condition holds at tick 3000, and a step of 50 ticks sent once it runs.

Standard library only; macOS, Linux and Windows. Usage:
  python3 tools/held_host_review.py [--pocket target/release/pocket] [--out FILE.json]
"""

import argparse
import http.client
import json
import os
import shutil
import statistics
import subprocess
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WINDOWS = os.name == "nt"
EXE = ".exe" if WINDOWS else ""
DEBUGME = REPO / "crates/pocket-debug/tests/fixtures/debugme"
SAILING = REPO / "samples/sailing"
NEXT_LINE = 22  # rules.ts of debugme: `const next = g.level[r] + 1; // MARK next`
LOG_LINE = 27  # rules.ts of sailing: `const speed = Math.abs(b.speed[r]);`


class Served:
    """`pocket serve` on a fresh copy of a project, and calls to it."""

    def __init__(self, pocket, src, tmp, port):
        self.dir = tmp / f"{src.name}-{port}-{time.monotonic_ns()}"
        shutil.copytree(src, self.dir)
        shutil.rmtree(self.dir / ".pocket", ignore_errors=True)
        self.port = port
        flags = subprocess.CREATE_NEW_PROCESS_GROUP if WINDOWS else 0
        self.proc = subprocess.Popen(
            [pocket, "serve", str(self.dir), "--port", str(port)],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            creationflags=flags,
        )
        self.proc.stdout.readline()  # the address line: serving
        self.local = threading.local()

    def call(self, method, params=None, timeout=30, fresh=False):
        """One call over this thread's kept-alive connection (a new connection per call costs a
        timer quantum on some Windows machines, which would hide what the host costs), or over a
        new one (`fresh`, as the CLI and most one-shot HTTP clients call)."""
        c = None if fresh else getattr(self.local, "conn", None)
        if c is None:
            c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
            if not fresh:
                self.local.conn = c
        body = json.dumps({"method": method, "params": params or {}})
        c.request("POST", "/api/call", body, {"content-type": "application/json"})
        r = json.loads(c.getresponse().read())
        if fresh:
            c.close()
        return r

    def held(self):
        return self.call("status").get("result", {}).get("state") == "breakpoint"

    def wait_held(self, within=5.0):
        end = time.time() + within
        while time.time() < end:
            if self.held():
                return True
            time.sleep(0.005)
        return False

    def close(self):
        self.proc.kill()
        self.proc.wait()


def outcome(r):
    if "result" in r:
        return "ok"
    e = r["error"]
    return f'{e["code"]} queued={e.get("detail", {}).get("queued")}'


def stop_after_continue(pocket, tmp, gauges):
    s = Served(pocket, DEBUGME, tmp, 7931)
    try:
        if gauges > 1:
            spawn = [{"spawn": {"name": f"G{k}", "components": {"Gauge": {}}}} for k in range(2, gauges + 1)]
            s.call("world.edit", {"ops": spawn})
        s.call("debug.breakpoints.set", {"file": "rules.ts", "line": NEXT_LINE})
        s.call("play.start")
        s.call("debug.wait", {"timeout_ms": 5000})
        time.sleep(1.0)
        s.call("debug.continue")
        t = time.perf_counter()
        r = s.call("play.stop")
        ms = (time.perf_counter() - t) * 1000
        st = s.call("status")["result"]
        return {
            "gauges": gauges,
            "play_stop": r["result"]["mode"] if "result" in r else outcome(r),
            "ms": round(ms, 1),
            "status": {"mode": st.get("mode"), "state": st.get("state", "not held")},
        }
    finally:
        s.close()


def continue_then_act(pocket, tmp, rounds):
    s = Served(pocket, SAILING, tmp, 7932)
    edit = {"ops": [{"set": {"entity": "Sloop", "component": "Log", "value": {"distance": 5}}}]}
    try:
        s.call("debug.breakpoints.set", {"file": "rules.ts", "line": LOG_LINE})
        s.call("play.start")
        s.wait_held()
        races = {}
        for how in ("kept_alive", "new_connection"):
            race = races[how] = {}
            for _ in range(rounds):
                s.call("debug.continue")
                k = outcome(s.call("world.edit", edit, fresh=how == "new_connection"))
                race[k] = race.get(k, 0) + 1
                s.wait_held()
        q = s.call("time.control", {"pause": True})
        stop_tick = s.call("status")["result"]["paused_at"]["tick"]
        s.call("debug.continue")
        time.sleep(0.3)
        st = s.call("status")["result"]
        after_edit = outcome(s.call("world.edit", edit))
        t = time.perf_counter()
        step = s.call("time.step", {"ticks": 3})["result"]
        step_ms = (time.perf_counter() - t) * 1000
        t = time.perf_counter()
        stop = s.call("play.stop")
        stop_ms = (time.perf_counter() - t) * 1000
        return {
            f"continue_then_world_edit_x{rounds}": races,
            "queued_pause": {
                "time_control": outcome(q),
                "held_tick": stop_tick,
                "after_continue": {"tick": st["tick"], "paused": st["paused"], "state": st.get("state", "not held")},
                "world_edit": after_edit,
                "time_step_3": {"paused": step.get("paused"), "tick": step.get("tick"), "ms": round(step_ms, 1)},
                "play_stop": {"mode": stop.get("result", {}).get("mode") or outcome(stop), "ms": round(stop_ms, 1)},
            },
        }
    finally:
        s.close()


def step_latency(pocket, tmp, rounds):
    """A `time.step` into a breakpoint, answered by the game thread at the stop, and reads while
    held (sailing, a breakpoint in the log system), medians in milliseconds."""
    s = Served(pocket, SAILING, tmp, 7934)
    ms = {"time_step_to_breakpoint": [], "status_held": [], "world_get_held": [], "apply_refused": []}

    def timed(name, method, params=None):
        t = time.perf_counter()
        r = s.call(method, params)
        ms[name].append((time.perf_counter() - t) * 1000)
        return r

    try:
        s.call("debug.breakpoints.set", {"file": "rules.ts", "line": LOG_LINE})
        for _ in range(rounds):
            r = timed("time_step_to_breakpoint", "time.step", {"ticks": 60})
            assert r["result"]["paused"] is True, r
            timed("status_held", "status")
            timed("world_get_held", "world.get", {"entity": "Sloop", "fields": ["Boat.speed"]})
            assert outcome(timed("apply_refused", "scripts.apply")) == "debug.paused queued=False"
            s.call("debug.continue")
        return {k: round(statistics.median(v), 2) for k, v in ms.items()}
    finally:
        s.close()


def two_steps(pocket, tmp):
    s = Served(pocket, DEBUGME, tmp, 7933)
    try:
        s.call("time.step", {"ticks": 20})  # past the fixture's `debugger;` statement at tick 8
        s.call("debug.breakpoints.set", {"file": "rules.ts", "line": NEXT_LINE, "condition": "ctx.tick === 3000"})
        answers = {}

        def step(name, ticks):
            answers[name] = s.call("time.step", {"ticks": ticks})

        a = threading.Thread(target=step, args=("A", 4000))
        a.start()
        # B once A runs (the status read waits for a boundary between A's ticks).
        while s.call("status")["result"]["tick"] <= 20:
            time.sleep(0.001)
        b = threading.Thread(target=step, args=("B", 50))
        b.start()
        a.join()
        b.join()
        held_tick = s.call("status")["result"]["tick"]
        s.call("debug.breakpoints.clear")
        s.call("debug.continue")
        time.sleep(0.5)
        after = s.call("status")["result"]

        def brief(r):
            if "result" in r:
                v = r["result"]
                return {"paused": v.get("paused"), "tick": v.get("tick"), "stopped_by": (v.get("stopped_by") or {}).get("reason")}
            return outcome(r)

        return {
            "A_ticks_4000": brief(answers["A"]),
            "B_ticks_50": brief(answers["B"]),
            "held_status_tick": held_tick,
            "after_continue": {"tick": after["tick"], "steps_due": after["steps_due"]},
        }
    finally:
        s.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pocket", default=str(REPO / f"target/release/pocket{EXE}"))
    ap.add_argument("--out", default=None)
    ap.add_argument("--rounds", type=int, default=60)
    a = ap.parse_args()
    pocket = str(Path(a.pocket).resolve())
    tmp = Path(tempfile.mkdtemp(prefix="pocket-review-"))
    try:
        report = {
            "stop_after_continue": [stop_after_continue(pocket, tmp, g) for g in (1, 1, 1, 3, 3, 3)],
            **continue_then_act(pocket, tmp, a.rounds),
            "two_steps_at_a_stop": two_steps(pocket, tmp),
            "held_ms_median_keepalive": step_latency(pocket, tmp, 20),
        }
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    text = json.dumps(report, indent=1)
    print(text)
    if a.out:
        Path(a.out).write_text(text + "\n")


if __name__ == "__main__":
    main()
