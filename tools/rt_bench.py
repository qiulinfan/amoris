#!/usr/bin/env python3
"""Runs the ray-query research tools on Windows backends and compares them with the Metal references.

Two suites (docs/bench/path-tracing-nrc.md and docs/bench/metal-gi.md, Windows sections):

- `correctness`: each configuration whose Apple M5 report is committed under docs/evidence/pt and
  docs/evidence/gi is rendered once per backend at the same settings. Every run is compared with the
  M5 report (scene signature, per-frame counters, mean HDR radiance) and its preview PNG (8-bit
  differences, and an approximate HDR error after inverting the preview's Reinhard/sRGB encoding,
  since the M5 runs committed no linear HDR), with this backend's own finite reference in linear HDR
  next to the RMSE the M5 recorded against its reference, and the backends with each other.
- `timing`: the documented timing settings (PT optimization variants, SHaRC raw/cached, PT vs NRC),
  each also with `--lean-shaders` (no naga loop bounding or ray-query tracking), `--rounds` times
  with the order reversed every other round, so that load from other work on the machine spreads
  over all configurations.

Build first: `cargo build --release -p pocket-render --examples`. Then, from the repository root:

    python tools/rt_bench.py correctness --backends vulkan,dx12 --adapter 5060 \
        --summary docs/evidence/rt/correctness-5060.json
    python tools/rt_bench.py timing --backends vulkan,dx12 --adapter 5060 --rounds 3 \
        --summary docs/evidence/rt/timing-5060.json

Linear HDR sidecars, previews and full reports go to `--out` (default `out/rt`, ignored by git).
Standard library only; PNG decoding is the minimal 8-bit RGB(A) subset the engine's tools write.
"""
import argparse
import json
import math
import os
import statistics
import struct
import subprocess
import sys
import zlib
from array import array
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
EVIDENCE = ROOT / "docs" / "evidence"


def example(name):
    return str(ROOT / "target" / "release" / "examples" / f"{name}{EXE}")


ROOM = ["--width", "128", "--height", "128"]
RESTIR = ["--width", "160", "--height", "120", "--frames", "32"]


def config(tool, project, arguments, metal=None, reference=None, metal_rmse=None):
    """One correctness configuration: `metal` is the committed M5 report (without extension),
    `reference` an earlier configuration of this suite to compute the HDR RMSE against, and
    `metal_rmse` (file, key, field) where the M5 RMSE against its own reference is recorded."""
    return {"tool": tool, "project": project, "arguments": arguments, "metal": metal,
            "reference": reference, "metal_rmse": metal_rmse}


