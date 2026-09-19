"""Tests of the environment client against the built runtime and the sprites sample.

Run from the repository root after `pocket build` and `pocket ts samples/sprites`
(`pocket test` runs this file as its `python` module when python3 is on PATH):

    python3 sdk/python/test_pocket_env.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pocket_env import PocketEnv, PocketError, play  # noqa: E402


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

    def test_play_helper(self):
        rows = play("sprites", policy="right", episodes=1, ticks=240, step=4, seed=1)
        self.assertEqual(len(rows), 1)
        self.assertGreaterEqual(rows[0]["score"], 1)
        self.assertEqual(rows[0]["t"], 240)


if __name__ == "__main__":
    unittest.main()
