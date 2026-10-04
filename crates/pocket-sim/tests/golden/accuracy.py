#!/usr/bin/env python3
"""Writes accuracy.txt beside this file: inputs and their correctly rounded results for every
function of pocket_sim::math (docs/spec/numeric.md 6.2 and 10, `math.accuracy`), computed with
mpmath 1.3.0 at 200 bits and rounded to the nearest double. Offline tooling: the test reads the
table and never runs Python.

    PYTHONPATH=<dir holding mpmath 1.3.0> python3 accuracy.py

Each line is `name x [y] result`, every number the 16 hex digits of a double's bits. Inputs come
from Python's `random` seeded per function, over the sampled ranges of numeric.md 6.2.
"""
import math
import os
import random
import struct

import mpmath

mpmath.mp.prec = 200
PER_FUNCTION = 120


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def hexd(x):
    return f"{bits(x):016x}"


def uniform(r, lo, hi):
    return lo + (hi - lo) * r.random()


def log_uniform(r, lo, hi):
    return math.ldexp(1.0 + r.random(), r.randrange(lo, hi))


M = mpmath
UNARY = [
    ("sin", lambda r: uniform(r, -2 * math.pi, 2 * math.pi), M.sin),
    ("cos", lambda r: uniform(r, -2 * math.pi, 2 * math.pi), M.cos),
    ("sin", lambda r: uniform(r, -1e9, 1e9), M.sin),
    ("tan", lambda r: uniform(r, -1.5, 1.5), M.tan),
    ("asin", lambda r: uniform(r, -1, 1), M.asin),
    ("acos", lambda r: uniform(r, -1, 1), M.acos),
    ("atan", lambda r: uniform(r, -1e3, 1e3), M.atan),
    ("sinh", lambda r: uniform(r, -10, 10), M.sinh),
    ("cosh", lambda r: uniform(r, -10, 10), M.cosh),
    ("tanh", lambda r: uniform(r, -10, 10), M.tanh),
    ("asinh", lambda r: uniform(r, -1e3, 1e3), M.asinh),
    ("acosh", lambda r: uniform(r, 1, 1e3), M.acosh),
    ("atanh", lambda r: uniform(r, -0.999, 0.999), M.atanh),
    ("exp", lambda r: uniform(r, -50, 50), M.exp),
    ("exp2", lambda r: uniform(r, -50, 50), lambda x: M.power(2, x)),
    ("expm1", lambda r: uniform(r, -5, 5), M.expm1),
    ("ln", lambda r: log_uniform(r, -34, 34), M.log),
    ("log2", lambda r: log_uniform(r, -34, 34), lambda x: M.log(x, 2)),
    ("log10", lambda r: log_uniform(r, -34, 34), M.log10),
    ("ln_1p", lambda r: uniform(r, -0.9, 10), M.log1p),
    ("cbrt", lambda r: uniform(r, -1e6, 1e6), lambda x: M.sign(x) * M.cbrt(abs(x))),
    ("sqrt", lambda r: log_uniform(r, -20, 20), M.sqrt),
]
BINARY = [
    ("atan2", lambda r: (uniform(r, -100, 100), uniform(r, -100, 100)), M.atan2),
    ("pow", lambda r: (log_uniform(r, -7, 7), uniform(r, -10, 10)), M.power),
    ("hypot", lambda r: (uniform(r, -1e3, 1e3), uniform(r, -1e3, 1e3)), M.hypot),
]


def main():
    lines = ["# name x [y] result: bits of doubles; from accuracy.py (mpmath 1.3.0, 200 bits)"]
    for i, (name, gen, f) in enumerate(UNARY):
        r = random.Random(f"{name}-{i}")
        for _ in range(PER_FUNCTION):
            x = gen(r)
            lines.append(f"{name} {hexd(x)} {hexd(float(f(M.mpf(x))))}")
    for name, gen, f in BINARY:
        r = random.Random(name)
        for _ in range(PER_FUNCTION):
            x, y = gen(r)
            lines.append(f"{name} {hexd(x)} {hexd(y)} {hexd(float(f(M.mpf(x), M.mpf(y))))}")
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "accuracy.txt")
    with open(out, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