PT_CMP = "pt/comparison.json"
SHARC_CMP = "gi/sharc/comparison-final-320.json"
RESTIR_CMP = "gi/restir/comparison-160-32.json"
# References come before the configurations compared with them.
CORRECTNESS = {
    "pt-reference": config("path_trace", "samples/gi-room",
                           ROOM + ["--samples", "4", "--frames", "512"], "pt/reference"),
    "pt128": config("path_trace", "samples/gi-room", ROOM + ["--frames", "128"], "pt/pt128",
                    "pt-reference", (PT_CMP, "pt128", "rmse")),
    "pt512": config("path_trace", "samples/gi-room", ROOM + ["--frames", "512"], "pt/pt512",
                    "pt-reference", (PT_CMP, "pt512", "rmse")),
    "bsdf-only": config("path_trace", "samples/gi-room", ROOM + ["--frames", "128", "--nee", "false"],
                        "pt/bsdf-only", "pt-reference", (PT_CMP, "bsdf-only", "rmse")),
    "no-rr": config("path_trace", "samples/gi-room", ROOM + ["--frames", "128", "--rr", "false"],
                    "pt/no-rr", "pt-reference", (PT_CMP, "no-rr", "rmse")),
    "changed-reference": config("path_trace", "samples/gi-room",
                                ROOM + ["--samples", "4", "--frames", "512", "--emission-scale", "2.5"],
                                "pt/changed-reference"),
    "nrc": config("path_trace", "samples/gi-room",
                  ROOM + ["--frames", "512", "--nrc", "true", "--nrc-warmup", "64"], "pt/nrc",
                  "pt-reference", (PT_CMP, "nrc", "rmse")),
    "nrc-change": config("path_trace", "samples/gi-room",
                         ROOM + ["--frames", "512", "--nrc", "true", "--nrc-warmup", "64",
                                 "--light-change-frame", "256", "--light-change-factor", "2.5"],
                         "pt/nrc-change", "changed-reference", (PT_CMP, "nrc-change", "rmse")),
    "materials": config("path_trace", "samples/pt-lab",
                        ["--width", "320", "--height", "240", "--samples", "4", "--frames", "256"],
                        "pt/materials"),
    "atmosphere": config("path_trace", "samples/pt-lab",
                         ["--scene", "scene-atmosphere.json", "--width", "160", "--height", "120",
                          "--frames", "128"], "pt/atmosphere"),
    "sharc-reference": config("gi_trace", "samples/gi-room",
                              ["--mode", "raw", "--width", "320", "--height", "240", "--samples",
                               "16", "--frames", "64"], "gi/sharc/reference-final-320"),
    "sharc-raw": config("gi_trace", "samples/gi-room",
                        ["--mode", "raw", "--width", "320", "--height", "240", "--frames", "128"],
                        "gi/sharc/raw-final-320", "sharc-reference",
                        (SHARC_CMP, "results/raw-final-320", "rmse_linear")),
    "sharc": config("gi_trace", "samples/gi-room",
                    ["--mode", "sharc", "--width", "320", "--height", "240", "--frames", "128"],
                    "gi/sharc/sharc-final-320", "sharc-reference",
                    (SHARC_CMP, "results/sharc-final-320", "rmse_linear")),
    "restir-reference": config("gi_trace", "samples/gi-room",
                               ["--mode", "raw", "--width", "160", "--height", "120", "--samples",
                                "64", "--frames", "32"]),
    "restir-raw": config("gi_trace", "samples/gi-room", ["--mode", "raw"] + RESTIR, None,
                         "restir-reference", (RESTIR_CMP, "results/raw-none-k1-32", "rmse_linear")),
    "restir-none": config("gi_trace", "samples/gi-room",
                          ["--mode", "restir", "--reuse", "none"] + RESTIR, None, "restir-reference",
                          (RESTIR_CMP, "results/restir-none-k1-32", "rmse_linear")),
    "restir-temporal": config("gi_trace", "samples/gi-room",
                              ["--mode", "restir", "--reuse", "temporal"] + RESTIR, None,
                              "restir-reference",
                              (RESTIR_CMP, "results/restir-temporal-k1-32", "rmse_linear")),
    "restir-spatial": config("gi_trace", "samples/gi-room",
                             ["--mode", "restir", "--reuse", "spatial"] + RESTIR, None,
                             "restir-reference",
                             (RESTIR_CMP, "results/restir-spatial-k1-32", "rmse_linear")),
    "restir-both": config("gi_trace", "samples/gi-room",
                          ["--mode", "restir", "--reuse", "both"] + RESTIR, "gi/restir/reuse-both",
                          "restir-reference", (RESTIR_CMP, "results/restir-both-32", "rmse_linear")),
}

