"""Specular quality (docs/design/rendering.md, Specular quality), each picture drawn by a runtime
from before the change and by the current one:

- tests/evidence/rendering/specular-metals.png: gold (front) and silver balls from roughness 0.05
  to 1 under a sky and the sun, with ambient occlusion; before (left), after (right): multiple
  scattering gives the rough metals back the light single-scattering GGX lost.
- tests/evidence/rendering/specular-aa.png: a glossy floor bumped by a normal map, running to the
  horizon toward a low sun, and the island sample's sea in the afternoon; columns before, after
  and a reference (the old shading at sixteen samples a pixel, 4x4 averaged in linear light), each
  frame with its far part enlarged four times below it.

It prints, for the floor's sun path and the sea toward its horizon, the share of specks (pixels
more than 40 grey levels off the median of their 3x3 neighbourhood: lone sparkles and dark dashes)
in each column; how far the sea's far part is from the reference (the mean difference of a pixel,
in 8-bit levels); and how much the floor's far part flickers while the camera creeps forward (the
mean change of a pixel from one frame to the next). The reference is no reference for the floor's
highlights: the old frames' sparkles are clipped to white before they are averaged.

    python3 tools/scripts/dev/specular_evidence.py --before <old pocket_runtime>
        (release build; samples/playground and samples/island bundled; Pillow)

The old runtime is a release build of the commit before the change, copied aside before building.
"""
import argparse
import math
import os
import sys

from PIL import Image, ImageChops, ImageFilter, ImageStat

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from runtime import ROOT, Runtime  # noqa: E402

EVIDENCE = os.path.join(ROOT, "tests", "evidence", "rendering")
BUILD = os.path.join(ROOT, "build")
W, H = 480, 270
SS = 4   # the reference's samples a pixel along each axis
BUMPS = os.path.join(ROOT, "samples", "playground", "assets", "specular_bumps.png")
# The far parts: under the floor's horizon about the sun's path, the sea toward its horizon; and
# where the specks are counted: the floor's sun path nearer the camera, the sea's far half.
FLOOR_BOX = (180, 138, 300, 168)
SEA_BOX = (180, 128, 300, 158)
FLOOR_PATH = (180, 168, 300, 230)
SEA_FAR = (0, 128, 480, 210)


def write_bumps(path, size=256):
    """A tiling normal map of shallow dents: a sum of sine ripples whole numbers of times across."""
    waves = [(3, 7, 0.3, 0.0), (11, -4, 0.22, 1.3), (-9, 13, 0.18, 2.1), (17, 5, 0.12, 0.7), (6, -19, 0.1, 4.2), (23, 21, 0.07, 3.3)]
    img = Image.new("RGB", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            u, v = x / size, y / size
            dx = dy = 0.0
            for fx, fy, a, ph in waves:
                c = math.cos(2 * math.pi * (fx * u + fy * v) + ph) * a * 2 * math.pi / 160
                dx += c * fx
                dy += c * fy
            n = (-dx, dy, 1.0)
            l = math.sqrt(n[0] ** 2 + n[1] ** 2 + n[2] ** 2)
            px[x, y] = tuple(int(round((c / l * 0.5 + 0.5) * 255)) for c in n)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.save(path)


def metals(r, path):
    r.rpc("world.clear")
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": 1}}})
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"yaw": 35, "pitch": -40}}, "Light": {"kind": 0, "intensity": 2.0}}})
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.1, "z": 0}, "scale": {"x": 40, "y": 0.2, "z": 40}},
                                                          "MeshRenderer": {"mesh": "cube", "color": "#8a8a8a", "roughness": 0.6}}})
    for row, (color, z) in enumerate([("#ffc356", 0.0), ("#e8e8ec", -2.2)]):
        for i, rough in enumerate([0.05, 0.25, 0.45, 0.65, 0.85, 1.0]):
            r.rpc("world.spawn", {"name": f"Ball{row}{i}", "components": {
                "Transform": {"position": {"x": -4.75 + 1.9 * i, "y": 0.8, "z": z}, "scale": {"x": 1.6, "y": 1.6, "z": 1.6}},
                "MeshRenderer": {"mesh": "sphere", "color": color, "metallic": 1.0, "roughness": rough}}})
    r.rpc("world.spawn", {"name": "View", "components": {"Transform": {"position": {"x": 0, "y": 4.6, "z": 7.4}, "look_at": {"x": 0, "y": 1.4, "z": -1.1}},
                                                         "Camera": {"fov_degrees": 44, "priority": 100}}})
    r.rpc("render.ao", {"enabled": True})
    r.rpc("render.tonemap", {"operator": "agx", "exposure": 1.1})
    r.rpc("step", {"ticks": 2, "render": "each"})
    r.rpc("capture", {"path": path})
    return Image.open(path).convert("RGB")


def floor(r, path, frames):
    """The floor's frames, the camera creeping forward 7 cm a frame."""
    r.rpc("world.clear")
    r.rpc("world.spawn", {"name": "Sky", "components": {"Sky": {"mode": 1}}})
    r.rpc("world.spawn", {"name": "Sun", "components": {"Transform": {"rotation": {"yaw": 180, "pitch": -14}}, "Light": {"kind": 0, "intensity": 2.5}}})
    r.rpc("world.spawn", {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.1, "z": 0}, "scale": {"x": 600, "y": 0.2, "z": 600}},
                                                          "MeshRenderer": {"mesh": "cube", "color": "#3a3d44", "metallic": 0.0, "roughness": 0.12,
                                                                           "normal_map": "assets/specular_bumps.png", "texture_tile": 1.5}}})
    r.rpc("render.tonemap", {"operator": "agx", "exposure": 1.0})
    shots = []
    for k in range(frames):
        at = {"position": {"x": 0, "y": 1.6, "z": -0.07 * k}, "rotation": {"yaw": 0, "pitch": -4}}
        if k == 0:
            r.rpc("world.spawn", {"name": "View", "components": {"Transform": at, "Camera": {"fov_degrees": 60, "far": 1000, "priority": 100}}})
        else:
            r.rpc("world.set", {"entity": "View", "component": "Transform", "value": at})
        r.rpc("step", {"ticks": 1, "render": "each"})
        r.rpc("capture", {"path": path})
        shots.append(Image.open(path).convert("RGB"))
    return shots


