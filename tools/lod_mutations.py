"""Puts known level-of-detail bugs into the code one at a time and runs the checks against each
(docs/spec/lod.md 8): every mutation must make at least one check fail.

    python tools/lod_mutations.py [--out out/bench-runs/lod/mutations.json] [filter ...]

Each mutation edits cull.wgsl, meshes.rs, renderer.rs, lod.rs or pocket-assets' lod.rs, runs
`cargo test --release -p pocket-render --test lod --test draw_paths --test occlusion` (and
pocket-assets' own tests for the generator's mutations), and restores the file with `git checkout`;
it refuses to start while those files have uncommitted changes. POCKET_BACKEND, POCKET_ADAPTER and
POCKET_GPU_MINIMAL pass through. A filter keeps the mutations whose name contains it.

Not here: the generator's guard that keeps errors increasing (`error = error.max(reached)`). On
every chain tried (the demo rocks and knot at five sizes) the simplifier's own errors already
increase, so no check can tell the guard from its absence (docs/bench/lod.md 5).
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CULL = "crates/pocket-render/shaders/cull.wgsl"
MESHES = "crates/pocket-render/src/meshes.rs"
RENDERER = "crates/pocket-render/src/renderer.rs"
SELECT = "crates/pocket-render/src/lod.rs"
GENERATE = "crates/pocket-assets/src/lod.rs"
RENDER_TESTS = ["-p", "pocket-render", "--test", "lod", "--test", "draw_paths", "--test",
                "occlusion"]
ASSET_TESTS = ["-p", "pocket-assets", "--lib"]

# (name, tests, [(file, text, replacement)]): each text must occur exactly once.
MUTATIONS = [
    ("cull: the camera's bound eight times too loose", RENDER_TESTS, [(
        CULL,
        "let bound = dist * cull.eye.w;",
        "let bound = dist * cull.eye.w * 8.0;",
    )]),
    ("cull: no hysteresis (coarser as soon as acceptable)", RENDER_TESTS, [(
        CULL,
        "let coarse = coarsest(first, count, smax, bound / (1.0 + cull.lod.y));",
        "let coarse = coarsest(first, count, smax, bound);",
    )]),
    ("cull: distance to the sphere's centre, not its nearest point", RENDER_TESTS, [(
        CULL,
        "dist = max(length(c - cull.eye.xyz) - r, 1e-4);",
        "dist = max(length(c - cull.eye.xyz), 1e-4);",
    )]),
    ("cull: the instance's scale ignored", RENDER_TESTS, [(
        CULL,
        "let fine = coarsest(first, count, smax, bound);",
        "let fine = coarsest(first, count, 1.0, bound);",
    )]),
    ("cull: the cascades draw the full meshes", RENDER_TESTS, [(
        CULL,
        "lv = coarsest(first, count, smax, cull.lod_cascades[v - 1u]);",
        "lv = 0u;",
    )]),
    ("cull: the cascades draw the camera's level", RENDER_TESTS, [(
        CULL,
        "lv = coarsest(first, count, smax, cull.lod_cascades[v - 1u]);",
        "lv = camera_level;",
    )]),
    ("cull: the cascades' bound four times too loose", RENDER_TESTS, [(
        CULL,
        "lv = coarsest(first, count, smax, cull.lod_cascades[v - 1u]);",
        "lv = coarsest(first, count, smax, cull.lod_cascades[v - 1u] * 4.0);",
    )]),
    ("lod: the cascades' bound ignores shadow_texels", RENDER_TESTS, [(
        SELECT,
        "texels.map(|t| t * self.shadow_texels)",
        "texels.map(|t| t)",
    )]),
    ("cull: a level's row one too far", RENDER_TESTS, [(
        CULL,
        "return (lods & 0xffffffu) + level - 1u;",
        "return (lods & 0xffffffu) + level;",
    )]),
    ("cull: the late pass draws the full mesh", RENDER_TESTS, [(
        CULL,
        "level = (st & LOD_MASK) >> LOD_SHIFT;",
        "level = 0u;",
    )]),
    ("cull: the late pass forgets the level", RENDER_TESTS, [(
        CULL,
        "state[idx] = select(0u, VIS_VISIBLE, seen) | (st & LOD_MASK);",
        "state[idx] = select(0u, VIS_VISIBLE, seen);",
    )]),
    ("meshes: level rows' errors are the full mesh's", RENDER_TESTS, [(
        MESHES,
        "                    mesh.lods[level - 1].error",
        "                    0.0",
    )]),
    ("meshes: a skinned copy's levels read the bind pose", RENDER_TESTS, [(
        MESHES,
        "                base_vertex,\n                batch_offset: 0,\n"
        "                lods: if level == 0 {\n"
        "                    pack_lods(levels as usize, id as usize + 1)",
        "                base_vertex: if level == 0 { base_vertex } else { from.base_vertex },\n"
        "                batch_offset: 0,\n"
        "                lods: if level == 0 {\n"
        "                    pack_lods(levels as usize, id as usize + 1)",
    )]),
    ("renderer: level rows get no regions", RENDER_TESTS, [(
        RENDERER,
        "if lod_on || o == row as u32 {",
        "if o == row as u32 {",
    )]),
    ("renderer: levels kept past the binding limit", RENDER_TESTS, [(
        RENDERER,
        "if lod_on && largest(stride) > limit {",
        "if false && lod_on && largest(stride) > limit {",
    )]),

    ("generate: vertices left in their first order", ASSET_TESTS, [(
        GENERATE,
        "        .rev()\n        .map(|l| &l.indices)",
        "        .map(|l| &l.indices)",
    )]),
]


def touch(path):
    """Moves a file's modification time two seconds ahead: a mutation written within the clock
    tick of the previous restore once left cargo's fingerprint fresh and the build stale."""
    t = time.time() + 2
    os.utime(path, (t, t))