PT_ROOM = ["--width", "320", "--height", "240", "--frames", "128"]
NRC_ROOM = ["--width", "128", "--height", "128", "--frames", "512"]
# name -> (tool, project, arguments, frames summarized: "warm32" = from frame 32, "last64")
TIMING = {
    "pt-closest-blocking": ("path_trace", "samples/gi-room",
                            PT_ROOM + ["--readback-every-frame", "true"], "warm32"),
    "pt-closest-queued": ("path_trace", "samples/gi-room", PT_ROOM, "warm32"),
    "pt-any-queued": ("path_trace", "samples/gi-room", PT_ROOM + ["--shadow-any-hit", "true"],
                      "warm32"),
    "pt-any-wg128": ("path_trace", "samples/gi-room",
                     PT_ROOM + ["--shadow-any-hit", "true", "--workgroup", "128"], "warm32"),
    "sharc-raw": ("gi_trace", "samples/gi-room", ["--mode", "raw"] + PT_ROOM, "last64"),
    "sharc-cached": ("gi_trace", "samples/gi-room", ["--mode", "sharc"] + PT_ROOM, "last64"),
    "nrc-pt": ("path_trace", "samples/gi-room", NRC_ROOM, "last64"),
    "nrc": ("path_trace", "samples/gi-room", NRC_ROOM + ["--nrc", "true", "--nrc-warmup", "64"],
            "last64"),
}
# The same work with shaders built without naga's loop bounding and ray-query tracking.
LEAN = ["--lean-shaders", "true"]
TIMING.update({
    "pt-closest-queued-lean": ("path_trace", "samples/gi-room", PT_ROOM + LEAN, "warm32"),
    "sharc-raw-lean": ("gi_trace", "samples/gi-room", ["--mode", "raw"] + PT_ROOM + LEAN, "last64"),
    "sharc-cached-lean": ("gi_trace", "samples/gi-room", ["--mode", "sharc"] + PT_ROOM + LEAN,
                          "last64"),
    "nrc-pt-lean": ("path_trace", "samples/gi-room", NRC_ROOM + LEAN, "last64"),
    "nrc-lean": ("path_trace", "samples/gi-room",
                 NRC_ROOM + ["--nrc", "true", "--nrc-warmup", "64"] + LEAN, "last64"),
})


def read_png(path):
    """8-bit RGB or RGBA, non-interlaced PNG -> (width, height, [r, g, b, ...] in 0..255)."""
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or color not in (2, 6) or interlace:
        raise ValueError(f"{path}: only 8-bit RGB/RGBA non-interlaced PNGs are supported")
    channels = 3 if color == 2 else 4
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        kind = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + a) & 255
            elif kind == 2:
                line[i] = (line[i] + b) & 255
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    rgb = []
    for line in rows:
        for x in range(width):
            rgb.extend(line[x * channels:x * channels + 3])
    return width, height, rgb


def read_linear(path):
    """Little-endian float32 RGBA sidecar -> RGB list."""
    values = array("f")
    values.frombytes(Path(path).read_bytes())
    if sys.byteorder != "little":
        values.byteswap()
    return [v for i, v in enumerate(values) if i % 4 != 3]


def inverse_preview(code, exposure_ev):
    """Undo the tools' preview encoding (exposure, Reinhard x/(1+x), sRGB) for one 8-bit code."""
    s = code / 255.0
    mapped = s / 12.92 if s <= 0.04045 else ((s + 0.055) / 1.055) ** 2.4
    mapped = min(mapped, 254.5 / 255.0)  # a saturated code only bounds the radiance from below
    return mapped / (1.0 - mapped) / 2.0 ** exposure_ev


