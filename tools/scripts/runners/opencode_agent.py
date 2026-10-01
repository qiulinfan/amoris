#!/usr/bin/env python3
"""A runner for the agent benchmark (tools/scripts/agent_eval.py, docs/agent-eval.md) that hands
each task to opencode (`opencode run`), by default with GLM 5.3 Flash on the Z.ai coding plan
(`zai-coding-plan/glm-5.3-flash`; --model to pick another). It prints the agent's answer with what
the task cost: tokens, money (0 on a subscription plan), tool calls, turns and seconds.

    python3 tools/scripts/agent_eval.py --timeout 900 \\
        --runner "python3 tools/scripts/runners/opencode_agent.py --via mcp"

How the agent reaches the engine (--via):
    mcp        the `pocket` MCP server (`pocket mcp`, attached to the harness's runtime through
               POCKET_RPC_URL), declared in an opencode.json the runner writes where the agent
               starts (opencode merges it over the user's own configuration, which is left alone)
    shell      `pocket rpc` in the shell

The agent starts in the project copy for a task that edits files and in an empty directory for the
rest, never in the repository. A call that reaches into the harness or its evidence (tools/scripts,
where the checks are, and tests/evidence/agent-eval, where earlier answers are) is reported under
`peeked`, and such a run's pass says nothing about the engine.
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
    ap.add_argument("--agent", default="opencode", help="the opencode binary (or a path to one)")
    ap.add_argument("--via", default="mcp", choices=["mcp", "shell"])
    ap.add_argument("--model", default="zai-coding-plan/glm-5.3-flash", help="provider/model")
    ap.add_argument("--variant", help="the model's reasoning effort, where the provider has one")
    ap.add_argument("--max-time", type=int, default=600, help="seconds the agent may take")
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
    else:
        how = (f"Reach the engine with `{pocket} rpc <method> '<params as JSON>'` in the shell: it sends the command to the running "
               f"runtime (POCKET_RPC_URL is set) and prints the result. `{pocket} rpc commands` lists every method.")
    where = f"The project's files are in {project_dir}." if edits_files else "The task is done through commands on the running runtime; do not edit the project's files."
    prompt = (
        "You are working on a game made with Pocket, an engine that an AI agent drives through commands.\n"
        f"A runtime of the project '{payload['project']}' is running, paused, with its control server at {payload['rpc_url']}.\n\n"
        f"Task: {payload['task']}\n\n"
        f"{payload.get('notes', '')}\n{where}\n{how}\n"
        f"The engine's documentation, to read as you need: {', '.join(docs)}.\n"
        "Do only what the task asks. Step the simulation only if the task needs it.\n"
        'When you are done, end your final message with one line of JSON: {"answer": <the answer the task asks for, or null>}.'
    )

    scratch = None
    if edits_files and project_dir:
        cwd = project_dir
    else:
        scratch = tempfile.mkdtemp(prefix="pocket-agent-")
        cwd = scratch
    env = {**os.environ, "POCKET_RPC_URL": payload["rpc_url"], "POCKET_ROOT": ROOT}
    if a.via == "mcp":
        server = {"type": "local", "command": [pocket, "--root", ROOT, "mcp"], "enabled": True,
                  "environment": {"POCKET_RPC_URL": payload["rpc_url"], "POCKET_ROOT": ROOT}}
        with open(os.path.join(cwd, "opencode.json"), "w") as f:
            json.dump({"$schema": "https://opencode.ai/config.json", "mcp": {"pocket": server}}, f, indent=2)
    agent = shutil.which(a.agent) or a.agent
    cmd = [agent, "run", "--format", "json", "--auto", "--model", a.model, "--dir", cwd]
    if a.variant:
        cmd += ["--variant", a.variant]
    cmd.append(prompt)
    started = time.time()
    # stdin closed: opencode run waits on a piped stdin for more of the message. The agent in a
    # process group of its own, so what it leaves running ends with it.
    proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
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
    traces = os.environ.get("POCKET_AGENT_TRACES")
    if traces:
        os.makedirs(traces, exist_ok=True)
        with open(os.path.join(traces, f"{payload.get('name', 'task')}.jsonl"), "w") as f:
            f.write(out)
            if err.strip():
                f.write("\n" + json.dumps({"type": "stderr", "text": err[-20000:]}) + "\n")

    usage = {"input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "total": 0}
    cost = 0.0
    tools = {}
    peeked = []
    harness = [os.path.join(ROOT, "tools", "scripts"), os.path.join(ROOT, "tests", "evidence", "agent-eval"), "agent_eval"]
    relative = ["tools/scripts", "tests/evidence/agent-eval", "evidence/agent-eval"]
    turns = 0
    errors = 0
    texts = []   # the text of each step, in order: the last step's is the final message
    step_text = []
    for line in out.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = e.get("type")
        part = e.get("part") or {}
        if kind == "tool_use":
            name = part.get("tool", "?")
            tools[name] = tools.get(name, 0) + 1
            state = part.get("state") or {}
            if state.get("status") == "error":
                errors += 1
            args = json.dumps(state.get("input") or {})
            reached = any(h in args for h in harness) or (ROOT in args and any(r in args.split(ROOT, 1)[1] for r in relative))
            if reached and len(peeked) < 10:
                peeked.append(f"{name}: {args[:160]}")
        elif kind == "text":
            step_text.append(part.get("text", ""))
        elif kind == "step_finish":
            turns += 1
            t = part.get("tokens") or {}
            usage["input"] += t.get("input", 0)
            usage["output"] += t.get("output", 0)
            usage["reasoning"] += t.get("reasoning", 0)
            usage["cache_read"] += (t.get("cache") or {}).get("read", 0)
            usage["total"] += t.get("total", 0)
            cost += part.get("cost") or 0.0
            if any(s.strip() for s in step_text):
                texts.append("\n".join(step_text))
            step_text = []
    if any(s.strip() for s in step_text):
        texts.append("\n".join(step_text))
    final_text = texts[-1] if texts else ""

    answer = None
    for tl in reversed(final_text.strip().splitlines()):
        tl = tl.strip().strip("`").strip()
        if tl.startswith("{") and "answer" in tl:
            try:
                answer = json.loads(tl).get("answer")
                break
            except json.JSONDecodeError:
                continue
    report = {"answer": answer, "agent": "opencode", "via": a.via, "model": a.model, "seconds": seconds, "turns": turns,
              "tool_calls": sum(tools.values()), "tools": tools, "tool_errors": errors, "tokens": usage, "cost_usd": round(cost, 6)}
    if peeked:
        report["peeked"] = peeked
    sys.stderr.write(err[-2000:])
    if proc.returncode != 0:
        sys.stderr.write(f"\nopencode exited {proc.returncode}\n")
    print(json.dumps(report))
    return 0 if proc.returncode == 0 else proc.returncode


if __name__ == "__main__":
    sys.exit(main())
