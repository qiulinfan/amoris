"""Offline curation fixtures, not benchmark results or provider usage."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import report_agent_showcase as report


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.base = Path(self.tmp.name)
        self.repo = self.base / "repo"
        self.root = self.base / "evidence"
        self.repo.mkdir()
        self.root.mkdir()
        source = self.repo / "tools/eval/agent_dev_bench.py"
        source.parent.mkdir(parents=True)
        source.write_text('TASKS=(Task("repair","Repair","bugfix","scripts/rules.ts",'
                          '"requirement","return 1;","return 0;"),)\n')
        template = self.repo / "bench/agent-dev/template/scripts/rules.ts"
        template.parent.mkdir(parents=True)
        template.write_text("export function rule(){return 1;}\n")
        self.task = {"id": "repair", "title": "Repair", "kind": "bugfix",
                     "allowed_files": ["scripts/rules.ts"], "prompt": "Fix the fixture"}
        digest = hashlib.sha256()
        digest.update(b"scripts/rules.ts\0" + template.read_bytes() + b"\0")
        digest.update(json.dumps([self.task], sort_keys=True).encode())
        self.protocol = {"protocol": "offline-fixture", "tasks": [self.task], "repeats_planned": 2,
                         "reasoning_effort": "high", "fixture_sha256": digest.hexdigest(),
                         "grader_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
        dump(self.root / "development-v1/protocol.json", self.protocol)
        self.trial = self.root / "development-v1/repair-1"
        dump(self.trial / "agent/summary.json", self.summary("run-a", "transport_or_tool_error"))
        dump(self.trial / "grade/result.json", {"passed": True, "types_passed": True,
             "integrity_passed": True, "regressions_passed": True, "scope_passed": True, "checks": {}})
        candidate = self.trial / "candidate/scripts/rules.ts"
        candidate.parent.mkdir(parents=True)
        candidate.write_text("export function rule(){return 2;}\n")
        trace = [{"type": "start", "user": "Fix fixture", "reasoning_effort": "high"},
                 {"type": "request", "reserved_output_tokens": 4096},
                 {"type": "response", "assistant": {"content": "Finished", "reasoning_content": "PRIVATE-THOUGHT"}},
                 {"type": "tool", "name": "scripts", "result": {"path": str(candidate),
                  "api_key": "sk-offline-fixture-secret-never-published", "reasoning_content": "PRIVATE-THOUGHT"}}]
        (self.trial / "agent/trace.jsonl").write_text("\n".join(json.dumps(e) for e in trace))
        dump(self.root / "shared-ledger.json", {"budget_usd": "5", "requests": [
            {"run_id": "run-a", "status": "settled", "cost_upper_usd": "0.2", "reserved_usd": "0.32"},
            {"run_id": "run-a", "status": "unsettled", "reserved_usd": "0.319488"}]})

    def tearDown(self):
        self.tmp.cleanup()

    @staticmethod
    def summary(run_id, finish):
        return {"run_id": run_id, "stop_reason": finish, "requested_model": "deepseek-flash",
            "response_models": ["deepseek-flash"], "wall_s": 12, "provider_calls": 3,
            "completed_provider_calls": 2, "tool_calls": 4, "tool_errors": 0,
            "usage_totals_known": {"prompt_tokens": 100, "prompt_cache_hit_tokens": 60,
                                  "prompt_cache_miss_tokens": 40, "completion_tokens": 80},
            "metered_cost_upper_usd": "0.2", "has_unsettled_request": True}

    def test_transport_finish_does_not_override_behavior(self):
        value = report.curate(self.root, self.repo)
        stats = value["development"]["statistics"]
        self.assertEqual(stats["behavior"], {"pass": 1, "fail": 0, "unknown": 0})
        self.assertEqual(stats["agent_finish_counts"], {"transport_or_tool_error": 1})
        self.assertEqual(stats["unknown_reserved_usd"], "0.319488")
        self.assertEqual(value["budget"]["known_peak_rate_cost_upper_usd"], "0.2")
        self.assertIsNone(value["budget"]["billing_cost_usd"])

    def test_diff_uses_mutated_fixture_not_golden(self):
        value = report.curate(self.root, self.repo)
        patch = value["development"]["suites"][0]["trials"][0]["patch"]
        self.assertEqual(patch["status"], "recorded")
        self.assertIn("-export function rule(){return 0;}", patch["diff"])
        self.assertIn("+export function rule(){return 2;}", patch["diff"])
        self.assertNotIn("return 1", patch["diff"])

    def test_paths_credentials_and_private_reasoning_are_omitted(self):
        value = report.curate(self.root, self.repo, include_tool_trace=True)
        encoded = json.dumps(value)
        self.assertNotIn(str(self.base), encoded)
        self.assertNotIn("PRIVATE-THOUGHT", encoded)
        self.assertNotIn("reasoning_content", encoded)
        self.assertNotIn("sk-offline", encoded)
        self.assertIn("$PROJECT/scripts/rules.ts", encoded)

    def test_incomplete_summary_does_not_invent_a_trial(self):
        file = self.root / "development-v1/repair-2/agent/summary.json"
        file.parent.mkdir(parents=True)
        file.write_text('{"run_id":')
        value = report.curate(self.root, self.repo)
        self.assertEqual(value["snapshot"]["observed_developer_trials"], 1)
        self.assertEqual(value["snapshot"]["planned_developer_trials"], 2)
        self.assertTrue(value["warnings"])

    def test_changed_fixture_disables_baseline_patch(self):
        (self.repo / "bench/agent-dev/template/scripts/rules.ts").write_text("different\n")
        value = report.curate(self.root, self.repo)
        patch = value["development"]["suites"][0]["trials"][0]["patch"]
        self.assertEqual(patch["status"], "unavailable")
        self.assertTrue(value["warnings"])

    def test_pretest_and_formal_statistics_are_separate(self):
        for group, success, limit in [("gameplay-v1", False, 2048), ("gameplay-v2", True, 8192),
                                      ("gameplay-v3", False, 4096)]:
            root = self.root / group
            dump(root / "protocol.json", {"repeats_planned": 3,
                                          "reasoning_effort": "none" if group == "gameplay-v3" else "low"})
            dump(root / "episode-1/agent/summary.json", self.summary(group, "stop"))
            dump(root / "episode-1/replay.json", {"passed": True, "actions": 4,
                 "judge": {"success": success, "collected": 4 if success else 1, "total": 4,
                           "score": 1 if success else 0.25}})
            (root / "episode-1/agent/trace.jsonl").write_text(json.dumps({"type": "request", "reserved_output_tokens": limit}))
        value = report.curate(self.root, self.repo)
        self.assertEqual(value["gameplay"]["all_recorded_n"], 3)
        self.assertEqual(value["gameplay"]["statistics_by_protocol"]["gameplay-v2"]["statistics"]["behavior"]["pass"], 1)
        self.assertEqual(value["gameplay"]["statistics_by_protocol"]["gameplay-v1"]["statistics"]["behavior"]["fail"], 1)
        self.assertEqual([r["max_output_tokens"] for r in value["gameplay"]["trials"]], [2048, 8192, 4096])
        self.assertEqual(value["gameplay"]["statistics_by_protocol"]["gameplay-v3"]["reasoning_effort"], "none")
        self.assertNotIn("statistics", value["gameplay"])
        self.assertEqual(value["gameplay"]["showcase_selection"]["trial_id"], "gameplay-v2/episode-1")


if __name__ == "__main__":
    unittest.main()
