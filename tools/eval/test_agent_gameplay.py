"""Focused tests of the model-facing capability boundary and native course feasibility.

The private evaluator may alter a temporary native fixture to test visibility boundaries;
these setup operations are never available through the player gateway.
"""
import json
import os
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from agent_gameplay import Gameplay, TOOL_NAMES, COURSE_VERSION, tools, replay_actions


POCKET = os.environ.get("AMORIS_TEST_POCKET")


class SchemaTests(unittest.TestCase):
    def test_only_five_disclosed_player_capabilities(self):
        schemas = tools()
        self.assertEqual({t["function"]["name"] for t in schemas}, TOOL_NAMES)
        self.assertTrue(all(t["function"]["parameters"]["additionalProperties"] is False for t in schemas))
        self.assertIn("disclosed game Helm executor", next(t["function"]["description"]
                      for t in schemas if t["function"]["name"] == "navigate"))


class OfflineReceiptTests(unittest.TestCase):
    def test_judge_identifies_course_and_preserves_world_hash(self):
        game = Gameplay.__new__(Gameplay)
        game.fixture_path = "site/demos/agent-sailing"
        game.host = SimpleNamespace(state=lambda: {"tick": 12, "world_hash": "native-world"})
        game._boat = lambda: {"Tally": {"taken": 2, "total": 4, "worth": 3}}
        verdict = game.judge()
        self.assertEqual(verdict["world_hash"], "native-world")
        self.assertEqual(verdict["course_version"], COURSE_VERSION)
        self.assertNotIn("fixture_sha256", verdict)
        self.assertFalse(verdict["success"])

    def test_replay_still_checks_per_action_world_hash_parity(self):
        expected = {"sequence": 0, "tool": "wait", "arguments": {"ticks": 1},
                    "before": {"tick": 0, "world_hash": "before"},
                    "after": {"tick": 1, "world_hash": "after"},
                    "result": {}, "native_intents": []}

        class StubReplay:
            divergent = False
            def __init__(self, *args, **kwargs): self.actions = []
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def dispatch(self, *args):
                action = json.loads(json.dumps(expected))
                if self.divergent: action["after"]["world_hash"] = "different"
                self.actions.append(action)
                return action["result"]
            def judge(self): return {"success": False, "world_hash": "after"}

        with patch("agent_gameplay.Gameplay", StubReplay):
            result = replay_actions([expected], pocket="unused")
            self.assertTrue(result["passed"])
            self.assertEqual(result["parity"][0]["expected_hash"], "after")
            self.assertEqual(result["parity"][0]["actual_hash"], "after")
            StubReplay.divergent = True
            with self.assertRaisesRegex(RuntimeError, "diverged"):
                replay_actions([expected], pocket="unused")


@unittest.skipUnless(POCKET, "Set AMORIS_TEST_POCKET to a verified existing native pocket binary.")
class NativeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="amoris-player-test-")
        self.game = Gameplay(POCKET, run_dir=Path(self.tmp.name))

    def tearDown(self):
        self.game.close()
        self.tmp.cleanup()

    def assert_projection(self, value):
        forbidden = {"world_hash", "hash", "url", "project", "path", "entity", "_entity",
                     "components", "fixture_path", "score", "judge", "grader", "scripts", "debug"}
        def visit(x):
            if isinstance(x, dict):
                self.assertFalse(set(x) & forbidden)
                for v in x.values(): visit(v)
            elif isinstance(x, list):
                for v in x: visit(v)
            elif isinstance(x, str):
                self.assertNotIn("http://", x)
                self.assertNotIn("/Users/", x)
        visit(value)

    def test_range_projection_and_no_hidden_cargo_leak(self):
        result = self.game.dispatch("observe", {})
        self.assert_projection(result)
        cargo = result["observation"]["cargo"]
        self.assertEqual([c["target"] for c in cargo], ["cargo-1", "cargo-2", "cargo-3"])
        self.assertTrue(all(c["distance_m"] <= 30 for c in cargo))
        result = self.game.dispatch("navigate", {"target": "cargo-4"})
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["code"], "target_not_observed")
        self.assert_projection(result)

    def test_forbidden_methods_fields_and_bounds_do_not_change_world(self):
        self.game.dispatch("observe", {})
        requests = [("world.get", {"entity": "Crate4"}), ("scripts.read", {"path": "main.ts"}),
                    ("helm", {"steer": 0, "sail": 1, "entity": "Crate4"}),
                    ("observe", {"radius": 999}), ("helm", {"steer": 1.001, "sail": 1}),
                    ("helm", {"steer": True, "sail": 1}),
                    ("helm", {"steer": float("nan"), "sail": 1}),
                    ("helm", {"steer": 0, "sail": -0.1}),
                    ("navigate", {"target": "Crate1"}),
                    ("take", {"target": "https://example.com"})]
        requests += [("wait", {"ticks": n}) for n in [-1, 0, 121, True, 1.5, None]]
        for name, args in requests:
            before = self.game.host.state()
            result = self.game.dispatch(name, args)
            self.assertFalse(result["ok"], (name, args))
            self.assertEqual(before, self.game.host.state())
            self.assert_projection(result)

    def test_alias_becomes_invalid_when_cargo_leaves_sight(self):
        self.game.dispatch("observe", {})
        self.game.host.call("world.edit", {"ops": [{"set": {"entity": "Crate1", "component": "Transform",
                            "value": {"position": [100, 0, 0]}}}]})
        before = self.game.host.state()
        result = self.game.dispatch("navigate", {"target": "cargo-1"})
        self.assertEqual(result["error"]["code"], "target_not_observed")
        self.assertEqual(before, self.game.host.state())
        self.assertNotIn("cargo-1", [c["target"] for c in result["observation"]["cargo"]])

    def test_collected_alias_cannot_be_reused(self):
        self.game.dispatch("observe", {})
        position = self.game.host.get("Sloop", "Transform")["Transform"]["position"]
        self.game.host.call("world.edit", {"ops": [{"set": {"entity": "Crate1", "component": "Transform",
                            "value": {"position": [position[0] + 1, position[1], position[2]]}}}]})
        result = self.game.dispatch("take", {"target": "cargo-1"})
        self.assertEqual(result["observation"]["progress"]["collected"], 1)
        before = self.game.host.state()
        result = self.game.dispatch("take", {"target": "cargo-1"})
        self.assertEqual(result["error"]["code"], "target_not_observed")
        self.assertEqual(before, self.game.host.state())

    def test_observe_does_not_tick_and_take_uses_original_range_rules(self):
        before = self.game.host.state()
        result = self.game.dispatch("observe", {})
        self.assertEqual(before, self.game.host.state())
        self.assertFalse(result["observation"]["cargo"][0]["can_take"])
        result = self.game.dispatch("take", {"target": "cargo-1"})
        self.assertTrue(result["ok"])
        self.assertEqual(result["observation"]["progress"]["collected"], 0)
        self.assertIsNone(self.game.host.get("Sloop", "Crew")["Crew"]["take"])
        self.assertEqual(result["observation"]["tick"], before["tick"] + 1)


if __name__ == "__main__": unittest.main()
