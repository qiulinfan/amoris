#!/usr/bin/env python3
"""Losslessly re-encode an existing engine-capture WebP for browser decoding.

This only changes the animation encoding. ImageMagick coalesces its displayed frames and
img2webp encodes their pixels losslessly. The default duration matches the showcase loops.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("source", type=Path)
    p.add_argument("output", type=Path)
    p.add_argument("--frame-ms", type=int, default=50)
    args = p.parse_args()
    if args.frame_ms <= 0:
        p.error("frame-ms must be positive")
    work = ROOT / "out"
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="loop-repack-", dir=work) as temp:
        frames = Path(temp)
        subprocess.run(["magick", str(args.source.resolve()), "-coalesce",
                        str(frames / "%04d.png")], check=True)
        images = sorted(frames.glob("*.png"))
        if not images:
            raise SystemExit("The source has no decoded frames")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["img2webp", "-kmax", "0", "-loop", "0", "-lossless", "-d",
                        str(args.frame_ms), *map(str, images), "-o", str(args.output.resolve())], check=True)


if __name__ == "__main__":
    main()
