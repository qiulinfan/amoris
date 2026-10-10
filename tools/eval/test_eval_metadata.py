"""Offline receipt checks. Native processes, provider requests and credentials are stubbed."""
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import agent_dev_bench as bench
import run_agent_showcase as runner


class MetadataTests(unittest.TestCase):
    def test_detailed_receipts_cannot_use_a_tracked_site_directory(self):
        with self.assertRaisesRegex(ValueError, "ignored out"):
            runner.private_output(runner.ROOT / "site/content/evidence")
        self.assertEqual(runner.private_output(runner.ROOT / "out/evidence"),
                         runner.ROOT / "out/evidence")

    def test_prepare_identifies_version_and_task_without_source_digest(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / "candidate"
            receipt = bench.prepare("collection-range", project)
            self.assertEqual(receipt["suite_version"], bench.VERSION)
            self.assertEqual(receipt["id"], "collection-range")
            self.assertEqual(receipt["fixture_path"], "bench/agent-dev/template")
            self.assertNotIn("sha256", json.dumps(receipt))

    def test_grade_still_uses_execution_verdicts(self):
        native_check = SimpleNamespace(returncode=0, stderr="", stdout=json.dumps({
            "verdict": "Pass", "steps": [{"name": "types", "verdict": "Pass"}]}))
        host = MagicMock()
        checks = {task.id: lambda _host: {"passed": True, "cases": []} for task in bench.TASKS}
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / "candidate"
            bench.prepare("collection-range", project)
            with patch.object(bench.subprocess, "run", return_value=native_check), \
                    patch.object(bench, "Host", return_value=host), patch.object(bench, "CHECKS", checks):
                passed = bench.grade("collection-range", project, "unused", Path(temp) / "pass")
                self.assertTrue(passed["passed"])
                self.assertTrue(passed["types_passed"])
                checks["collection-range"] = lambda _host: {"passed": False, "cases": []}
                failed = bench.grade("collection-range", project, "unused", Path(temp) / "fail")
                self.assertFalse(failed["passed"])
                self.assertFalse(failed["feature_passed"])
                self.assertTrue(failed["regressions_passed"])
            self.assertNotIn("sha256", json.dumps(passed))

    def test_runner_records_task_paths_without_invoking_provider(self):
        host = MagicMock()
        host.__enter__.return_value.url = "unused"
        native = SimpleNamespace(schemas=[], dispatch=lambda *_args: None)
        agent_summary = {"stop_reason": "stop", "provider_calls": 0, "tool_calls": 0,
                         "metered_cost_upper_usd": "0"}
        with tempfile.TemporaryDirectory() as temp:
            args = SimpleNamespace(output=Path(temp), pocket=Path("unused"), repeats=1,
                                   tasks=["collection-range"])
            with patch.object(runner, "Host", return_value=host), \
                    patch.object(runner, "DeveloperTools", return_value=native), \
                    patch.object(runner, "run_agent", return_value=agent_summary) as run, \
                    patch.object(runner, "grade", return_value={"passed": True}), \
                    contextlib.redirect_stdout(io.StringIO()):
                runner.developer_runs(args, "unused", None)
            data = json.loads((Path(temp) / "results.json").read_text())
            self.assertEqual(run.call_count, 1)
        self.assertEqual(data["protocol"]["suite_version"], bench.VERSION)
        self.assertEqual(data["trials"][0]["candidate_script"], "scripts/collect.ts")
        self.assertTrue(data["trials"][0]["grade"]["passed"])
        self.assertNotIn("sha256", json.dumps(data))


if __name__ == "__main__":
    unittest.main()
