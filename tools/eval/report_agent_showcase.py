#!/usr/bin/env python3
"""Curate completed native agent runs into portable public JSON and Markdown.

Read-only inputs; no provider calls, credentials, grader execution or candidate
mutation. Generation is a snapshot: unstarted or incomplete trials are not invented.
"""
from __future__ import annotations

import argparse
import ast
import difflib
import hashlib
import json
import re
import statistics
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path


def sha_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


class Curator:
    def __init__(self, input_root: Path, repo: Path, *, traces=False, trace_limit=64000):
        self.root, self.repo = input_root.resolve(strict=True), repo.resolve(strict=True)
        if not self.root.is_dir() or not self.repo.is_dir():
            raise ValueError("Input evidence and repository must be existing directories")
        self.traces, self.trace_limit = traces, trace_limit
        self.bindings = []
        self.warnings = []

    def sanitize(self, value, project: Path | None = None):
        if isinstance(value, dict):
            private = {"reasoning_content", "authorization", "api_key", "apikey", "credential",
                       "credentials", "credential_path", "credential_file", "key_path", "api_key_path",
                       "access_token", "secret", "private_host", "host_url"}
            return {self.sanitize(str(k), project): self.sanitize(v, project)
                    for k, v in value.items() if str(k).casefold() not in private}
        if isinstance(value, list):
            return [self.sanitize(v, project) for v in value]
        if not isinstance(value, str):
            return value
        if re.search(r"myrunbook|\.codex/memories|opencode/auth|/secrets?/|credential[-_ ]file", value, re.I):
            return "[PRIVATE CONTEXT OMITTED]"
        for source, target in [(project, "$PROJECT"), (self.root, "$EVIDENCE"), (self.repo, "$REPO")]:
            if source is not None:
                aliases = {str(source), str(source.resolve())}
                for alias in tuple(aliases):
                    if alias.startswith(("/private/var/", "/private/tmp/")):
                        aliases.add(alias.removeprefix("/private"))
                for alias in sorted(aliases, key=len, reverse=True):
                    value = value.replace(alias, target)
        value = re.sub(r"https?://(?:127\.0\.0\.1|localhost|\[::1\])(?::\d+)?", "$HOST", value)
        value = re.sub(r"\bsk-[A-Za-z0-9_-]{12,}\b", "[REDACTED]", value)
        value = re.sub(r"(?i)Bearer\s+[^\s\"']+", "Bearer [REDACTED]", value)
        return re.sub(r"/(?:Users|home|private|var|tmp|opt|Applications)/[^\s`\"'<>]+", "$LOCAL", value)

    def read(self, path: Path):
        if not path.is_file():
            return None
        try:
            raw = path.read_bytes()
            value = json.loads(raw)
        except (OSError, ValueError):
            self.warnings.append(f"Skipped unreadable or incomplete JSON: {self.sanitize(str(path))}")
            return None
        self.bindings.append({"path": self.sanitize(str(path)), "sha256": sha_bytes(raw)})
        return value

    def trace(self, path: Path, project: Path):
        if not path.is_file():
            return [], None
        raw = path.read_bytes()
        digest = sha_bytes(raw)
        self.bindings.append({"path": self.sanitize(str(path)), "sha256": digest})
        events = []
        for line in raw.splitlines():
            try:
                events.append(json.loads(line))
            except ValueError:
                self.warnings.append(f"Skipped incomplete trace line: {self.sanitize(str(path))}")
        start = next((e for e in events if e.get("type") == "start"), {})
        public = []
        used = 0
        eligible = [e for e in events if e.get("type") in {"tool", "error"}]
        if self.traces:
            for event in eligible:
                curated = self.sanitize(event, project)
                # Keep JSON valid; truncation is explicit on each long string.
                def bounded(v):
                    if isinstance(v, str) and len(v) > 2000:
                        return v[:2000] + f"\n[TRUNCATED: {len(v)} original characters]"
                    if isinstance(v, dict): return {k: bounded(x) for k, x in v.items()}
                    if isinstance(v, list): return [bounded(x) for x in v]
                    return v
                curated = bounded(curated)
                size = len(json.dumps(curated, ensure_ascii=False).encode())
                if used + size > self.trace_limit:
                    break
                used += size
                public.append(curated)
        last = next((e.get("assistant", {}).get("content") for e in reversed(events)
                     if e.get("type") == "response" and e.get("assistant", {}).get("content")), None)
        metadata = {"source_sha256": digest, "source_events": len(events),
                    "eligible_tool_error_events": len(eligible), "included_events": len(public),
                    "byte_limit": self.trace_limit, "recording": "curated bounded excerpt" if self.traces else "omitted",
                    "reasoning_effort": start.get("reasoning_effort"),
                    "max_output_tokens": next((e.get("reserved_output_tokens") for e in events
                                               if e.get("type") == "request"), None),
                    "prompt": self.sanitize(start.get("user"), project),
                    "final_public_answer": self.sanitize(last, project)}
        return public, metadata

    def mutations(self, protocol):
        """Static parse of the hash-bound frozen grader; never import or execute it."""
        grader = self.repo / "tools/eval/agent_dev_bench.py"
        if not grader.is_file():
            self.warnings.append("Frozen grader unavailable; patch baselines omitted")
            return {}
        raw = grader.read_bytes()
        if sha_bytes(raw) != protocol.get("grader_sha256"):
            self.warnings.append("Current grader hash differs from frozen protocol; patch baselines omitted")
            return {}
        fixture = hashlib.sha256()
        template = self.repo / "bench/agent-dev/template"
        for file in sorted(template.rglob("*")):
            if file.is_file() and ".pocket" not in file.parts:
                fixture.update(file.relative_to(template).as_posix().encode() + b"\0" + file.read_bytes() + b"\0")
        fixture.update(json.dumps(protocol["tasks"], sort_keys=True).encode())
        if fixture.hexdigest() != protocol.get("fixture_sha256"):
            self.warnings.append("Current fixture differs from frozen protocol; patch baselines omitted")
            return {}
        self.bindings.append({"kind": "fixture_set", "path": "$REPO/bench/agent-dev/template",
                              "sha256": fixture.hexdigest()})
        self.bindings.append({"path": "$REPO/tools/eval/agent_dev_bench.py", "sha256": sha_bytes(raw)})
        tree = ast.parse(raw)
        assignment = next((node for node in tree.body if isinstance(node, ast.Assign)
            and any(isinstance(name, ast.Name) and name.id == "TASKS" for name in node.targets)), None)
        if not assignment or not isinstance(assignment.value, (ast.Tuple, ast.List)):
            return {}
        result = {}
        for node in assignment.value.elts:
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name) or node.func.id != "Task":
                continue
            values = [ast.literal_eval(arg) for arg in node.args]
            result[values[0]] = {"file": values[3], "old": values[5], "new": values[6]}
        return result

    def patch(self, task, project: Path, mutations: dict):
        mutation = mutations.get(task["id"])
        if mutation is None:
            return {"status": "unavailable", "reason": "No matching frozen mutation"}
        relative = mutation["file"]
        target = (project / relative).resolve()
        baseline = (self.repo / "bench/agent-dev/template" / relative).resolve()
        if not target.is_relative_to(project.resolve()) or not baseline.is_relative_to(
                (self.repo / "bench/agent-dev/template").resolve()):
            return {"status": "unavailable", "reason": "Unsafe file path"}
        if not target.is_file() or not baseline.is_file():
            return {"status": "unavailable", "reason": "Candidate or fixture file missing"}
        golden, final = baseline.read_text(), target.read_text()
        if golden.count(mutation["old"]) != 1:
            return {"status": "unavailable", "reason": "Frozen mutation anchor is not unique"}
        before = golden.replace(mutation["old"], mutation["new"])
        patch = "".join(difflib.unified_diff(before.splitlines(True), final.splitlines(True),
                fromfile="mutated-fixture/" + relative, tofile="agent-candidate/" + relative))
        digest = sha_bytes(patch.encode())
        public = self.sanitize(patch, project)
        return {"status": "recorded", "file": relative, "baseline": "mutated fixture, not golden solution",
                "mutated_file_sha256": sha_bytes(before.encode()), "candidate_file_sha256": sha_bytes(final.encode()),
                "diff_sha256": digest, "public_diff_sha256": sha_bytes(public[:14000].encode()),
                "diff_original_chars": len(public),
                "diff_truncated": len(public) > 14000, "diff": public[:14000]}


