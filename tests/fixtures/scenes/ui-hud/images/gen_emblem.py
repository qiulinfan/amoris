"""Writes emblem.png, the UI scene's 64 x 64 image (docs/spec/ui.md): a ring on a blue disc with a
gold diamond, its edges antialiased and the outside transparent. Standard library only:
python tests/fixtures/scenes/ui-hud/images/gen_emblem.py"""

import math
import struct
import zlib
from pathlib import Path

SIZE = 64


def pixel(x: float, y: float) -> tuple[int, int, int, int]:
    """The emblem's colour at a point, 4 x 4 supersampled."""
    acc = [0.0, 0.0, 0.0, 0.0]
    for sy in range(4):
        for sx in range(4):
            u = (x + (sx + 0.5) / 4) / SIZE * 2 - 1
            v = (y + (sy + 0.5) / 4) / SIZE * 2 - 1
            r = math.hypot(u, v)
            if r > 0.96:
                continue
            if r > 0.82:
                c = (0.95, 0.95, 0.98)
            elif abs(u) + abs(v) < 0.5:
                c = (1.0, 0.78, 0.22)
            else:
                t = (v + 1) / 2
                c = (0.10 + 0.08 * t, 0.32 + 0.20 * t, 0.78 - 0.10 * t)
            for k in range(3):
                acc[k] += c[k]
            acc[3] += 1
    a = acc[3] / 16
    if acc[3] == 0:
        return (0, 0, 0, 0)
    return tuple(round(255 * acc[k] / acc[3]) for k in range(3)) + (round(255 * a),)


def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + r for r in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main() -> None:
    rows = [bytes(c for x in range(SIZE) for c in pixel(x, y)) for y in range(SIZE)]
    out = Path(__file__).with_name("emblem.png")
    out.write_bytes(png(SIZE, SIZE, rows))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
