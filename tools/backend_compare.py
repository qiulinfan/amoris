#!/usr/bin/env python3
"""Captures the same frames on two native backends and compares them pixel for pixel.

docs/bench/dx12.md: Direct3D 12 against Vulkan on each adapter of a two-GPU laptop. Every case is
a deterministic frame: the renderer examples' offscreen captures (many_cubes, splats), static
project scenes drawn at a fixed time (showcase_bench --capture) and, for projects that need the
game runtime, `pocket serve` paused, stepped to a tick and captured through the host's `capture`
method (captured twice; the second frame is the one compared, drawn at the tick's own poses).

Build first: `cargo build --release -p pocket-render --examples` and `cargo build --release -p
pocket-app`. Then, from the repository's root:

    python tools/backend_compare.py --adapters nvidia,780m --out target/backend-compare \
        --evidence docs/evidence/dx12/compare --summary docs/evidence/dx12/compare.json

Each pair is compared by the `image_diff` example (RMSE and maximum difference in 8-bit units,
pixels over thresholds); with --evidence a strip (first backend, second backend, heat map) per
pair is kept. `--cross-adapter` also compares the first backend across the first two adapters,
and `--noise` captures each case twice on the first backend and adapter: the comparisons' own
floor.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""


def example(name):
    return str(ROOT / "target" / "release" / "examples" / f"{name}{EXE}")


POCKET = str(ROOT / "target" / "release" / f"pocket{EXE}")
STATIC = ["--width", "1280", "--height", "720", "--frames", "5", "--warm", "5", "--capture"]

# name -> command with {out}, or ("serve", project, ticks)
CASES = {
    "cubes-sphere": [example("many_cubes"), "--capture", "{out}"],
    "cubes-dense": [example("many_cubes"), "--dense", "--capture", "{out}"],
    "cubes-shadows": [example("many_cubes"), "--count", "200000", "--shadows", "--capture", "{out}"],
    "splats": [example("splats"), "--capture", "{out}"],
    "gi-room-static": [example("showcase_bench"), "samples/gi-room", *STATIC, "{out}"],
    "pt-lab-static": [example("showcase_bench"), "samples/pt-lab", *STATIC, "{out}"],
    "sailing": ("serve", "samples/sailing", 120),
    "anim": ("serve", "samples/anim", 90),
    "gi-room": ("serve", "samples/gi-room", 60),
}


def run(cmd, env, timeout):
    t = time.time()
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=timeout)
    return p, time.time() - t


def gpu_line(text):
    for line in text.splitlines():
        if "GPU:" in line or '"adapter"' in line:
            return line.strip()
    return None


def serve_capture(project, ticks, out, env, port):
    log = open(out.with_suffix(".serve.log"), "w")
    proc = subprocess.Popen([POCKET, "serve", project, "--port", str(port)], cwd=ROOT, env=env,
                            stdout=log, stderr=subprocess.STDOUT)
    host = ["--host", str(port)]
    try:
        deadline = time.time() + 60
        while subprocess.run([POCKET, "status", *host], cwd=ROOT, capture_output=True).returncode:
            if time.time() > deadline or proc.poll() is not None:
                raise RuntimeError(f"pocket serve {project} did not come up")
            time.sleep(0.5)
        step = subprocess.run([POCKET, "step", str(ticks), *host], cwd=ROOT, capture_output=True,
                              text=True, timeout=120)
        if step.returncode:
            raise RuntimeError(f"step failed: {step.stdout[-400:]}{step.stderr[-400:]}")
        status = subprocess.run([POCKET, "status", "--json", *host], cwd=ROOT, capture_output=True,
                                text=True, timeout=60)
        try:
            world_tick = json.loads(status.stdout).get("tick")
        except ValueError:
            world_tick = None
        params = json.dumps({"width": 1280, "height": 720})
        result = None
        for _ in range(2):
            c = subprocess.run([POCKET, "call", "capture", params, "--json", *host], cwd=ROOT,
                               capture_output=True, text=True, timeout=300)
            if c.returncode:
                raise RuntimeError(f"capture failed: {c.stdout[-400:]}{c.stderr[-400:]}")
            result = json.loads(c.stdout)
        path = result.get("path") or result.get("result", {}).get("path")
        shutil.copyfile(path, out)
        result["world_tick"] = world_tick
        return result
    finally:
        proc.terminate()
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            proc.kill()
        log.close()


def capture(case, backend, adapter, out, port):
    env = dict(os.environ, POCKET_BACKEND=backend, RUST_LOG="info")
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    spec = CASES[case]
    png = out.relative_to(ROOT).as_posix() if out.is_relative_to(ROOT) else str(out)
    record = {"backend": backend, "adapter": adapter, "png": png}
    t = time.time()
    try:
        if isinstance(spec, tuple):
            _, project, ticks = spec
            result = serve_capture(project, ticks, out, env, port)
            record["capture"] = {k: result.get(k) for k in ("world_tick", "tick", "instances")}
        else:
            cmd = [a.replace("{out}", str(out)) for a in spec]
            p, _ = run(cmd, env, 600)
            record["gpu"] = gpu_line(p.stderr) or gpu_line(p.stdout)
            if p.returncode or not out.exists():
                raise RuntimeError(f"exit {p.returncode}: {p.stderr[-600:]}")
        record["ok"] = True
    except Exception as e:  # recorded, the other cases still run
        record["ok"] = False
        record["error"] = str(e)
    record["seconds"] = round(time.time() - t, 1)
    print(f"  {case:16} {backend:7} {adapter or '-':8} {'ok' if record['ok'] else record['error'][:200]}",
          file=sys.stderr)
    return record


def diff(a, b, strip):
    cmd = [example("image_diff"), str(a), str(b)]
    if strip:
        cmd += ["--strip", str(strip)]
    p, _ = run(cmd, os.environ, 120)
    if p.returncode:
        return {"error": p.stderr[-400:]}
    d = json.loads(p.stdout)
    return {k: d[k] for k in ("rmse_8bit", "psnr_db", "max_diff_8bit", "pixels_over_percent")}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--backends", default="vulkan,dx12", help="first is the reference")
    ap.add_argument("--adapters", default="", help="POCKET_ADAPTER values, comma separated")
    ap.add_argument("--cases", default=",".join(CASES))
    ap.add_argument("--out", default="target/backend-compare")
    ap.add_argument("--evidence", help="directory for the downscaled comparison strips")
    ap.add_argument("--summary", help="write the JSON summary here too")
    ap.add_argument("--cross-adapter", action="store_true")
    ap.add_argument("--noise", action="store_true")
    args = ap.parse_args()
    backends = args.backends.split(",")
    adapters = [a for a in args.adapters.split(",") if a] or [None]
    cases = args.cases.split(",")
    out = ROOT / args.out
    out.mkdir(parents=True, exist_ok=True)
    evidence = ROOT / args.evidence if args.evidence else None
    if evidence:
        evidence.mkdir(parents=True, exist_ok=True)
    port = 7940
    shots, results = {}, {}
    for case in cases:
        results[case] = {}
        for adapter in adapters:
            tag = adapter or "default"
            for backend in backends:
                port += 1
                png = out / f"{case}-{tag}-{backend}.png"
                shots[(case, tag, backend)] = capture(case, backend, adapter, png, port)
            ref, other = shots[(case, tag, backends[0])], shots[(case, tag, backends[1])]
            entry = {"captures": [ref, other]}
            if ref["ok"] and other["ok"]:
                strip = evidence / f"{case}-{tag}.png" if evidence else None
                entry[f"{backends[1]}_vs_{backends[0]}"] = diff(ref["png"], other["png"], strip)
            results[case][tag] = entry
        if args.noise:
            port += 1
            tag = adapters[0] or "default"
            png = out / f"{case}-{tag}-{backends[0]}-repeat.png"
            again = capture(case, backends[0], adapters[0], png, port)
            first = shots[(case, tag, backends[0])]
            if again["ok"] and first["ok"]:
                results[case]["noise"] = {"backend": backends[0], "adapter": tag,
                                          "repeat_vs_first": diff(first["png"], png, None)}
        if args.cross_adapter and len(adapters) > 1:
            a, b = (shots[(case, x or "default", backends[0])] for x in adapters[:2])
            if a["ok"] and b["ok"]:
                results[case]["cross_adapter"] = {
                    "backend": backends[0], "adapters": adapters[:2],
                    "second_vs_first": diff(a["png"], b["png"], None)}
    summary = {"tool": "tools/backend_compare.py", "date": time.strftime("%Y-%m-%d %H:%M"),
               "backends": backends, "adapters": adapters, "cases": results}
    text = json.dumps(summary, indent=1)
    if args.summary:
        Path(ROOT / args.summary).parent.mkdir(parents=True, exist_ok=True)
        Path(ROOT / args.summary).write_text(text + "\n")
    print(text)


if __name__ == "__main__":
    main()
