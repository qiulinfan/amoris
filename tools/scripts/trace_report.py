#!/usr/bin/env python3
"""What a benchmark run's agents spent their context on (docs/agent-eval.md, Results): reads the
event streams a runner keeps with POCKET_AGENT_TRACES=<dir> (one <task>.jsonl each; opencode's
format, as tools/scripts/runners/opencode_agent.py writes it) and prints, per task, the turns, the
tokens and the tool calls, and over the run where the tools' output went (by tool and command),
which files were read and how much of them, and the calls that failed.

    python3 tools/scripts/trace_report.py build/agent-traces/<run> [--task NAME] [--json]

The context an agent carries grows with what it reads: every later turn sends it again, so a large
early read costs once per turn after it. That is what to look for here.
"""
import argparse
import collections
import glob
import json
import os
import sys


def events(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def tool_key(part):
    tool = part.get("tool", "?")
    inp = (part.get("state") or {}).get("input") or {}
    if tool.endswith("runtime_command"):
        return f"{tool}:{inp.get('method') or ('calls' if 'calls' in inp else '?')}"
    return tool


def output_size(state):
    o = state.get("output")
    if isinstance(o, str):
        return len(o)
    return len(json.dumps(o)) if o is not None else 0


def task_report(path):
    turns = 0
    tokens = {"input": 0, "cache_read": 0, "output": 0}
    first_context = None
    calls = []
    for e in events(path):
        kind = e.get("type")
        part = e.get("part") or {}
        if kind == "tool_use":
            st = part.get("state") or {}
            inp = st.get("input") or {}
            calls.append({
                "tool": tool_key(part),
                "bytes": output_size(st),
                "error": st.get("status") == "error",
                "file": inp.get("filePath") or "",
                "input": json.dumps(inp)[:120],
            })
        elif kind == "step_finish":
            turns += 1
            t = part.get("tokens") or {}
            tokens["input"] += t.get("input", 0)
            tokens["cache_read"] += (t.get("cache") or {}).get("read", 0)
            tokens["output"] += t.get("output", 0)
            if first_context is None:
                first_context = t.get("input", 0) + (t.get("cache") or {}).get("read", 0)
    return {"task": os.path.splitext(os.path.basename(path))[0], "turns": turns, "tokens": tokens,
            "total_tokens": sum(tokens.values()), "first_context": first_context or 0, "calls": calls}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("traces", help="the directory POCKET_AGENT_TRACES pointed at")
    ap.add_argument("--task", help="one task's calls in order")
    ap.add_argument("--json", action="store_true", help="the reports as JSON")
    a = ap.parse_args()
    paths = sorted(glob.glob(os.path.join(a.traces, "*.jsonl")))
    if a.task:
        paths = [p for p in paths if os.path.basename(p) == f"{a.task}.jsonl"]
    if not paths:
        sys.exit(f"no traces in {a.traces}")
    reports = [task_report(p) for p in paths]
    if a.json:
        print(json.dumps(reports, indent=1))
        return
    if a.task:
        r = reports[0]
        print(f"{r['task']}: {r['turns']} turns, {r['total_tokens']:,} tokens, {len(r['calls'])} calls")
        for i, c in enumerate(r["calls"], 1):
            print(f"{i:3} {'ERR ' if c['error'] else '    '}{c['bytes']:>7}  {c['tool']}  {c['input']}")
        return
    print(f"{'task':<16} {'turns':>5} {'calls':>5} {'tokens':>11} {'read KB':>8}  biggest output")
    by_tool = collections.Counter()
    count_tool = collections.Counter()
    by_file = collections.Counter()
    failed = []
    for r in sorted(reports, key=lambda r: -r["total_tokens"]):
        read = sum(c["bytes"] for c in r["calls"] if c["tool"] in ("read", "grep"))
        big = max(r["calls"], key=lambda c: c["bytes"], default=None)
        print(f"{r['task']:<16} {r['turns']:>5} {len(r['calls']):>5} {r['total_tokens']:>11,} {read / 1024:>8.1f}  " + (f"{big['bytes'] / 1024:.1f} KB {big['tool']} {os.path.basename(big['file'])}" if big else ""))
        for c in r["calls"]:
            by_tool[c["tool"]] += c["bytes"]
            count_tool[c["tool"]] += 1
            if c["tool"] == "read" and c["file"]:
                by_file["/".join(c["file"].split("/")[-2:])] += c["bytes"]
            if c["error"]:
                failed.append((r["task"], c["tool"], c["input"]))
    total = sum(r["total_tokens"] for r in reports)
    contexts = sorted(r["first_context"] for r in reports if r["first_context"])
    print(f"\n{len(reports)} tasks, {sum(r['turns'] for r in reports)} turns, {total:,} tokens; a first turn's context {contexts[len(contexts) // 2]:,} tokens (median)" if contexts else "")
    print("\ntool output by tool (KB, calls, KB a call):")
    for k, v in by_tool.most_common(15):
        print(f"  {v / 1024:>8.1f} {count_tool[k]:>5} {v / 1024 / max(count_tool[k], 1):>7.1f}  {k}")
    print("\nfiles read (KB):")
    for k, v in by_file.most_common(12):
        print(f"  {v / 1024:>8.1f}  {k}")
    if failed:
        print(f"\nfailed calls ({len(failed)}):")
        for t, tool, inp in failed[:20]:
            print(f"  {t}: {tool} {inp}")


if __name__ == "__main__":
    main()
