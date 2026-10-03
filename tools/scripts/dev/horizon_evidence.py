"""tests/evidence/rendering/horizon.png: the horizon before and after the sea's mirror direction was
kept above it and the sky's lower half was made a ground seen through the horizon's air
(docs/design/water.md, Drawing it; docs/design/rendering.md, Sky and environment light). Left the
runtime given by --before, right build/release's. Rows: the island's sea at half past nine and the
hills made an ocean at eleven (the band under their horizon, enlarged twice: island_evidence.py's
and ocean_evidence.py's first views), the procedural sky beyond a glossy floor (ssr_evidence.py's
scene, reflections off), and an atmosphere beyond the village's grass (village_evidence.py's scene
at 640x360).

    python3 tools/scripts/dev/horizon_evidence.py --before <an older build's pocket_runtime>
        (release build; samples/island, hills, playground and hello bundled; Pillow)
"""
import argparse
import os
import sys

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))
import island_evidence  # noqa: E402
import ocean_evidence  # noqa: E402
import ssr_evidence  # noqa: E402
import village_evidence  # noqa: E402
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "horizon.png")
W, GAP = 640, 8


def floor_view(path, exe):
    with Runtime("playground", port=4993, size="640x360", release=True, exe=exe) as r:
        ssr_evidence.build(r)
        r.rpc("step", {"ticks": 3, "render": "each"})
        r.rpc("capture", {"path": path})


def panels(label, exe):
    """The four panels of one column, each 640 wide."""
    d = os.path.join(ROOT, "build", "horizon")
    os.makedirs(d, exist_ok=True)
    p = {k: os.path.join(d, f"{label}-{k}.png") for k in ("island", "ocean", "floor", "village")}
    island_evidence.capture(island_evidence.VIEWS[0], p["island"], exe, port=4994)
    ocean_evidence.capture(ocean_evidence.VIEWS[0], p["ocean"], exe, port=4995)
    floor_view(p["floor"], exe)
    village_evidence.capture(p["village"], "640x360", exe)
    im = {k: Image.open(v).convert("RGB") for k, v in p.items()}
    zoom = lambda img, box: img.crop(box).resize((W, (box[3] - box[1]) * 2), Image.NEAREST)  # noqa: E731
    return [zoom(im["island"], (0, 135, 320, 225)), zoom(im["ocean"], (0, 150, 320, 240)), im["floor"], im["village"]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", required=True, help="the runtime binary to compare against")
    ap.add_argument("--out", default=OUT)
    a = ap.parse_args()
    cols = [panels("before", os.path.abspath(a.before)), panels("after", None)]
    h = sum(p.height for p in cols[0]) + GAP * (len(cols[0]) - 1)
    sheet = Image.new("RGB", (W * 2 + GAP, h), (0, 0, 0))
    for c, col in enumerate(cols):
        y = 0
        for p in col:
            sheet.paste(p, (c * (W + GAP), y))
            y += p.height + GAP
    sheet.save(a.out)
    print(a.out)


if __name__ == "__main__":
    main()
