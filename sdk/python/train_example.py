"""A learned player on the environment interface (docs/design/environment.md, Learning).

The cross-entropy method learns an open-loop plan for the sprites sample: a sequence of macro
actions (left, right, right and jump, left and jump), each held for a number of ticks, scored by
the coins collected within an episode. Every generation plays its population in parallel
runtimes (PocketEnvPool), keeps the best plans and moves the action probabilities toward them.
It is small on purpose: the point is that a program can play, be scored and improve through
env.reset/step alone, deterministically (the same seed gives the same run).

    python3 sdk/python/train_example.py --generations 8 --population 16 --envs 4
"""
import argparse
import json
import os
import random
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pocket_env import PocketEnvPool  # noqa: E402

MACROS = [
    {"move_x": -1},                    # left
    {"move_x": 1},                     # right
    {"move_x": 1, "jump": True},       # right and jump
    {"move_x": -1, "jump": True},      # left and jump
]


def play_plan(env, plan, seed, hold, max_ticks):
    """One episode under a plan: each macro held for `hold` ticks (jumps pressed once at its start)."""
    obs = env.reset(seed=seed, max_ticks=max_ticks)
    for macro in plan:
        if obs["done"]:
            break
        obs = env.step(macro, ticks=hold)
    return obs["score"] if obs["score"] is not None else 0.0, obs["t"]


def train(project="sprites", generations=8, population=16, envs=4, plan_length=20, hold=30, elite=0.25, seed=1, max_ticks=600, log=print):
    rng = random.Random(seed)
    probs = [[1.0 / len(MACROS)] * len(MACROS) for _ in range(plan_length)]   # per step, one distribution over macros
    history = []
    started = time.time()
    with PocketEnvPool(project, envs, seed=seed) as pool:
        for gen in range(generations):
            plans = [[rng.choices(range(len(MACROS)), weights=probs[i])[0] for i in range(plan_length)] for _ in range(population)]
            scores = [0.0] * population
            # Play the population in parallel: each environment takes every n-th plan.
            for first in range(0, population, len(pool)):
                batch = list(range(first, min(first + len(pool), population)))
                results = pool._fan(lambda env, k: play_plan(env, [MACROS[m] for m in plans[k]], seed, hold, max_ticks), [(k,) for k in batch])
                for k, (score, _t) in zip(batch, results):
                    scores[k] = score
            order = sorted(range(population), key=lambda k: -scores[k])
            keep = order[: max(1, int(population * elite))]
            for i in range(plan_length):
                counts = [0.0] * len(MACROS)
                for k in keep:
                    counts[plans[k][i]] += 1.0
                total = sum(counts)
                probs[i] = [0.7 * (c / total) + 0.3 * probs[i][m] for m, c in enumerate(counts)]   # smoothed toward the elites
            best = scores[order[0]]
            mean = sum(scores) / population
            history.append({"generation": gen, "best": best, "mean": round(mean, 3), "best_plan": [MACROS[m] for m in plans[order[0]]], "seconds": round(time.time() - started, 1)})
            log(f"generation {gen}: best {best}, mean {mean:.2f} ({population} episodes, {time.time() - started:.1f} s)")
    return history


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="learn a plan for a Pocket project through the environment interface")
    ap.add_argument("--project", default="sprites")
    ap.add_argument("--generations", type=int, default=8)
    ap.add_argument("--population", type=int, default=16)
    ap.add_argument("--envs", type=int, default=4, help="runtimes playing in parallel")
    ap.add_argument("--plan-length", type=int, default=20)
    ap.add_argument("--hold", type=int, default=30, help="ticks each macro action is held")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--ticks", type=int, default=600, help="ticks per episode")
    ap.add_argument("--out", help="write the history as JSON here")
    a = ap.parse_args()
    hist = train(a.project, a.generations, a.population, a.envs, a.plan_length, a.hold, seed=a.seed, max_ticks=a.ticks)
    if a.out:
        with open(a.out, "w") as f:
            json.dump(hist, f, indent=2)
            f.write("\n")
    print(f"best score {max(h['best'] for h in hist)} after {len(hist)} generations")
