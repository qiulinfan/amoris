#!/usr/bin/env python3
"""The R1 physics benchmark (docs/bench/physics-r1.md): Rapier against Unity's PhysX on the Windows
R1 laptop, the scenes of bench/physics/rapier, interleaved, several rounds.

    python bench/physics/scripts/r1_bench.py build [--isa] [--unity]
    python bench/physics/scripts/r1_bench.py run --out DIR [--rounds 5] [--isa] [--unity]
        [--configs NAME,...] [--lock .../out/locks/bench_lock.py]
    python bench/physics/scripts/r1_bench.py round --out DIR --round K [--isa] [--unity]
        [--configs NAME,...]
    python bench/physics/scripts/r1_bench.py check --out DIR [--unity]
    python bench/physics/scripts/r1_bench.py summarize DIR [--max-other-cpu 9] > DIR/summary.md
    python bench/physics/scripts/r1_bench.py unity-compile

`build` compiles the Rapier bench offline in three configurations (`det`, what Amoris ships;
Rapier's default; `det,parallel`) into $CARGO_TARGET_DIR/physics-bench (default target/); `--isa`
adds the `det` build for x86-64-v2 and v3, and `--unity` builds the PGS and TGS Unity players
(bench/physics/unity/Builds, needs a licensed Unity editor; UNITY_EDITOR names it). `round` runs
every configuration on every scene once, in the round's row of a Williams design (each
configuration takes each position once in n rounds, 2n for an odd n, and follows each other one
equally often), and appends one JSON line per run to DIR/runs.jsonl, tagged with the round, the
run's position and predecessor, the whole machine's CPU use during the run and, on Windows, the
share of it that other processes used. `--configs` keeps only the named configurations (as
runs.jsonl names them: rapier-det-t1, unity-pgs-t8, ...), and the Williams design is then over
their number. `run` repeats `round`, each round under the machine's benchmark lock when `--lock`
names it, and waits ROUND_GAP_S between rounds so that another agent waiting for the lock gets it.
`check` runs the deterministic builds untimed for 300 steps and compares their end hashes with the
Apple M5's (docs/bench/physics.md, "Snapshots and forks"); with `--unity` it also runs every scene
untimed for 500 steps on Rapier det. and on each PhysX player with `-probe` (the settings and mass
properties PhysX used) into DIR/check-unity.jsonl, the scene-fidelity check, and exits with status 1
unless every end hash equals UNITY_CHECK_HASHES. `summarize` prints the tables: medians over rounds,
never the best run, and the paired ratios again over the quiet pairs (other processes at most
--max-other-cpu % of the machine during each run: for a Unity player during its steps, which it
measures itself, for Rapier during its whole process). `unity-compile`
compiles the Unity scripts with the editor's own Roslyn and assemblies, without starting Unity (no
license needed): it shows that they build, not that they run.
"""
import argparse
import ctypes
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target") / "physics-bench"
UNITY_PROJECT = REPO / "bench" / "physics" / "unity"
UNITY_EDITOR = os.environ.get(
    "UNITY_EDITOR", r"C:\Program Files\Unity\Hub\Editor\6000.3.13f1\Editor\Unity.exe")
