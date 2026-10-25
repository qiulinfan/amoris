#!/usr/bin/env python3
"""Measures how much the water shimmers (docs/spec/water.md 68, docs/bench/liquids.md 20): draws a
project every tick from a fixed camera with the amoris-app example `scene_shots`, then counts the
pixels whose luminance flips, changing by more than 0.02 one way and then back the next frame, in
more than 30 and more than 50 percent of the frames, with the mean share of flipping frames and the
mean second difference of luminance. Water moving smoothly changes a pixel over several frames;
shimmer flips it.

Switches go to the drawing process's environment (`--env AMORIS_CAUSTICS=off`, `--env
AMORIS_RIPPLES=off`, `--env AMORIS_AA=msaa+taa`), so one cause at a time can be left out. Needs numpy
and ffmpeg (which decodes the frames). Build first: `cargo build --release -p amoris-app --example
scene_shots`. From the repository's root:

    python tools/water/water_flicker.py samples/pool-party --eye -2,1.8,-1.5 --at 8,-0.5,4 \\
        --from 600 --to 660 --box 380,280,1040,560 --out out/flicker/pool
"""
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""


def frames(out, first, last):
    """The frames `scene_shots` wrote, as luminance (frames, height, width)."""
    lum = []
    for tick in range(first, last + 1):
        path = out / f"f-{tick:05d}.png"
        probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                                "stream=width,height", "-of", "csv=p=0", str(path)],
                               capture_output=True, text=True, check=True).stdout
        w, h = (int(v) for v in probe.strip().split(",")[:2])
        raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-f", "rawvideo",
                              "-pix_fmt", "rgb24", "-"], capture_output=True, check=True).stdout
        rgb = np.frombuffer(raw, np.uint8).reshape(h, w, 3).astype(np.float32) / 255.0
        lum.append(rgb @ np.array([0.2126, 0.7152, 0.0722], np.float32))
    return np.stack(lum)


def measure(lum):
    d1 = lum[1:] - lum[:-1]
    flips = ((d1[1:] * d1[:-1]) < 0) & (np.abs(d1[1:]) > 0.02) & (np.abs(d1[:-1]) > 0.02)
    rate = flips.mean(axis=0)
    second = np.abs(lum[2:] - 2.0 * lum[1:-1] + lum[:-2]).mean(axis=0)
    return {"over_30": int((rate > 0.3).sum()), "over_50": int((rate > 0.5).sum()),
            "flip_rate": float(rate.mean()), "second_difference": float(second.mean())}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("project")
    ap.add_argument("--eye", required=True, help="x,y,z")
    ap.add_argument("--at", required=True, help="x,y,z")
    ap.add_argument("--from", dest="first", type=int, default=600)
    ap.add_argument("--to", dest="last", type=int, default=660)
    ap.add_argument("--box", help="x0,y0,x1,y1: only these pixels count")
    ap.add_argument("--env", action="append", default=[], help="NAME=VALUE for the drawing process")
    ap.add_argument("--out", default="out/flicker/run", help="where the frames go (kept)")
    a = ap.parse_args()
    missing = [tool for tool in ("ffmpeg", "ffprobe") if shutil.which(tool) is None]
    if missing:
        sys.exit(f"{' and '.join(missing)} not found on PATH: install ffmpeg")
    out = Path(a.out)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    env = dict(os.environ)
    for item in a.env:
        name, _, value = item.partition("=")
        env[name] = value
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    shots = target / "release" / "examples" / f"scene_shots{EXE}"
    if not shots.exists():
        sys.exit(f"{shots} is missing: cargo build --release -p amoris-app --example scene_shots")
    subprocess.run([str(shots), a.project, "--out", str(out), "--name", "f", "--eye", a.eye, "--at",
                    a.at, "--every", "1", "--from", str(a.first), "--to", str(a.last)],
                   env=env, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    lum = frames(out, a.first, a.last)
    if a.box:
        x0, y0, x1, y1 = (int(v) for v in a.box.split(","))
        lum = lum[:, y0:y1, x0:x1]
    m = measure(lum)
    print(f"{len(lum)} frames: pixels flipping in over 30% of frames {m['over_30']}, over 50% "
          f"{m['over_50']}; mean flip rate {m['flip_rate']:.4f}; mean second difference "
          f"{m['second_difference']:.4f}")


if __name__ == "__main__":
    main()
