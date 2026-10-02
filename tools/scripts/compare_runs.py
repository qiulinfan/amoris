#!/usr/bin/env python3
"""Two benchmark runs side by side (docs/agent-eval.md, Results): per task in both, whether it
passed, its seconds, tool calls and tokens in each and the change, then the totals over the tasks
both ran. One run each says how much runs vary as much as what changed; read it with that in mind.

    python3 tools/scripts/compare_runs.py tests/evidence/agent-eval/opencode-glm-full55.json build/agent-traces/full57/report.json
"""
import json
import sys


def load(path):
    r = json.load(open(path))
    return {t["name"]: t for t in r["tasks"]}


def stats(t):
    m = t.get("metrics") or {}
    return t["ok"], t.get("seconds", 0), m.get("tool_calls") or 0, (m.get("tokens") or {}).get("total") or 0


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    a, b = load(sys.argv[1]), load(sys.argv[2])
    both = [n for n in a if n in b]
    print(f"{'task':<18} {'pass':>9} {'seconds':>15} {'calls':>11} {'tokens (k)':>17}")
    tot = [[0, 0, 0, 0], [0, 0, 0, 0]]
    for n in both:
        sa, sb = stats(a[n]), stats(b[n])
        for i, s in enumerate((sa, sb)):
            tot[i][0] += s[0]
            tot[i][1] += s[1]
            tot[i][2] += s[2]
            tot[i][3] += s[3]
        mark = lambda ok: "ok" if ok else "FAIL"  # noqa: E731
        print(f"{n:<18} {mark(sa[0]):>4}/{mark(sb[0]):<4} {sa[1]:>6.0f} {sb[1]:>6.0f} {sb[1] - sa[1]:>+5.0f}"
              f" {sa[2]:>4} {sb[2]:>4} {sb[2] - sa[2]:>+3} {sa[3] / 1000:>6.0f} {sb[3] / 1000:>6.0f} {(sb[3] - sa[3]) / 1000:>+6.0f}")
    print(f"\n{len(both)} tasks in both: passed {tot[0][0]} -> {tot[1][0]}, {tot[0][1] / 60:.0f} -> {tot[1][1] / 60:.0f} minutes, "
          f"{tot[0][2]} -> {tot[1][2]} calls, {tot[0][3] / 1e6:.1f} -> {tot[1][3] / 1e6:.1f} M tokens")
    only = [n for n in b if n not in a]
    if only:
        print("only in the second:", ", ".join(only))


if __name__ == "__main__":
    main()
