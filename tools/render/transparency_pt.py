#!/usr/bin/env python3
"""The forward renderer against the path tracer on tests/fixtures/scenes/pt-lab, by region (docs/bench/transparency.md).

pt-lab holds an alpha-blended card (`BlendCard`, glTF BLEND, alpha 0.45) and a glass sphere (`Glass`,
transmission 1, IOR 1.5), which the path tracer (`path_trace`, docs/spec/path-tracing-nrc.md) renders
with stochastic coverage and refraction. The forward renderer drew both opaque before the transparent
stage (charter 4.4, Pioneer 2026-10-16). This draws the scene's linear radiance with each given build's
`showcase_bench --hdr` (4x MSAA, GTAO off unless `--gtao` says, occlusion culling off, at a fixed time)
and compares it with
one path-traced reference, over the whole image and over each named entity's pixels: the entity-id
image of the first build (`--ids`; that build's silhouettes, so every build is compared on the same
pixels).

Build the examples first (`cargo build --release -p amoris-render --example showcase_bench --example
path_trace`; `path_trace` needs ray queries). Then, from the repository's root:

    python tools/render/transparency_pt.py --build before=out/transparency/before --build after=. \\
        --out out/transparency/pt-lab --backend dx12 --adapter 5060

The reference is traced once per size and sample count into `--out` and reused. Each build's image,
id image and preview, and `summary.json` (RMSE, relative RMSE and mean error per region), stay there.
"""
import argparse
import json
import os
import subprocess
import sys
from array import array
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import compare_pt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
PROJECT = "tests/fixtures/scenes/pt-lab"
REGIONS = ["BlendCard", "Glass"]


def example(build, name):
    return str(Path(build).resolve() / "target" / "release" / "examples" / f"{name}{EXE}")


def run(argv, env):
    p = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
    if p.returncode:
        raise SystemExit(f"{' '.join(argv)} failed:\n{p.stderr[-2000:]}")
    return p


def rgb(path):
    return compare_pt.load(path)


def mae(reference, candidate):
    """Mean absolute error: unlike the RMSE, not ruled by the reference's few bright outliers
    (fireflies of caustic paths)."""
    return sum(abs(b - a) for a, b in zip(reference, candidate)) / len(reference)


def entity_names():
    scene = json.loads((ROOT / PROJECT / "scene.json").read_text(encoding="utf-8"))
    return {e.get("name", ""): i + 1 for i, e in enumerate(scene["entities"])}


