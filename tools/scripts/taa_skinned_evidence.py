#!/usr/bin/env python3
"""TAA on an animated character (docs/design/rendering.md, Temporal anti-aliasing): a built-in
humanoid running in place, seen from its side in front of a dark wall, drawn frame by frame for 40
ticks and captured, four ways: without TAA, with TAA whose motion ignores the joints (a runtime
built before skinned motion vectors, passed as --before), with TAA following the joints (this
build), and the reference, the same frame without TAA at four times the size averaged down (what
anti-aliasing tries to reach). Writes tests/evidence/rendering/taa-skinned.png (the character's
crop of each, three times enlarged, the frame's tick under it) and prints how far each is from the
reference over the crop (mean absolute difference in 8-bit levels, all channels), for the frames
of three ticks of the stride.

    python3 tools/scripts/taa_skinned_evidence.py --before <old pocket_runtime>

Without --before the second column is left out.
"""
import argparse
import json
import math
import os
import socket
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops, ImageDraw

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "taa-skinned.png")
W, H = 480, 270
TICKS = (36, 40, 44)          # the frames compared (the run's stride takes 42 ticks)
CROP = (165, 28, 300, 188)    # around the character, in the frame's pixels
SCALE = 3


def stage_project():
    stage = os.path.join(tempfile.mkdtemp(), "stage")
    os.makedirs(os.path.join(stage, "scripts"))
    with open(os.path.join(stage, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('name = "stage"\nentry = "scripts/main.ts"\n')
    with open(os.path.join(stage, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("export {};\n")
    subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "ts", stage], env=ENV, cwd=ROOT, check=True, capture_output=True)
    return stage


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def run(stage, size, taa, exe=None, tag=""):
    """The frames at TICKS, as images (the runtime at `size`, TAA on or off)."""
    r = Runtime(stage, free_port(), size=size, release=True, exe=exe)
    shots = {}
    try:
        r.rpc("world.clear")
        r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"scale": {"x": 30, "y": 0.2, "z": 30}, "position": {"y": -0.1}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.55, "g": 0.57, "b": 0.6, "a": 1}}}})
        r.rpc("world.spawn", {"name": "Wall", "components": {"Transform": {"scale": {"x": 30, "y": 8, "z": 0.2}, "position": {"y": 4, "z": -2}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.08, "g": 0.09, "b": 0.12, "a": 1}}}})
        r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0, "intensity": 1.6, "shadows": True}}})
        # Facing +X (a quarter turn from -Z), so its limbs swing across the view.
        r.rpc("world.spawn", {"name": "Runner", "components": {"Transform": {"rotation": {"x": 0, "y": -math.sin(math.pi / 4), "z": 0, "w": math.cos(math.pi / 4)}},
                                                              "MeshRenderer": {"mesh": "humanoid?shirt=#e8b04a&trousers=#3a4a8a&hair=brown"}, "Animator": {"clip": "run"}}})
        pitch = math.radians(-6) / 2
        r.rpc("world.spawn", {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 1.1, "z": 5}, "rotation": {"x": math.sin(pitch), "y": 0, "z": 0, "w": math.cos(pitch)}}, "Camera": {"fov_degrees": 40}}})
        r.rpc("render.taa", {"enabled": taa})
        last = max(TICKS)
        for t in range(1, last + 1):
            r.rpc("step", {"ticks": 1, "render": True})
            if t in TICKS:
                path = os.path.join(tempfile.gettempdir(), f"taa-skinned-{tag}-{t}.png")
                r.rpc("capture", {"path": path})
                shots[t] = Image.open(path).convert("RGB")
    finally:
        r.close()
    return shots


def mad(a, b):
    """Mean absolute difference over the crop, in 8-bit levels."""
    d = ImageChops.difference(a.crop(CROP), b.crop(CROP))
    hist = d.histogram()
    total = sum(i % 256 * n for i, n in enumerate(hist))
    return total / (3 * (CROP[2] - CROP[0]) * (CROP[3] - CROP[1]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", help="a pocket_runtime built before skinned motion vectors")
    args = ap.parse_args()
    stage = stage_project()
    columns = [("no TAA", run(stage, f"{W}x{H}", False, tag="off"))]
    if args.before:
        columns.append(("TAA, joints ignored (before)", run(stage, f"{W}x{H}", True, exe=os.path.abspath(args.before), tag="before")))
    columns.append(("TAA, joints followed (after)", run(stage, f"{W}x{H}", True, tag="after")))
    big = run(stage, f"{W * 4}x{H * 4}", False, tag="reference")
    reference = {t: im.resize((W, H), Image.BOX) for t, im in big.items()}
    columns.append(("reference (4x4 supersampled)", reference))
    report = {label: {t: round(mad(shots[t], reference[t]), 3) for t in TICKS} for label, shots in columns}
    cw, ch = (CROP[2] - CROP[0]) * SCALE, (CROP[3] - CROP[1]) * SCALE
    sheet = Image.new("RGB", (cw * len(columns), (ch + 18) * len(TICKS) + 20), (20, 20, 20))
    draw = ImageDraw.Draw(sheet)
    for c, (label, shots) in enumerate(columns):
        draw.text((c * cw + 6, 4), label, fill=(235, 235, 235))
        for k, t in enumerate(TICKS):
            y = 20 + k * (ch + 18)
            sheet.paste(shots[t].crop(CROP).resize((cw, ch), Image.NEAREST), (c * cw, y))
            note = f"tick {t}" + ("" if label.startswith("reference") else f", {report[label][t]:.2f} from the reference")
            draw.text((c * cw + 6, y + ch + 3), note, fill=(200, 200, 200))
    sheet.save(OUT)
    print(json.dumps({"out": os.path.relpath(OUT, ROOT), "mean_abs_diff_from_reference": report}, indent=1))


if __name__ == "__main__":
    main()
