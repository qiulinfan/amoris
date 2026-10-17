#!/usr/bin/env python3
"""The serve loop under `amoris serve`: the game thread's time per tick with clients connected,
and the host's CPU while the edit world idles (docs/bench/profiler.md, "The serve loop").

For each build (`--amoris LABEL=path`; several take turns), one `amoris serve` on a copy of
samples/sailing, all started at once, then:

- **play**: for each round and each client set (rotated per round), Play starts, the clients
  connect (editor/tools/serve-clients.ts), after `--settle` seconds `--window` seconds of ticks are
  read with `profile.frame` (the last 600 ticks), the clients leave and Play stops. `--speed`
  sets Play's speed: at 1 each tick runs after a sleep of about 16 ms, at 8 after about 1.5 ms (a
  control for whether a load slows the tick that follows a long sleep). Client sets:
  `none`; `sockets`, the editor's /ws (every topic) and /render without a page; `sockets-reads`,
  the same sending the editor's reads on each `world.changed` (`world.get` per entity named, one
  `world.tree`); `editor`, the editor in headless Chrome drawing on Chrome's default WebGPU
  adapter (what the page sends and receives is counted); `editor-dgpu`, the same with
  `--force_high_performance_gpu`; `busy1` and `busy3`, no client but one or three processes
  spinning on the CPU (what any load beside the host does).
- **idle** (`--idle SECONDS`, 0 to skip): the edit world paused, the host's CPU over that many
  seconds with nothing connected and with the sockets client connected, `--idle-rounds` times per
  build (builds alternating). Two readings: the process's cycles (`QueryProcessCycleTime`, exact;
  as a share of one core at the cycle counter's rate, which a thread spinning here measures first)
  and its CPU time (`GetProcessTimes`, in 15.6 ms steps, too coarse for a near-idle process).

`--host-cpus` and `--client-cpus` (e.g. `2-5` and `0-1,6-15`; Windows) pin the hosts and the
clients (Chrome inherits bun's mask) to disjoint logical processors, so they never share a core.

Build first: `cargo build --release -p amoris-app --bin amoris` and `cd editor && bun install
--frozen-lockfile && bun run build`. From the repository root, on the shared laptop inside the
bench lock:

    python tools/serve_loop_bench.py --out out/serveloop/run1 --rounds 3 \
        --amoris base=<a base build's amoris.exe> --amoris this=target/release/amoris.exe

A base from a commit before the 2026-10-16 rename is built from its exported tree after
`tools/amoris_rename.py --tree` (AGENTS.md, Building; docs/bench/profiler.md 6.6): a build from
before it ignores the AMORIS_* variables this script sets and refuses the renamed samples.

Raw JSON goes to --out (ignored): one file per build, the conditions, and summary.md.
"""

import argparse
import ctypes
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
CLIENTS = ("none", "sockets", "sockets-reads", "editor", "editor-dgpu", "busy1", "busy3")


def conditions(out: Path, label: str) -> None:
    script = REPO / "tools/machine_conditions.py"
    if WINDOWS and script.exists():
        subprocess.run([sys.executable, str(script), str(out / f"conditions-{label}.json"),
                        "--label", f"serveloop-{label}"], check=False, capture_output=True)


def amoris_version(amoris) -> dict:
    """`amoris version` of the build measured (commit and source hash), kept with the results."""
    r = subprocess.run([str(amoris), "version"], capture_output=True, text=True, timeout=60)
    try:
        return json.loads(r.stdout)
    except ValueError:
        return {"error": (r.stdout + r.stderr).strip()}