def metrics(rows):
    result = {}
    keys = ["wall_s", "provider_calls", "completed_provider_calls", "tool_calls", "tool_errors",
            "prompt_tokens", "prompt_cache_hit_tokens", "prompt_cache_miss_tokens", "completion_tokens"]
    for key in keys:
        values = [row["metrics"][key] for row in rows if type(row["metrics"].get(key)) in {int, float}]
        result[key] = {"samples": len(values), "mean": statistics.mean(values) if values else None,
                       "median": statistics.median(values) if values else None}
    prompt = sum(row["metrics"]["prompt_tokens"] for row in rows
                 if type(row["metrics"].get("prompt_tokens")) in {int, float})
    hit = sum(row["metrics"]["prompt_cache_hit_tokens"] for row in rows
              if type(row["metrics"].get("prompt_cache_hit_tokens")) in {int, float})
    result["known_prompt_cache_hit_fraction"] = hit / prompt if prompt else None
    return result


def count_verdict(rows, key):
    values = [row.get("grade", {}).get(key) for row in rows]
    return {"pass": sum(value is True for value in values),
            "fail": sum(value is False for value in values),
            "unknown": sum(value is not True and value is not False for value in values)}


def aggregate(rows):
    finish = {}
    for row in rows:
        finish[row["agent_finish"]] = finish.get(row["agent_finish"], 0) + 1
    return {"n": len(rows), "behavior": count_verdict(rows, "passed"),
            **{key: count_verdict(rows, key) for key in ["types_passed", "integrity_passed",
                                                       "regressions_passed", "scope_passed"]},
            "agent_finish_counts": finish, "metrics": metrics(rows),
            "known_peak_cost_upper_usd": str(sum((Decimal(row["known_peak_cost_upper_usd"])
                                                    for row in rows), Decimal(0))),
            "unknown_reserved_usd": str(sum((Decimal(row["unknown_reserved_usd"])
                                               for row in rows), Decimal(0)))}


