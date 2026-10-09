"""Puts known occlusion-culling bugs into the code one at a time and runs the checks against each
(docs/spec/occlusion.md 9): every mutation must make at least one check fail.

    python tools/occlusion_mutations.py [--out docs/evidence/hiz/mutations.json] [filter ...]

Each mutation edits hiz.wgsl, cull.wgsl or renderer.rs, runs
`cargo test --release -p pocket-render --test hiz --test occlusion`, and restores the file with
`git checkout`; it refuses to start while those files have uncommitted changes. POCKET_BACKEND,
POCKET_ADAPTER and POCKET_GPU_MINIMAL pass through. A filter keeps the mutations whose name
contains it.
"""

import argparse
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HIZ = "crates/pocket-render/shaders/hiz.wgsl"
CULL = "crates/pocket-render/shaders/cull.wgsl"
RENDERER = "crates/pocket-render/src/renderer.rs"

# (name, [(file, text, replacement)]): each text must occur exactly once.
MUTATIONS = [
    ("hiz: a level's last texel ignores the odd remainder", [(
        HIZ,
        "let b = select(min(2u * t + 1u, src_size - 1u), src_size - 1u, t == dst_size - 1u);",
        "let b = min(2u * t + 1u, src_size - 1u);",
    )]),
    ("hiz: level 0 reads sample 0 only", [(
        HIZ,
        "for (var s = 0u; s < samples; s++) {",
        "for (var s = 0u; s < 1u; s++) {",
    )]),
    ("cull: one of the 2x2 texels read", [(
        CULL,
        """    let far = min(
        min(textureLoad(hiz, t0, k).x, textureLoad(hiz, vec2u(t1.x, t0.y), k).x),
        min(textureLoad(hiz, vec2u(t0.x, t1.y), k).x, textureLoad(hiz, t1, k).x),
    );""",
        "    let far = textureLoad(hiz, t0, k).x;",
    )]),
    ("cull: a level one too fine", [(
        CULL,
        "k = firstLeadingBit(extent - 1u);",
        "k = firstLeadingBit(extent - 1u) - 1u;",
    )]),
    ("cull: the farthest corner's depth for the nearest", [
        (CULL, "var near = 0.0;", "var near = 1.0;"),
        (CULL, "near = max(near, n.z);", "near = min(near, n.z);"),
    ]),
    ("cull: the late pass tests and draws at alpha 1", [(
        CULL,
        """            let pose = instance_pose(inst, cull.alpha);
            let c = pose.pos + quat_rotate(pose.rot, m.box_center * inst.scale);""",
        """            let pose = instance_pose(inst, 1.0);
            let c = pose.pos + quat_rotate(pose.rot, m.box_center * inst.scale);""",
    )]),
    ("renderer: the late id-pass draws skipped", [(
        RENDERER,
        """            for variant in 0..VARIANTS {
                self.batches.draw(pass, set, variant);
            }""",
        """            for variant in 0..VARIANTS {
                if set != LATE {
                    self.batches.draw(pass, set, variant);
                }
            }""",
    )]),
]


def run(name, edits):
    files = sorted({f for f, _, _ in edits})
    try:
        for f, text, replacement in edits:
            path = os.path.join(ROOT, f)
            with open(path, encoding="utf-8") as fh:
                source = fh.read()
            if source.count(text) != 1:
                raise SystemExit(f"{name}: {f} holds the text {source.count(text)} times")
            with open(path, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(source.replace(text, replacement))
        r = subprocess.run(
            ["cargo", "test", "--release", "-p", "pocket-render", "--no-fail-fast",
             "--test", "hiz", "--test", "occlusion"],
            cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")
    finally:
        subprocess.run(["git", "checkout", "--", *files], cwd=ROOT, check=True)
    lines = (r.stdout + r.stderr).splitlines()
    tests = {}
    for line in lines:
        m = re.match(r"test (\w+) \.\.\. (ok|FAILED)$", line)
        if m:
            tests[m.group(1)] = m.group(2)
    failures = [lines[i + 1][:300] for i, line in enumerate(lines[:-1]) if "panicked at" in line]
    if not tests:
        failures = [line for line in lines if "error" in line][:5]
    return {"tests": tests, "failures": failures}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", help="write the results as JSON")
    ap.add_argument("filters", nargs="*")
    args = ap.parse_args()
    files = sorted({f for _, edits in MUTATIONS for f, _, _ in edits})
    if subprocess.run(["git", "diff", "--quiet", "--", *files], cwd=ROOT).returncode != 0:
        sys.exit(f"uncommitted changes in {files}: commit or stash them first")
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()
    env = {k: os.environ[k] for k in ("POCKET_BACKEND", "POCKET_ADAPTER", "POCKET_GPU_MINIMAL")
           if k in os.environ}
    results = {"commit": head, "env": env, "mutations": {}}
    survived = []
    for name, edits in MUTATIONS:
        if args.filters and not any(f in name for f in args.filters):
            continue
        res = run(name, edits)
        results["mutations"][name] = res
        caught = [t for t, v in res["tests"].items() if v == "FAILED"]
        print(f"{name}: {'caught by ' + ', '.join(caught) if caught else 'NOT CAUGHT'}",
              flush=True)
        if not caught:
            survived.append(name)
    if args.out:
        with open(os.path.join(ROOT, args.out), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(json.dumps(results, indent=1) + "\n")
    sys.exit(1 if survived else 0)


if __name__ == "__main__":
    main()
