"""What a script's calls cost inside a runtime: a plain JavaScript loop, a native call that does little,
and the SDK's world.get and world.set, each timed over many calls with script.eval (release build,
the swarm sample). For comparing script hosts and machines (docs/evidence/swarm.md).

    python tools/scripts/dev/script_costs.py [calls]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from runtime import Runtime  # noqa: E402

n = int(sys.argv[1]) if len(sys.argv) > 1 else 30000
cases = {
    "js loop (n iterations of arithmetic)": f"let s = 0; for (let i = 0; i < {n}; i++) s += i * 0.5; s",
    "object allocation (n objects)": f"let s = 0; for (let i = 0; i < {n}; i++) {{ const o = {{ x: i }}; s += o.x; }} s",
    "performance.now (n calls)": f"let s = 0; for (let i = 0; i < {n}; i++) s += performance.now(); s > 0",
    "world.get Transform (n calls)": f"const id = world.find('Bee_0') ?? world.query({{with: ['Transform']}})[0].id; let s = 0; for (let i = 0; i < {n}; i++) s += world.get(id, 'Transform').position.x; s",
    "world.set Transform (n calls)": f"const id = world.query({{with: ['Transform']}})[0].id; for (let i = 0; i < {n}; i++) world.set(id, 'Transform', {{ position: {{ x: 1, y: 2, z: 3 }} }}); 1",
}
with Runtime("swarm", 4721, release=True) as rt:
    rt.rpc("step", {"ticks": 2})
    for label, src in cases.items():
        timed = f"(() => {{ const t0 = performance.now(); const r = (() => {{ {src} }})(); return performance.now() - t0; }})()"
        # script.eval evaluates an expression: wrap the statements so their last value is returned.
        body = src.rsplit(";", 1)
        timed = "(() => { const t0 = performance.now(); " + body[0] + "; const r = " + body[1] + "; return performance.now() - t0; })()"
        ms = rt.rpc("script.eval", {"source": timed})
        if isinstance(ms, dict):
            print(label, "->", ms)
            continue
        print(f"{label}: {ms:.2f} ms, {ms * 1000 / n:.3f} us a call")
