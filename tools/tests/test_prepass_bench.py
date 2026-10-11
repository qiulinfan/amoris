"""Failed benchmark exits cannot become successful measurements. No GPU is started."""
import importlib.util
import json
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "prepass_bench", Path(__file__).resolve().parents[1] / "prepass_bench.py")
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)


class PrepassBenchTests(unittest.TestCase):
    REPORTS = {
        "dense-headless": json.dumps({"submit_to_idle_ms": {"p50": 1.0},
                                      "gpu_ms": {"mean": 2.0}}),
        "dense-window": "BenchReport { frame_ms_p50: 1.0, gpu_ms_mean: 2.0, }",
    }

    def run_benchmark(self, case, returncode):
        process = SimpleNamespace(returncode=returncode, stdout=self.REPORTS[case],
                                  stderr="GPU device failed during teardown" if returncode else "")
        args = SimpleNamespace(frames=3, size="128x128", settle=0)
        with patch.object(bench, "gpu_state", return_value=None), \
                patch.object(bench.subprocess, "run", return_value=process):
            return bench.run_one("unused", case, "metal", None, "auto", "off", args), process

    def test_nonzero_exit_rejects_valid_metrics_and_keeps_failure_output(self):
        for case in self.REPORTS:
            with self.subTest(case=case):
                record, process = self.run_benchmark(case, 1)
                self.assertNotIn("result", record)
                self.assertNotIn("headline", record)
                self.assertEqual(record["returncode"], 1)
                self.assertIn("exit 1", record["error"])
                self.assertEqual(record["stdout"], process.stdout)
                self.assertEqual(record["stderr"], process.stderr)

    def test_zero_exit_keeps_valid_headless_and_window_metrics(self):
        for case in self.REPORTS:
            with self.subTest(case=case):
                record, _ = self.run_benchmark(case, 0)
                self.assertNotIn("error", record)
                self.assertEqual(record["returncode"], 0)
                self.assertEqual(record["headline"], (1.0, 2.0))


if __name__ == "__main__":
    unittest.main()