def run_metrics(summary):
    usage = summary.get("usage_totals_known", {})
    return {key: summary.get(key) for key in ["wall_s", "provider_calls", "completed_provider_calls",
                                             "tool_calls", "tool_errors"]} | {
        key: usage.get(key) for key in ["prompt_tokens", "prompt_cache_hit_tokens",
                                      "prompt_cache_miss_tokens", "completion_tokens"]}


def reservation_for(ledger, run_id):
    return str(sum((Decimal(item["reserved_usd"]) for item in ledger.get("requests", [])
                    if item.get("run_id") == run_id and item.get("status") != "settled"
                    and "cost_upper_usd" not in item), Decimal(0)))


def curate(input_root: Path, repo: Path, *, include_tool_trace=False, gameplay_repeats=3):
    c = Curator(input_root, repo, traces=include_tool_trace)
    ledger = c.read(c.root / "shared-ledger.json") or {}
    developer_sets = []
    all_development = []
    for protocol_path in sorted(c.root.glob("development*/protocol.json")):
        protocol = c.read(protocol_path)
        if not protocol or not isinstance(protocol.get("tasks"), list):
            continue
        mutations = c.mutations(protocol)
        rows = []
        for task in protocol["tasks"]:
            for trial_path in sorted(protocol_path.parent.glob(task["id"] + "-*")):
                summary = c.read(trial_path / "agent/summary.json")
                if not summary:
                    continue
                grade = c.read(trial_path / "grade/result.json")
                try:
                    repetition = int(trial_path.name.rsplit("-", 1)[1])
                except ValueError:
                    continue
                project = trial_path / "candidate"
                tools, trace_meta = c.trace(trial_path / "agent/trace.jsonl", project)
                selected_grade = {key: grade.get(key) for key in ["passed", "feature_passed", "types_passed",
                    "integrity_passed", "regressions_passed", "scope_passed", "changed_files", "out_of_scope_files",
                    "fixture_sha256", "grader_sha256", "suite_version"]} if grade else {}
                checks = grade.get("checks", {}) if grade else {}
                selected_grade["checks"] = {name: {"passed": check.get("passed"),
                    "case_count": len(check.get("cases", [])),
                    "failed_cases": [case for case in check.get("cases", []) if case.get("passed") is False]}
                    for name, check in checks.items()}
                selected_grade = c.sanitize(selected_grade, project)
                row = {"id": protocol_path.parent.name + "/" + trial_path.name, "task": task["id"],
                    "kind": task.get("kind"), "repetition": repetition,
                    "prompt": c.sanitize(task.get("prompt"), project),
                    "agent_finish": summary.get("stop_reason", "unknown"),
                    "behavior_pass": grade.get("passed") if grade else None,
                    "grade_state": "complete" if grade else "not available in snapshot",
                    "grade": selected_grade, "metrics": run_metrics(summary),
                    "requested_model": summary.get("requested_model"),
                    "response_models": summary.get("response_models", []),
                    "reasoning_effort": (trace_meta or {}).get("reasoning_effort") or protocol.get("reasoning_effort"),
                    "max_output_tokens": (trace_meta or {}).get("max_output_tokens"),
                    "known_peak_cost_upper_usd": summary.get("metered_cost_upper_usd", "0"),
                    "unknown_reserved_usd": reservation_for(ledger, summary.get("run_id")),
                    "billing_cost_usd": None, "has_unsettled_request": summary.get("has_unsettled_request"),
                    "patch": c.patch(task, project, mutations), "trace_metadata": trace_meta,
                    "tool_trace": tools}
                rows.append(row)
        tasks = [{**c.sanitize(task), "trial_ids": [r["id"] for r in rows if r["task"] == task["id"]],
                  "statistics": aggregate([r for r in rows if r["task"] == task["id"]])}
                 for task in protocol["tasks"]]
        developer_sets.append({"id": protocol_path.parent.name, "protocol": c.sanitize(protocol),
            "planned_trials": len(tasks) * protocol.get("repeats_planned", 0),
            "observed_trials": len(rows), "tasks": tasks, "trials": rows,
            "statistics": aggregate(rows)})
        all_development.extend(rows)

    gameplay_rows, gameplay_sets = [], []
    gateway = c.repo / "tools/eval/agent_gameplay.py"
    gateway_snapshot = {"path": "$REPO/tools/eval/agent_gameplay.py",
        "sha256": sha_bytes(gateway.read_bytes()) if gateway.is_file() else None,
        "identity_scope": "File at report generation; not a substitute for a pre-run implementation hash"}
    if gateway_snapshot["sha256"]:
        c.bindings.append({"kind": "post_run_source_snapshot", **gateway_snapshot})
    for protocol_path in sorted(c.root.glob("gameplay*/protocol.json")):
        protocol = c.read(protocol_path)
        if not protocol:
            continue
        phase = protocol.get("phase") or ("pretest" if protocol_path.parent.name == "gameplay-v1"
                 else "formal" if protocol_path.parent.name in {"gameplay-v2", "gameplay-v3"} else "unclassified")
        aggregate_source = c.read(protocol_path.parent / "results.json") or {}
        source_rows = {r.get("repetition"): r for r in aggregate_source.get("trials", [])}
        rows = []
        for summary_path in sorted(protocol_path.parent.glob("episode-*/agent/summary.json")):
            trial = summary_path.parent.parent
            summary = c.read(summary_path)
            if not summary:
                continue
            repetition = int(trial.name.rsplit("-", 1)[1])
            source_row = source_rows.get(repetition, {})
            replay = c.read(trial / "replay.json")
            judge = replay.get("judge") if isinstance(replay, dict) else source_row.get("grade")
            project = trial / "project"
            tools, trace_meta = c.trace(trial / "agent/trace.jsonl", project)
            judge_public = {key: judge.get(key) for key in ["collected", "total", "worth", "score", "success",
                "tick", "world_hash", "fixture_sha256"]} if isinstance(judge, dict) else {}
            action_file = trial / "actions.json"
            if action_file.is_file():
                raw = action_file.read_bytes()
                c.bindings.append({"path": c.sanitize(str(action_file)), "sha256": sha_bytes(raw)})
            row = {"id": trial.relative_to(c.root).as_posix(), "phase": phase, "repetition": repetition,
                   "agent_finish": summary.get("stop_reason", "unknown"),
                   "behavior_pass": judge_public.get("success"), "grade": {"passed": judge_public.get("success")},
                   "judge": judge_public, "replay_passed": replay.get("passed") if replay else source_row.get("replay_passed"),
                   "native_actions": replay.get("actions") if replay else source_row.get("actions"),
                   "prompt": (trace_meta or {}).get("prompt") or protocol.get("prompt"),
                   "metrics": run_metrics(summary), "requested_model": summary.get("requested_model"),
                   "response_models": summary.get("response_models", []),
                   "reasoning_effort": (trace_meta or {}).get("reasoning_effort") or protocol.get("reasoning_effort"),
                   "max_output_tokens": (trace_meta or {}).get("max_output_tokens"),
                   "known_peak_cost_upper_usd": summary.get("metered_cost_upper_usd", "0"),
                   "unknown_reserved_usd": reservation_for(ledger, summary.get("run_id")),
                   "billing_cost_usd": None, "has_unsettled_request": summary.get("has_unsettled_request"),
                   "trace_metadata": trace_meta, "tool_trace": tools}
            rows.append(c.sanitize(row, project))
        gameplay_sets.append({"id": protocol_path.parent.name, "phase": phase, "protocol": c.sanitize(protocol),
            "planned_trials": protocol.get("repeats_planned"), "observed_trials": len(rows),
            "gateway_sha256_at_run": protocol.get("gateway_sha256") or protocol.get("grader_sha256"),
            "trial_ids": [r["id"] for r in rows], "statistics": aggregate(rows)})
        gameplay_rows.extend(rows)
    formal_rows = [r for r in gameplay_rows if r["phase"] == "formal"]
    pretest_rows = [r for r in gameplay_rows if r["phase"] == "pretest"]
    planned_formal = sum(s.get("planned_trials") or 0 for s in gameplay_sets if s["phase"] == "formal")
    conditions = {}
    for suite in gameplay_sets:
        rows = [r for r in gameplay_rows if r["id"] in suite["trial_ids"]]
        conditions[suite["id"]] = {
            "phase": suite["phase"], "reasoning_effort": suite["protocol"].get("reasoning_effort"),
            "max_output_tokens": suite["protocol"].get("max_output_tokens") or next(
                (r["max_output_tokens"] for r in rows if r["max_output_tokens"] is not None), None),
            "statistics": suite["statistics"],
        }
    scored = [r for r in gameplay_rows if type(r["judge"].get("score")) in {int, float}]
    selected = max(scored, key=lambda r: r["judge"]["score"]) if scored else None
    known_ledger = sum((Decimal(item["cost_upper_usd"]) for item in ledger.get("requests", [])
                        if "cost_upper_usd" in item), Decimal(0))
    uncertain = [item for item in ledger.get("requests", []) if "cost_upper_usd" not in item]
    unknown_ledger = sum((Decimal(item["reserved_usd"]) for item in uncertain), Decimal(0))
    known_models = sorted({model for row in all_development + gameplay_rows for model in row["response_models"]})
    output = {"schema_version": 1, "generated_at": datetime.now(timezone.utc).isoformat(),
        "generator": {"file": "tools/eval/report_agent_showcase.py",
                      "sha256": sha_bytes(Path(__file__).read_bytes())},
        "snapshot": {"planned_developer_trials": sum(s["planned_trials"] for s in developer_sets),
            "observed_developer_trials": len(all_development),
            "graded_developer_trials": sum(r["grade_state"] == "complete" for r in all_development),
            "planned_gameplay_trials": planned_formal or gameplay_repeats,
            "observed_gameplay_trials": len(formal_rows),
            "planned_gameplay_all_recorded": sum(s.get("planned_trials") or 0 for s in gameplay_sets),
            "observed_gameplay_pretests": len(pretest_rows), "observed_gameplay_all_recorded": len(gameplay_rows),
            "includes_only_completed_agent_summaries": True},
        "provider": {"requested_model": "deepseek-flash", "actual_response_models": known_models,
            "execution": "Direct Chat Completions tool loop; genuine model-selected native tool calls"},
        "development": {"suites": developer_sets, "statistics": aggregate(all_development)},
        "gameplay": {"suites": gameplay_sets, "trials": gameplay_rows,
            "gateway_source_snapshot": gateway_snapshot,
            "statistics_scope": "Each frozen protocol/effort/output condition separately; no pooled formal rate",
            "statistics_by_protocol": conditions, "all_recorded_n": len(gameplay_rows),
            "showcase_selection": {"trial_id": selected["id"] if selected else None,
                "native_score": selected["judge"]["score"] if selected else None,
                "rule": "Highest recorded native collection score; ties first in protocol/repetition order",
                "label": "Illustrative selected run; all recorded trials and conditions remain visible"}},
        "budget": {"authorized_total_usd": ledger.get("budget_usd"),
            "known_peak_rate_cost_upper_usd": str(known_ledger), "unknown_reserved_usd": str(unknown_ledger),
            "unknown_request_count": len(uncertain), "unknown_request_statuses": [item.get("status") for item in uncertain],
            "committed_upper_usd": str(known_ledger + unknown_ledger), "billing_cost_usd": None,
            "completed_trial_known_peak_rate_cost_upper_usd": str(sum((Decimal(r["known_peak_cost_upper_usd"])
                for r in all_development + gameplay_rows), Decimal(0))),
            "pricing_url": ledger.get("pricing_url"), "peak_usd_per_million": ledger.get("peak_usd_per_million"),
            "policy": "Exact reported tokens at peak rates are an upper bound, not an invoice. Unknown reservations are not measured spend."},
        "evidence_bindings": c.bindings, "warnings": c.warnings,
        "limitations": ["Six fixed development tasks and three planned sailing runs are a small, engine-specific study.",
            "Behavior is graded independently from model finish status; a transport interruption can leave a passing patch.",
            "Gameplay uses a project-level restricted gateway and a disclosed native Helm executor; this is not global player authorization.",
            "The model never receives the grader, golden files, private native state or shared budget ledger.",
            "Tool traces are optional bounded excerpts; their omission or truncation does not remove trials from statistics.",
            "The 2048-token gameplay-v1 pretests, low-thinking 8192-token gameplay-v2 runs and non-thinking 4096-token gameplay-v3 runs are separate frozen conditions; their success rates and performance averages are not pooled.",
            "Any highest-score media selection is disclosed and does not replace the complete trial table."]}
    return c.sanitize(output)