EXE = ".exe" if sys.platform == "win32" else ""
SCENES = ["Pyramid", "Pyramid30", "ConvexVsMesh", "Raycast"]
THREADS = [1, 8, 16]
RAPIER_BUILDS = {"det": "det", "default": "", "det-parallel": "det,parallel"}
# The shipped det. build compiled for newer x86-64 levels (`--isa`): v2 adds SSE4.2 and POPCNT,
# v3 AVX2 and FMA. The engine builds for the baseline (SSE2), as the det. build above does.
ISA_BUILDS = {"det-v2": "x86-64-v2", "det-v3": "x86-64-v3"}
SOLVERS = ["PGS", "TGS"]
# Rapier det.'s end hashes after 300 steps on the Apple M5 (aarch64), from docs/bench/physics.md,
# "Snapshots and forks" (each fork's end hash equals the original's there).
M5_HASHES_300 = {"Pyramid": "0x37508ea9e0269ef8", "ConvexVsMesh": "0x1f4ff7d50ed9ecbb"}
RUN_TIMEOUT_S = 1800
# `check --unity` runs every scene this long, the timed runs' default, to compare outcomes.
UNITY_CHECK_STEPS = 500
# The end hashes `check --unity` must reproduce (one thread, UNITY_CHECK_STEPS steps), recorded on
# R1 on 2026-10-16 (docs/bench/physics-r1.md, "Fidelity"): Rapier det.'s equal the M5's 500-step
# runs; the PhysX players' are Unity 6000.3.13f1's with this repository's PhysxBench.cs, repeatable
# across runs and rebuilds of the players. Raycast steps ConvexVsMesh's simulation, so it ends with
# ConvexVsMesh's hash.
UNITY_CHECK_HASHES = {
    ("Pyramid", "rapier-det"): "0x35b8325978916b7f",
    ("Pyramid30", "rapier-det"): "0xae1548844c93a7b8",
    ("ConvexVsMesh", "rapier-det"): "0x9ae30402cc5cb77b",
    ("Raycast", "rapier-det"): "0x9ae30402cc5cb77b",
    ("Pyramid", "unity-pgs"): "0x5541581afb294c21",
    ("Pyramid", "unity-tgs"): "0x9f99482f68342568",
    ("Pyramid30", "unity-pgs"): "0x393f90c47438397d",
    ("Pyramid30", "unity-tgs"): "0xd8054f0e43884c19",
    ("ConvexVsMesh", "unity-pgs"): "0x0d0cf6c2e0213970",
    ("ConvexVsMesh", "unity-tgs"): "0x788070317d3eb1e6",
    ("Raycast", "unity-pgs"): "0x0d0cf6c2e0213970",
    ("Raycast", "unity-tgs"): "0x788070317d3eb1e6",
}
# bench_lock.py polls every 2 s; a longer gap between rounds lets a waiting agent take the lock.
ROUND_GAP_S = 10


def cargo_tool(name):
    """cargo or rustc from PATH, else from ~/.cargo/bin (Windows shells often lack it)."""
    found = shutil.which(name)
    return found or str(Path.home() / ".cargo" / "bin" / f"{name}{EXE}")


def rapier_exe(build):
    return TARGET / f"rapier-{build}" / "release" / f"physics-bench{EXE}"


def unity_player(solver):
    return UNITY_PROJECT / "Builds" / solver / f"PhysxBench{EXE}"


def configs(unity, threads, isa=False):
    """(run name, command builder) in a fixed base order; rounds permute it (williams_row)."""
    out = [("rapier-det-t1", lambda s, n, _: [rapier_exe("det"), "--scene", s, "--steps", n]),
           ("rapier-default-t1", lambda s, n, _: [rapier_exe("default"), "--scene", s, "--steps", n])]
    if isa:
        for build in ISA_BUILDS:
            out.append((f"rapier-{build}-t1",
                        lambda s, n, _, b=build: [rapier_exe(b), "--scene", s, "--steps", n]))
    for t in threads:
        out.append((f"rapier-det-parallel-t{t}",
                    lambda s, n, _, t=t: [rapier_exe("det-parallel"), "--scene", s, "--steps", n,
                                          "--threads", t]))
    if unity:
        for solver in SOLVERS:
            for t in threads:
                out.append((f"unity-{solver.lower()}-t{t}",
                            lambda s, n, log, solver=solver, t=t: [
                                unity_player(solver), "-batchmode", "-nographics", "-logFile", log,
                                "-job-worker-count", t - 1,
                                "-physxbench", "-scene", s, "-steps", n, "-threads", t]))
    return out


