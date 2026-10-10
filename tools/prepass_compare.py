#!/usr/bin/env python3
"""Draws the same frames with the depth prepass off and forced on and compares them.

docs/spec/prepass.md: the prepass must never change the image. tools/occlusion_compare.py's two
kinds of comparison on one backend and adapter, switching POCKET_PREPASS instead of
POCKET_OCCLUSION (occlusion culling stays as the environment says, auto by default):

- captures: every case of tools/backend_compare.py (many_cubes sphere, dense and with shadows,
  splats, gi-room and pt-lab drawn statically, `pocket serve` captures of sailing, anim and
  gi-room) plus the many_cubes orbit, each captured by its own process with POCKET_PREPASS=off and
  on and compared by the `image_diff` example. Separate processes do not draw animated scenes at
  exactly the same moment (`anim` differs between two runs with the same settings);
- scenes: the pocket-app example `draw_paths --compare prepass` draws each scene twice in one
  process at one fixed moment (prepass off, forced on) and compares the pixels and the entity-id
  pass's coverage, so animated scenes compare exactly.

Build first: `cargo build --release -p pocket-render --examples`, `cargo build --release -p
pocket-app` and `cargo build --release -p pocket-app --example draw_paths`. Then, from the
repository's root:

    python tools/prepass_compare.py --backend dx12 --adapter nvidia --out out/prepass/compare/dx12-nvidia \\
        --summary out/prepass/compare/dx12-nvidia.json

With POCKET_GPU_MINIMAL=features,limits,timestamps in the environment every capture runs on
WebGPU's baseline device.
"""
import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import backend_compare as bc  # noqa: E402
import occlusion_compare as oc  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--backend", default="dx12")
    ap.add_argument("--adapter", default="", help="POCKET_ADAPTER value")
    ap.add_argument("--cases", default=",".join(bc.CASES), help="capture cases ('' for none)")
    ap.add_argument("--scenes", default=oc.SCENES, help="draw_paths scenes ('' for none)")
    ap.add_argument("--out", default="out/prepass/compare")
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
    summary = {"tool": "tools/prepass_compare.py", "date": time.strftime("%Y-%m-%d %H:%M"),
               "backend": args.backend, "adapter": adapter,
               "cases": oc.captures(cases, args.backend, adapter, out, evidence, "prepass"),
               "scenes": oc.scenes(names, args.backend, adapter, out, "prepass") if names else []}
    text = json.dumps(summary, indent=1)
    if args.summary:
        Path(bc.ROOT / args.summary).parent.mkdir(parents=True, exist_ok=True)
        Path(bc.ROOT / args.summary).write_text(text + "\n", newline="\n")
    print(text)


if __name__ == "__main__":
    main()
