#!/usr/bin/env python3
"""Repack embedded GLB textures for interactive walking without changing scene geometry.

Only image buffer-view bytes and their offsets/lengths change. Normal-only RG textures whose
entire blue channel is zero have their missing positive tangent-space Z reconstructed. Green
is already in glTF convention and is never flipped here. Mesh accessors, node transforms,
materials (including transmission), and scene instances are preserved. Requires existing
Pillow and NumPy installations; installs nothing.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import struct
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_INPUT = ROOT / "out/town-gi/project/models/third-party/BistroExterior.glb"
SOURCE_URL = "https://developer.nvidia.com/orca/amazon-lumberyard-bistro"
LICENSE_URL = "https://creativecommons.org/licenses/by/4.0/"


def image_roles(document: dict) -> dict[int, set[str]]:
    """Resolve all material texture references, including material extensions."""
    roles: dict[int, set[str]] = {}
    textures = document.get("textures", [])
    def visit(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key.endswith("Texture") and isinstance(item, dict) and type(item.get("index")) is int:
                    texture = textures[item["index"]]
                    if "source" in texture:
                        roles.setdefault(texture["source"], set()).add("normal" if "normal" in key.casefold() else key)
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)
    visit(document.get("materials", []))
    return roles


def reconstruct_normal_z(image):
    """Keep R/G/alpha exactly; reconstruct +Z from signed UNORM R/G, without a G flip."""
    try:
        import numpy as np
        from PIL import Image
    except ImportError as error:
        raise ValueError("normal reconstruction needs an existing Pillow/NumPy interpreter; no dependencies are installed") from error
    rgba = np.array(image.convert("RGBA"), dtype=np.uint8, copy=True)
    x = rgba[:, :, 0].astype(np.float32) * (2.0 / 255.0) - 1.0
    y = rgba[:, :, 1].astype(np.float32) * (2.0 / 255.0) - 1.0
    z = np.sqrt(np.maximum(1.0 - x * x - y * y, 0.0))
    rgba[:, :, 2] = np.clip(np.floor((z * 0.5 + 0.5) * 255.0 + 0.5), 0, 255).astype(np.uint8)
    return Image.fromarray(rgba)


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_glb(path: Path):
    raw = path.read_bytes()
    if len(raw) < 20:
        raise ValueError("file is too short for a GLB header")
    magic, version, length = struct.unpack_from("<III", raw)
    if magic != 0x46546C67 or version != 2 or length != len(raw):
        raise ValueError("expected a complete GLB version 2 file")
    document = None
    binary = None
    offset = 12
    while offset < len(raw):
        if offset + 8 > len(raw):
            raise ValueError("truncated GLB chunk header")
        size, kind = struct.unpack_from("<II", raw, offset)
        offset += 8
        if size % 4 or offset + size > len(raw):
            raise ValueError("invalid GLB chunk length/alignment")
        if kind == 0x4E4F534A and document is None:
            document = json.loads(raw[offset:offset + size])
        elif kind == 0x004E4942 and binary is None:
            binary = memoryview(raw)[offset:offset + size]
        else:
            raise ValueError("unexpected or duplicate GLB chunk")
        offset += size
    if document is None or binary is None:
        raise ValueError("GLB must contain JSON and BIN chunks")
    buffers = document.get("buffers", [])
    if len(buffers) != 1 or "uri" in buffers[0] or buffers[0]["byteLength"] > len(binary):
        raise ValueError("expected one embedded GLB buffer")
    return document, binary


def prepare(source: Path, output: Path, maximum: int, report: Path):
    try:
        from PIL import Image
    except ImportError as error:
        raise ValueError("Pillow is not available in this Python; select an existing Pillow interpreter, without installing dependencies") from error
    if not 1 <= maximum <= 16_384:
        raise ValueError("--max-texture must be in 1..16384")
    source = source.resolve()
    output = output.resolve()
    report = report.resolve()
    if source == output or source == report or output == report:
        raise ValueError("input, output, and report must be different files")
    if output.suffix.lower() != ".glb":
        raise ValueError("output must have the .glb extension")
    started = time.perf_counter()
    document, binary = read_glb(source)
    views = document.get("bufferViews", [])
    images = document.get("images", [])
    roles = image_roles(document)
    reconstructed = 0
    replacements = {}
    image_report = []
    for index, image in enumerate(images):
        if "bufferView" not in image or "uri" in image:
            raise ValueError(f"image {index} must be embedded in a GLB buffer view")
        view_index = image["bufferView"]
        view = views[view_index]
        begin = view.get("byteOffset", 0)
        end = begin + view["byteLength"]
        if view.get("buffer", 0) != 0 or begin < 0 or end > len(binary):
            raise ValueError(f"image {index} buffer view is invalid")
        original = binary[begin:end]
        mime = image.get("mimeType")
        if mime not in ("image/png", "image/jpeg"):
            raise ValueError(f"image {index} uses unsupported MIME type {mime!r}")
        with Image.open(io.BytesIO(original)) as loaded:
            original_size = list(loaded.size)
            image_format = "PNG" if mime == "image/png" else "JPEG"
            normal_only = "normal" in roles.get(index, set())
            blue_zero = normal_only and loaded.convert("RGBA").getextrema()[2] == (0, 0)
            if blue_zero and roles[index] != {"normal"}:
                raise ValueError(f"image {index} with a missing normal Z is also used as {sorted(roles[index] - {'normal'})}; separate normal and nonnormal image/texture references before conversion")
            resized = max(loaded.size) > maximum
            if resized:
                loaded.load()
                loaded.thumbnail((maximum, maximum), Image.Resampling.LANCZOS)
            resized_size = list(loaded.size)
            if blue_zero:
                loaded = reconstruct_normal_z(loaded)
                image_format = "PNG"  # Lossless R/G/alpha preservation, also for JPEG inputs.
                mime = "image/png"
                image["mimeType"] = mime
                reconstructed += 1
            if resized or blue_zero:
                encoded = io.BytesIO()
                if image_format == "JPEG" and loaded.mode not in ("RGB", "L"):
                    loaded = loaded.convert("RGB")
                loaded.save(encoded, format=image_format, **({"quality": 95, "subsampling": 0} if image_format == "JPEG" else {}))
                replacement = encoded.getvalue()
            else:
                replacement = bytes(original)
        if view_index in replacements and replacements[view_index] != replacement:
            raise ValueError("conflicting images share one buffer view")
        replacements[view_index] = replacement
        image_report.append({"index": index, "name": image.get("name", ""), "mime_type": mime,
                             "source_dimensions": original_size, "dimensions": resized_size,
                             "roles": sorted(roles.get(index, set())), "normal_z_reconstructed": bool(blue_zero),
                             "source_sha256": hashlib.sha256(original).hexdigest(),
                             "sha256": hashlib.sha256(replacement).hexdigest()})
        print(json.dumps({"image": index + 1, "images": len(images), "source": original_size,
                          "output": resized_size}), flush=True)
    # No accessor may share the buffer view of an image: preserve every geometry byte exactly.
    for accessor in document.get("accessors", []):
        referenced = [accessor.get("bufferView")]
        sparse = accessor.get("sparse", {})
        referenced.extend(sparse.get(kind, {}).get("bufferView") for kind in ("indices", "values"))
        if any(value in replacements for value in referenced if value is not None):
            raise ValueError("an image buffer view is also referenced by a mesh accessor")
    packed = bytearray()
    geometry_digest = hashlib.sha256()
    for index, view in enumerate(views):
        if view.get("buffer", 0) != 0:
            raise ValueError("external buffer views are unsupported")
        if "EXT_meshopt_compression" in view.get("extensions", {}):
            raise ValueError("compressed buffer views require a separate offset-aware repacker")
        begin = view.get("byteOffset", 0)
        end = begin + view["byteLength"]
        if begin < 0 or end > len(binary):
            raise ValueError(f"buffer view {index} exceeds the BIN chunk")
        while len(packed) % 4:
            packed.append(0)
        payload = replacements.get(index)
        if payload is None:
            payload = binary[begin:end]
            geometry_digest.update(struct.pack("<I", index))
            geometry_digest.update(payload)
        view["byteOffset"] = len(packed)
        view["byteLength"] = len(payload)
        packed.extend(payload)
    while len(packed) % 4:
        packed.append(0)
    document["buffers"][0]["byteLength"] = len(packed)
    json_bytes = json.dumps(document, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    json_bytes += b" " * ((-len(json_bytes)) % 4)
    total = 12 + 8 + len(json_bytes) + 8 + len(packed)
    if total >= 2**32:
        raise ValueError("derived GLB exceeds the version 2 size limit")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + ".pending")
    with temporary.open("wb") as stream:
        stream.write(struct.pack("<III", 0x46546C67, 2, total))
        stream.write(struct.pack("<II", len(json_bytes), 0x4E4F534A))
        stream.write(json_bytes)
        stream.write(struct.pack("<II", len(packed), 0x004E4942))
        stream.write(packed)
    temporary.replace(output)
    receipt = {"format": "amoris-walk-model-v1", "source": str(source), "source_sha256": sha(source),
               "output": str(output), "sha256": sha(output), "source_bytes": source.stat().st_size,
               "bytes": output.stat().st_size, "maximum_texture_dimension": maximum,
               "preserved_nonimage_buffer_views_sha256": geometry_digest.hexdigest(),
               "preserved": "All nonimage buffer-view bytes, accessors, meshes, node transforms, materials, extensions and scene instances.",
               "image_count": len(images), "images": image_report,
               "normal_z_reconstructed_images": reconstructed,
               "normal_z_rule": "Only material normal-usage images with an entirely zero source B channel; +Z=sqrt(max(1-X^2-Y^2,0)); R/G/alpha retained and no green flip.",
               "attribution": "Amazon Lumberyard Bistro, Open Research Content Archive (ORCA), Amazon Lumberyard, July 2017.",
               "source_url": SOURCE_URL, "license": "CC-BY-4.0", "license_url": LICENSE_URL,
               "changes": "Embedded textures resized with Lanczos to the stated maximum dimension; missing positive Z reconstructed only for normal-only RG/B=0 textures, without flipping green; GLB buffer-view offsets and lengths repacked at 4-byte alignment.",
               "seconds": time.perf_counter() - started}
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"output": str(output), "report": str(report), "source_bytes": receipt["source_bytes"],
                      "bytes": receipt["bytes"], "images": len(images), "normal_z_reconstructed_images": reconstructed,
                      "seconds": receipt["seconds"]}), flush=True)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, default=DEFAULT_INPUT)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--max-texture", type=int, default=1024)
    args = parser.parse_args()
    try:
        prepare(args.input, args.output, args.max_texture,
                args.report or args.output.with_suffix(".receipt.json"))
    except (OSError, ValueError, KeyError, IndexError, json.JSONDecodeError) as error:
        parser.exit(1, f"Walk-model preparation failed: {error}\n")


if __name__ == "__main__":
    main()
