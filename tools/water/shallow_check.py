"""The shallow-water spike's checks on one machine (docs/spikes/shallow-water.md): the solver's
hash chains natively, under Node and in Chrome, compared with the M5's, its timings, and the
drawing's cost from per-frame GPU traces.

    python tools/water/shallow_check.py [--sizes 128,256,512,1024] [--draw 128,256,512] [--no-chrome]

Builds what it runs (release, wasm32), writes the raw lines and traces under out/shallow-check/
and prints a summary, which ends with `chains: same as the M5` or names each one that differs.
Needs cargo, node and, for the Chrome row, Google Chrome (tools/web_bench.mjs finds it). Run it on
a quiet machine; on a two-GPU laptop, AMORIS_ADAPTER picks the GPU for the drawing rows.
"""

import argparse
import functools
import http.server
import json
import os
import platform
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "out" / "shallow-check"
WASM = ROOT / "target" / "wasm32-unknown-unknown" / "release" / "examples" / "shallow_web.wasm"

# The chains the M5 printed natively (release and debug), under Node and in Chrome 155, at commit
# 155a52ac's solver: the dam break of `bench_scene(128)`, a hash every 60 substeps of 1/120 s.
M5 = {
    "advect": [
        "190eb1aec7fc2dad", "b7ea21a5a8954f0d", "b7304214de5f3635", "3eda93415551990d",
        "652069534a805dcc", "a6b6e0b8a4ddfd82", "a8b78e13f91090da", "04ba01c8ea1f88d9",
        "dcdfbf15369dbf19", "aa917e7e2b8fc9a3",
    ],
    "pipes": [
        "870689ce5060c2d5", "28d7f1ca775a35ed", "990dc43bec4b9f37", "2a9cceb03c561def",
        "d875fa3f4b6a5681", "a0f934d17a4d9a5a", "805a7113a8a896aa", "e43af086bc5f906b",
        "4508cb06a6578933", "d29f48fe05325fac",
    ],
}


def run(cmd, env=None):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    r = subprocess.run(cmd, cwd=ROOT, env={**os.environ, **(env or {})}, capture_output=True,
                       text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stdout[-4000:] + r.stderr[-4000:])
        raise SystemExit(f"failed: {cmd}")
    return r.stdout


def json_lines(text):
    out = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("{"):
            out.append(json.loads(line))
    return out


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve():
    handler = functools.partial(Quiet, directory=str(ROOT))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def gpu_rows(diff):
    """The rows of `frame_trace.py diff` the summary keeps: name -> (A, B, B-A)."""
    rows = {}
    for line in diff.splitlines():
        parts = line.split()
        for name in ("GPU pass interval sum", "GPU interval coverage", "GPU frame span", "opaque+sky"):
            words = name.split()
            if parts[: len(words)] == words and len(parts) >= len(words) + 3:
                rows[name] = tuple(float(x) for x in parts[len(words): len(words) + 3])
    return rows


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--sizes", default="128,256,512,1024")
    p.add_argument("--draw", default="128,256,512")
    p.add_argument("--no-chrome", action="store_true")
    a = p.parse_args()
    sizes = a.sizes.split(",")
    OUT.mkdir(parents=True, exist_ok=True)
    lines = []

    native = run(["cargo", "run", "--release", "-p", "amoris-physics", "--example",
                  "shallow_bench", "--", *sizes])
    lines += json_lines(native)
    debug = run(["cargo", "run", "-p", "amoris-physics", "--example", "shallow_bench", "--", "128"])
    lines += [dict(l, target="native debug") for l in json_lines(debug) if "chain" in l]

    run(["cargo", "build", "-p", "amoris-physics", "--example", "shallow_web", "--target",
         "wasm32-unknown-unknown", "--release"])
    lines += json_lines(run(["node", "crates/amoris-physics/tests/web/shallow.mjs", str(WASM),
                             *sizes]))
    if not a.no_chrome:
        server = serve()
        url = (f"http://127.0.0.1:{server.server_address[1]}/crates/amoris-physics/tests/web/"
               f"shallow.html?wasm=/target/wasm32-unknown-unknown/release/examples/"
               f"shallow_web.wasm&sizes={a.sizes}")
        out = run(["node", "tools/web_bench.mjs", url, "1"], env={"EVAL": "shallowRun()"})
        server.shutdown()
        for line in out.splitlines():
            if line.startswith("eval: "):
                lines += json_lines(json.loads(line[len("eval: "):]))

    draws = []
    if a.draw:
        run(["cargo", "build", "--release", "-p", "amoris-render", "--example", "shallow_draw"])
        exe = ROOT / "target" / "release" / "examples" / (
            "shallow_draw.exe" if os.name == "nt" else "shallow_draw")
        for n in a.draw.split(","):
            traces = {}
            for mode in ("off", "on"):
                trace = OUT / f"draw-{n}-{mode}.json"
                args = [str(exe), "--n", n] + (["--off"] if mode == "off" else [])
                run(args, env={"AMORIS_TRACE": f"frames=240,skip=60,out={trace}"})
                traces[mode] = trace
            diff = run([sys.executable, "tools/render/frame_trace.py", "diff", str(traces["off"]),
                        str(traces["on"])])
            draws.append({"n": int(n), **{k: v for k, v in gpu_rows(diff).items()}})

    (OUT / "lines.jsonl").write_text("\n".join(json.dumps(l) for l in lines) + "\n")
    print(f"\nmachine: {platform.platform()}, {platform.machine()}, {platform.processor()}")
    print("solver, ms a substep (ns a column):")
    for l in lines:
        if "ms_per_substep" in l:
            print(f"  {l['target']:<28} {l['scheme']:<7} {l['n']:>5}²  "
                  f"{l['ms_per_substep']:.3f} ({l['ns_per_column']:.1f})")
    for d in draws:
        cells = ", ".join(f"{k} {v[2]:+.3f} ms" for k, v in d.items() if k != "n")
        print(f"draw {d['n']}²: {cells}")
    differ = []
    for l in lines:
        if "chain" in l and l["chain"] != M5[l["scheme"]]:
            differ.append(f"{l.get('target', 'native release')} {l['scheme']}")
    chains = sum(1 for l in lines if "chain" in l)
    print(f"chains: {'same as the M5' if not differ else 'DIFFER: ' + '; '.join(differ)} "
          f"({chains} compared)")
    (OUT / "summary.json").write_text(json.dumps({"machine": platform.platform(),
                                                  "lines": lines, "draws": draws,
                                                  "differ": differ}, indent=1))


if __name__ == "__main__":
    main()
