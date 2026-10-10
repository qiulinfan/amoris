#!/usr/bin/env python3
"""Draws a viewport page in headless Chrome on both WebGPU draw paths and compares them.

The page runs once as is (with `indirect-first-instance` when the adapter has it: the renderer's
`first-instance` path) and once with `gpu_minimal=first-instance` appended (WebGPU's `baseline`
path, crates/pocket-render/src/batches.rs). Each run reports frame and GPU times
(tools/web_bench.mjs), the entity-id pass's coverage per entity, and a screenshot with the HUD
hidden; the two runs' coverage and pixels are compared (pixels need Pillow).

    python3 tools/web_draw_paths.py "http://127.0.0.1:8090/viewport/?demo=mixed" --name mixed \
        --out out/bench-runs/webgpu/browser [--seconds 6] [--size 960x540] [--chrome-flags ...]

Writes <name>-<path>.json per run, <name>.json (the comparison) and the screenshots (one when
both are identical; downscaled to --keep-width) to --out.

`--compare occlusion` runs the page with `occlusion=off` and with `occlusion=on` instead
(docs/spec/occlusion.md): occlusion culling must not change the coverage or the pixels.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

COVERAGE = r"""(async () => {
  const vp = window.pocketViewport;
  const hud = document.getElementById("hud");
  if (hud) hud.style.display = "none";
  const s = window.pocketStats || {};
  const a = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  const t0 = performance.now();
  vp.raw.request_visible();
  // The readback resolves on the event loop; give it up to five seconds.
  for (let i = 0; i < 100; i++) {
    await new Promise((r) => setTimeout(r, 50));
    const v = vp.raw.take_visible();
    if (v) {
      const arr = Array.from(v);
      const pairs = [];
      for (let k = 0; k + 1 < arr.length; k += 2) pairs.push([arr[k], Math.round(arr[k + 1] * 1e5) / 1e5]);
      pairs.sort((x, y) => x[0] - y[0]);
      return { adapter: a ? `${a.info.vendor} ${a.info.architecture}` : null, draw_path: s.draw_path,
        draw_calls: s.draw_calls, readback_ms: Math.round(performance.now() - t0),
        entities: pairs.length, coverage: pairs };
    }
  }
  return { error: "no coverage", debug: vp.raw.pick_debug() };
})()"""


def run(url, shot, seconds, size, flags):
    env = dict(os.environ, EVAL=COVERAGE, SHOT=str(shot), LOGS="1")
    if flags:
        env["CHROME_FLAGS"] = flags
    w, h = size.split("x")
    out = subprocess.run(
        ["node", str(ROOT / "tools" / "web_bench.mjs"), url, str(seconds), w, h],
        env=env, capture_output=True, text=True, timeout=180, check=True,
    ).stdout
    stats = json.loads(out[: out.index("\neval:")])
    evaluated = json.loads(out.split("eval:", 1)[1].splitlines()[0])
    logs = [l for l in out.splitlines() if l.startswith("[")]
    return stats, evaluated, logs


def pixels(a, b, diff_path):
    try:
        from PIL import Image, ImageChops
    except ImportError:
        return None
    ia, ib = Image.open(a).convert("RGB"), Image.open(b).convert("RGB")
    if ia.size != ib.size:
        return {"error": f"sizes differ: {ia.size} {ib.size}"}
    d = ImageChops.difference(ia, ib)
    data = list(d.get_flattened_data()) if hasattr(d, "get_flattened_data") else list(d.getdata())
    differing = sum(1 for p in data if max(p) > 8)
    mx = max((max(p) for p in data), default=0)
    if differing:
        d.point(lambda v: min(255, v * 8)).save(diff_path)
    return {"pixels": len(data), "differing_gt8": differing, "max_abs_diff": mx}


def shrink(paths, width):
    """Downscales the kept screenshots for the evidence directory (needs Pillow)."""
    try:
        from PIL import Image
    except ImportError:
        return
    for p in paths:
        if p.exists():
            im = Image.open(p)
            if im.width > width:
                im.resize((width, round(im.height * width / im.width)), Image.LANCZOS).save(p)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("--name", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--seconds", type=float, default=6)
    ap.add_argument("--size", default="960x540")
    ap.add_argument("--chrome-flags", default="")
    ap.add_argument("--keep-width", type=int, default=480, help="downscale kept images to this width")
    ap.add_argument("--compare", choices=["paths", "occlusion"], default="paths")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    sep = "&" if "?" in a.url else "?"
    runs = {}
    if a.compare == "occlusion":
        variants = [("off", f"{a.url}{sep}occlusion=off"), ("on", f"{a.url}{sep}occlusion=on")]
    else:
        variants = [("first-instance", a.url), ("baseline", f"{a.url}{sep}gpu_minimal=first-instance")]
    (la, _), (lb, _) = variants
    for label, url in variants:
        shot = out / f"{a.name}-{label}.png"
        stats, evaluated, logs = run(url, shot, a.seconds, a.size, a.chrome_flags)
        if not isinstance(evaluated, dict):
            evaluated = {"error": "coverage evaluation returned an invalid result"}
        kept = dict(evaluated)
        cov = kept.get("coverage", [])
        if len(cov) > 100:
            # Large scenes keep a digest of the coverage, not every entity.
            kept["coverage"] = cov[:20]
            kept["coverage_sha1"] = hashlib.sha1(json.dumps(cov).encode()).hexdigest()
        record = {"url": url, "stats": stats, "id_pass": kept, "logs": logs[-12:]}
        (out / f"{a.name}-{label}.json").write_text(json.dumps(record, indent=1))
        runs[label] = (shot, stats, evaluated)
    (sa, ta, ea), (sb, tb, eb) = runs[la], runs[lb]
    errors = {label: e.get("error") or "coverage data missing"
              for label, e in ((la, ea), (lb, eb))
              if "error" in e or not isinstance(e.get("coverage"), list)}
    if errors:
        summary = {"name": a.name, "compare": [la, lb], "error": "coverage readback failed",
                   "coverage_errors": errors}
        (out / f"{a.name}.json").write_text(json.dumps(summary, indent=1))
        print(json.dumps(summary))
        return 1
    ca = dict((e, s) for e, s in ea.get("coverage", []))
    cb = dict((e, s) for e, s in eb.get("coverage", []))
    ids = sorted(set(ca) | set(cb))
    summary = {
        "name": a.name,
        "compare": [la, lb],
        "paths": [ea.get("draw_path"), eb.get("draw_path")],
        "occlusion": [ta.get("occlusion"), tb.get("occlusion")],
        "adapter": ea.get("adapter"),
        "entities": [len(ca), len(cb)],
        "id_readback_ms": [ea.get("readback_ms"), eb.get("readback_ms")],
        "coverage_l1": round(sum(abs(ca.get(e, 0) - cb.get(e, 0)) for e in ids), 6),
        f"only_{la.replace('-', '_')}": [e for e in ids if e not in cb],
        f"only_{lb.replace('-', '_')}": [e for e in ids if e not in ca],
        "pixels": pixels(sa, sb, out / f"{a.name}-diff.png"),
    }
    if summary["pixels"] and summary["pixels"].get("differing_gt8") == 0:
        # Identical screenshots: keep one.
        sb.unlink()
        summary["pixels"]["kept"] = sa.name
    shrink([sa, sb, out / f"{a.name}-diff.png"], a.keep_width)
    (out / f"{a.name}.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