def hdr_compare(reference, candidate, keep=None):
    """Linear HDR RMSE and means; `keep` (one flag per pixel) restricts it to some pixels."""
    if len(reference) != len(candidate):
        raise ValueError("image dimensions differ")
    if keep is not None:
        pick = lambda values: [values[3 * p + c] for p, k in enumerate(keep) if k for c in range(3)]
        reference, candidate = pick(reference), pick(candidate)
    errors = [b - a for a, b in zip(reference, candidate)]
    mse = math.fsum(e * e for e in errors) / len(errors)
    rms_reference = math.sqrt(math.fsum(x * x for x in reference) / len(reference))
    mean = lambda values: [math.fsum(values[c::3]) / (len(values) // 3) for c in range(3)]
    # A few fireflies dominate the linear RMSE of these short runs; Reinhard x/(1+x) bounds them
    # (the M5 SHaRC and ReSTIR comparisons record both).
    reinhard = lambda x: max(x, 0.0) / (1.0 + max(x, 0.0))
    reinhard_mse = math.fsum((reinhard(b) - reinhard(a)) ** 2 for a, b in zip(reference, candidate))
    return {
        "rmse": math.sqrt(mse),
        "relative_rmse": math.sqrt(mse) / max(rms_reference, 1e-12),
        "rmse_reinhard": math.sqrt(reinhard_mse / len(errors)),
        "maximum_absolute_error": max(abs(e) for e in errors),
        "reference_mean_rgb": mean(reference),
        "candidate_mean_rgb": mean(candidate),
    }


def png_compare(reference_png, candidate_png):
    wa, ha, a = read_png(reference_png)
    wb, hb, b = read_png(candidate_png)
    if (wa, ha) != (wb, hb):
        raise ValueError("preview sizes differ")
    diffs = [abs(x - y) for x, y in zip(a, b)]
    pixels = len(diffs) // 3
    per_pixel = [max(diffs[3 * i:3 * i + 3]) for i in range(pixels)]
    return {
        "rmse_8bit": math.sqrt(math.fsum(d * d for d in diffs) / len(diffs)),
        "maximum_8bit": max(diffs),
        "identical_pixels": sum(1 for d in per_pixel if d == 0) / pixels,
        "pixels_over_2": sum(1 for d in per_pixel if d > 2) / pixels,
        "pixels_over_8": sum(1 for d in per_pixel if d > 8) / pixels,
    }


def frame_counters(report, keys=None):
    """Per-frame counter vectors of a path_trace or gi_trace report (gi_trace: the integer fields
    in `keys`, those both reports have)."""
    frames = report["frames"]
    if frames and "counters" in frames[0]:
        return [f["counters"] for f in frames]
    return [[f[k] for k in keys] for f in frames]


def counter_compare(reference, candidate):
    first_a, first_b = reference["frames"][0], candidate["frames"][0]
    keys = [k for k in first_a if k != "frame" and isinstance(first_a[k], int)
            and isinstance(first_b.get(k), int)]
    a, b = frame_counters(reference, keys), frame_counters(candidate, keys)
    if len(a) != len(b):
        return {"frames": [len(a), len(b)]}
    identical = sum(1 for x, y in zip(a, b) if x == y)
    worst = 0.0
    for x, y in zip(a, b):
        for p, q in zip(x, y):
            worst = max(worst, abs(p - q) / max(abs(p), 1))
    return {"frames": len(a), "identical_frames": identical,
            "maximum_relative_counter_difference": worst}


def run(tool, project, arguments, output, backend, adapter):
    env = dict(os.environ, POCKET_BACKEND=backend)
    if adapter:
        env["POCKET_ADAPTER"] = adapter
    output.parent.mkdir(parents=True, exist_ok=True)
    command = [example(tool), str(ROOT / project)] + arguments + ["--output", str(output)]
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=1800)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(command)} failed: {result.stderr.strip()[-2000:]}")
    return json.loads(output.with_suffix(".json").read_text())


