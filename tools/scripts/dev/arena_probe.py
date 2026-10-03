"""The arena scenario's play, step by step: Red runs north at the ball while Blue stands in its way
(samples/arena/scenarios/goal.ts). Prints the ball's, Red's and Blue's places every few ticks, to
see whether the ball glances past Blue into the goal or Blue is walked back.

    python tools/scripts/dev/arena_probe.py [ticks] [--config release]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from runtime import Runtime  # noqa: E402

ticks = int(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1].isdigit() else 330
config = sys.argv[sys.argv.index("--config") + 1] if "--config" in sys.argv else "release"
with Runtime("arena", 4719, release=config == "release") as rt:
    rt.rpc("step", {"ticks": 12})
    rt.rpc("input.hold", {"action": "move_z", "value": -1, "ticks": ticks})

    def where(name):
        p = rt.rpc("world.get", {"entity": name, "component": "Transform"})["position"]
        return f"{name} ({p['x']:6.2f} {p['z']:6.2f})"

    for t in range(0, ticks, 15):
        rt.rpc("step", {"ticks": 15})
        state = rt.rpc("state", {})["state"]
        print(f"tick {t + 15:4}  {where('Ball')}  {where('Red')}  {where('Blue')}  red {state.get('red')}")
