"""Failure-path checks only: no GPU, browser, host process or paid API is started."""
import contextlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

TOOLS = Path(__file__).resolve().parents[1]


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, TOOLS / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


conditions = load("bench_conditions", "machine_conditions.py")
ab = load("bench_ab", "splats/ab.py")
matrix = load("bench_matrix", "splats/matrix.py")
backend = load("bench_backend", "backend_compare.py")
build = load("bench_build", "build_compare.py")
web = load("bench_web", "web_draw_paths.py")


def result(code=0, stdout="", stderr=""):
    return SimpleNamespace(returncode=code, stdout=stdout, stderr=stderr)


class ConditionsTests(unittest.TestCase):
    def test_gpu_users_only_keeps_basename_and_percentage(self):
        raw = {"process": r"C:\private\fake.exe", "pct": 4.5, "pid": 123,
               "engine": "private-device-id", "cmd": "PRIVATE_NOT_FOR_JSON"}
        self.assertEqual(conditions.gpu_users(raw), [{"process": "fake.exe", "pct": 4.5}])
        self.assertEqual(conditions.gpu_users(None), [])

    def test_invalid_powershell_output_is_not_published(self):
        with patch.object(conditions, "ps", return_value="PRIVATE_NOT_FOR_JSON"):
            self.assertIsNone(conditions.ps_json("unused"))

    def test_default_record_has_only_public_fields(self):
        scripts = []

        def ps_json(script):
            scripts.append(script)
            if "Win32_Battery" in script:
                return {"BatteryStatus": 2}
            if "Win32_VideoController" in script:
                return {"Name": "GPU", "DriverVersion": "1"}
            return {"process": "engine", "pct": 2, "pid": 1, "engine": "private-id"}

        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "conditions.json"
            with patch.object(sys, "argv", ["conditions", str(out)]), \
                    patch.object(conditions, "ps_json", side_effect=ps_json), \
                    patch.object(conditions, "ps", return_value="Balanced"), \
                    patch.object(conditions, "nvidia", return_value={"query": {}}), \
                    patch.object(conditions, "cpu_load", return_value={"mean_pct": 3}), \
                    contextlib.redirect_stderr(io.StringIO()):
                conditions.main()
            record = json.loads(out.read_text())
        self.assertEqual(set(record), {"label", "time", "on_ac_power", "power_plan", "adapters",
                                       "nvidia", "cpu_load", "gpu_engine_users"})
        self.assertTrue(record["on_ac_power"])
        self.assertEqual(record["gpu_engine_users"], [{"process": "engine", "pct": 2}])
        self.assertFalse(any("CommandLine" in s or "Win32_Process" in s for s in scripts))


