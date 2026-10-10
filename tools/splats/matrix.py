"""The splat rasterizers against each other over rounds, backends and adapters (docs/bench/splats.md).

Every round runs every scene with both rasterizers on every adapter and backend, so a disturbance
or a thermal drift spreads over all configurations instead of landing on one; within a round the
rasterizer that goes first alternates by round. Each run is one `splats --headless-bench FRAMES`
process (20 warm-up frames, then FRAMES timed, per-pass GPU timestamps averaged), parsed by ab.py.
`--cool C` waits before every round until the NVIDIA GPU is at C degrees or below (nvidia-smi);
`--settle S` passes the example's `--settle S` (wait after start-up before the first frame: a
start-up that loads every core lowers the laptop GPU's clocks for a few seconds, dx12.md 10.6).

Build first: `cargo build --release -p pocket-render --example splats`. From the repository root:

    python tools/splats/matrix.py --adapters nvidia,780m --backends dx12,vulkan --rounds 5 \
        --frames 60 --scene "1M=--count 1000000" --scene "3M 1440p=--count 3000000 --size 2560x1440" \
        --out docs/evidence/quiet/splats/matrix.json

`--build NAME=EXE` (repeatable) times other builds of the example too, interleaved within each
round (for example a copy of an earlier build's `splats.exe`), and `--rasters tile` times one
rasterizer only. `python tools/quiet_tables.py splatm FILE` tabulates the result.
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ab  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent.parent
EXE = ".exe" if os.name == "nt" else ""


def gpu_temp():
    try:
        p = subprocess.run(["nvidia-smi", "--query-gpu=temperature.gpu", "--format=csv,noheader"],
                           capture_output=True, text=True, timeout=30)
        return int(p.stdout.strip().splitlines()[0])
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired):
        return None


def cool(limit):
    t0 = time.time()
    while (t := gpu_temp()) is not None and t > limit and time.time() - t0 < 900:
        time.sleep(5)
    return gpu_temp(), round(time.time() - t0, 1)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", action="append", default=[], metavar="NAME=EXE",
                    help="time this build of the example (repeatable; default: this checkout's)")
    ap.add_argument("--rasters", default="quad,tile")
    ap.add_argument("--adapters", default="nvidia,780m")
    ap.add_argument("--backends", default="dx12,vulkan")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--frames", type=int, default=60)
    ap.add_argument("--scene", action="append", required=True, metavar="NAME=ARGS")
    ap.add_argument("--cool", type=float, default=0.0)
    ap.add_argument("--settle", type=float, default=0.0)
    ap.add_argument("--note", default="")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    builds = [tuple(b.split("=", 1)) for b in a.build] or [
        ("this", str(ROOT / "target" / "release" / "examples" / f"splats{EXE}"))]
    builds = [(name, os.path.abspath(exe)) for name, exe in builds]
    rasters = a.rasters.split(",")
    scenes = [s.split("=", 1) for s in a.scene]
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    doc = {"tool": "tools/splats/matrix.py", "date": time.strftime("%Y-%m-%d %H:%M"),
           "note": a.note, "frames": a.frames, "settle_s": a.settle, "scenes": dict(scenes),
           "rounds": a.rounds, "builds": dict(builds), "cooling": [], "runs": []}
    for rnd in range(a.rounds):
        if a.cool:
            temp, waited = cool(a.cool)
            doc["cooling"].append({"round": rnd, "temp_c": temp, "waited_s": waited})
            print(f"round {rnd}: RTX {temp} C after {waited} s", file=sys.stderr, flush=True)
        order = [(r, b) for r in rasters for b in builds]
        if rnd % 2:
            order.reverse()
        for adapter in a.adapters.split(","):
            for backend in a.backends.split(","):
                env = dict(os.environ, POCKET_BACKEND=backend, POCKET_ADAPTER=adapter)
                for name, args in scenes:
                    for raster, (build, exe) in order:
                        t = time.time()
                        extra = f" --settle {a.settle}" if a.settle else ""
                        r = ab.run(exe, args + extra, raster, a.frames, env)
                        if len(builds) > 1:
                            r["build"] = build
                        r.update({"round": rnd, "requested_adapter": adapter,
                                  "requested_backend": backend, "scene": name, "args": args,
                                  "raster": raster, "temp_after_c": gpu_temp(),
                                  "seconds": round(time.time() - t, 1)})
                        doc["runs"].append(r)
                        print(f"r{rnd} {adapter:6} {backend:6} {name:16} {raster} {r.get('build', '')} splats "
                              f"{r['splat_total']:7.3f} ms GPU {r['gpu_total'] or 0:7.3f} ms "
                              f"({r['seconds']} s, {r['temp_after_c']} C)", file=sys.stderr,
                              flush=True)
                        # Written after every run: an interrupted suite keeps what it measured.
                        out.write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8",
                                       newline="\n")


if __name__ == "__main__":
    main()
