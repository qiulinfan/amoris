#!/usr/bin/env python3
"""tests/evidence/assets/obj-import-fixed.png: the OBJ reader's fixes on the stress files, each
row the release runtime's capture before the fix (left) beside after it (right), the last row the
machined part at the origin beside the same part 4.5 km out, both read now.

    1. with the old runtime built: measure.py ngons, machined, cube_soff; copy
       build/objstress/runs/captures/*.png to build/objstress/before/
    2. with the new one: measure.py ngons, machined, cube_soff, machined_near, machined_far
    3. python3 tools/scripts/dev/objstress/evidence.py [--before DIR] [--out PNG]

Needs Pillow.
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
AFTER = os.path.join(ROOT, "build", "objstress", "runs", "captures")
ROWS = [
    ("ngons.obj: concave polygons fanned from their first corner (left), ear-clipped now (right)", "before/ngons.png", "after/ngons.png"),
    ("machined_2m.obj, no vn, welded: smoothed per position (left), crease-angle normals, read Z up in mm (right)", "before/machined_closeup.png", "after/machined_closeup.png"),
    ("cube_soff.obj: s off ignored (left), flat (right)", "before/cube_soff.png", "after/cube_soff.png"),
    ("machined_near.obj at the origin (left) and machined_far.obj 4.5 km out (right), both read as doubles and recentred: the same pixels", "after/machined_near.png", "after/machined_far.png"),
]
HALF = (640, 360)
LABEL = 22


def main():
    args = sys.argv[1:]
    before = args[args.index("--before") + 1] if "--before" in args else os.path.join(ROOT, "build", "objstress", "before")
    out = args[args.index("--out") + 1] if "--out" in args else os.path.join(ROOT, "tests", "evidence", "assets", "obj-import-fixed.png")
    dirs = {"before": before, "after": AFTER}
    font = ImageFont.load_default(size=14)
    sheet = Image.new("RGB", (HALF[0] * 2 + 4, len(ROWS) * (HALF[1] + LABEL)), "white")
    draw = ImageDraw.Draw(sheet)
    for i, (label, left, right) in enumerate(ROWS):
        y = i * (HALF[1] + LABEL)
        draw.text((4, y + 3), label, fill="black", font=font)
        for k, name in enumerate((left, right)):
            kind, file = name.split("/")
            img = Image.open(os.path.join(dirs[kind], file)).convert("RGB").resize(HALF, Image.LANCZOS)
            sheet.paste(img, (k * (HALF[0] + 4), y + LABEL))
    sheet.save(out, optimize=True)
    print(out)


if __name__ == "__main__":
    main()
