#!/usr/bin/env python3
"""A runner for the agent benchmark (tools/scripts/agent_eval.py, docs/agent-eval.md) that hands
each task to a pi-family coding agent: oh-my-pi (`omp`) or pi (`pi`), with whatever model it is
set up for (--model to pick one). It prints the agent's answer with what the task cost: tokens,
money, tool calls, turns and seconds.

    python3 tools/scripts/agent_eval.py --timeout 900 \\
        --runner "python3 tools/scripts/runners/pi_agent.py --agent omp --via mcp"

How the agent reaches the engine (--via):
    mcp        the `pocket` MCP server (`pocket mcp`, attached to the harness's runtime through
               POCKET_RPC_URL), declared in a .mcp.json the runner writes where the agent starts
    shell      `pocket rpc` in the shell
    extension  pi's own tools from integrations/pi/pocket.ts (`pocket`, `pocket_look`)

The agent starts in the project copy for a task that edits files and in an empty directory for the
rest, never in the repository, so nothing it writes can land in the engine's sources. The harness
puts the copies and the documentation outside the repository; a call that still reaches into the
harness or its evidence (tools/scripts, where the checks are, and tests/evidence/agent-eval, where
earlier answers are) is reported under `peeked`, and such a run's pass says nothing about the engine.
The agent's other reading in the repository (the SDK's or the tool's sources) is not counted.
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--agent", default="omp", help="omp or pi (or a path to one)")
    ap.add_argument("--via", default="mcp", choices=["mcp", "shell", "extension"])
    ap.add_argument("--model", help="model pattern, e.g. deepseek/deepseek-v4-flash (default: the agent's own)")
    ap.add_argument("--thinking", help="thinking level: off, low, medium, high")
    ap.add_argument("--max-time", type=int, default=600, help="seconds the agent may take")
    ap.add_argument("--env-file", help="a KEY=value file (such as ~/.omp/agent/.env) loaded into the agent's environment; its values are passed on, never printed")
    a = ap.parse_args()

    payload = json.load(sys.stdin)
    project_dir = payload.get("project_dir") or ""
    edits_files = "This task edits files" in payload.get("notes", "")
    pocket = os.path.join(ROOT, ".pocket", "pocket")
    docs = [os.path.join(ROOT, d) for d in payload.get("docs", [])]
    if a.via == "mcp":
        how = ("Reach the engine through the `pocket` MCP server's tools: runtime_command {method, params} sends any command "
               "(runtime_commands lists them); world_tree, world_query, world_describe, step, events_since, transcript and capture are "
               "shortcuts. The server is already attached to the running runtime; do not start or stop a runtime.")
    elif a.via == "shell":
        how = (f"Reach the engine with `{pocket} rpc <method> '<params as JSON>'` in the shell: it sends the command to the running "
               f"runtime (POCKET_RPC_URL is set) and prints the result. `{pocket} rpc commands` lists every method.")
    else:
        how = "Reach the engine with the `pocket` tool (a method and its params); `pocket_look` shows the current frame."
    where = f"The project's files are in {project_dir}." if edits_files else "The task is done through commands on the running runtime; do not edit the project's files."
    prompt = (
        "You are working on a game made with Pocket, an engine that an AI agent drives through commands.\n"
        f"A runtime of the project '{payload['project']}' is running, paused, with its control server at {payload['rpc_url']}.\n\n"
        f"Task: {payload['task']}\n\n"
        f"{payload.get('notes', '')}\n{where}\n{how}\n"
        f"The engine's documentation, to read as you need: {', '.join(docs)}. `{pocket} docs <topic>` "
        f"(the MCP tool docs_search) answers the few sections on a topic, which is cheaper than reading a whole file.\n"
        "Do only what the task asks. Step the simulation only if the task needs it.\n"
        'When you are done, end your final message with one line of JSON: {"answer": <the answer the task asks for, or null>}.'
    )

    scratch = None
    if edits_files and project_dir:
        cwd = project_dir
    else:
        scratch = tempfile.mkdtemp(prefix="pocket-agent-")
        cwd = scratch
    if a.via == "mcp":
        server = {"command": pocket, "args": ["--root", ROOT, "mcp"]}
        with open(os.path.join(cwd, ".mcp.json"), "w", encoding="utf-8", newline="\n") as f:
            json.dump({"mcpServers": {"pocket": server}}, f, indent=2)
    agent = shutil.which(a.agent) or a.agent
    is_omp = os.path.basename(agent).startswith("omp")
    cmd = [agent, "-p", "--no-session", "--mode", "json"]
    if is_omp:
        cmd += ["--no-title", "--auto-approve", "--max-time", str(a.max_time), "--cwd", cwd]
        if project_dir and cwd != project_dir:
            cmd += ["--add-dir", project_dir]
    if a.via == "extension":
        cmd += ["-e", os.path.join(ROOT, "integrations", "pi", "pocket.ts")]
    if a.model:
        cmd += ["--model", a.model]
    if a.thinking:
        cmd += ["--thinking", a.thinking]
    cmd.append(prompt)

    env = {**os.environ, "POCKET_RPC_URL": payload["rpc_url"], "POCKET_ROOT": ROOT}
    if a.env_file:
        with open(os.path.expanduser(a.env_file), encoding="utf-8") as f:
            for raw in f:
                raw = raw.strip()
                if raw and not raw.startswith("#") and "=" in raw:
                    k, v = raw.split("=", 1)
                    env[k.strip().removeprefix("export ").strip()] = v.strip().strip('"').strip("'")
    started = time.time()
    # The agent in a process group of its own, so what it leaves running (a runtime it started in
    # the background) ends with it instead of outliving the task.
    proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True, encoding="utf-8", errors="replace")
    try:
        out, err = proc.communicate(timeout=a.max_time + 60)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, err = proc.communicate()
    finally:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    seconds = round(time.time() - started, 1)
    if scratch:
        shutil.rmtree(scratch, ignore_errors=True)
    # The agent's own event stream, kept when asked (POCKET_AGENT_TRACES=<dir>): what it called and
    # why, for reading a run afterwards rather than only its counts.
    traces = os.environ.get("POCKET_AGENT_TRACES")
    if traces:
        os.makedirs(traces, exist_ok=True)
        with open(os.path.join(traces, f"{payload.get('name', 'task')}.jsonl"), "w", encoding="utf-8", newline="\n") as f:
            f.write(out)
            if err.strip():
                f.write("\n" + json.dumps({"type": "stderr", "text": err[-20000:]}) + "\n")

    usage = {"input": 0, "output": 0, "cache_read": 0, "total": 0}
    cost = 0.0
    tools = {}
    peeked = []
    # The harness and the evidence (where the checks and earlier answers are), by absolute path or
    # relative to a `cd` into the repository in the same call.
    harness = [os.path.join(ROOT, "tools", "scripts"), os.path.join(ROOT, "tests", "evidence", "agent-eval"), "agent_eval"]
    relative = ["tools/scripts", "tests/evidence/agent-eval", "evidence/agent-eval"]
    turns = 0
    final_text = ""
    errors = []
    for line in out.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = e.get("type")
        if kind == "tool_execution_start":
            name = e.get("toolName", "?")
            # oh-my-pi serves an extension's tools as devices: a write to xd://pocket is a call of the
            # `pocket` tool, a read of xd://pocket its description. Counted under the tool's own name.
            path = str((e.get("args") or {}).get("path", ""))
            if name in ("write", "read") and path.startswith("xd://"):
                name = path[5:] + ("" if name == "write" else " (docs)")
            tools[name] = tools.get(name, 0) + 1
            args = json.dumps(e.get("args") or {})
            reached = any(h in args for h in harness) or (ROOT in args and any(r in args.split(ROOT, 1)[1] for r in relative))
            if reached and len(peeked) < 10:
                peeked.append(f"{e.get('toolName', '?')}: {args[:160]}")
        elif kind == "tool_execution_end" and e.get("isError"):
            errors.append(e.get("toolName", "?"))
        elif kind == "message_end":
            m = e.get("message", {})
            if m.get("role") != "assistant":
                continue
            turns += 1
            u = m.get("usage", {})
            usage["input"] += u.get("input", 0)
            usage["output"] += u.get("output", 0)
            usage["cache_read"] += u.get("cacheRead", 0)
            usage["total"] += u.get("totalTokens", 0)
            cost += (u.get("cost") or {}).get("total", 0.0)
            texts = [c.get("text", "") for c in m.get("content", []) if c.get("type") == "text"]
            if any(t.strip() for t in texts):
                final_text = "\n".join(texts)

    answer = None
    for tl in reversed(final_text.strip().splitlines()):
        tl = tl.strip().strip("`").strip()
        if tl.startswith("{") and "answer" in tl:
            try:
                answer = json.loads(tl).get("answer")
                break
            except json.JSONDecodeError:
                continue
    report = {"answer": answer, "agent": os.path.basename(agent), "via": a.via, "model": a.model, "seconds": seconds, "turns": turns,
              "tool_calls": sum(tools.values()), "tools": tools, "tool_errors": len(errors), "tokens": usage, "cost_usd": round(cost, 6)}
    if peeked:
        report["peeked"] = peeked
    sys.stderr.write(err[-2000:])
    if proc.returncode != 0:
        sys.stderr.write(f"\n{os.path.basename(agent)} exited {proc.returncode}\n")
    print(json.dumps(report))
    return 0 if proc.returncode == 0 else proc.returncode


if __name__ == "__main__":
    sys.exit(main())
