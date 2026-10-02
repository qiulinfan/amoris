"""Summarise a Ninja log run with -k 0: each failed step and its first errors.

    ninja -C build/release -k 0 > build/ninja.txt 2>&1
    python3 tools/scripts/dev/ninja_failures.py build/ninja.txt [lines-per-step]
"""
import os
import sys

text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
limit = int(sys.argv[2]) if len(sys.argv) > 2 else 14
root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
prefixes = [root + os.sep, root.replace("\\", "/") + "/"]
steps = text.split("FAILED: ")[1:]
print(len(steps), "failed steps")
for step in steps:
    lines = step.splitlines()
    head = lines[0]
    for p in prefixes:
        head = head.replace(p, "")
    print("=====", head[:160])
    shown = 0
    for line in lines[1:]:
        if shown >= limit:
            break
        if "error" in line or "undefined symbol" in line or "referenced by" in line:
            for p in prefixes:
                line = line.replace(p, "")
            print("   ", line[:240])
            shown += 1