def post(base: str, method: str, params=None) -> dict:
    req = urllib.request.Request(base + "api/call", method="POST",
                                 data=json.dumps({"method": method,
                                                  "params": params or {}}).encode(),
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        reply = json.loads(r.read())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def kernel32():
    """kernel32 with the signatures used here (handles and masks are pointer-sized)."""
    k32 = ctypes.windll.kernel32
    h, size = ctypes.c_void_p, ctypes.c_size_t
    k32.GetCurrentProcess.restype = h
    k32.OpenProcess.restype = h
    k32.CloseHandle.argtypes = [h]
    k32.GetProcessTimes.argtypes = [h] + [ctypes.POINTER(ctypes.c_ulonglong)] * 4
    k32.QueryProcessCycleTime.argtypes = [h, ctypes.POINTER(ctypes.c_ulonglong)]
    k32.QueryThreadCycleTime.argtypes = [h, ctypes.POINTER(ctypes.c_ulonglong)]
    k32.GetCurrentThread.restype = h
    k32.GetProcessAffinityMask.argtypes = [h, ctypes.POINTER(size), ctypes.POINTER(size)]
    k32.SetProcessAffinityMask.argtypes = [h, size]
    return k32


def cpu_seconds(proc: subprocess.Popen) -> float:
    """The process's user and kernel CPU time so far (Windows: GetProcessTimes)."""
    if not WINDOWS:
        return 0.0
    k32 = kernel32()
    handle = k32.OpenProcess(0x1000, False, proc.pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    times = [ctypes.c_ulonglong() for _ in range(4)]
    k32.GetProcessTimes(handle, *[ctypes.byref(t) for t in times])
    k32.CloseHandle(handle)
    return (times[2].value + times[3].value) / 1e7


def cycles(proc: subprocess.Popen) -> int:
    """The cycles the process's threads have run so far (Windows: QueryProcessCycleTime)."""
    if not WINDOWS:
        return 0
    k32 = kernel32()
    handle = k32.OpenProcess(0x1000, False, proc.pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    n = ctypes.c_ulonglong()
    k32.QueryProcessCycleTime(handle, ctypes.byref(n))
    k32.CloseHandle(handle)
    return n.value


def cycle_rate() -> float:
    """Cycles a second of one busy core, as the cycle counter counts them: this thread spins for
    0.3 s (the best of three)."""
    if not WINDOWS:
        return 0.0
    k32 = kernel32()
    best = 0.0
    for _ in range(3):
        a, b = ctypes.c_ulonglong(), ctypes.c_ulonglong()
        t0 = time.perf_counter()
        k32.QueryThreadCycleTime(k32.GetCurrentThread(), ctypes.byref(a))
        while time.perf_counter() - t0 < 0.3:
            pass
        k32.QueryThreadCycleTime(k32.GetCurrentThread(), ctypes.byref(b))
        best = max(best, (b.value - a.value) / (time.perf_counter() - t0))
    return best


CPUS = {"host": 0, "client": 0}


def cpu_mask(spec: str) -> int:
    """`0-1,6-15` as an affinity mask (0: none)."""
    mask = 0
    for part in filter(None, spec.split(",")):
        lo, _, hi = part.partition("-")
        for i in range(int(lo), int(hi or lo) + 1):
            mask |= 1 << i
    return mask


class affinity:
    """While inside, this process (and so every child it starts, which inherits the mask) runs on
    the logical processors of `mask` (0: unchanged)."""

    def __init__(self, mask: int):
        self.mask = mask

    def __enter__(self):
        if self.mask and WINDOWS:
            k32 = kernel32()
            proc, system = ctypes.c_size_t(), ctypes.c_size_t()
            me = k32.GetCurrentProcess()
            k32.GetProcessAffinityMask(me, ctypes.byref(proc), ctypes.byref(system))
            self.old = proc.value
            if not k32.SetProcessAffinityMask(me, self.mask):
                raise OSError(f"SetProcessAffinityMask({self.mask:#x}) failed")

    def __exit__(self, *_):
        if self.mask and WINDOWS:
            k32 = kernel32()
            k32.SetProcessAffinityMask(k32.GetCurrentProcess(), self.old)


class Busy:
    """`n` Python processes spinning for `seconds`."""

    def __init__(self, n: int, seconds: float):
        code = (f"import time; end = time.perf_counter() + {seconds}\n"
                "while time.perf_counter() < end: pass")
        with affinity(CPUS["client"]):
            self.ps = [subprocess.Popen([sys.executable, "-c", code]) for _ in range(n)]

    def report(self) -> dict:
        for p in self.ps:
            p.wait(60)
        return {"busy": len(self.ps)}


class Client:
    """editor/tools/serve-clients.ts held on the host until it prints its report."""

    def __new__(cls, base: str, kind: str, seconds: float):
        if kind.startswith("busy"):
            return Busy(int(kind[4:]), seconds)
        return super().__new__(cls)

    def __init__(self, base: str, kind: str, seconds: float):
        mode = "editor" if kind.startswith("editor") else "sockets"
        env = dict(os.environ)
        flags = env.get("CHROME_FLAGS", "")
        if kind == "editor-dgpu":
            flags = (flags + " --force_high_performance_gpu").strip()
        env["CHROME_FLAGS"] = flags
        extra = ["--reads"] if kind == "sockets-reads" else []
        with affinity(CPUS["client"]):
            self.p = subprocess.Popen([shutil.which("bun"), "tools/serve-clients.ts", "--url", base,
                                       "--mode", mode, "--seconds", str(seconds), *extra],
                                      cwd=REPO / "editor", env=env, stdout=subprocess.PIPE,
                                      stderr=subprocess.STDOUT, text=True)
        for line in self.p.stdout:
            if line.strip() == "ready":
                return
        raise RuntimeError(f"{kind}: the client ended before it was ready")

    def report(self) -> dict:
        out = self.p.stdout.read()
        self.p.wait(60)
        for line in reversed(out.splitlines()):
            if line.startswith("{"):
                return json.loads(line)
        return {"error": out[-2000:]}


def row(summary: dict, name: str) -> float:
    for s in summary.get("systems", []):
        if s["name"] == name:
            return s["ms"]
    return float("nan")


def play(base: str, kind: str, a) -> dict:
    post(base, "play.start", {"speed": a.speed})
    client = Client(base, kind, a.settle + a.window + 1) if kind != "none" else None
    time.sleep(a.settle + a.window)
    frame = post(base, "profile.frame", {"ticks": int(a.window * 60)})
    report = client.report() if client else None
    post(base, "play.stop")
    time.sleep(1)
    return {"client": kind, "profile": frame, "received": report}


def idle(base: str, server: subprocess.Popen, kind: str, seconds: float, rate: float) -> dict:
    client = Client(base, kind, seconds + 1) if kind != "none" else None
    c0, n0, t0 = cpu_seconds(server), cycles(server), time.perf_counter()
    time.sleep(seconds)
    c1, n1, t1 = cpu_seconds(server), cycles(server), time.perf_counter()
    report = client.report() if client else None
    span = t1 - t0
    return {"client": kind, "seconds": span, "cpu_s": c1 - c0,
            "cpu_pct_of_one_core": 100 * (c1 - c0) / span, "cycles": n1 - n0,
            "cycle_rate": rate, "mcycles_per_s": (n1 - n0) / span / 1e6,
            "cycles_pct_of_one_core": 100 * (n1 - n0) / span / rate if rate else 0.0,
            "received": report}


class Served:
    """One build's `amoris serve` on its own copy of samples/sailing."""

    def __init__(self, label: str, amoris: Path, out: Path):
        self.label = label
        self.tmp = Path(tempfile.mkdtemp(prefix="amoris-serveloop-"))
        proj = self.tmp / "sailing"
        shutil.copytree(REPO / "samples/sailing", proj)
        env = dict(os.environ, AMORIS_EDITOR_DIST=str(REPO / "editor/dist"),
                   AMORIS_WEB_VIEWPORT=str(REPO / "web/viewport"))
        group = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP} if WINDOWS else {}
        self.errlog = open(out / f"serve-{label}.err", "w")
        with affinity(CPUS["host"]):
            self.server = subprocess.Popen([str(amoris), "serve", str(proj), "--port", "0"],
                                           env=env, stdout=subprocess.PIPE, stderr=self.errlog,
                                           text=True, **group)
        info = json.loads(self.server.stdout.readline())
        self.base = info["url"].rstrip("/") + "/"
        self.result = {"label": label, "amoris": str(amoris),
                       "version": amoris_version(amoris), "plays": [], "idle": []}
        print(f"{label}: host at {self.base}", flush=True)

    def stop(self):
        self.server.send_signal(signal.CTRL_BREAK_EVENT if WINDOWS else signal.SIGTERM)
        try:
            self.server.wait(10)
        except subprocess.TimeoutExpired:
            self.server.kill()
        self.errlog.close()
        shutil.rmtree(self.tmp, ignore_errors=True)


def measure(served: list, out: Path, a) -> None:
    """The idle phase of each build, then the rounds: per round every build in turn (rotated),
    and per build every client set (rotated)."""
    time.sleep(2)
    if a.idle > 0:
        rate = cycle_rate()
        for i in range(a.idle_rounds):
            for s in served[i % len(served):] + served[:i % len(served)]:
                for kind in ("none", "sockets"):
                    r = idle(s.base, s.server, kind, a.idle, rate)
                    r["round"] = i + 1
                    print(f"  {s.label} idle {kind}: {r['cycles_pct_of_one_core']:.3f} % of a "
                          f"core by cycles, {r['cpu_pct_of_one_core']:.2f} % by CPU time",
                          flush=True)
                    s.result["idle"].append(r)
    clients = [c for c in CLIENTS if c in a.clients]
    for i in range(a.rounds):
        builds = served[i % len(served):] + served[:i % len(served)]
        for s in builds:
            order = clients[i % len(clients):] + clients[:i % len(clients)]
            for kind in order:
                r = play(s.base, kind, a)
                r["round"] = i + 1
                p = r["profile"]
                print(f"  round {i + 1} {s.label} {kind}: busy {p['busy_ms']:.3f} "
                      f"tick {p['tick_ms']:.3f} publish {row(p, 'loop.publish'):.4f} "
                      f"present {row(p, 'loop.present'):.4f}", flush=True)
                s.result["plays"].append(r)
    for s in served:
        (out / f"serve-loop-{s.label}.json").write_text(json.dumps(s.result, indent=2) + "\n")


def summarize(results: list) -> str:
    lines = ["| Build | Clients | Plays | busy_ms | tick_ms | loop.publish | loop.present | "
             "loop.after | script:log | physics.step | /render frames/s (each play) |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for res in results:
        by, rates = {}, {}
        for p in res["plays"]:
            by.setdefault(p["client"], []).append(p["profile"])
            rate = ((p.get("received") or {}).get("render") or {}).get("per_s")
            if rate is not None:
                rates.setdefault(p["client"], []).append(rate)
        for kind, ps in by.items():
            med = lambda f: statistics.median(f(p) for p in ps)  # noqa: E731
            rate = rates.get(kind, [])
            lines.append(
                f"| {res['label']} | {kind} | {len(ps)} | {med(lambda p: p['busy_ms']):.3f} | "
                f"{med(lambda p: p['tick_ms']):.3f} | {med(lambda p: row(p, 'loop.publish')):.4f} | "
                f"{med(lambda p: row(p, 'loop.present')):.4f} | "
                f"{med(lambda p: row(p, 'loop.after')):.4f} | "
                f"{med(lambda p: row(p, 'script:log')):.4f} | "
                f"{med(lambda p: row(p, 'physics.step')):.4f} | "
                + (f"{statistics.median(rate):.1f} ({', '.join(f'{r:.1f}' for r in rate)})"
                   if rate else "") + " |")
    if any(r["idle"] for r in results):
        lines += ["", "Idle, every try: % of one core by cycles (by CPU time); median by cycles",
                  "", "| Build | Clients | Tries | Median | /render messages/s |",
                  "|---|---|---|---|---|"]
        for res in results:
            by = {}
            for r in res["idle"]:
                by.setdefault(r["client"], []).append(r)
            for kind, rs in by.items():
                tries = ", ".join(f"{r.get('cycles_pct_of_one_core', 0):.3f} "
                                  f"({r['cpu_pct_of_one_core']:.2f})" for r in rs)
                med = statistics.median(r.get("cycles_pct_of_one_core", 0) for r in rs)
                rec = statistics.median(((r.get("received") or {}).get("render") or {})
                                        .get("per_s", 0) for r in rs)
                lines.append(f"| {res['label']} | {kind} | {tries} | {med:.3f} | {rec:.1f} |")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", default="out/serveloop/run")
    ap.add_argument("--amoris", action="append", default=[], metavar="LABEL=PATH")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--settle", type=float, default=3.0)
    ap.add_argument("--window", type=float, default=10.0)
    ap.add_argument("--speed", type=float, default=1.0, help="Play's speed (8: a tick every 2 ms)")
    ap.add_argument("--idle", type=float, default=20.0)
    ap.add_argument("--idle-rounds", type=int, default=1)
    ap.add_argument("--clients", nargs="+", default=list(CLIENTS), choices=CLIENTS)
    ap.add_argument("--host-cpus", default="", help="logical processors for the hosts, e.g. 2-5")
    ap.add_argument("--client-cpus", default="", help="logical processors for the clients")
    a = ap.parse_args()
    CPUS["host"], CPUS["client"] = cpu_mask(a.host_cpus), cpu_mask(a.client_cpus)
    out = (REPO / a.out).resolve() if not Path(a.out).is_absolute() else Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    builds = [(x.split("=", 1)[0], Path(x.split("=", 1)[1])) for x in a.amoris] or \
        [("this", REPO / f"target/release/amoris{'.exe' if WINDOWS else ''}")]
    conditions(out, "start")
    served = []
    try:
        for label, path in builds:
            served.append(Served(label, path if path.is_absolute() else REPO / path, out))
        measure(served, out, a)
    finally:
        for s in served:
            s.stop()
    conditions(out, "end")
    table = summarize([s.result for s in served])
    (out / "summary.md").write_text(table + "\n")
    print(table)


if __name__ == "__main__":
    main()
