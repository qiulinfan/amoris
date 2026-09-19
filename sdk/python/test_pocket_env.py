"""Tests of the environment client against the built runtime and the sprites sample.

Run from the repository root after `pocket build` and `pocket ts samples/sprites`
(`pocket test` runs this file as its `python` module when python3 is on PATH):

    python3 sdk/python/test_pocket_env.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pocket_env import PocketEnv, PocketEnvPool, PocketError, play  # noqa: E402
from train_example import train  # noqa: E402
from policy_example import act, train as train_policy  # noqa: E402
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tools", "scripts"))
from agent_eval import run as run_benchmark  # noqa: E402


class SpritesEpisodes(unittest.TestCase):
    def test_reset_step_observe(self):
        with PocketEnv("sprites") as env:
            d = env.describe()
            self.assertIn("move_x", d["actions"])
            self.assertIn("jump", d["actions"])
            self.assertEqual(d["tick_rate"], 60)
            first = env.reset(seed=3)
            self.assertEqual(first["episode"], 1)
            self.assertEqual(first["t"], 0)
            self.assertEqual(first["state"]["score"], 0)
            self.assertFalse(first["done"])
            self.assertEqual(first["reward"], 0)
            walked = env.step({"move_x": 1}, ticks=30)
            self.assertEqual(walked["t"], 30)
            self.assertGreater(walked["state"]["player.x"], 0.5)
            self.assertGreaterEqual(walked["score"], 1)                      # the coin ahead
            self.assertEqual(walked["reward"], walked["score"])
            self.assertTrue(any(e["type"] == "coin.collected" for e in walked["events"]))
            jumped = env.step({"jump": True}, ticks=60)
            self.assertEqual(jumped["t"], 90)
            self.assertTrue(any(e["type"] == "player.jumped" for e in jumped["events"]))
            self.assertTrue(any(e["type"] == "body2d.landed" for e in jumped["events"]))
            self.assertEqual(env.observe()["t"], 90)
            # A second episode with the same seed and acts is the same episode.
            again = env.reset(seed=3)
            self.assertEqual(again["episode"], 2)
            self.assertEqual(again["t"], 0)
            self.assertEqual(again["state"]["player.x"], 0)
            self.assertEqual(env.step({"move_x": 1}, ticks=30)["state"], walked["state"])
            # Unknown actions are refused; max_ticks ends an episode.
            with self.assertRaises(PocketError) as caught:
                env.step({"fly": 1})
            self.assertEqual(caught.exception.code, "no_such_action")
            env.reset(max_ticks=10)
            self.assertTrue(env.step(ticks=10)["done"])
            # Any other command works through the same connection.
            self.assertGreater(env.command("world.summary")["entities"], 0)

    def test_pool_plays_parallel_episodes(self):
        with PocketEnvPool("sprites", 2) as pool:
            first = pool.reset(seeds=[1, 2], max_ticks=120)
            self.assertEqual([o["t"] for o in first], [0, 0])
            walked = pool.step([{"move_x": 1}, {"move_x": -1}], ticks=30)
            self.assertEqual([o["t"] for o in walked], [30, 30])
            self.assertGreater(walked[0]["state"]["player.x"], 0.5)
            self.assertLess(walked[1]["state"]["player.x"], -0.5)
            same = pool.step({"jump": True}, ticks=90)
            self.assertTrue(all(o["done"] for o in same))

    def test_training_improves_or_holds(self):
        hist = train("sprites", generations=2, population=4, envs=2, plan_length=6, hold=30, seed=3, max_ticks=180, log=lambda *a: None)
        self.assertEqual(len(hist), 2)
        self.assertGreaterEqual(max(h["best"] for h in hist), 1)     # some plan collects a coin
        self.assertGreaterEqual(hist[1]["mean"], hist[0]["mean"] - 1e-9)  # the elites pull the population up (or it stays)

    def test_policy_reads_the_observation(self):
        # A reactive policy: the weights decide from the state; a coin to the left with a leftward
        # weight moves left, and the learning loop runs (two small generations over two starts).
        obs = {"t": 0, "state": {"coin.dx": -2.0, "coin.dy": 0.0, "player.grounded": True, "player.vx": 0.0, "player.wall": 0}}
        toward = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0] + [0.0] * 6      # move by the coin's direction, never jump
        self.assertEqual(act(toward, obs)["move_x"], -1)
        self.assertNotIn("jump", act(toward, obs))
        jumper = [0.0] * 6 + [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]      # always jump when standing
        self.assertTrue(act(jumper, obs).get("jump"))
        result = train_policy("sprites", generations=2, population=4, envs=2, spots=[-2.3, 2.0], hold=4, seed=1, max_ticks=120, log=lambda *_: None)
        self.assertEqual(len(result["history"]), 2)
        self.assertEqual(len(result["weights"]["move"]), 6)
        self.assertGreaterEqual(result["best"], 0.0)
        self.assertIn("-2.3", result["per_start"])

    def test_agent_benchmark_reference_passes_and_null_fails(self):
        # The harness checks itself: its own solutions pass, an empty runner does not.
        ref = run_benchmark("reference", ["spawn_named", "tile_edit", "count_overlap", "expose_count"], log=lambda *_: None)
        self.assertEqual(ref["passed"], 4, ref)
        null = run_benchmark("null", ["spawn_named", "count_overlap"], log=lambda *_: None)
        self.assertEqual(null["passed"], 0, null)
        self.assertEqual(null["tasks"][1]["answer"], None)

    def test_play_helper(self):
        rows = play("sprites", policy="right", episodes=1, ticks=240, step=4, seed=1)
        self.assertEqual(len(rows), 1)
        self.assertGreaterEqual(rows[0]["score"], 1)
        self.assertEqual(rows[0]["t"], 240)


if __name__ == "__main__":
    unittest.main()
