#!/usr/bin/env python3
"""Draws the same frames with occlusion culling off and forced on and compares them.

docs/bench/occlusion.md: occlusion culling must never change the image. Two kinds of comparison
on one backend and adapter:

- captures: every case of tools/backend_compare.py (many_cubes, splats, static project scenes, and
  `pocket serve` captures of the sailing, anim and gi-room samples) plus the many_cubes orbit, each
  captured by its own process with POCKET_OCCLUSION=off and with POCKET_OCCLUSION=on, compared by
  the `image_diff` example (with --evidence a strip: off, on, heat map). Separate processes do not
  draw animated scenes at exactly the same moment: `anim` differs between two runs with occlusion
  off as much as between off and on;
- scenes: the pocket-app example `draw_paths --compare occlusion` draws each scene twice in one
  process at one fixed moment (occlusion off, forced on) and compares the pixels and the entity-id
  pass's coverage, so animated scenes compare exactly.

Build first: `cargo build --release -p pocket-render --examples`, `cargo build --release -p
pocket-app` and `cargo build --release -p pocket-app --example draw_paths`. Then, from the
repository's root:

    python tools/occlusion_compare.py --backend vulkan --adapter nvidia --out out/bench-runs/hiz/compare-captures \
        --evidence out/bench-runs/hiz/compare --summary out/bench-runs/hiz/compare-vulkan.json
"""
import argparse
import json
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import backend_compare as bc  # noqa: E402

bc.CASES["cubes-orbit"] = [bc.example("many_cubes"), "--orbit", "--capture", "{out}"]
SCENES = "mixed,cubes,samples/anim,samples/sailing,samples/gi-room,samples/pt-lab"


def captures(cases, backend, adapter, out, evidence, feature="occlusion"):
    """`feature`: what is switched off and on, `occlusion` (POCKET_OCCLUSION) or `prepass`
    (POCKET_PREPASS, tools/prepass_compare.py)."""
    var = f"POCKET_{feature.upper()}"
    port = 7990
    results = {}
    for case in cases:
        shots = {}
        for mode in ("off", "on"):
            os.environ[var] = mode
            port += 1
            png = out / f"{case}-{mode}.png"
            shots[mode] = bc.capture(case, backend, adapter, png, port)
            shots[mode][feature] = mode
        entry = {"captures": [shots["off"], shots["on"]]}
        if shots["off"]["ok"] and shots["on"]["ok"]:
            strip = evidence / f"{case}.png" if evidence else None
            entry["on_vs_off"] = bc.diff(shots["off"]["png"], shots["on"]["png"], strip)
        results[case] = entry
        print(f"{case:16} {json.dumps(entry.get('on_vs_off'))}", file=sys.stderr)
    os.environ.pop(var, None)
    return results


def scenes(names, backend, adapter, out, feature="occlusion"):
    env = dict(os.environ, POCKET_BACKEND=backend)
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    paths_out = out / "scenes"
    cmd = [bc.example("draw_paths"), "--compare", feature, "--out", str(paths_out), *names]
    p, _ = bc.run(cmd, env, 1800)
    if p.returncode:
        return {"error": p.stderr[-600:]}
    rows = []
    for line in json.loads((paths_out / "draw_paths.json").read_text()):
        c = line["compare"]
        rows.append({"scene": line["scene"], "adapter": line["on"]["adapter"],
                     "backend": line["on"]["backend"], "instances": line["on"]["instances"],
                     "draw_path": line["on"]["draw_path"], **c})
        print(f"{line['scene']:16} max diff {c['max_abs_diff']}, entities "
              f"{c['entities_visible_off']} / {c['entities_visible_on']}, coverage L1 "
              f"{c['coverage_l1']}", file=sys.stderr)
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--backend", default="vulkan")
    ap.add_argument("--adapter", default="", help="POCKET_ADAPTER value")
    ap.add_argument("--cases", default=",".join(bc.CASES), help="capture cases ('' for none)")
    ap.add_argument("--scenes", default=SCENES, help="draw_paths scenes ('' for none)")
    ap.add_argument("--out", default="out/bench-runs/hiz/compare-captures")
    ap.add_argument("--evidence", help="directory for the downscaled comparison strips")
    ap.add_argument("--summary", help="write the JSON summary here too")
    args = ap.parse_args()
    adapter = args.adapter or None
    out = bc.ROOT / args.out
    out.mkdir(parents=True, exist_ok=True)
    evidence = bc.ROOT / args.evidence if args.evidence else None
    if evidence:
        evidence.mkdir(parents=True, exist_ok=True)
    cases = [c for c in args.cases.split(",") if c]
    names = [s for s in args.scenes.split(",") if s]
    summary = {"tool": "tools/occlusion_compare.py", "date": time.strftime("%Y-%m-%d %H:%M"),
               "backend": args.backend, "adapter": adapter,
               "cases": captures(cases, args.backend, adapter, out, evidence),
               "scenes": scenes(names, args.backend, adapter, out) if names else []}
    text = json.dumps(summary, indent=1)
    if args.summary:
        Path(bc.ROOT / args.summary).parent.mkdir(parents=True, exist_ok=True)
        Path(bc.ROOT / args.summary).write_text(text + "\n", newline="\n")
    print(text)


if __name__ == "__main__":
    main()
