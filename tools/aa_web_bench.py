#!/usr/bin/env python3
"""Times TAA and GTAO in Chrome's WebGPU (docs/bench/taa-gtao.md): the viewport's check scene
(`?demo=aa`) under every anti-aliasing mode, with and without GTAO, through tools/web_bench.mjs.

Serve `web/` first (`python -m http.server 8197 -d web`) with a viewport package built from this
tree (`tools/build_viewport.sh`). Then, from the repository root:

    python tools/aa_web_bench.py --url http://127.0.0.1:8197 --gpu nvidia --rounds 2 \
        --out docs/evidence/aa/web-nvidia.json

`--gpu nvidia` passes `--force_high_performance_gpu` (the RTX 5060); `--gpu default` leaves
Chrome's choice (the Radeon 780M on this machine). Each row is the median of the timestamped
frames' pass times over the last frames of a run; rounds interleave the configurations.
"""
import argparse
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

EVAL = """(() => {
  const s = (window.pocketSamples || []).filter((x) => x.passes && x.passes.length);
  const med = (a) => { a = a.slice().sort((x, y) => x - y); return a.length ? a[Math.floor(a.length / 2)] : null; };
  const by = {};
  for (const x of s) for (const p of x.passes) (by[p.pass] ||= []).push(p.ms);
  const passes = {};
  for (const k in by) passes[k] = med(by[k]);
  const last = s.length ? s[s.length - 1] : (window.pocketSamples || []).slice(-1)[0] || {};
  return { timed_frames: s.length, gpu_ms: med(s.map((x) => x.gpu_ms)), passes,
           frame_ms: med((window.pocketSamples || []).map((x) => x.frame_ms)),
           aa: last.antialiasing, gtao: last.gtao,
           canvas: (() => { const c = document.querySelector("canvas"); return [c.width, c.height]; })() };
})()"""


def run(url, aa, gtao, seconds, gpu, size):
    env = dict(os.environ, EVAL=EVAL)
    if gpu == "nvidia":
        env["CHROME_FLAGS"] = "--force_high_performance_gpu"
    page = f"{url}/viewport/?demo=aa&aa={aa.replace('+', '%2B')}&gtao={gtao}"
    p = subprocess.run(["node", str(ROOT / "tools" / "web_bench.mjs"), page, str(seconds),
                        str(size[0]), str(size[1])], cwd=ROOT, env=env, capture_output=True,
                       text=True, timeout=seconds + 120)
    for line in p.stdout.splitlines():
        if line.startswith("eval:"):
            return json.loads(line[5:])
    return {"error": p.stdout[-400:] + p.stderr[-400:]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8197")
    ap.add_argument("--gpu", choices=["nvidia", "default"], default="nvidia")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--seconds", type=int, default=8)
    ap.add_argument("--size", default="1600x900")
    ap.add_argument("--out")
    a = ap.parse_args()
    size = [int(v) for v in a.size.split("x")]
    configs = [(aa, g) for aa in ["off", "msaa", "taa", "msaa+taa"] for g in ["off", "on"]]
    rows = []
    for r in range(a.rounds):
        for aa, g in configs:
            res = run(a.url, aa, g, a.seconds, a.gpu, size)
            res.update({"round": r, "config_aa": aa, "config_gtao": g})
            print(json.dumps(res))
            rows.append(res)
    if a.out:
        Path(a.out).write_text(json.dumps({"gpu": a.gpu, "url": a.url, "rows": rows}, indent=1))


if __name__ == "__main__":
    main()