def run(name, tests, edits):
    files = sorted({f for f, _, _ in edits})
    try:
        for f, text, replacement in edits:
            path = os.path.join(ROOT, f)
            with open(path, encoding="utf-8", newline="") as fh:
                source = fh.read()
            if source.count(text) != 1:
                raise SystemExit(f"{name}: {f} holds the text {source.count(text)} times")
            with open(path, "w", encoding="utf-8", newline="") as fh:
                fh.write(source.replace(text, replacement))
            touch(path)
        r = subprocess.run(
            ["cargo", "test", "--release", "--no-fail-fast", *tests],
            cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")
    finally:
        subprocess.run(["git", "checkout", "--", *files], cwd=ROOT, check=True)
        for f in files:
            touch(os.path.join(ROOT, f))
    lines = (r.stdout + r.stderr).splitlines()
    results = {}
    for line in lines:
        m = re.match(r"test (\S+) \.\.\. (ok|FAILED)$", line)
        if m:
            results[m.group(1)] = m.group(2)
    failures = [lines[i + 1][:300] for i, line in enumerate(lines[:-1]) if "panicked at" in line]
    if not results:
        failures = [line for line in lines if "error" in line][:5]
    return {"tests": results, "failures": failures}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", help="write the results as JSON")
    ap.add_argument("filters", nargs="*")
    args = ap.parse_args()
    files = sorted({f for _, _, edits in MUTATIONS for f, _, _ in edits})
    if subprocess.run(["git", "diff", "--quiet", "--", *files], cwd=ROOT).returncode != 0:
        sys.exit(f"uncommitted changes in {files}: commit or stash them first")
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True).stdout.strip()
    env = {k: os.environ[k] for k in ("POCKET_BACKEND", "POCKET_ADAPTER", "POCKET_GPU_MINIMAL")
           if k in os.environ}
    results = {"commit": head, "env": env, "mutations": {}}
    survived = []
    for name, tests, edits in MUTATIONS:
        if args.filters and not any(f in name for f in args.filters):
            continue
        res = run(name, tests, edits)
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