def williams_row(n, r):
    """Row r (from 0) of a Williams design for n treatments: rows 0 to n - 1 form a Latin square
    (every treatment in every position once) balanced for first-order carryover (every treatment
    directly after every other once); for an odd n the balance needs the mirrored square of rows n
    to 2n - 1 as well."""
    base, lo, hi = [0], 1, n - 1
    while len(base) < n:
        base.append(lo)
        lo += 1
        if len(base) < n:
            base.append(hi)
            hi -= 1
    row = [(b + r) % n for b in base]
    if n % 2 and (r // n) % 2:
        row.reverse()
    return row


def missing_programs(cfgs):
    """The executables of these configurations that are not built."""
    exes = {Path(build("Pyramid", 1, "x.log")[0]) for _, build in cfgs}
    return sorted(str(e) for e in exes if not e.exists())


class CpuMeter:
    """Whole-machine CPU use between start and stop (all logical processors, this run included),
    and on Windows the share of it that other processes used (other_pct)."""

    def __init__(self):
        self.t0 = self._times()

    @staticmethod
    def _times():
        if sys.platform == "win32":
            ft = [ctypes.c_ulonglong() for _ in range(3)]
            ctypes.windll.kernel32.GetSystemTimes(*[ctypes.byref(f) for f in ft])
            idle, kernel, user = (f.value for f in ft)
            return idle, kernel + user  # kernel time includes idle time
        return None

    def busy_pct(self, proc=None):
        """(whole-machine CPU %, CPU % of everything but `proc`); the second needs the finished
        process `proc` (a Popen) on Windows, else it is None."""
        t1 = self._times()
        if t1 is None or self.t0 is None:
            load = round(os.getloadavg()[0], 2) if hasattr(os, "getloadavg") else None
            return load, None
        idle, total = t1[0] - self.t0[0], t1[1] - self.t0[1]
        if not total:
            return None, None
        own = process_cpu_100ns(proc) if proc is not None else None
        other = None if own is None else round(100.0 * (total - idle - own) / total, 1)
        return round(100.0 * (total - idle) / total, 1), other


def process_cpu_100ns(proc):
    """Kernel plus user time of a finished Popen's process (Windows), in 100 ns units, or None."""
    handle = getattr(proc, "_handle", None)
    if sys.platform != "win32" or handle is None:
        return None
    ft = [ctypes.c_ulonglong() for _ in range(4)]
    if not ctypes.windll.kernel32.GetProcessTimes(int(handle), *[ctypes.byref(f) for f in ft]):
        return None
    return ft[2].value + ft[3].value


def run_one(cmd, out_dir, tag):
    """Runs one benchmark process; returns its JSON line as a dict, or None."""
    log = out_dir / "logs" / f"{tag}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    unity = "-physxbench" in cmd
    result_file = None
    if unity:
        fd, result_file = tempfile.mkstemp(prefix="physxbench-", suffix=".jsonl", dir=out_dir)
        os.close(fd)
        cmd = cmd + ["-out", result_file]
    cmd = [str(c) for c in cmd]
    try:
        meter = CpuMeter()
        t0 = time.perf_counter()
        try:
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        except OSError as e:
            print(f"FAILED to start {tag}: {e}", file=sys.stderr)
            return None
        try:
            stdout, stderr = p.communicate(timeout=RUN_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            p.kill()
            p.communicate()
            print(f"TIMEOUT: {tag}", file=sys.stderr)
            return None
        wall = time.perf_counter() - t0
        busy, other = meter.busy_pct(p)
        text = Path(result_file).read_text(encoding="utf-8") if unity else stdout
    finally:
        if result_file and os.path.exists(result_file):
            os.remove(result_file)
    if not unity and stderr:
        log.write_text(stderr, encoding="utf-8")
    lines = [ln for ln in text.splitlines() if ln.startswith("{")]
    if p.returncode != 0 or not lines:
        print(f"FAILED ({p.returncode}): {tag}: {' '.join(cmd)}\n{stderr[-2000:]}",
              file=sys.stderr)
        return None
    d = json.loads(lines[-1])
    d["cpu_busy_pct"] = busy
    if other is not None:
        d["cpu_other_pct"] = other
    d["wall_s"] = round(wall, 2)
    return d


def conditions(out_dir, label):
    tool = REPO / "tools" / "machine_conditions.py"
    if sys.platform == "win32" and tool.exists():
        subprocess.run([sys.executable, str(tool), str(out_dir / f"conditions-{label}.json"),
                        "--label", label, "--cpu-seconds", "3"], timeout=300)


def machine(out_dir):
    path = out_dir / "machine.json"
    if path.exists() or sys.platform != "win32":
        return
    q = ("$c = Get-CimInstance Win32_Processor; $o = Get-CimInstance Win32_OperatingSystem; "
         "[pscustomobject]@{ cpu = $c.Name.Trim(); cores = $c.NumberOfCores; "
         "logical = $c.NumberOfLogicalProcessors; ram_gb = [math]::Round("
         "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1); "
         "os = $o.Caption + ' ' + $o.Version } | ConvertTo-Json -Compress")
    p = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", q],
                       capture_output=True, text=True, timeout=120)
    info = json.loads(p.stdout) if p.stdout.strip() else {}
    rc = subprocess.run([cargo_tool("rustc"), "--version"], capture_output=True, text=True)
    info["rustc"] = rc.stdout.strip()
    path.write_text(json.dumps(info, indent=1) + "\n", encoding="utf-8")


def cmd_build(a):
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "6"))
    for name, features in RAPIER_BUILDS.items():
        cmd = [cargo_tool("cargo"), "build", "--offline", "--release"]
        if features:
            cmd += ["--features", features]
        env["CARGO_TARGET_DIR"] = str(TARGET / f"rapier-{name}")
        print("+", " ".join(cmd), f"(CARGO_TARGET_DIR={env['CARGO_TARGET_DIR']})", file=sys.stderr)
        subprocess.run(cmd, cwd=REPO / "bench" / "physics" / "rapier", env=env, check=True)
    for name, cpu in (ISA_BUILDS.items() if a.isa else []):
        cmd = [cargo_tool("cargo"), "build", "--offline", "--release", "--features", "det"]
        env["CARGO_TARGET_DIR"] = str(TARGET / f"rapier-{name}")
        env["RUSTFLAGS"] = f"-C target-cpu={cpu}"
        print("+", " ".join(cmd), f"(RUSTFLAGS={env['RUSTFLAGS']})", file=sys.stderr)
        subprocess.run(cmd, cwd=REPO / "bench" / "physics" / "rapier", env=env, check=True)
    env.pop("RUSTFLAGS", None)
    if a.unity:
        log = TARGET / "unity-build.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        cmd = [UNITY_EDITOR, "-batchmode", "-nographics", "-quit", "-projectPath",
               str(UNITY_PROJECT), "-executeMethod", "PhysxBenchEditor.BuildPlayers",
               "-logFile", str(log)]
        print("+", " ".join(cmd), file=sys.stderr)
        p = subprocess.run(cmd, timeout=3600)
        if p.returncode != 0 or not all(unity_player(s).exists() for s in SOLVERS):
            sys.exit(f"Unity player build failed ({p.returncode}); see {log}")


