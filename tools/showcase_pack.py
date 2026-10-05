#!/usr/bin/env python3
"""Pack ordinary local glTF buffers and PNG/JPEG images into GLB without re-exporting geometry."""
import argparse
import base64
import json
from pathlib import Path
import struct
from urllib.parse import unquote


def pack(source: Path, output: Path):
    source = source.resolve()
    output = output.resolve()
    if source == output:
        raise ValueError("Preserve the source asset")
    document = json.loads(source.read_text())
    binary = bytearray()

    def resource(uri):
        if uri.startswith("data:"):
            header, value = uri.split(",", 1)
            if not header.endswith(";base64"):
                raise ValueError("Only base64 data URIs supported")
            return base64.b64decode(value, validate=True)
        path = (source.parent / unquote(uri)).resolve()
        if not path.is_relative_to(source.parent) or not path.is_file():
            raise ValueError(f"Missing or nonlocal resource: {uri}")
        return path.read_bytes()

    def append(data):
        binary.extend(b"\0" * (-len(binary) % 4))
        offset = len(binary)
        binary.extend(data)
        return offset

    offsets = []
    for buffer in document.get("buffers", []):
        data = resource(buffer["uri"])
        if len(data) < buffer["byteLength"]:
            raise ValueError("Truncated buffer")
        offsets.append(append(data[:buffer["byteLength"]]))
    for view in document.get("bufferViews", []):
        view["byteOffset"] = offsets[view["buffer"]] + view.get("byteOffset", 0)
        view["buffer"] = 0
    for image in document.get("images", []):
        if "uri" not in image:
            continue
        data = resource(image.pop("uri"))
        if data.startswith(b"\x89PNG\r\n\x1a\n"):
            image["mimeType"] = "image/png"
        elif data.startswith(b"\xff\xd8"):
            image["mimeType"] = "image/jpeg"
        else:
            raise ValueError("Only PNG and JPEG supported by the showcase renderer")
        offset = append(data)
        image["bufferView"] = len(document.setdefault("bufferViews", []))
        document["bufferViews"].append({"buffer": 0, "byteOffset": offset, "byteLength": len(data)})
    document["buffers"] = [{"byteLength": len(binary)}]
    js = json.dumps(document, ensure_ascii=False, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    binary.extend(b"\0" * (-len(binary) % 4))
    size = 12 + 8 + len(js) + 8 + len(binary)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as f:
        f.write(struct.pack("<III", 0x46546C67, 2, size))
        f.write(struct.pack("<II", len(js), 0x4E4F534A))
        f.write(js)
        f.write(struct.pack("<II", len(binary), 0x004E4942))
        f.write(binary)
    return {"source": str(source), "output": str(output), "bytes": size,
            "meshes": len(document.get("meshes", [])), "materials": len(document.get("materials", []))}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(json.dumps(pack(args.source, args.output)))
