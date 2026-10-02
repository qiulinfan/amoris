"""Summarise a `pocket build --json` report: errors per file, then the first few of each.

    python3 tools/scripts/dev/build_errors.py build-release.json [--all]
"""
import collections
import json
import os
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..")).lower()
print(report["ok"], report["summary"][:400])
errors = [d for d in report.get("diagnostics", []) if d["severity"] in ("error", "fatal error")]
print(len(errors), "errors")


def rel(path):
    p = (path or "?").replace("\\", "/")
    r = root.replace("\\", "/")
    return p[len(r) + 1:] if p.lower().startswith(r) else p


by_file = collections.defaultdict(list)
for d in errors:
    by_file[rel(d.get("file"))].append(d)
for f, ds in sorted(by_file.items(), key=lambda kv: -len(kv[1])):
    print(f"{len(ds):4} {f}")
    for d in ds[: (None if "--all" in sys.argv else 3)]:
        print(f"       {d.get('line')}: {d['message'][:160]}")
if not errors:
    print(report.get("data", {}).get("ninja_output_tail", "")[-3000:])
