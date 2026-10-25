#!/usr/bin/env python3
"""Compare linear float32 RGBA path-tracer outputs, never tone-mapped previews."""
import argparse
from array import array
import json
import math
from pathlib import Path
import sys


def load(path):
    raw = Path(path).read_bytes()
    if len(raw) % 16:
        raise ValueError(f"{path}: expected float32 RGBA pixels")
    values = array("f")
    values.frombytes(raw)
    if sys.byteorder != "little":
        values.byteswap()
    rgb = [v for i, v in enumerate(values) if i % 4 != 3]
    if not rgb or not all(math.isfinite(v) for v in rgb):
        raise ValueError(f"{path}: empty or non-finite radiance")
    return rgb


def compare(reference, candidate):
    if len(reference) != len(candidate):
        raise ValueError("image dimensions differ")
    errors = [b - a for a, b in zip(reference, candidate)]
    mse = math.fsum(e*e for e in errors) / len(errors)
    peak = max(reference)
    rms_reference = math.sqrt(math.fsum(x*x for x in reference) / len(reference))
    mean = lambda values: [math.fsum(values[c::3]) / (len(values) // 3) for c in range(3)]
    return {
        "pixels": len(reference) // 3,
        "rmse": math.sqrt(mse),
        "relative_rmse": math.sqrt(mse) / max(rms_reference, 1e-12),
        "maximum_absolute_error": max(abs(e) for e in errors),
        "reference_peak": peak,
        "psnr_reference_peak_db": 10*math.log10(peak*peak / mse) if mse > 0 and peak > 0 else None,
        "reference_mean_rgb": mean(reference),
        "candidate_mean_rgb": mean(candidate),
        "mean_error_rgb": mean(errors),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference")
    parser.add_argument("candidate")
    parser.add_argument("--output")
    args = parser.parse_args()
    result = dict(reference=args.reference, candidate=args.candidate, **compare(load(args.reference), load(args.candidate)))
    text = json.dumps(result, indent=2, allow_nan=False) + "\n"
    if args.output:
        Path(args.output).write_text(text)
    print(text, end="")
