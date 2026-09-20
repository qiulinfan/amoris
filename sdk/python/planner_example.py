"""A player that plans on the environment interface (docs/design/environment.md, Learning).

Runs are deterministic per seed, so a plan can be tried before it is taken: at every decision
the planner brings a spare runtime to the decision point, holds one candidate macro action
(left, right, either with a jump, or nothing) for `hold` ticks and lets `horizon` more ticks
run, and reads the score; the candidates play across a pool in parallel, the best one is
committed, and the next decision starts from there. The decision points are save slots: the
runtime that tried the winning candidate wrote one right after its hold, and every runtime
loads it for the next decision, so a candidate costs its own ticks and not a replay of the
plan so far. The runtimes share the save directory, and a save carries the episode's clock and
the script's own state (the sprites sample saves its score, its facing and the coins' bobbing
tweens in their phase), so a branch plays out exactly as the run it was taken from; the
committed plan replayed from the start reaches the same score, which the test checks. Projects
without save handlers can still plan with `snapshots=False`, which replays the history instead.

    python3 sdk/python/planner_example.py --decisions 12 --hold 20 --horizon 20 --envs 5
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pocket_env import PocketEnvPool  # noqa: E402

MACROS = [
    {"move_x": 1},
    {"move_x": -1},
    {"move_x": 1, "jump": True},
    {"move_x": -1, "jump": True},
    {},
]


def replay(env, seed, history, hold, max_ticks):
    """A fresh episode brought to the end of the committed history."""
    obs = env.reset(seed=seed, max_ticks=max_ticks)
    for macro in history:
        obs = env.step(macro, ticks=hold)
    return obs


def value(obs):
    """What a candidate is worth: the score, and for ties how near the next coin is."""
    s = obs.get("state", {})
    near = abs(float(s.get("coin.dx", 0.0))) + abs(float(s.get("coin.dy", 0.0)))
    return float(obs.get("score", 0.0)) - 0.01 * near


def try_macro(env, seed, history, macro, hold, horizon, max_ticks, slot=None, keep=None):
    """The observation after the decision point (the save `slot`, or the history replayed), the
    macro held, and the horizon run on with it; `keep` names the slot written right after the
    hold, the next decision point should this macro win."""
    if slot is None:
        replay(env, seed, history, hold, max_ticks)
    else:
        env.command("save.read", {"slot": slot})
    obs = env.step(macro, ticks=hold)
    if keep is not None:
        env.command("save.write", {"slot": keep})
    if horizon > 0 and not obs.get("done"):
        obs = env.step(macro, ticks=horizon)
    return obs


def plan(project="sprites", decisions=12, hold=20, horizon=20, envs=5, seed=1, max_ticks=600, macros=MACROS, snapshots=True, log=print):
    """Build a plan of `decisions` macros by lookahead; returns the history, the score it reaches and the ticks used."""
    t0 = time.time()
    history = []
    steps = []
    prefix = "plan-%d-%d" % (os.getpid() % 100000, int(time.time()) % 100000)
    slot = None          # the committed decision point, once there is one
    written = []         # every slot this run wrote, removed at the end
    with PocketEnvPool(project, envs, seed=seed, max_ticks=max_ticks) as pool:
        if snapshots:
            pool.reset(max_ticks=max_ticks)   # every runtime has an episode to load a save into
        for d in range(decisions):
            best = None
            for start in range(0, len(macros), len(pool)):
                batch = macros[start:start + len(pool)]
                keeps = ["%s-%d-%d" % (prefix, d, start + i) if snapshots else None for i in range(len(batch))]
                written.extend(k for k in keeps if k)
                results = pool._fan(lambda env, macro, keep: try_macro(env, seed, history, macro, hold, horizon, max_ticks, slot if snapshots else None, keep), list(zip(batch, keeps)))
                for macro, keep, obs in zip(batch, keeps, results):
                    v = value(obs)
                    if best is None or v > best[0]:
                        best = (v, macro, obs, keep)
            v, macro, obs, keep = best
            history.append(macro)
            slot = keep
            steps.append({"decision": d, "macro": macro, "value": round(v, 4), "score": obs.get("score", 0), "t": obs.get("t", 0), "done": bool(obs.get("done"))})
            log(f"decision {d}: {macro} -> score {obs.get('score', 0)} (value {v:.3f}) at t {obs.get('t', 0)}")
            if obs.get("done"):
                # The episode ended inside the horizon: keep holding the macro that far, so the
                # committed plan reaches the same end when it is played on its own.
                history.extend([macro] * max(0, -(-horizon // hold)))
                break
        # The committed plan, played once more from the start: its score is the planner's, and
        # with snapshots it is also the check that every branch was taken from an exact copy.
        final = replay(pool.envs[0], seed, history, hold, max_ticks)
        for s in written:
            try:
                pool.envs[0].command("save.delete", {"slot": s})
            except Exception:  # noqa: BLE001 - a missing slot is nothing to report
                pass
    result = {"project": project, "seed": seed, "hold": hold, "horizon": horizon, "snapshots": snapshots, "decisions": len(history), "history": history, "steps": steps,
              "score": final.get("score", 0), "t": final.get("t", 0), "seconds": round(time.time() - t0, 1)}
    log(f"plan of {len(history)} macros reaches score {result['score']} at t {result['t']} in {result['seconds']} s")
    return result


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="plan a route for a Pocket project by lookahead through the environment interface")
    ap.add_argument("--project", default="sprites")
    ap.add_argument("--decisions", type=int, default=12)
    ap.add_argument("--hold", type=int, default=20, help="ticks each macro action is held")
    ap.add_argument("--horizon", type=int, default=20, help="ticks a candidate runs on before it is judged")
    ap.add_argument("--envs", type=int, default=5, help="runtimes trying candidates in parallel")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--ticks", type=int, default=600, help="ticks per episode")
    ap.add_argument("--replay", action="store_true", help="replay the history per candidate instead of branching from save slots")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    out = plan(a.project, a.decisions, a.hold, a.horizon, a.envs, a.seed, a.ticks, snapshots=not a.replay, log=(lambda *_: None) if a.json else print)
    if a.json:
        print(json.dumps(out, indent=2))