def markdown(report):
    snapshot, budget = report["snapshot"], report["budget"]
    lines = ["# Native agent showcase evidence", "",
        f"Snapshot generated {report['generated_at']}. Only completed agent summaries are counted.", "",
        f"Developer trials: {snapshot['observed_developer_trials']} observed / {snapshot['planned_developer_trials']} planned; "
        f"{snapshot['graded_developer_trials']} independently graded. Gameplay trials: "
        f"{snapshot['observed_gameplay_trials']} formal observed / {snapshot['planned_gameplay_trials']} formal planned; "
        f"{snapshot['observed_gameplay_pretests']} pretests retained separately.", "",
        "Behavioral acceptance and agent completion are reported separately. Every recorded trial, including timeouts and transport errors, remains in the denominator.", "",
        "| Task | n | Behavioral pass | Types | Integrity | Regressions | Scope | Wall mean / median (s) | API mean / median | Tool mean / median |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    def pair(stats, name):
        m = stats["metrics"][name]
        return "—" if m["mean"] is None else f"{m['mean']:.2f} / {m['median']:.2f}"
    def ratio(stats, name):
        v = stats[name]
        return f"{v['pass']}/{v['pass'] + v['fail']}" + (f" (+{v['unknown']} unknown)" if v['unknown'] else "")
    for suite in report["development"]["suites"]:
        for task in suite["tasks"]:
            s = task["statistics"]
            lines.append(f"| {task['title']} | {s['n']} | {ratio(s, 'behavior')} | {ratio(s, 'types_passed')} | "
                f"{ratio(s, 'integrity_passed')} | {ratio(s, 'regressions_passed')} | {ratio(s, 'scope_passed')} | "
                f"{pair(s, 'wall_s')} | {pair(s, 'provider_calls')} | {pair(s, 'tool_calls')} |")
    lines += ["", "| Task | Prompt tokens mean / median | Cache-hit tokens mean / median | Output tokens mean / median | Known cache hit | Peak-rate known cost upper | Unknown reserved |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for suite in report["development"]["suites"]:
        for task in suite["tasks"]:
            s = task["statistics"]
            rate = s["metrics"]["known_prompt_cache_hit_fraction"]
            rate_text = "—" if rate is None else f"{rate * 100:.2f}%"
            lines.append(f"| {task['title']} | {pair(s, 'prompt_tokens')} | {pair(s, 'prompt_cache_hit_tokens')} | "
                f"{pair(s, 'completion_tokens')} | {rate_text} | ${s['known_peak_cost_upper_usd']} | ${s['unknown_reserved_usd']} |")
    lines += ["", "Known provider-token totals use exact response usage; unknown requests contribute no invented tokens.", "",
        f"Known peak-rate cost upper bound across the shared ledger: **${budget['known_peak_rate_cost_upper_usd']}**. "
        f"Unknown/unsettled request reservations: **${budget['unknown_reserved_usd']}** "
        f"({budget['unknown_request_count']} requests). Actual provider invoice: **unknown**.", "",
        "Reservations protect the shared budget and are not measured spend.", "", "## Trial outcomes", "",
        "| Trial | Behavior | Agent finish | API / tool calls | Peak-rate known cost upper | Unknown reserved |",
        "| --- | --- | --- | ---: | ---: | ---: |"]
    for suite in report["development"]["suites"]:
        for r in suite["trials"]:
            passed = "pass" if r["behavior_pass"] is True else "fail" if r["behavior_pass"] is False else "unknown"
            lines.append(f"| {r['id']} | {passed} | {r['agent_finish']} | {r['metrics']['provider_calls']} / "
                f"{r['metrics']['tool_calls']} | ${r['known_peak_cost_upper_usd']} | ${r['unknown_reserved_usd']} |")
    lines += ["", "## Gameplay", ""]
    lines += ["| Protocol / effort / max output | Phase | n | Native success | Wall mean / median (s) | API mean / median | Output tokens mean / median | Known peak upper | Unknown reserved |",
              "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for suite in report["gameplay"]["suites"]:
        s = suite["statistics"]
        condition = report["gameplay"]["statistics_by_protocol"][suite["id"]]
        lines.append(f"| {suite['id']} / {condition['reasoning_effort']} / {condition['max_output_tokens']} | {suite['phase']} | {s['n']} | {ratio(s, 'behavior')} | "
            f"{pair(s, 'wall_s')} | {pair(s, 'provider_calls')} | {pair(s, 'completion_tokens')} | "
            f"${s['known_peak_cost_upper_usd']} | ${s['unknown_reserved_usd']} |")
    lines.append("")
    for r in report["gameplay"]["trials"]:
        j = r["judge"]
        lines.append(f"- {r['id']} ({r['phase']}, max output {r['max_output_tokens']}): {j.get('collected')} / {j.get('total')} collected; "
            f"native success {j.get('success')}; agent finish `{r['agent_finish']}`; "
            f"{r['metrics']['provider_calls']} API calls and {r['metrics']['tool_calls']} tool calls.")
    if not report["gameplay"]["trials"]:
        lines.append("No completed gameplay summary is present in this snapshot.")
    selection = report["gameplay"]["showcase_selection"]
    if selection["trial_id"]:
        lines += ["", f"Media candidate: **{selection['trial_id']}**. {selection['rule']}. "
                         f"{selection['label']}."]
    lines += ["", "## Frozen protocol", ""]
    for suite in report["development"]["suites"]:
        p = suite["protocol"]
        lines += [f"- {suite['id']}: `{p.get('protocol')}`; effort `{p.get('reasoning_effort')}`.",
                  f"- Fixture SHA-256: `{p.get('fixture_sha256')}`.",
                  f"- Grader SHA-256: `{p.get('grader_sha256')}`."]
    for suite in report["gameplay"]["suites"]:
        p = suite["protocol"]
        lines += [f"- {suite['id']}: fixture `{p.get('fixture_sha256')}`; "
                  f"gateway hash recorded at run: `{suite.get('gateway_sha256_at_run') or 'not recorded'}`."]
    source = report["gameplay"]["gateway_source_snapshot"]
    if source["sha256"]:
        lines += [f"- Gateway source snapshot SHA-256: `{source['sha256']}`. {source['identity_scope']}."]
    lines += ["", "Actual response model aliases: " + ", ".join(f"`{m}`" for m in report["provider"]["actual_response_models"]),
              "", *["- " + limitation for limitation in report["limitations"]], ""]
    return "\n".join(lines)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--input-root", required=True, type=Path)
    p.add_argument("--repo", required=True, type=Path)
    p.add_argument("--output-root", required=True, type=Path)
    p.add_argument("--include-tool-trace", action="store_true")
    p.add_argument("--gameplay-repeats", type=int, default=3)
    args = p.parse_args()
    output = args.output_root.resolve()
    if output.is_relative_to(args.input_root.resolve()):
        raise ValueError("Write reports outside the read-only input evidence directory")
    if output.exists() and any(output.iterdir()):
        raise ValueError("Use a fresh output directory")
    report = curate(args.input_root, args.repo, include_tool_trace=args.include_tool_trace,
                    gameplay_repeats=args.gameplay_repeats)
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if re.search(r"/Users/|/home/|\bsk-[A-Za-z0-9_-]{12,}|Bearer\s+(?!\[REDACTED\])", encoded):
        raise ValueError("Public report contains an unnormalized local path or credential pattern")
    output.mkdir(parents=True, exist_ok=True)
    (output / "result.json").write_text(encoded)
    (output / "result.md").write_text(markdown(report))
    print(json.dumps({"snapshot": report["snapshot"], "budget": report["budget"],
                      "output": str(output), "warnings": report["warnings"]}, indent=2))


if __name__ == "__main__":
    main()