class SplatTests(unittest.TestCase):
    def test_failed_child_is_error_not_zero_milliseconds(self):
        with patch.object(ab.subprocess, "run", return_value=result(1)):
            record = ab.run("unused", "", "tile", 1)
        self.assertIn("error", record)
        self.assertEqual(record["returncode"], 1)
        self.assertNotIn("splat_total", record)

    def test_empty_stdout_is_error(self):
        with patch.object(ab.subprocess, "run", return_value=result()):
            self.assertIn("error", ab.run("unused", "", "tile", 1))

    def test_timeout_is_recorded(self):
        with patch.object(ab.subprocess, "run", side_effect=subprocess.TimeoutExpired("unused", 1)):
            self.assertIn("error", ab.run("unused", "", "tile", 1))

    def test_valid_timings_are_preserved(self):
        text = ("Metal on GPU, 1280x720, key bits 24\n"
                "draw on: frame (submit to idle) mean 2.0 ms, p50 1.8 ms; GPU 1.0 ms; "
                "10 submitted, 8 visible, 0.5 M quad pixels\n  splat preprocess 0.1 ms\n"
                "  splat raster 0.2 ms\ndraw off: ignored\n")
        with patch.object(ab.subprocess, "run", return_value=result(stdout=text)):
            record = ab.run("unused", "", "tile", 1)
        self.assertNotIn("error", record)
        self.assertAlmostEqual(record["splat_total"], 0.3)
        self.assertEqual(record["gpu_total"], 1)

    def test_matrix_keeps_every_failed_run(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "matrix.json"
            argv = ["matrix", "--adapters", "stub", "--backends", "metal", "--rounds", "1",
                    "--scene", "fake=", "--out", str(out)]
            with patch.object(sys, "argv", argv), \
                    patch.object(matrix.ab, "run", side_effect=lambda *a, **k: {"error": "failed"}), \
                    patch.object(matrix, "gpu_temp", return_value=None), \
                    contextlib.redirect_stderr(io.StringIO()):
                matrix.main()
            records = json.loads(out.read_text())["runs"]
        self.assertEqual(len(records), 2)
        self.assertTrue(all(r["error"] == "failed" and "splat_total" not in r for r in records))

    def test_ab_cli_keeps_failed_runs_and_returns_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            out = Path(temp) / "ab.json"
            with patch.object(sys, "argv", ["ab", "unused", "1", "1", str(out), "--count 1"]), \
                    patch.object(ab, "run", side_effect=lambda *a, **k: {"error": "failed"}), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(ab.main(), 1)
            record = json.loads(out.read_text())["--count 1"]
        self.assertIn("error", record)
        self.assertEqual(record["quad"]["runs"], [{"error": "failed"}])
        self.assertEqual(record["tile"]["runs"], [{"error": "failed"}])


class OwnedHostTests(unittest.TestCase):
    def test_busy_port_is_refused_before_spawn_or_commands(self):
        with patch.object(backend.socket, "create_connection") as connect, \
                patch.object(backend.subprocess, "Popen") as spawn, \
                patch.object(backend.subprocess, "run") as call:
            with self.assertRaisesRegex(RuntimeError, "already in use"):
                backend.serve_capture("samples/anim", 1, Path("unused.png"), {}, 12345)
        connect.assert_called_once()
        spawn.assert_not_called()
        call.assert_not_called()

    def test_dead_child_never_probes_another_host(self):
        proc = Mock(pid=42)
        proc.poll.return_value = 1
        with tempfile.TemporaryDirectory() as temp:
            with patch.object(backend, "refuse_busy_port"), \
                    patch.object(backend.subprocess, "Popen", return_value=proc), \
                    patch.object(backend.subprocess, "run") as call:
                with self.assertRaisesRegex(RuntimeError, "exited 1"):
                    backend.serve_capture("samples/anim", 1, Path(temp) / "out.png", {}, 12345)
        call.assert_not_called()
        proc.terminate.assert_not_called()

    def test_unmatched_ready_record_cannot_authorize_commands(self):
        proc = Mock(pid=42)
        proc.poll.return_value = None
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "ready.log"
            log.write_text(json.dumps({"pid": 999, "port": 12345,
                                      "url": "http://127.0.0.1:12345", "project": "wrong"}))
            with patch.object(backend.time, "monotonic", side_effect=[0, 1, 61]), \
                    patch.object(backend.time, "sleep"), \
                    patch.object(backend.subprocess, "run") as call:
                with self.assertRaisesRegex(RuntimeError, "readiness timed out"):
                    backend.wait_ready(proc, log, "samples/anim", 12345)
        call.assert_not_called()

    def test_timeout_terminates_kills_and_reaps_only_owned_child(self):
        proc = Mock(pid=42)
        proc.poll.return_value = None
        proc.wait.side_effect = [subprocess.TimeoutExpired("owned", 15), 0]
        with tempfile.TemporaryDirectory() as temp:
            with patch.object(backend, "refuse_busy_port"), \
                    patch.object(backend.subprocess, "Popen", return_value=proc), \
                    patch.object(backend, "wait_ready", return_value=12345), \
                    patch.object(backend.subprocess, "run",
                                 side_effect=subprocess.TimeoutExpired("step", 120)):
                with self.assertRaises(subprocess.TimeoutExpired):
                    backend.serve_capture("samples/anim", 1, Path(temp) / "out.png", {}, 12345)
        proc.terminate.assert_called_once()
        proc.kill.assert_called_once()
        self.assertEqual(proc.wait.call_count, 2)

    def test_readiness_timeout_stops_owned_child_without_commands(self):
        proc = Mock(pid=42)
        proc.poll.return_value = None
        with tempfile.TemporaryDirectory() as temp:
            with patch.object(backend, "refuse_busy_port"), \
                    patch.object(backend.subprocess, "Popen", return_value=proc), \
                    patch.object(backend.time, "monotonic", side_effect=[0, 1, 61]), \
                    patch.object(backend.time, "sleep"), \
                    patch.object(backend.subprocess, "run") as call:
                with self.assertRaisesRegex(RuntimeError, "readiness timed out"):
                    backend.serve_capture("samples/anim", 1, Path(temp) / "out.png", {}, 12345)
        call.assert_not_called()
        proc.terminate.assert_called_once()
        proc.wait.assert_called_once()

    def test_ready_child_capture_succeeds_and_is_stopped(self):
        proc = Mock(pid=42)
        proc.poll.return_value = None
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "capture.png"
            source.write_bytes(b"fixture")
            out = Path(temp) / "out.png"

            def spawn(*args, **kwargs):
                kwargs["stdout"].write(json.dumps({"pid": 42, "port": 12345,
                    "url": "http://127.0.0.1:12345",
                    "project": str((backend.ROOT / "samples/anim").resolve())}) + "\n")
                kwargs["stdout"].flush()
                return proc

            replies = [result(), result(stdout='{"tick": 1}'),
                       result(stdout=json.dumps({"path": str(source)})),
                       result(stdout=json.dumps({"path": str(source)}))]
            with patch.object(backend, "refuse_busy_port"), \
                    patch.object(backend.subprocess, "Popen", side_effect=spawn), \
                    patch.object(backend.subprocess, "run", side_effect=replies):
                record = backend.serve_capture("samples/anim", 1, out, {}, 12345)
            self.assertEqual(record["world_tick"], 1)
            self.assertEqual(out.read_bytes(), b"fixture")
        proc.terminate.assert_called_once()
        proc.wait.assert_called_once()


class ComparisonTests(unittest.TestCase):
    def test_missing_capture_is_retained_and_returns_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            summary = Path(temp) / "summary.json"
            argv = ["build", "--before", "unused", "--backends", "metal", "--cases", "splats",
                    "--modes", "msaa", "--out", temp, "--summary", str(summary)]
            with patch.object(sys, "argv", argv), \
                    patch.object(build.subprocess, "run", return_value=result(1)), \
                    contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                code = build.main()
            record = json.loads(summary.read_text())
        self.assertEqual(code, 1)
        self.assertEqual(record["successful_pairs"], 0)
        self.assertEqual(record["failed_pairs"], 1)
        self.assertIn("error", record["results"][0])

    def test_successful_pair_returns_success(self):
        def run(argv, **kwargs):
            if "image_diff" in argv[0]:
                return result(stdout=json.dumps({"rmse_8bit": 0, "max_diff_8bit": 0,
                                                 "pixels_over_percent": {">1": 0}}))
            Path(argv[-1]).write_bytes(b"fixture")
            return result()

        with tempfile.TemporaryDirectory() as temp:
            argv = ["build", "--before", "unused", "--backends", "metal", "--cases", "splats",
                    "--modes", "msaa", "--out", temp]
            with patch.object(sys, "argv", argv), patch.object(build.subprocess, "run", side_effect=run), \
                    contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(build.main(), 0)

    def test_failed_coverage_does_not_become_zero_difference(self):
        with tempfile.TemporaryDirectory() as temp:
            argv = ["web", "http://127.0.0.1:1/", "--name", "case", "--out", temp]
            with patch.object(sys, "argv", argv), \
                    patch.object(web, "run", return_value=({}, {"error": "no coverage"}, [])), \
                    patch.object(web, "pixels") as pixels, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(web.main(), 1)
            record = json.loads((Path(temp) / "case.json").read_text())
        self.assertIn("error", record)
        self.assertNotIn("coverage_l1", record)
        self.assertEqual(len(record["coverage_errors"]), 2)
        pixels.assert_not_called()

    def test_explicit_valid_empty_coverage_is_still_valid(self):
        with tempfile.TemporaryDirectory() as temp:
            argv = ["web", "http://127.0.0.1:1/", "--name", "case", "--out", temp]
            with patch.object(sys, "argv", argv), \
                    patch.object(web, "run", return_value=({}, {"coverage": []}, [])), \
                    patch.object(web, "pixels", return_value=None), patch.object(web, "shrink"), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(web.main(), 0)


if __name__ == "__main__":
    unittest.main()
