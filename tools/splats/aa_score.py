"""Scores captures against a supersampled reference (PIL only, no numpy).

python aa_score.py DIR TAG SSTAG FACTOR [x0 y0 x1 y1]

Downsamples DIR/SSTAG_{off,on}.png by FACTOR in linear light (box filter), then reports PSNR and
mean absolute difference (of 255) of DIR/TAG_{off,on}.png against each reference, over the image
and over a crop.
"""
import array
import json
import math
import sys

from PIL import Image

d, tag, ss, f = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
crop = tuple(int(v) for v in sys.argv[5:9]) if len(sys.argv) >= 9 else None


def lin(v):
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4


def srgb(v):
    v = min(max(v, 0.0), 1.0)
    return v * 12.92 if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055


LUT = [lin(i / 255) for i in range(256)]


def down(path):
    img = Image.open(path).convert('RGB')
    w, h = img.size
    out = []
    for ch in img.split():
        a = array.array('f', [LUT[b] for b in ch.tobytes()])
        fi = Image.frombytes('F', (w, h), a.tobytes())
        small = fi.resize((w // f, h // f), Image.Resampling.BOX)
        vals = array.array('f')
        vals.frombytes(small.tobytes())
        out.append(bytes(int(srgb(v) * 255 + 0.5) for v in vals))
    chans = [Image.frombytes('L', (w // f, h // f), o) for o in out]
    return Image.merge('RGB', chans)


def score(a, b, region=None):
    if region:
        a = a.crop(region)
        b = b.crop(region)
    x, y = a.tobytes(), b.tobytes()
    se = 0
    ae = 0
    for p, q in zip(x, y):
        e = p - q
        se += e * e
        ae += abs(e)
    n = len(x)
    mse = se / n
    return {'psnr': round(10 * math.log10(255.0 ** 2 / max(mse, 1e-12)), 2), 'mad': round(ae / n, 3)}


refs = {}
for m in ('off', 'on'):
    refs[m] = down(f'{d}/{ss}_{m}.png')
    refs[m].save(f'{d}/{ss}_{m}_down.png')
out = {}
for m in ('off', 'on'):
    img = Image.open(f'{d}/{tag}_{m}.png').convert('RGB')
    for rm, r in refs.items():
        out[f'{m} vs ref_{rm}'] = {'full': score(img, r), 'crop': score(img, r, crop) if crop else None}
out['ref_off vs ref_on'] = {'full': score(refs['off'], refs['on']),
                            'crop': score(refs['off'], refs['on'], crop) if crop else None}
print(json.dumps(out, indent=1))
