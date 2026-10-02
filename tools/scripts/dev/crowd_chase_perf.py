#!/usr/bin/env python3
"""Navigation under a crowd that chases (docs/design/navigation.md): N humanoids in the fps yard
follow the player, who walks a circle; prints the navigation system's time and the replans a tick.

    python3 tools/scripts/dev/crowd_chase_perf.py [N]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import Runtime  # noqa: E402

N = int(sys.argv[1]) if len(sys.argv) > 1 else 300
with Runtime("fps", port=4815, size="640x360", release=True) as r:
    r.rpc("step", {"ticks": 5})
    for i in range(3):
        r.rpc("world.destroy", {"entity": f"Raider{i}"})
    r.rpc("script.eval", {"source": f"""
for (let i = 0; i < {N}; i++) {{
  const x = -16 + (i % 20) * 1.6, z = -16 + Math.floor(i / 20) * 1.6 % 30;
  world.spawn("Mob" + i, {{ components: {{ Transform: {{ position: {{ x, y: 0, z }} }}, NavAgent: {{ speed: 3, mode: "follow", target: "Player" }} }} }});
}}
let t = 0;
onTick((s) => {{ t += s.dt; world.set("Player", "Transform", {{ position: {{ x: Math.cos(t * 0.5) * 8, y: 0.9, z: Math.sin(t * 0.5) * 8 }} }}); }});
0"""})
    r.rpc("step", {"ticks": 30})
    r.rpc("perf", {"reset": True})
    r.rpc("step", {"ticks": 240})
    p = r.rpc("perf")
    replans = 0
    for _ in range(60):   # nav.info counts the last tick's
        r.rpc("step", {"ticks": 1})
        replans += r.rpc("nav.info", {}).get("replans", 0)
    nav = next((s for s in p["systems"] if s["system"] == "navigation"), {})
    print(f"{N} chasers: navigation {nav.get('avg_ms', 0):.3f} ms a tick (max {nav.get('max_ms', 0):.2f}), tick {p['tick']['avg_ms']:.2f} ms, replans {replans / 60:.1f} a tick")
