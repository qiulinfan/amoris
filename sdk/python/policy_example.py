"""A reactive learned player on the environment interface (docs/design/environment.md, Learning).

Where train_example.py learns one open-loop plan, this learns a policy that reads the observation
every few ticks: six features from the sprites sample's state (a bias, the nearest coin's offset,
whether the player stands, its speed, the wall it touches) go through two rows of weights, one
deciding the move and one the jump. The weights are learned by the cross-entropy method over
episodes that start the player at several places along the level, so the policy has to react to
where the coins are rather than replay a route. The scripted "run right, jump now and then" player
is scored on the same starts for comparison.

    python3 sdk/python/policy_example.py --generations 8 --population 12 --envs 4
"""
import argparse
import json
import math
import os
import random
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pocket_env import PocketEnvPool  # noqa: E402

FEATURES = ["bias", "coin.dx", "coin.dy", "grounded", "vx", "wall"]
START_SPOTS = [-7.5, -2.3, 2.0]   # x along the sprites level's ground (the default start is -2.3)
GROUND_Y = -3.0


def features(obs):
    s = obs["state"]
    return [1.0,
            max(-1.0, min(1.0, s.get("coin.dx", 0.0) / 5.0)),
            max(-1.0, min(1.0, s.get("coin.dy", 0.0) / 3.0)),
            1.0 if s.get("player.grounded") else 0.0,
            max(-1.0, min(1.0, s.get("player.vx", 0.0) / 6.0)),
            float(s.get("player.wall", 0))]


def act(weights, obs):
    """The action a weight vector takes on an observation: the move's sign and whether to jump."""
    f = features(obs)
    n = len(FEATURES)
    move = sum(w * x for w, x in zip(weights[:n], f))
    jump = sum(w * x for w, x in zip(weights[n:], f))
    action = {"move_x": 1 if move > 0.1 else (-1 if move < -0.1 else 0)}
    if jump > 0 and obs["state"].get("player.grounded"):
        action["jump"] = True
    return action


def scripted(obs):
    """The comparison player: run right, jump every forty ticks."""
    action = {"move_x": 1}
    if obs["t"] % 40 == 0:
        action["jump"] = True
    return action


def play_policy(env, policy, seed, start_x, hold, max_ticks):
    """One episode from a start spot; returns the score."""
    obs = env.reset(seed=seed, max_ticks=max_ticks)
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": start_x, "y": GROUND_Y, "z": 0}}})
    env.command("world.set", {"entity": "Player", "component": "Body2D", "value": {"velocity": {"x": 0, "y": 0}}})
    obs = env.observe()
    while not obs["done"]:
        obs = env.step(policy(obs), ticks=hold)
    return obs["score"] if obs["score"] is not None else 0.0


def evaluate(pool, policies, seed, spots, hold, max_ticks):
    """Mean score of each policy over the start spots, the episodes spread over the pool."""
    jobs = [(k, x) for k in range(len(policies)) for x in spots]
    scores = [[] for _ in policies]
    for first in range(0, len(jobs), len(pool)):
        batch = jobs[first:first + len(pool)]
        results = pool._fan(lambda env, k, x: play_policy(env, policies[k], seed, x, hold, max_ticks), [(k, x) for k, x in batch])
        for (k, _x), score in zip(batch, results):
            scores[k].append(score)
    return [sum(s) / len(s) for s in scores]


def train(project="sprites", generations=8, population=12, envs=4, spots=None, hold=4, elite=0.25, seed=1, max_ticks=300, log=print):
    spots = spots or START_SPOTS
    rng = random.Random(seed)
    n = 2 * len(FEATURES)
    mean = [0.0] * n
    std = [1.0] * n
    history = []
    started = time.time()
    with PocketEnvPool(project, envs, seed=seed) as pool:
        baseline = evaluate(pool, [scripted], seed, spots, hold, max_ticks)[0]
        log(f"scripted player: mean {baseline:.2f} over {len(spots)} starts")
        best_weights = list(mean)
        best_score = -1.0
        for gen in range(generations):
            candidates = [[rng.gauss(mean[i], std[i]) for i in range(n)] for _ in range(population)]
            policies = [(lambda w: (lambda obs: act(w, obs)))(w) for w in candidates]
            scores = evaluate(pool, policies, seed, spots, hold, max_ticks)
            order = sorted(range(population), key=lambda k: -scores[k])
            keep = order[: max(2, int(population * elite))]
            for i in range(n):
                m = sum(candidates[k][i] for k in keep) / len(keep)
                v = sum((candidates[k][i] - m) ** 2 for k in keep) / len(keep)
                mean[i] = 0.7 * m + 0.3 * mean[i]
                std[i] = max(0.7 * math.sqrt(v) + 0.3 * std[i], 0.05)
            if scores[order[0]] > best_score:
                best_score = scores[order[0]]
                best_weights = list(candidates[order[0]])
            gen_mean = sum(scores) / population
            history.append({"generation": gen, "best": scores[order[0]], "mean": round(gen_mean, 3), "seconds": round(time.time() - started, 1)})
            log(f"generation {gen}: best {scores[order[0]]:.2f}, mean {gen_mean:.2f} ({population * len(spots)} episodes, {time.time() - started:.1f} s)")
        per_start = {}
        for x in spots:
            per_start[str(x)] = {"learned": evaluate(pool, [lambda obs: act(best_weights, obs)], seed, [x], hold, max_ticks)[0], "scripted": evaluate(pool, [scripted], seed, [x], hold, max_ticks)[0]}
    return {"features": FEATURES, "starts": spots, "hold": hold, "max_ticks": max_ticks, "scripted": baseline, "history": history, "best": best_score, "weights": {"move": best_weights[: len(FEATURES)], "jump": best_weights[len(FEATURES):]}, "per_start": per_start}


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="learn a reactive player for a Pocket project through the environment interface")
    ap.add_argument("--project", default="sprites")
    ap.add_argument("--generations", type=int, default=8)
    ap.add_argument("--population", type=int, default=12)
    ap.add_argument("--envs", type=int, default=4, help="runtimes playing in parallel")
    ap.add_argument("--hold", type=int, default=4, help="ticks between decisions")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--ticks", type=int, default=300, help="ticks per episode")
    ap.add_argument("--spots", type=float, nargs="*", help="start x positions along the ground")
    ap.add_argument("--out", help="write the result as JSON here")
    a = ap.parse_args()
    result = train(a.project, a.generations, a.population, a.envs, a.spots or None, a.hold, seed=a.seed, max_ticks=a.ticks)
    if a.out:
        with open(a.out, "w") as f:
            json.dump(result, f, indent=2)
            f.write("\n")
    print(f"learned {result['best']:.2f} against scripted {result['scripted']:.2f} (mean coins over {len(result['starts'])} starts)")