def preview(path, values, width, height, exposure):
    """A Reinhard-mapped sRGB PNG of linear RGB `values` (with Pillow; skipped without it)."""
    try:
        from PIL import Image
    except ImportError:
        return
    out = bytearray()
    for v in values:
        x = max(v, 0.0) * exposure
        x = x / (1.0 + x)
        x = 12.92 * x if x <= 0.0031308 else 1.055 * x ** (1 / 2.4) - 0.055
        out.append(max(0, min(255, round(x * 255))))
    Image.frombytes("RGB", (width, height), bytes(out)).save(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", action="append", required=True, help="LABEL=CHECKOUT (its target/release)")
    ap.add_argument("--out", default="out/transparency/pt-lab")
    ap.add_argument("--backend", default="dx12")
    ap.add_argument("--adapter", default="")
    ap.add_argument("--width", type=int, default=320)
    ap.add_argument("--height", type=int, default=240)
    ap.add_argument("--frames", type=int, default=256)
    ap.add_argument("--samples", type=int, default=4)
    ap.add_argument("--gtao", default="off",
                    help="the forward images' GTAO (AMORIS_GTAO: off, depth, target); not off adds "
                         "-gtao-MODE to the names of the summary and of the forward images")
    ap.add_argument("--pt-args", default="",
                    help="more path_trace options, e.g. '--emission-scale 0' (the forward renderer is "
                         "not lit by emissive surfaces)")
    args = ap.parse_args()
    out = (ROOT / args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    builds = [b.split("=", 1) for b in args.build]
    env = dict(os.environ, AMORIS_BACKEND=args.backend, AMORIS_AA="msaa", AMORIS_GTAO=args.gtao,
               AMORIS_OCCLUSION="off")
    if args.adapter:
        env["AMORIS_ADAPTER"] = args.adapter
    size = ["--width", str(args.width), "--height", str(args.height)]
    tag = "".join(c if c.isalnum() else "-" for c in args.pt_args.strip()).strip("-")
    tag = f"-{tag}" if tag else ""
    # The references depend on the path tracer's options only; the forward images on GTAO too.
    gtao_tag = f"-gtao-{args.gtao}" if args.gtao != "off" else ""

    def trace(seed):
        name = f"reference-{args.width}x{args.height}-{args.frames}x{args.samples}{tag}-seed{seed}.png"
        linear = (out / name).with_suffix(".linear.f32")
        if not linear.exists():
            run([example(builds[-1][1], "path_trace"), PROJECT, *size, "--frames", str(args.frames),
                 "--samples", str(args.samples), "--seed", str(seed), *args.pt_args.split(),
                 "--output", str(out / name)], env)
        return linear

    linear = trace(1)
    ref = rgb(linear)
    names = entity_names()
    masks = None
    summary = {"tool": "tools/render/transparency_pt.py", "backend": args.backend, "adapter": args.adapter or None,
               "width": args.width, "height": args.height, "reference": linear.name,
               "reference_paths_per_pixel": args.frames * args.samples, "path_trace_args": args.pt_args,
               "gtao": args.gtao, "builds": {}}
    images = []
    for label, build in builds:
        hdr, ids = out / f"{label}{gtao_tag}.f32", out / f"{label}{gtao_tag}.ids.u32"
        p = run([example(build, "showcase_bench"), PROJECT, *size, "--frames", "5", "--warm", "5",
                 "--hdr", str(hdr), "--ids", str(ids)], env)
        report = json.loads(p.stdout[p.stdout.index("{"):])
        id_image = array("I")
        id_image.frombytes(ids.read_bytes())
        if masks is None:
            # The first build's silhouettes are every build's regions.
            masks = {name: [i for i, e in enumerate(id_image) if e == names[name]] for name in REGIONS}
        images.append((label, rgb(hdr), {"adapter": report.get("adapter"), "backend": report.get("backend")}))
    # The reference's own noise: a second path-traced image with another seed against it.
    images.append(("pt-seed2", rgb(trace(2)), {"note": "the path tracer with seed 2: the comparison's floor"}))
    for label, image, result in images:
        result["all"] = compare_pt.compare(ref, image)
        result["all"]["mae"] = mae(ref, image)
        for name, pixels in masks.items():
            pick = lambda values: [values[3 * i + c] for i in pixels for c in range(3)]  # noqa: E731
            result[name] = dict(compare_pt.compare(pick(ref), pick(image)), mae=mae(pick(ref), pick(image)))
        summary["builds"][label] = result
        suffix = tag if label.startswith("pt-") else gtao_tag
        preview(out / f"{label}{suffix}.png", image, args.width, args.height, 1.0)
    preview(out / f"reference{tag}-preview.png", ref, args.width, args.height, 1.0)
    summary["region_pixels"] = {name: len(p) for name, p in masks.items()}
    (out / f"summary{tag}{gtao_tag}.json").write_text(json.dumps(summary, indent=1) + "\n", newline="\n")
    print(f"{'image':10} {'region':10} {'pixels':>7} {'rmse':>8} {'rel rmse':>8} {'mae':>8}  mean error rgb")
    for label, result in summary["builds"].items():
        for region in ["all", *REGIONS]:
            r = result[region]
            err = ", ".join(f"{e:+.4f}" for e in r["mean_error_rgb"])
            print(f"{label:10} {region:10} {r['pixels']:7} {r['rmse']:8.4f} {r['relative_rmse']:8.3f} "
                  f"{r['mae']:8.4f}  {err}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