def sea(r, path):
    r.rpc("world.set", {"entity": "Sky", "component": "Sky", "value": {"time_of_day": 16.8, "day_length": 0}})
    r.rpc("world.spawn", {"name": "Shot", "components": {"Transform": {"position": {"x": 40, "y": 7, "z": 120}, "rotation": {"yaw": 200, "pitch": -2}},
                                                         "Camera": {"fov_degrees": 60, "far": 1500, "priority": 100}}})
    r.rpc("step", {"ticks": 60, "render": "each"})
    r.rpc("capture", {"path": path})
    return Image.open(path).convert("RGB")


def to_linear(v):
    c = v / 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def to_srgb(c):
    c = min(max(c, 0.0), 1.0)
    return round(255 * (c * 12.92 if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055))


def reduce_linear(img, k):
    """The picture k times smaller each way, each pixel the mean of k by k in linear light."""
    lin = [to_linear(v) for v in range(256)]
    w, h = img.width // k, img.height // k
    out = Image.new("RGB", (w, h))
    src = img.load()
    dst = out.load()
    for y in range(h):
        for x in range(w):
            acc = [0.0, 0.0, 0.0]
            for dy in range(k):
                for dx in range(k):
                    p = src[x * k + dx, y * k + dy]
                    acc[0] += lin[p[0]]
                    acc[1] += lin[p[1]]
                    acc[2] += lin[p[2]]
            dst[x, y] = tuple(to_srgb(a / (k * k)) for a in acc)
    return out


def mean_diff(a, b, box):
    return sum(ImageStat.Stat(ImageChops.difference(a.crop(box), b.crop(box))).mean) / 3


def flicker(shots, box):
    """The mean change of a pixel inside box from one frame to the next, in 8-bit levels."""
    return sum(mean_diff(a, b, box) for a, b in zip(shots, shots[1:])) / (len(shots) - 1)


def specks(img, box, threshold=40):
    """The share of the pixels inside box more than threshold grey levels off their 3x3 median."""
    grey = img.convert("L")
    med = grey.filter(ImageFilter.MedianFilter(3))
    g, m = grey.load(), med.load()
    hits = sum(1 for y in range(box[1], box[3]) for x in range(box[0], box[2]) if abs(g[x, y] - m[x, y]) > threshold)
    return hits / ((box[2] - box[0]) * (box[3] - box[1]))


def enlarged(img, box):
    return img.crop(box).resize(((box[2] - box[0]) * 4, (box[3] - box[1]) * 4), Image.NEAREST)


def sheet(rows, path):
    width = sum(im.width for im in rows[0])
    out = Image.new("RGB", (width, sum(row[0].height for row in rows)))
    y = 0
    for row in rows:
        x = 0
        for im in row:
            out.paste(im, (x, y))
            x += im.width
        y += row[0].height
    out.save(path)
    print(path)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", required=True, help="the old pocket_runtime")
    ap.add_argument("--after", default=None, help="the new pocket_runtime (build/release's by default)")
    ap.add_argument("--frames", type=int, default=12)
    args = ap.parse_args()
    write_bumps(BUMPS)
    runs = {}
    try:
        for k, (tag, exe, scale) in enumerate([("before", args.before, 1), ("after", args.after, 1), ("reference", args.before, SS)]):
            size = f"{W * scale}x{H * scale}"
            with Runtime("playground", port=4981 + k, size=size, release=True, exe=exe) as r:
                m = metals(r, os.path.join(BUILD, f"specular-metals-{tag}.png")) if scale == 1 else None
                f = floor(r, os.path.join(BUILD, f"specular-floor-{tag}.png"), args.frames if scale == 1 else 1)
            with Runtime("island", port=4985 + k, size=size, release=True, exe=exe) as r:
                s = sea(r, os.path.join(BUILD, f"specular-sea-{tag}.png"))
            if scale != 1:
                f = [reduce_linear(f[0], scale)]
                s = reduce_linear(s, scale)
            runs[tag] = (m, f, s)
    finally:
        os.remove(BUMPS)
        if not os.listdir(os.path.dirname(BUMPS)):
            os.rmdir(os.path.dirname(BUMPS))
    ref_sea = runs["reference"][2]
    for tag in ("before", "after", "reference"):
        _, f, s = runs[tag]
        line = f"{tag}: floor's sun path specks {specks(f[0], FLOOR_PATH):.4f}, sea specks {specks(s, SEA_FAR):.4f}"
        if tag != "reference":
            line += f"; sea {mean_diff(s, ref_sea, SEA_BOX):.2f} from the reference, floor flicker {flicker(f, FLOOR_BOX):.2f}"
        print(line)
    sheet([[runs["before"][0], runs["after"][0]]], os.path.join(EVIDENCE, "specular-metals.png"))
    tags = ("before", "after", "reference")
    sheet([[runs[t][1][0] for t in tags], [enlarged(runs[t][1][0], FLOOR_BOX) for t in tags],
           [runs[t][2] for t in tags], [enlarged(runs[t][2], SEA_BOX) for t in tags]], os.path.join(EVIDENCE, "specular-aa.png"))
