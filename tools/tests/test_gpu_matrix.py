"""tools/gpu_matrix.py on fixed libtest output: no GPU, cargo or test binary is started.

    python -m unittest tools/tests/test_gpu_matrix.py
"""
import importlib.util
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gpu_matrix", TOOLS / "gpu_matrix.py")
gpu_matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_matrix)

OUTPUT = """
running 6 tests
test a::draws ... ok
test a::times ... ok
test a::panics - should panic ... ok
test a::slow ... ignored, 4.1M splats
test a::wrong ... FAILED
test a::stuck has been running for over 60 seconds
successes:

---- a::draws stdout ----
lod 2: worst difference 1
Vulkan: no GPU (AMORIS_ADAPTER=780m matches no adapter; have []); skipped
at 5 m, cascade 1: bound 0.2 on a level's error, skipped

---- a::times stdout ----
no GPU timestamps: placement not checked

successes:
    a::draws
    a::times

failures:

---- a::wrong stdout ----
f16 true: worst difference 28

thread 'a::wrong' (11172) panicked at crates\\amoris-render\\tests\\x.rs:9:5:
f16 true: the neural material differs by up to 28
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    a::wrong

test result: FAILED. 3 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.00s
"""


class GpuMatrixTests(unittest.TestCase):
    def test_results_skips_failures_and_hangs(self):
        status, output = gpu_matrix.parse_output(OUTPUT)
        cell = lambda n: gpu_matrix.classify(status.get(n), output.get(n, []))
        # A capability skip makes a pass a skip; a test's own "skipped" case does not.
        self.assertEqual(cell("a::draws"), (
            "skip", ["Vulkan: no GPU (AMORIS_ADAPTER=780m matches no adapter; have []); skipped"],
            ""))
        self.assertEqual(cell("a::times")[:2], ("skip", ["no GPU timestamps: placement not checked"]))
        self.assertEqual(cell("a::panics"), ("pass", [], ""))
        self.assertEqual(cell("a::slow")[0], "ign")
        self.assertEqual(cell("a::wrong"),
                         ("FAIL", [], "f16 true: the neural material differs by up to 28"))
        self.assertEqual(cell("a::stuck")[0], "hang")
        self.assertEqual(cell("a::missing")[0], "crash")

    def test_configurations_name_their_environment(self):
        self.assertEqual(gpu_matrix.parse_config("780m-vulkan"),
                         {"AMORIS_BACKEND": "vulkan", "AMORIS_ADAPTER": "780m"})
        self.assertEqual(gpu_matrix.parse_config("default-dx12-minimal"),
                         {"AMORIS_BACKEND": "dx12", "AMORIS_GPU_MINIMAL": gpu_matrix.MINIMAL})
        # `auto` leaves AMORIS_BACKEND unset, as `cargo xtask check` does.
        self.assertEqual(gpu_matrix.parse_config("780m-auto"), {"AMORIS_ADAPTER": "780m"})
        for bad in ("780m", "780m-gl", "780m-dx12-full", "a-b-c-d"):
            with self.assertRaises(SystemExit):
                gpu_matrix.parse_config(bad)

    def test_the_table_lists_what_did_not_pass_everywhere(self):
        results = {
            "a-dx12": {"adapter": "A", "tests": {
                "lib x": {"status": "pass", "notes": [], "message": ""},
                "lib y": {"status": "pass", "notes": [], "message": ""}}},
            "b-dx12": {"adapter": "B", "tests": {
                "lib x": {"status": "pass", "notes": [], "message": ""},
                "lib y": {"status": "FAIL", "notes": [], "message": "off by 28"}}},
        }
        text = gpu_matrix.table(results, markdown=True)
        self.assertIn("| `lib y` | pass | FAIL [1] |", text)
        self.assertNotIn("`lib x`", text)
        self.assertIn("1. b-dx12, `lib y`: off by 28", text)
        self.assertIn("| totals | 2 pass | 1 pass, 1 FAIL |", text)

    def test_a_configuration_without_a_gpu_is_an_error(self):
        cell = lambda status, notes=(): {"status": status, "notes": list(notes), "message": ""}
        results = {
            # A mistyped adapter: nothing named, every test a "no GPU" skip.
            "780n-dx12": {"adapter": None, "tests": {
                "lib x": cell("skip", ["no GPU: skipped"])}},
            # Capability skips are what the minimal device is for.
            "a-dx12-minimal": {"adapter": "A", "tests": {
                "lib t": cell("skip", ["no GPU timestamps: skipped"]),
                "lib f": cell("skip", ["no shader-f16: f16 not checked"])}},
            "b-vulkan": {"adapter": "B", "tests": {
                "lib v": cell("skip", ["Vulkan: no GPU, skipped"]), "lib w": cell("FAIL")}},
        }
        self.assertEqual(gpu_matrix.problems(results), [
            "780n-dx12: no adapter reported",
            "780n-dx12: 1 tests found no GPU (lib x, ...)",
            "b-vulkan: 1 tests found no GPU (lib v, ...)",
            "b-vulkan: 1 tests failed, hung or crashed",
        ])

    def test_partial_skips_are_skips(self):
        for note in ("no shader-f16: f16 not checked",
                     "no multi-draw: the default device draws batch by batch; multi-draw not "
                     "checked",
                     "no INDIRECT_FIRST_INSTANCE: the default device takes the baseline path; "
                     "first-instance paths not checked"):
            self.assertEqual(gpu_matrix.classify("ok", [note]), ("skip", [note], ""))
        # Naming a backend that is not under test is not a skip of this configuration's own.
        self.assertEqual(
            gpu_matrix.classify("ok", ["Vulkan: not under test (AMORIS_BACKEND names Dx12)"]),
            ("pass", [], ""))

    def test_findings_say_what_failed_where(self):
        cell = lambda status, notes=(): {"status": status, "notes": list(notes), "message": ""}
        results = {
            "a-dx12": {"adapter": "A", "tests": {"lib x": cell("pass"), "lib y": cell("hang")}},
            "b-vulkan": {"adapter": None, "tests": {"lib v": cell("skip", ["no GPU: skipped"])}},
        }
        self.assertEqual(gpu_matrix.findings(results), [
            {"config": "a-dx12", "kind": "failed", "tests": ["lib y"]},
            {"config": "b-vulkan", "kind": "no_adapter", "tests": []},
            {"config": "b-vulkan", "kind": "no_gpu", "tests": ["lib v"]},
        ])

    def test_the_machine_listing_is_read(self):
        text = """
running 1 test
test gpu::tests::the_machine_adapters_are_listed ... ok

successes:

---- gpu::tests::the_machine_adapters_are_listed stdout ----
machine adapter: vulkan 0 DiscreteGpu NVIDIA GeForce RTX 5060 Laptop GPU
machine adapter: vulkan 1 IntegratedGpu AMD Radeon 780M Graphics
machine default adapter: vulkan NVIDIA GeForce RTX 5060 Laptop GPU
machine adapter: dx12 0 DiscreteGpu NVIDIA GeForce RTX 5060 Laptop GPU
machine adapter: dx12 1 IntegratedGpu AMD Radeon 780M Graphics
machine adapter: dx12 2 Cpu Microsoft Basic Render Driver
machine default adapter: dx12 NVIDIA GeForce RTX 5060 Laptop GPU
"""
        adapters, defaults = gpu_matrix.parse_adapters(text)
        self.assertEqual(len(adapters), 5)
        self.assertEqual(adapters[4], {"backend": "dx12", "index": 2, "type": "Cpu",
                                       "name": "Microsoft Basic Render Driver"})
        self.assertEqual(defaults["dx12"], "NVIDIA GeForce RTX 5060 Laptop GPU")
        configs, left_out = gpu_matrix.machine_configs(adapters, defaults, ["dx12", "vulkan"])
        # The default adapter first, each on every native backend; auto on the other one only
        # (cargo xtask check's test step runs the default's); the minimal device on the default.
        self.assertEqual(configs, ["5060-dx12", "5060-vulkan", "780m-dx12", "780m-vulkan",
                                   "780m-auto", "5060-dx12-minimal"])
        self.assertEqual(left_out, [
            "Microsoft Basic Render Driver (dx12): a software adapter",
            "5060 auto: cargo xtask check's test step runs it"])
        for c in configs:
            gpu_matrix.parse_config(c)
        # A check whose test step did not run that configuration asks for it: no AMORIS_ADAPTER
        # and no AMORIS_BACKEND, exactly as the test step runs.
        configs, left_out = gpu_matrix.machine_configs(adapters, defaults, ["dx12", "vulkan"],
                                                       default_auto=True)
        self.assertEqual(configs, ["5060-dx12", "5060-vulkan", "780m-dx12", "780m-vulkan",
                                   "default-auto", "780m-auto", "5060-dx12-minimal"])
        self.assertEqual(left_out, ["Microsoft Basic Render Driver (dx12): a software adapter"])
        self.assertEqual(gpu_matrix.parse_config("default-auto"), {})

    def test_adapters_get_a_word_of_their_own(self):
        names = ["NVIDIA GeForce RTX 5060 Laptop GPU", "AMD Radeon 780M Graphics",
                 "Microsoft Basic Render Driver"]
        token = lambda n: gpu_matrix.adapter_token(n, [o for o in names if o != n])
        self.assertEqual(token(names[0]), "5060")
        self.assertEqual(token(names[1]), "780m")
        # No model number: the first word no other name contains.
        self.assertEqual(gpu_matrix.adapter_token("Apple M5", ["Apple Paravirtual device"]), "m5")
        # A model number part of another name is not one's own.
        arc, uhd = "Intel(R) Arc(TM) A770 Graphics", "Intel(R) UHD Graphics 770"
        self.assertEqual(gpu_matrix.adapter_token(arc, [uhd]), "a770")
        self.assertEqual(gpu_matrix.adapter_token(uhd, [arc]), "uhd")
        # A word another name contains, or a number AMORIS_ADAPTER reads as an index, will not do.
        self.assertEqual(gpu_matrix.adapter_token("Radeon 780", ["Radeon 7800 XT"]), None)
        self.assertEqual(gpu_matrix.adapter_token("GPU 2", ["GPU 1"]), None)

    def test_twin_adapters_are_named_by_index_and_none_means_nothing_to_run(self):
        twins = [{"backend": b, "index": i, "type": "DiscreteGpu", "name": n}
                 for b in ("vulkan",) for i, n in enumerate(["Radeon 780", "Radeon 7800 XT"])]
        configs, left_out = gpu_matrix.machine_configs(
            twins, {"vulkan": "Radeon 7800 XT"}, ["vulkan"])
        self.assertEqual(configs, ["7800-vulkan", "0-vulkan", "7800-vulkan-minimal"])
        self.assertEqual(left_out, ["7800 auto: cargo xtask check's test step runs it",
                                    "Radeon 780 auto: no word of its name names it alone"])
        software = [{"backend": "dx12", "index": 0, "type": "Cpu",
                     "name": "Microsoft Basic Render Driver"}]
        self.assertEqual(gpu_matrix.machine_configs(software, {}, ["dx12", "vulkan"])[0], [])

    def test_merging_keeps_repeated_configurations(self):
        import json
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for i, status in enumerate(("pass", "FAIL")):
                path = Path(d) / f"{i}.json"
                path.write_text(json.dumps({"configs": {
                    "780m-dx12": {"adapter": "A", "tests": {
                        "taa x": {"status": status, "notes": [], "message": ""}}},
                    f"only{i}-dx12": {"adapter": "A", "tests": {}}}}), encoding="utf-8")
                paths.append(path)
            results = gpu_matrix.merge(paths)
        self.assertEqual(sorted(results),
                         ["780m-dx12#1", "780m-dx12#2", "only0-dx12", "only1-dx12"])
        self.assertEqual(results["780m-dx12#2"]["tests"]["taa x"]["status"], "FAIL")

    def test_every_binary_gets_an_empty_driver_cache(self):
        import tempfile
        from unittest import mock
        with tempfile.TemporaryDirectory() as d:
            deps = Path(d) / "release" / "deps"
            deps.mkdir(parents=True)
            bins = {"lib": str(deps / "amoris_render-1.exe"), "taa": str(deps / "taa-2.exe")}
            root = Path(d) / "release" / "gpu-matrix-vkcache"
            old = root / "780m-vulkan" / "taa"
            old.mkdir(parents=True)
            (old / "A9113209E63BE76D3.parc").write_bytes(b"an earlier run's archive")
            (root / "stale.parc").write_bytes(b"left at the top")
            caches = gpu_matrix.driver_caches(bins, ["780m-vulkan", "780m-auto"])
            self.assertEqual(caches[("780m-vulkan", "taa")], old)
            self.assertEqual(caches[("780m-auto", "lib")], root / "780m-auto" / "lib")
            self.assertEqual(len(caches), 4)
            self.assertEqual(len(set(caches.values())), 4)
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["780m-auto", "780m-vulkan"])
            for c in caches.values():
                self.assertTrue(c.is_dir())
                self.assertEqual(list(c.iterdir()), [])
            # An archive that cannot be removed (a process still holds it) stops the run.
            held = PermissionError("held")
            with mock.patch.object(gpu_matrix.shutil, "rmtree", side_effect=held):
                with self.assertRaises(SystemExit):
                    gpu_matrix.driver_caches(bins, ["780m-vulkan"])

    def test_every_process_of_a_configuration_gets_its_driver_cache(self):
        import argparse
        import tempfile
        from unittest import mock
        calls = []

        def run(cmd, **kw):
            calls.append((cmd, kw["env"]))
            if cmd[-1] == "--list":
                return mock.Mock(stdout="t::a: test\n", stderr="", returncode=0)
            if gpu_matrix.ADAPTER_TEST in cmd:
                return mock.Mock(stdout=gpu_matrix.ADAPTER_PREFIX + "A\n", stderr="",
                                 returncode=0)
            return mock.Mock(stdout="test t::a ... ok\n", stderr="", returncode=0)

        args = argparse.Namespace(bins=None, no_ignored=False, filter=None, test_threads=None,
                                  timeout=10)
        with tempfile.TemporaryDirectory() as d, \
                mock.patch.object(gpu_matrix.subprocess, "run", side_effect=run):
            deps = Path(d) / "release" / "deps"
            deps.mkdir(parents=True)
            bins = {"lib": str(deps / "amoris_render-1.exe"), "taa": str(deps / "taa-2.exe")}
            caches = gpu_matrix.driver_caches(bins, ["780m-vulkan"])
            r = gpu_matrix.run_config("780m-vulkan", bins, args, Path(d) / "logs", caches)
        self.assertEqual(r["adapter"], "A")
        # The adapter-naming run, then per binary its listing and its tests.
        self.assertEqual(len(calls), 5)
        for cmd, env in calls:
            b = "taa" if cmd[0] == bins["taa"] else "lib"
            self.assertEqual(env[gpu_matrix.DRIVER_CACHE], str(caches[("780m-vulkan", b)]))
            self.assertEqual(env["AMORIS_BACKEND"], "vulkan")


if __name__ == "__main__":
    unittest.main()
