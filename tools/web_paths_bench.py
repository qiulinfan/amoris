#!/usr/bin/env python3
"""Times viewport pages in headless Chrome on both WebGPU draw paths, interleaved by round
(docs/bench/web.md, Timing).

Each run is one `node tools/web_bench.mjs <url> SECONDS` (a fresh Chrome with a temporary profile,
the page's uncapped frame loop, the last 300 frames' frame time and timestamped GPU time). For every
round, GPU and page the page runs as is (`first-instance`: the renderer's path when the adapter
offers `indirect-first-instance`) and with `gpu_minimal=first-instance` appended (WebGPU's
`baseline` path), so that the two paths alternate. `--gpus nvidia` adds Chrome's
`--force_high_performance_gpu` (the RTX 5060 here); `default` is Chrome's own choice (the Radeon
780M here). Serve `web/` first (`python -m http.server -d web 8090`).

    python tools/web_paths_bench.py --base http://127.0.0.1:8090/viewport/ \
        --pages "cubes-100k=demo=cubes&count=100000&occlusion=off" --gpus nvidia,default \
        --rounds 3 --out docs/evidence/quiet/web/cubes.json
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GPU_FLAGS = {"nvidia": "--force_high_performance_gpu", "default": ""}
PATHS = {"first-instance": "", "baseline": "&gpu_minimal=first-instance"}


def chrome_version():
    exe = os.environ.get("CHROME") or os.path.join(
        os.environ.get("PROGRAMFILES", r"C:\Program Files"), r"Google\Chrome\Application\chrome.exe")
    if os.name != "nt" or not os.path.exists(exe):
        return None
    p = subprocess.run(["powershell", "-NoProfile", "-Command",
                        f"(Get-Item '{exe}').VersionInfo.ProductVersion"], capture_output=True,
                       text=True)
    return p.stdout.strip() or None


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="http://127.0.0.1:8090/viewport/")
    ap.add_argument("--pages", required=True, help="name=query, comma separated")
    ap.add_argument("--gpus", default="nvidia,default")
    ap.add_argument("--paths", default="first-instance,baseline")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--seconds", type=int, default=10)
    ap.add_argument("--size", default="1280x720")
    ap.add_argument("--note", default="")
    ap.add_argument("--out")
    a = ap.parse_args()
    pages = [p.split("=", 1) for p in a.pages.split(",")]
    w, h = a.size.split("x")
    runs = []
    for rnd in range(a.rounds):
        for gpu in a.gpus.split(","):
            for name, query in pages:
                for path in a.paths.split(","):
                    url = f"{a.base}?{query}{PATHS[path]}"
                    env = dict(os.environ, CHROME_FLAGS=GPU_FLAGS[gpu])
                    t = time.time()
                    p = subprocess.run(["node", str(ROOT / "tools" / "web_bench.mjs"), url,
                                        str(a.seconds), w, h], cwd=ROOT, env=env,
                                       capture_output=True, text=True, timeout=a.seconds + 120)
                    try:
                        r, _ = json.JSONDecoder().raw_decode(p.stdout.strip())
                    except json.JSONDecodeError:
                        r = {"error": (p.stdout + p.stderr)[-600:]}
                    r.update({"round": rnd, "gpu": gpu, "page": name, "path": path,
                              "chrome_flags": GPU_FLAGS[gpu], "seconds": round(time.time() - t, 1)})
                    runs.append(r)
                    print(f"r{rnd} {gpu:7} {name:12} {path:14} frame p50 "
                          f"{r.get('frame_ms_p50') or 0:6.2f} ms GPU {r.get('gpu_ms_timed_mean') or 0:6.2f} ms "
                          f"({r.get('gpu_timed_frames')} timed) {r.get('draw_path')}", file=sys.stderr,
                          flush=True)
    doc = {"tool": "tools/web_paths_bench.py", "date": time.strftime("%Y-%m-%d %H:%M"),
           "chrome": chrome_version(), "note": a.note, "seconds": a.seconds, "size": a.size,
           "switches": ["--headless=new", "--enable-unsafe-webgpu", "--disable-frame-rate-limit",
                        "--disable-gpu-vsync", f"--window-size={w},{h}", "--no-first-run",
                        "--no-default-browser-check", "(+ chrome_flags per run)"],
           "runs": runs}
    if a.out:
        out = ROOT / a.out
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(doc, indent=1) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