def select(cfgs, names):
    """The configurations named in `names` (all when it is empty), in the base order."""
    if not names:
        return cfgs
    known = [n for n, _ in cfgs]
    unknown = [n for n in names if n not in known]
    if unknown:
        sys.exit(f"unknown configurations {unknown}; these flags give {known}")
    return [c for c in cfgs if c[0] in names]


def unity_settings(out_dir):
    """Copies each player's DynamicsManager.asset (written by BuildPlayers) into out_dir once."""
    for solver in SOLVERS:
        src = unity_player(solver).parent / "DynamicsManager.asset"
        dst = out_dir / f"unity-{solver.lower()}-DynamicsManager.asset"
        if src.exists() and not dst.exists():
            shutil.copyfile(src, dst)


def cmd_round(a):
    cfgs = select(configs(a.unity, a.threads, a.isa), a.configs)
    missing = missing_programs(cfgs)
    if missing:
        sys.exit("not built (run `r1_bench.py build` with the same flags first):\n  "
                 + "\n  ".join(missing))
    out_dir = Path(a.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    machine(out_dir)
    if a.unity:
        unity_settings(out_dir)
    conditions(out_dir, f"round{a.round}")
    cfgs = [cfgs[i] for i in williams_row(len(cfgs), a.round - 1)]
    with open(out_dir / "runs.jsonl", "a", encoding="utf-8") as f:
        for scene in a.scenes:
            before = None
            for position, (name, build) in enumerate(cfgs):
                tag = f"r{a.round}-{scene}-{name}"
                d = run_one(build(scene, a.steps, out_dir / "logs" / f"{tag}.unity.log"), out_dir,
                            tag)
                after, before = before, name
                if d is None:
                    continue
                d = {"run": name, "round": a.round, "position": position, "after": after, **d}
                f.write(json.dumps(d) + "\n")
                f.flush()
                ray = f", rays {d['ray_mean_ms']:.2f} ms" if "ray_mean_ms" in d else ""
                print(f"{tag}: {d['mean_ms']:.3f} ms/step{ray}, CPU {d['cpu_busy_pct']}% "
                      f"(others {other_share(d)}%)", file=sys.stderr)


def cmd_run(a):
    if a.lock and not Path(a.lock).is_file():
        sys.exit(f"no lock script at {a.lock}")
    for k in range(a.first_round, a.first_round + a.rounds):
        if k > a.first_round and a.lock:
            time.sleep(ROUND_GAP_S)
        me = [sys.executable, str(Path(__file__).resolve()), "round", "--out", a.out, "--round",
              str(k), "--steps", str(a.steps), "--scenes", ",".join(a.scenes), "--threads",
              ",".join(map(str, a.threads))] + (["--unity"] if a.unity else []) + (
                  ["--isa"] if a.isa else []) + (
                  ["--configs", ",".join(a.configs)] if a.configs else [])
        if a.lock:
            me = [sys.executable, a.lock, "run", "--label", a.label, "--max-hold-min", "30",
                  "--"] + me
        subprocess.run(me, check=True)


def cmd_check(a):
    out_dir = Path(a.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    rows = []
    for scene, want in M5_HASHES_300.items():
        runs = [("det", 1), ("default", 1)] + [("det-parallel", t) for t in a.threads]
        runs += [(b, 1) for b in ISA_BUILDS if rapier_exe(b).exists()]
        for build, t in runs:
            if not rapier_exe(build).exists():
                sys.exit(f"not built: {rapier_exe(build)} (run `r1_bench.py build` first)")
            cmd = [rapier_exe(build), "--scene", scene, "--steps", "300"]
            if build == "det-parallel":
                cmd += ["--threads", t]
            d = run_one(cmd, out_dir, f"check-{scene}-{build}-t{t}")
            got = d["hash"] if d else None
            rows.append({"scene": scene, "build": build, "threads": t, "hash": got,
                         "m5_det_hash": want, "equal": got == want})
    with open(out_dir / "check.jsonl", "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
            print(f"{r['scene']} {r['build']} t{r['threads']}: {r['hash']} "
                  f"{'=' if r['equal'] else '!='} M5 det. {r['m5_det_hash']}")
    if a.unity:
        check_unity(out_dir)


def check_unity(out_dir):
    """Scene fidelity, untimed: every scene for UNITY_CHECK_STEPS steps on one thread, Rapier det.
    beside each PhysX player run with -probe (the settings and mass properties PhysX used), into
    check-unity.jsonl; prints the outcomes side by side and each kind of body's mass properties,
    and exits with status 1 if an end hash differs from UNITY_CHECK_HASHES (the quiet protocol's
    timing run must not start then)."""
    unity_settings(out_dir)
    rows = []
    for scene in SCENES:
        runs = [("rapier-det", [rapier_exe("det"), "--scene", scene, "--steps", UNITY_CHECK_STEPS])]
        for solver in SOLVERS:
            tag = f"check-{scene}-unity-{solver.lower()}"
            runs.append((f"unity-{solver.lower()}", [
                unity_player(solver), "-batchmode", "-nographics", "-logFile",
                out_dir / "logs" / f"{tag}.unity.log", "-job-worker-count", 0, "-physxbench",
                "-scene", scene, "-steps", UNITY_CHECK_STEPS, "-threads", 1, "-probe"]))
        for name, cmd in runs:
            if not Path(cmd[0]).exists():
                sys.exit(f"not built: {cmd[0]} (run `r1_bench.py build --unity` first)")
            d = run_one(cmd, out_dir, f"check-{scene}-{name}")
            if d is None:
                sys.exit(f"check run failed: {scene} {name}")
            rows.append({"check": name, **d})
    with open(out_dir / "check-unity.jsonl", "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    print(f"\nOutcomes after {UNITY_CHECK_STEPS} steps, one thread:")
    for r in rows:
        hits = f", ray hits {r['ray_hits']}, fraction sum {r['ray_fraction_sum']}" \
            if "ray_hits" in r else ""
        print(f"  {r['scene']:12} {r['check']:10} bodies {r['bodies']}, "
              f"active {r['active_at_end']}, top y {r['top_y']:.3f}, mean y {r['mean_y']:.3f}{hits}")
    print("\nPhysX's bodies (first of each kind; mass kg, principal inertia kg m^2, centre of mass "
          "m, size before the first step m):")
    for r in rows:
        if r["scene"] in ("Pyramid", "ConvexVsMesh") and "probe" in r:
            for kind, k in r["probe"]["kinds"].items():
                print(f"  {r['scene']:12} {r['check']:10} {kind:8} n {k['count']}, active "
                      f"{k['active_at_end']}, mean y {k['mean_y']}, mass {k['mass']}, inertia "
                      f"{k['inertia']}, com {k['center_of_mass']}, size {k.get('initial_size')}")
    print("\nEnd hashes against the recorded ones (UNITY_CHECK_HASHES):")
    wrong = 0
    for r in rows:
        want = UNITY_CHECK_HASHES.get((r["scene"], r["check"]))
        ok = r["hash"] == want
        wrong += not ok
        print(f"  {r['scene']:12} {r['check']:10} {r['hash']} {'=' if ok else '!='} {want}")
    if wrong:
        sys.exit(f"{wrong} end hash(es) differ from the recorded ones: the players or the scenes "
                 "changed; do not start the timing run")


def other_share(d):
    """Other processes' share of the machine (%) during a run, as the quiet rule reads it: a Unity
    player's own measure over its steps (steps_cpu_other_pct) where it has one, else the runner's
    over the whole process (cpu_other_pct, which for a player includes 2 to 3 s of start-up and
    teardown), else None (runs before 2026-10-16)."""
    v = d.get("steps_cpu_other_pct")
    return v if v is not None else d.get("cpu_other_pct")


def load_runs(out_dir):
    runs = []
    for line in (Path(out_dir) / "runs.jsonl").read_text(encoding="utf-8").splitlines():
        if line.strip():
            runs.append(json.loads(line))
    return runs


def med(xs):
    return statistics.median(xs) if xs else None


def label(run):
    names = {"rapier-det-t1": "Rapier det. (shipped), 1 thr.",
             "rapier-default-t1": "Rapier default, 1 thr."}
    if run in names:
        return names[run]
    for build, cpu in ISA_BUILDS.items():
        if run == f"rapier-{build}-t1":
            return f"Rapier det., {cpu}, 1 thr."
    if run.startswith("rapier-det-parallel-t"):
        return f"Rapier det.+parallel, {run.rsplit('-t', 1)[1]} thr."
    if run.startswith("unity-"):
        _, solver, t = run.split("-")
        return f"PhysX {solver.upper()}, {t[1:]} thr."
    return run


def cmd_summarize(a):
    sys.stdout.reconfigure(encoding="utf-8")
    runs = load_runs(a.out)
    if not runs:
        sys.exit(f"no runs in {a.out}/runs.jsonl")
    by = defaultdict(list)
    for d in runs:
        by[(d["run"], d["scene"])].append(d)
    run_names = list(dict.fromkeys(d["run"] for d in runs))
    scenes = [s for s in SCENES if any(d["scene"] == s for d in runs)]
    rounds = sorted({d["round"] for d in runs})
    busy = [d["cpu_busy_pct"] for d in runs if d.get("cpu_busy_pct") is not None]
    cpu = f"median {med(busy):.1f}%, range {min(busy)}–{max(busy)}%" if busy else "not recorded"
    others = [other_share(d) for d in runs if other_share(d) is not None]
    if others:
        cpu += (f"; other processes' share median {med(others):.1f}%, range "
                f"{min(others)}–{max(others)}%")
    print(f"Rounds {rounds[0]}–{rounds[-1]} ({len(rounds)}), {len(runs)} runs; whole-machine CPU "
          f"use during the runs: {cpu}.\n")

    print("### Step time\n")
    steps = sorted({d["steps"] for d in runs})
    print(f"Mean ms per step over {', '.join(map(str, steps))} steps: median over rounds "
          "(fastest–slowest round); p95 of the steps, median over rounds, in brackets.\n")
    print("| Configuration | " + " | ".join(scenes) + " |")
    print("|---|" + "---|" * len(scenes))
    for r in run_names:
        cells = []
        for s in scenes:
            ds = by.get((r, s), [])
            if not ds:
                cells.append("–")
                continue
            m = [d["mean_ms"] for d in ds]
            cells.append(f"{med(m):.2f} ({min(m):.2f}–{max(m):.2f}) "
                         f"[{med([d['p95_ms'] for d in ds]):.2f}]")
        print(f"| {label(r)} | " + " | ".join(cells) + " |")
    print()

    if "Raycast" in scenes:
        print("### Ray casts (Raycast scene, 10,000 closest-hit rays after every step)\n")
        print("| Configuration | ms per 10,000 rays, median (range) | Hits in the run |")
        print("|---|---|---|")
        for r in run_names:
            ds = [d for d in by.get((r, "Raycast"), []) if "ray_mean_ms" in d]
            if ds:
                m = [d["ray_mean_ms"] for d in ds]
                hits = sorted({d["ray_hits"] for d in ds})
                print(f"| {label(r)} | {med(m):.2f} ({min(m):.2f}–{max(m):.2f}) | "
                      f"{', '.join(map(str, hits))} |")
        print()

    pairs = [("rapier-det-t1", "rapier-default-t1", "Rapier det. / Rapier default, 1 thr."),
             ("rapier-det-parallel-t1", "rapier-det-t1", "det.+parallel / det., 1 thr.")]
    for build, cpu in ISA_BUILDS.items():
        pairs.append((f"rapier-{build}-t1", "rapier-det-t1", f"det. {cpu} / det. baseline, 1 thr."))
    for t in a.threads:
        if t != 1:
            pairs.append((f"rapier-det-parallel-t{t}", "rapier-det-t1",
                          f"det.+parallel {t} thr. / det. 1 thr."))
    for solver in ("pgs", "tgs"):
        pairs.append(("rapier-det-t1", f"unity-{solver}-t1",
                      f"Rapier det. / PhysX {solver.upper()}, 1 thr."))
        for t in a.threads:
            if t != 1:
                pairs.append((f"rapier-det-parallel-t{t}", f"unity-{solver}-t{t}",
                              f"Rapier det.+parallel / PhysX {solver.upper()}, {t} thr."))
    for t in a.threads:
        pairs.append((f"unity-tgs-t{t}", f"unity-pgs-t{t}", f"PhysX TGS / PGS, {t} thr."))
    for solver in ("pgs", "tgs"):
        for t in a.threads:
            if t != 1:
                pairs.append((f"unity-{solver}-t{t}", f"unity-{solver}-t1",
                              f"PhysX {solver.upper()} {t} thr. / 1 thr."))

    def quiet(d):
        """False for a run during which other processes used more than --max-other-cpu of the
        machine (other_share; runs from 2026-10-16 on record it), or, for older runs, a one-thread
        run above --max-cpu whole-machine CPU (the run itself keeps one of the 16 logical
        processors busy, about 6%). The bound is the same for every thread count, although a
        16-thread run leaves other processes no idle processor (docs/bench/physics-r1.md)."""
        other = other_share(d)
        if other is not None:
            return other <= a.max_other_cpu
        busy = d.get("cpu_busy_pct")
        return d.get("threads") != 1 or busy is None or busy <= a.max_cpu

    def ratio_rows(keep):
        rows = []
        for num, den, title in pairs:
            cells = []
            for s in scenes:
                n = {d["round"]: d["mean_ms"] for d in by.get((num, s), []) if keep(d)}
                q = {d["round"]: d["mean_ms"] for d in by.get((den, s), []) if keep(d)}
                rs = [n[k] / q[k] for k in sorted(n) if k in q and q[k] > 0]
                cells.append(f"{med(rs):.2f} (n={len(rs)}, {min(rs):.2f}–{max(rs):.2f})" if rs
                             else "–")
            if any(c != "–" for c in cells):
                rows.append(f"| {title} | " + " | ".join(cells) + " |")
        return rows

    head = "| Ratio | " + " | ".join(scenes) + " |\n|---|" + "---|" * len(scenes)
    if others:
        print("### Other processes during the runs\n")
        print("Share of the machine other processes used during each run (%; a Unity player's own "
              "measure over its steps, Rapier's over its process): median and maximum over every "
              f"scene and round, and the runs above --max-other-cpu {a.max_other_cpu:g}%.\n")
        print("| Configuration | median | max | runs above |")
        print("|---|---|---|---|")
        for r in run_names:
            v = [other_share(d) for d in runs if d["run"] == r and other_share(d) is not None]
            if v:
                above = sum(x > a.max_other_cpu for x in v)
                print(f"| {label(r)} | {med(v):.1f} | {max(v):.1f} | {above} of {len(v)} |")
        print()

    print("### Paired ratios of step time (within a round; median, n, range; above 1 means the "
          "first is slower)\n")
    print(head)
    print("\n".join(ratio_rows(lambda d: True)) + "\n")
    print(f"### Paired ratios, quiet pairs only (other processes at most {a.max_other_cpu:g}% of "
          f"the machine during each run; runs without that share: each one-thread run at most "
          f"{a.max_cpu:g}% whole-machine CPU)\n")
    print(head)
    print("\n".join(ratio_rows(quiet)) + "\n")

    print("### Outcomes (where the dynamic bodies ended; every round)\n")
    print("| Configuration | Scene | top y | mean y | active at end | end hashes over rounds |")
    print("|---|---|---|---|---|---|")
    for r in run_names:
        for s in scenes:
            ds = by.get((r, s), [])
            if not ds:
                continue
            span = lambda k: (f"{min(d[k] for d in ds):.2f}" if
                              min(d[k] for d in ds) == max(d[k] for d in ds) else
                              f"{min(d[k] for d in ds):.2f}–{max(d[k] for d in ds):.2f}")
            act = sorted({d["active_at_end"] for d in ds})
            hashes = sorted({d["hash"] for d in ds})
            h = f"`{hashes[0]}` (all {len(ds)})" if len(hashes) == 1 else f"{len(hashes)} distinct"
            print(f"| {label(r)} | {s} | {span('top_y')} | {span('mean_y')} | "
                  f"{', '.join(map(str, act))} | {h} |")
    print()


def cmd_unity_compile(a):
    data = Path(UNITY_EDITOR).parent / "Data"
    managed = data / "Managed" / "UnityEngine"
    out = Path(a.out) if a.out else Path(tempfile.mkdtemp(prefix="physxbench-csc-"))
    out.mkdir(parents=True, exist_ok=True)
    scripts = UNITY_PROJECT / "Assets" / "PhysxBench"
    refs = [data / "NetStandard" / "ref" / "2.1.0" / "netstandard.dll",
            managed / "UnityEngine.CoreModule.dll", managed / "UnityEngine.PhysicsModule.dll"]
    csc = [data / "NetCoreRuntime" / f"dotnet{EXE}", data / "DotNetSdkRoslyn" / "csc.dll",
           "-nologo", "-noconfig", "-nostdlib", "-langversion:9", "-target:library",
           "-warnaserror+"]
    for name, extra, sources in (
            ("PhysxBench", [], [scripts / "PhysxBench.cs"]),
            ("PhysxBenchEditor", [managed / "UnityEditor.CoreModule.dll", out / "PhysxBench.dll"],
             [scripts / "Editor" / "PhysxBenchEditor.cs"])):
        cmd = csc + [f"-out:{out / (name + '.dll')}"] + [f"-r:{r}" for r in refs + extra] + sources
        p = subprocess.run([str(c) for c in cmd], capture_output=True, text=True)
        print(f"{name}: {'OK' if p.returncode == 0 else 'FAILED'}\n{p.stdout}{p.stderr}".strip())
        if p.returncode != 0:
            sys.exit(1)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    csv_scenes = lambda s: [x for x in s.split(",") if x]
    csv_ints = lambda s: [int(x) for x in s.split(",") if x]

    def common(p, out_positional=False):
        if out_positional:
            p.add_argument("out")
        else:
            p.add_argument("--out", required=True)
        p.add_argument("--threads", type=csv_ints, default=THREADS)

    p = sub.add_parser("build")
    p.add_argument("--unity", action="store_true")
    p.add_argument("--isa", action="store_true", help="also the det. build for x86-64-v2 and v3")
    p.set_defaults(fn=cmd_build)
    for name, fn in (("round", cmd_round), ("run", cmd_run)):
        p = sub.add_parser(name)
        common(p)
        p.add_argument("--steps", type=int, default=500)
        p.add_argument("--scenes", type=csv_scenes, default=SCENES)
        p.add_argument("--unity", action="store_true")
        p.add_argument("--isa", action="store_true",
                       help="add the det. x86-64-v2 and v3 builds (one thread)")
        p.add_argument("--configs", type=csv_scenes, default=[],
                       help="run only these configurations (names as in runs.jsonl's \"run\"), "
                            "in the Williams order of their own count")
        if name == "round":
            p.add_argument("--round", type=int, required=True)
        else:
            p.add_argument("--rounds", type=int, default=5)
            p.add_argument("--first-round", type=int, default=1)
            p.add_argument("--lock", help="bench_lock.py to hold for each round")
            p.add_argument("--label", default="physx")
        p.set_defaults(fn=fn)
    p = sub.add_parser("check")
    common(p)
    p.add_argument("--unity", action="store_true",
                   help="also the scene-fidelity check of the PhysX players against Rapier det.")
    p.set_defaults(fn=cmd_check)
    p = sub.add_parser("summarize")
    common(p, out_positional=True)
    p.add_argument("--max-cpu", type=float, default=15.0,
                   help="whole-machine CPU %% above which a one-thread run without "
                        "cpu_other_pct leaves the quiet pairs")
    p.add_argument("--max-other-cpu", type=float, default=9.0,
                   help="other processes' share of the machine (%%) above which a run leaves the "
                        "quiet pairs")
    p.set_defaults(fn=cmd_summarize)
    p = sub.add_parser("unity-compile")
    p.add_argument("--out", help="directory for the compiled DLLs (default: a temporary one)")
    p.set_defaults(fn=cmd_unity_compile)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
