#!/usr/bin/env python3
"""Run real DeepSeek agents against isolated native Amoris projects and one shared budget."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from smoke_server import MCP
from agent_dev_bench import Host, TASKS, prepare, grade, suite_fingerprint
from deepseek_agent import Ledger, Limits, run_agent


def credential(path: Path | None):
    """Read one explicitly supplied external credential without copying or printing it."""
    if path is None:
        value = os.environ.get("DEEPSEEK_API_KEY", "")
    else:
        path = path.expanduser().resolve()
        if path.is_relative_to(ROOT):
            raise ValueError("Credentials must stay outside the repository")
        text = path.read_text().strip()
        if path.name == ".env" or path.suffix == ".env":
            match = re.search(r"(?m)^\s*(?:export\s+)?DEEPSEEK_API_KEY\s*=\s*(.*?)\s*$", text)
            if not match:
                raise ValueError("External env file has no DEEPSEEK_API_KEY")
            value = match.group(1).strip().strip("\"'")
        else:
            value = text
    if not value or any(c in value for c in "\r\n"):
        raise ValueError("A valid external credential is required")
    return value


class DeveloperTools:
    """Native MCP tools; evaluator/reference files never enter the candidate host."""
    def __init__(self, url: str, allowed_files: list[str]):
        self.mcp = MCP(url)
        self.allowed_files = set(allowed_files)
        self.mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                  "clientInfo": {"name": "Amoris native development benchmark", "version": "1"}})
        self.mcp.rpc("notifications/initialized", notify=True)
        native = self.mcp.rpc("tools/list")["result"]["tools"]
        self.schemas = [{"type": "function", "function": {
            "name": tool["name"], "description": tool["description"], "parameters": tool["inputSchema"]}}
            for tool in native]

    def dispatch(self, name, arguments):
        # Native asset import accepts external resources; that capability is outside this task.
        if name == "assets" and arguments.get("action") not in {"list", "read", "info", "scan"}:
            return {"error": {"code": "task_scope", "message": "External acquisition is outside this development task"}}
        if name == "scripts" and arguments.get("action") == "write" and arguments.get("path") not in self.allowed_files:
            return {"error": {"code": "task_scope", "message": "Write only the task's allowed script"}}
        response = self.mcp.rpc("tools/call", {"name": name, "arguments": arguments})
        if "error" in response:
            return {"error": response["error"]}
        result = response["result"]
        text = next((block["text"] for block in result.get("content", []) if block.get("type") == "text"), "{}")
        try:
            parsed = json.loads(text)
        except json.JSONDecodeError:
            parsed = {"text": text}
        return {"isError": True, "error": parsed} if result.get("isError") else parsed


def developer_runs(args, key, ledger):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    metadata = {"protocol": "courier-v1", "fixture_sha256": suite_fingerprint(),
                "grader_sha256": hashlib.sha256((ROOT / "tools/eval/agent_dev_bench.py").read_bytes()).hexdigest(),
                "tasks": [t.metadata() for t in TASKS], "repeats_planned": args.repeats,
                "method": "native MCP tools; direct DeepSeek Chat Completions loop; no shell or evaluator access",
                "reasoning_effort": "high", "budget_usd_shared": "5"}
    (output / "protocol.json").write_text(json.dumps(metadata, indent=2)+"\n")
    rows = []
    for repetition in range(1, args.repeats + 1):
        for task in TASKS:
            if args.tasks and task.id not in args.tasks:
                continue
            trial = output / f"{task.id}-{repetition}"
            trial.mkdir()
            candidate = trial / "candidate"
            prepared = prepare(task.id, candidate)
            with Host(args.pocket.resolve(), candidate, trial / "host.log") as host:
                native = DeveloperTools(host.url, prepared["allowed_files"])
                summary = run_agent(
                    "You are a game developer using the native Amoris MCP API. Use the provided tools. "
                    "Do not request shell access or outside files. Keep state in components and systems stateless. "
                    "Complete the user's task, apply the script and verify it. Finish with a concise result.",
                    prepared["prompt"], native.schemas, native.dispatch, trial / "agent",
                    Limits(max_requests=60, max_tool_calls=180, max_seconds=600,
                           max_output_tokens=4096, reasoning_effort="high"), key, ledger)
                # Avoid keeping a paused debugger host alive after a trial.
            verdict = grade(task.id, candidate, args.pocket.resolve(), trial / "grade")
            row = {"task": task.id, "kind": task.kind, "repetition": repetition,
                   "agent": summary, "grade": verdict,
                   "candidate_script_sha256": hashlib.sha256((candidate / task.file).read_bytes()).hexdigest()}
            rows.append(row)
            (output / "results.json").write_text(json.dumps({"protocol": metadata, "trials": rows}, indent=2)+"\n")
            print(json.dumps({"task": task.id, "repetition": repetition, "passed": verdict["passed"],
                              "provider_calls": summary["provider_calls"], "tool_calls": summary["tool_calls"],
                              "cost_upper_usd": summary["metered_cost_upper_usd"],
                              "stop": summary["stop_reason"]}), flush=True)
            if summary["stop_reason"] == "budget_preflight":
                return


def gameplay_runs(args, key, ledger):
    from agent_gameplay import Gameplay, fixture_fingerprint, replay_actions
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    prompt = ("Collect all four cargo crates in this sailing course, using only the player tools. "
              "Your radar observes cargo within 30 metres. Navigation uses the game's disclosed helmsman "
              "and real sailing physics. The helmsman is not a route planner. Choose your targets and actions. "
              "Take when can_take is true; collection is judged by the unchanged 3-metre native rule. "
              "At 60 ticks/second the boat can overshoot nearby cargo, so use short waits near a target and "
              "adjust sail or heading as needed. Complete within 3600 simulation ticks. "
              "When progress.collected equals progress.total, finish with a short result.")
    protocol = {"course": "agent-sailing-downwind-v1", "fixture_sha256": fixture_fingerprint(),
                "repeats_planned": args.repeats, "seed": 1, "max_ticks": 3600,
                "reasoning_effort": args.game_effort, "max_output_tokens": args.game_output_tokens, "prompt": prompt,
                "method": "project-level range-limited player gateway; native physics; no developer tools"}
    (output / "protocol.json").write_text(json.dumps(protocol, indent=2)+"\n")
    trials = []
    for repetition in range(1, args.repeats + 1):
        trial = output / f"episode-{repetition}"
        with Gameplay(args.pocket.resolve(), run_dir=trial / "native") as game:
            def dispatch(name, arguments):
                if name != "observe" and game.judge()["tick"] >= 3600:
                    return {"error": {"code": "episode_time_limit", "message": "The course's time budget is exhausted"}}
                return game.dispatch(name, arguments)
            summary = run_agent("You are a sailing game player. Use only the supplied observation and intention tools. "
                                "Every choice of target, steering, sail, time step and collection must be yours. "
                                "Do not ask for developer, filesystem or debugging access.",
                                prompt, game.tools(), dispatch, trial / "agent",
                                Limits(max_requests=90, max_tool_calls=200, max_seconds=600,
                                       max_output_tokens=args.game_output_tokens, reasoning_effort=args.game_effort), key, ledger)
            verdict, actions = game.judge(), game.actions
            (trial / "actions.json").write_text(json.dumps(actions, indent=2)+"\n")
        parity = replay_actions(actions, pocket=args.pocket.resolve(), run_dir=trial / "replay")
        (trial / "replay.json").write_text(json.dumps(parity, indent=2)+"\n")
        row = {"repetition": repetition, "agent": summary, "grade": verdict,
               "replay_passed": parity["passed"], "actions": len(actions)}
        trials.append(row)
        (output / "results.json").write_text(json.dumps({"protocol": protocol, "trials": trials}, indent=2)+"\n")
        print(json.dumps({"episode": repetition, "collected": verdict["collected"], "total": verdict["total"],
                          "ticks": verdict["tick"], "replay_passed": parity["passed"],
                          "cost_upper_usd": summary["metered_cost_upper_usd"], "stop": summary["stop_reason"]}), flush=True)
        if summary["stop_reason"] == "budget_preflight":
            return


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=["development", "gameplay"], default="development")
    parser.add_argument("--ledger", type=Path, default=ROOT / "out/agent-showcase/shared-ledger.json")
    parser.add_argument("--credential-file", type=Path)
    parser.add_argument("--pocket", type=Path, default=ROOT / "target/release/pocket")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--tasks", nargs="*")
    parser.add_argument("--game-output-tokens", type=int, default=8192)
    parser.add_argument("--game-effort", choices=["low", "high", "max", "none"], default="low")
    args = parser.parse_args()
    if not 1 <= args.repeats <= 3:
        raise ValueError("This first public protocol has at most three repetitions")
    (gameplay_runs if args.mode == "gameplay" else developer_runs)(
        args, credential(args.credential_file), Ledger(args.ledger, "5"))


if __name__ == "__main__":
    main()
