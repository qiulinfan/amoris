#!/usr/bin/env python3
"""TAA in the samples without it (docs/design/rendering.md, Temporal anti-aliasing, TAA in the
samples): hills, island, walker and drive, each drawn frame by frame from its start at 640x360, the
camera still for a second and then following the player (hills, walker), the boat (island) or the
car (drive) as an action held from tick 61 moves it off, in the sample's own settings and with
TAA, against a reference: the same ticks drawn at four times the size without anti-aliasing and
averaged down. For the frames at ticks 60 (still) and 120 (moving) it prints

- `off`: the mean absolute difference from the reference, in 8-bit levels over the frame and its
  channels (how far the frame is from an anti-aliased one, softness and jaggies alike);
- `jaggies`: the pixels more than 16 levels off in a channel (stair-stepped edges, ghosts);
- `flicker`: the mean absolute difference between the frame's change from the tick before and the
  reference's (what crawls or shimmers from frame to frame that should not).

and writes tests/evidence/rendering/taa-samples.png, a crop of each frame three times enlarged.

    python3 tools/scripts/taa_samples_evidence.py [sample...]   # all four by default
"""
import json
import os
import socket
import sys
import tempfile

from PIL import Image, ImageChops, ImageDraw

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ROOT, Runtime  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "rendering", "taa-samples.png")
W, H = 640, 360
STILL, MOVING = 60, 120
TICKS = (STILL - 1, STILL, MOVING - 1, MOVING)
TAA = {"render.taa": {"enabled": True}}
SAMPLES = {
    # sample: (the hold that moves it, the settings compared, the crop shown)
    "hills": ({"action": "move_x", "ticks": 200}, [("its own (no anti-aliasing)", {}), ("TAA", TAA)], (300, 120, 460, 230)),
    "island": ({"action": "move_z", "sign": -1, "ticks": 200}, [("its own (MSAA 4x)", {}), ("TAA and MSAA 4x", TAA),
                                                                ("TAA instead of MSAA", {**TAA, "render.msaa": {"samples": 1}})], (220, 150, 380, 260)),
    "walker": ({"action": "move_z", "sign": -1, "ticks": 200}, [("its own (no anti-aliasing)", {}), ("TAA", TAA)], (240, 110, 400, 220)),
    "drive": ({"action": "throttle", "ticks": 200}, [("its own (no anti-aliasing)", {}), ("TAA", TAA)], (240, 130, 400, 240)),
}


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def frames(sample, size, settings, hold, tag):
    """The frames at TICKS, every tick drawn; the hold starts after STILL."""
    r = Runtime(sample, free_port(), size=size, release=True)
    shots = {}
    try:
        for method, params in settings.items():
            r.rpc(method, params)
        for t in range(1, MOVING + 1):
            if t == STILL + 1:
                r.rpc("input.hold", hold)
            r.rpc("step", {"ticks": 1, "render": True})
            if t in TICKS:
                path = os.path.join(tempfile.gettempdir(), f"taa-samples-{sample}-{tag}-{t}.png")
                r.rpc("capture", {"path": path})
                shots[t] = Image.open(path).convert("RGB")
    finally:
        r.close()
    return shots


def measure(shots, reference, t):
    d = ImageChops.difference(shots[t], reference[t])
    off = sum(i % 256 * n for i, n in enumerate(d.histogram())) / (3 * W * H)
    jaggies = sum(1 for p in d.get_flattened_data() if max(p) > 16)
    a, b = shots[t].tobytes(), shots[t - 1].tobytes()
    ra, rb = reference[t].tobytes(), reference[t - 1].tobytes()
    flicker = sum(abs((a[i] - b[i]) - (ra[i] - rb[i])) for i in range(len(a))) / len(a)
    return {"off": round(off, 3), "jaggies": jaggies, "flicker": round(flicker, 3)}


def main():
    rows, report = [], {}
    for sample in sys.argv[1:] or SAMPLES:
        hold, configs, crop = SAMPLES[sample]
        big = frames(sample, f"{W * 4}x{H * 4}", {"render.msaa": {"samples": 1}}, hold, "reference")
        reference = {t: im.resize((W, H), Image.BOX) for t, im in big.items()}
        report[sample] = {}
        cells = []
        for k, (label, settings) in enumerate(configs):
            shots = frames(sample, f"{W}x{H}", settings, hold, f"c{k}")
            report[sample][label] = {"still": measure(shots, reference, STILL), "moving": measure(shots, reference, MOVING)}
            cells.append((label, shots))
        cells.append(("reference (4x4 supersampled)", reference))
        rows.append((sample, crop, cells))
    cw = max((c[2] - c[0]) for _, c, _ in rows) * 3
    ch = max((c[3] - c[1]) for _, c, _ in rows) * 3
    columns = max(len(cells) for _, _, cells in rows)
    sheet = Image.new("RGB", (cw * columns, (ch + 20) * 2 * len(rows)), (20, 20, 20))
    draw = ImageDraw.Draw(sheet)
    y = 0
    for sample, crop, cells in rows:
        for when, t in (("still", STILL), ("moving", MOVING)):
            for c, (label, shots) in enumerate(cells):
                sheet.paste(shots[t].crop(crop).resize(((crop[2] - crop[0]) * 3, (crop[3] - crop[1]) * 3), Image.NEAREST), (c * cw, y + 20))
                note = f"{sample}, {label}, tick {t} ({when})"
                if label in report[sample]:
                    m = report[sample][label][when]
                    note += f": off {m['off']:.2f}, flicker {m['flicker']:.2f}"
                draw.text((c * cw + 6, y + 4), note, fill=(230, 230, 230))
            y += ch + 20
    sheet.save(OUT)
    print(json.dumps({"out": os.path.relpath(OUT, ROOT), "measures": report}, indent=1))


if __name__ == "__main__":
    main()