def mean_rgb(linear):
    return [math.fsum(linear[c::3]) / (len(linear) // 3) for c in range(3)]


def metal_rmse(spec):
    """The M5 HDR RMSE against its own finite reference, from a committed comparison file."""
    if not spec:
        return None
    file, key, field = spec
    value = json.loads((EVIDENCE / file).read_text())
    for part in key.split("/"):
        value = value[part]
    return {"rmse": value[field], "rmse_reinhard": value.get("rmse_reinhard_linear")}


def compare_with_metal(metal, report, hdr, output):
    """A run against the committed M5 report and preview of the same configuration."""
    metal_report = json.loads((EVIDENCE / f"{metal}.json").read_text())
    row = {
        "scene_signature_matches_metal":
            report.get("scene_signature") == metal_report.get("scene_signature"),
        "counters_vs_metal": counter_compare(metal_report, report),
    }
    mean = mean_rgb(hdr)
    # path_trace reports carry the mean; gi_trace's M5 reports do not.
    if metal_report.get("mean_radiance"):
        row["metal_mean_rgb"] = metal_report["mean_radiance"]
        row["mean_relative_difference_vs_metal"] = [
            (m - r) / r for m, r in zip(mean, metal_report["mean_radiance"])]
    metal_png = EVIDENCE / f"{metal}.png"
    if metal_png.exists():
        row["preview_vs_metal_png"] = png_compare(metal_png, output)
        exposure = (report.get("preview") or {}).get("exposure_ev", 0.0)
        _, _, codes = read_png(metal_png)
        # Saturated codes only bound the radiance from below: leave those pixels out.
        keep = [max(codes[3 * p:3 * p + 3]) < 250 for p in range(len(codes) // 3)]
        inverse = hdr_compare([inverse_preview(c, exposure) for c in codes], hdr, keep)
        inverse["pixels_left_out"] = 1.0 - sum(keep) / len(keep)
        inverse["note"] = ("M5 radiance reconstructed from its 8-bit Reinhard/sRGB preview; the "
                           "quantization of that preview dominates, so this bounds rather than "
                           "measures the difference")
        row["hdr_vs_metal_preview_inverse"] = inverse
    return row


def correctness(args):
    out = Path(args.out)
    names = args.only.split(",") if args.only else list(CORRECTNESS)
    summary = {"suite": "correctness", "adapter_filter": args.adapter, "configurations": {}}
    linear = {}  # (configuration, backend) -> HDR, for references and backend comparisons
    for name in names:
        c = CORRECTNESS[name]
        entry = {"tool": c["tool"], "project": c["project"], "arguments": c["arguments"],
                 "metal_report": c["metal"] and f"docs/evidence/{c['metal']}.json",
                 "reference": c["reference"], "metal_rmse_vs_reference": metal_rmse(c["metal_rmse"]),
                 "runs": {}}
        backends = args.backends.split(",")
        for backend in backends:
            output = out / name / f"{backend}.png"
            report = run(c["tool"], c["project"], c["arguments"], output, backend, args.adapter)
            hdr = read_linear(output.with_suffix(".linear.f32"))
            linear[(name, backend)] = hdr
            row = {"adapter": report.get("adapter"), "backend": report.get("backend"),
                   "mean_rgb": mean_rgb(hdr), "gpu_errors": report.get("gpu_errors")}
            if c["metal"]:
                row.update(compare_with_metal(c["metal"], report, hdr, output))
            if (c["reference"], backend) in linear:
                row["hdr_vs_reference"] = hdr_compare(linear[(c["reference"], backend)], hdr)
            entry["runs"][backend] = row
            line = f"{name:18} {backend:7} mean {['%.6f' % v for v in row['mean_rgb']]}"
            if "counters_vs_metal" in row:
                line += f" counters vs M5 {row['counters_vs_metal']}"
            if "hdr_vs_reference" in row:
                line += (f" rmse vs reference {row['hdr_vs_reference']['rmse']:.5f}"
                         f" reinhard {row['hdr_vs_reference']['rmse_reinhard']:.5f}"
                         f" (M5 {entry['metal_rmse_vs_reference']})")
            print(line, flush=True)
        for i, a in enumerate(backends):
            for b in backends[i + 1:]:
                entry[f"hdr_{a}_vs_{b}"] = hdr_compare(linear[(name, a)], linear[(name, b)])
        summary["configurations"][name] = entry
    return summary


def window(frames, which):
    return frames[32:] if which == "warm32" else frames[-64:]


def timing_row(tool, report, which, linear):
    frames = window(report["frames"], which)
    mean = lambda key: statistics.fmean(f[key] or 0.0 for f in frames)
    if tool == "path_trace":
        row = {"gpu_trace_ms": mean("gpu_trace_ms"),
               "gpu_training_ms": mean("gpu_training_ms"),
               "path_and_shadow_rays_per_frame": statistics.fmean(
                   f["path_rays"] + f["shadow_rays"] for f in frames),
               "cache_queries_per_frame": statistics.fmean(
                   (f["nrc"] or {}).get("cache_queries", 0) for f in frames),
               "trace_wall_ms": report["trace_wall_ms"],
               "mean_rgb": report["mean_radiance"]}
        row["gpu_total_ms"] = row["gpu_trace_ms"] + row["gpu_training_ms"]
    else:
        row = {"gpu_total_ms": statistics.fmean(
                   (f["gpu_render_ms"] or 0) + (f["gpu_update_ms"] or 0) + (f["gpu_resolve_ms"] or 0)
                   for f in frames),
               "rays_per_frame": statistics.fmean(
                   f["update_trace_rays"] + f["update_shadow_rays"] + f["render_trace_rays"]
                   + f["render_shadow_rays"] for f in frames),
               "cache_hits_per_frame": statistics.fmean(f["cache_hits"] for f in frames),
               "trace_wall_ms": report["trace_wall_ms"],
               "mean_rgb": mean_rgb(linear)}
    return row


def timing(args):
    out = Path(args.out)
    names = args.only.split(",") if args.only else list(TIMING)
    backends = args.backends.split(",")
    configurations = [(n, b) for n in names for b in backends]
    runs = []
    for r in range(args.rounds):
        order = configurations if r % 2 == 0 else list(reversed(configurations))
        for name, backend in order:
            tool, project, arguments, which = TIMING[name]
            output = out / "timing" / f"{name}-{backend}.png"
            report = run(tool, project, arguments, output, backend, args.adapter)
            linear = read_linear(output.with_suffix(".linear.f32"))
            row = timing_row(tool, report, which, linear)
            row.update({"name": name, "requested_backend": backend, "backend": report.get("backend"),
                        "adapter": report.get("adapter"), "round": r, "frames": which,
                        "gpu_errors": report.get("gpu_errors")})
            runs.append(row)
            print(f"round {r} {name:20} {backend:7} gpu {row['gpu_total_ms']:.3f} ms/frame "
                  f"wall {row['trace_wall_ms']:.1f} ms", flush=True)
    table = {}
    for name, backend in configurations:
        mine = [x for x in runs if x["name"] == name and x["requested_backend"] == backend]
        row = {
            "gpu_ms_median": statistics.median(x["gpu_total_ms"] for x in mine),
            "gpu_ms_min": min(x["gpu_total_ms"] for x in mine),
            "gpu_ms_max": max(x["gpu_total_ms"] for x in mine),
            "trace_wall_ms_median": statistics.median(x["trace_wall_ms"] for x in mine),
            "mean_rgb": mine[0]["mean_rgb"],
        }
        if "gpu_training_ms" in mine[0]:
            row["gpu_trace_ms_median"] = statistics.median(x["gpu_trace_ms"] for x in mine)
            row["gpu_training_ms_median"] = statistics.median(x["gpu_training_ms"] for x in mine)
        table[f"{name}/{backend}"] = row
    for key, row in table.items():
        print(f"{key:34} gpu {row['gpu_ms_median']:.4f} ms/frame "
              f"[{row['gpu_ms_min']:.4f}, {row['gpu_ms_max']:.4f}] "
              f"wall {row['trace_wall_ms_median']:.1f} ms")
    return {"suite": "timing", "rounds": args.rounds, "adapter_filter": args.adapter,
            "provisional": args.note,
            "summary": table, "runs": runs}


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("suite", choices=["correctness", "timing"])
    parser.add_argument("--backends", default="vulkan,dx12")
    parser.add_argument("--adapter", default=None, help="POCKET_ADAPTER (index or name part)")
    parser.add_argument("--only", default=None, help="comma-separated configuration names")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--out", default=str(ROOT / "out" / "rt"))
    parser.add_argument("--summary", default=None)
    parser.add_argument("--note", default="other agents used the CPU and both GPUs during these runs",
                        help="the conditions, kept in the timing summary's `provisional` field")
    args = parser.parse_args()
    summary = correctness(args) if args.suite == "correctness" else timing(args)
    if args.summary:
        Path(args.summary).parent.mkdir(parents=True, exist_ok=True)
        text = json.dumps(summary, indent=1, allow_nan=False) + "\n"
        Path(args.summary).write_text(text, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
