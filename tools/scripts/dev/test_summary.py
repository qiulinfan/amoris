#!/usr/bin/env python3
"""Summarize a `pocket test --json` report (AGENTS.md, Development helpers): the verdict, the
modules, scenarios and TypeScript tests passed, and each failure with the tail of its output.

    python3 tools/scripts/dev/test_summary.py build/test-reports/latest.json

The file may hold build noise around the report; the last test report in it is the one read.
"""
import json
import sys


def last_report(text):
    dec = json.JSONDecoder()
    found, i = None, 0
    while True:
        j = text.find("{", i)
        if j < 0:
            return found
        try:
            obj, end = dec.raw_decode(text, j)
            if isinstance(obj, dict) and obj.get("command") == "test":
                found = obj
            i = end
        except ValueError:
            i = j + 1


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    rep = last_report(open(sys.argv[1], encoding="utf-8").read())
    if rep is None:
        print("no test report found")
        return 1
    data = rep.get("data", {})
    print("ok", rep.get("ok"), rep.get("summary"))

    def count(key):
        xs = data.get(key) or []
        return f"{sum(1 for x in xs if x.get('ok'))} / {len(xs)}"

    print("results", count("results"), "scenarios", count("scenarios"), "ts", count("ts"),
          "tool", (data.get("tool") or {}).get("ok"), "python", (data.get("python") or {}).get("ok"))
    for key in ("results", "scenarios", "ts"):
        for x in data.get(key) or []:
            if not x.get("ok"):
                print("FAIL", key, x.get("module"), json.dumps(x, ensure_ascii=False)[:1500])
    for key in ("tool", "python"):
        x = data.get(key) or {}
        if x and not x.get("ok"):
            print("FAIL", key, json.dumps(x, ensure_ascii=False)[:1500])
    return 0 if rep.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
