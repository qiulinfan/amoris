#!/usr/bin/env python3
"""Reads and copies AMD's Vulkan pipeline archives (`.parc`, PAL archive files) for
docs/bench/gpu-matrix.md 9. It never writes to `%LOCALAPPDATA%/AMD/VkCache`: copy archives out of it
first, and point the driver at a copy with `AMD_VK_PIPELINE_CACHE_PATH`.

    python tools/amd_vkcache.py list DIR_OR_FILES...     entries: file, ordinal, key, sizes, kind,
                                                         payload digest
    python tools/amd_vkcache.py diff A_DIR B_DIR         keys in both with different payloads (a
                                                         non-ELF payload's bytes 32 to 40, which
                                                         differ in every run, masked), and keys
                                                         in one only
    python tools/amd_vkcache.py dump DIR KEY OUT         one entry's decompressed payload
    python tools/amd_vkcache.py seed SRC_DIR DST_DIR PREFIX [KEY_PREFIX...]
        copies SRC_DIR's archives into a new DST_DIR under another executable path's prefix, the
        entries whose keys start with a KEY_PREFIX made to miss (one key byte flipped)

An archive's file name is 16 hex digits that follow the executable's path, then 16 that were the
same in every run here (`9113209E63BE76D3`), then `_N` for further files: run a binary once with
`AMD_VK_PIPELINE_CACHE_PATH` at an empty directory to learn its path's prefix. Layout as observed
(2026-10-16, driver 24.30.62.03): a 0x34-byte header (16-byte marker, version 1.3, first entry
0x34, a 20-byte platform key, a 4-byte check), then entries chained by offset: "NTRY", ordinal,
next entry, data size, data position, two check words, 4 zero bytes, a 16-byte key, 4 bytes, the
uncompressed size; the data is 20 bytes of digest, "4ZLP", the uncompressed size and one LZ4 block.
Most payloads are AMDGPU ELF pipeline binaries with their PAL metadata.
"""
import hashlib
import re
import shutil
import struct
import sys
from pathlib import Path

SUFFIX = re.compile(r"^[0-9A-F]{16}([0-9A-F]{16}(?:_\d+)?\.parc)$")


def lz4_block(src, size):
    """One LZ4 block, decompressed to at most `size` bytes."""
    out = bytearray()
    i = 0
    while i < len(src):
        token = src[i]
        i += 1
        lit = token >> 4
        if lit == 15:
            while True:
                b = src[i]
                i += 1
                lit += b
                if b != 255:
                    break
        out += src[i:i + lit]
        i += lit
        if i >= len(src) or len(out) >= size:
            break
        off = src[i] | (src[i + 1] << 8)
        i += 2
        ml = token & 15
        if ml == 15:
            while True:
                b = src[i]
                i += 1
                ml += b
                if b != 255:
                    break
        ml += 4
        start = len(out) - off
        for k in range(ml):
            out.append(out[start + k])
    return bytes(out[:size])


def offsets(d):
    """The byte offsets of an archive's entries, in chain order."""
    o = struct.unpack_from("<I", d, 0x18)[0]
    while 0 < o < len(d) and d[o:o + 4] == b"NTRY":
        yield o
        nxt = struct.unpack_from("<I", d, o + 8)[0]
        if nxt <= o:
            break
        o = nxt


def entries(path):
    d = Path(path).read_bytes()
    for o in offsets(d):
        _, ordinal, _, size, _, _, _ = struct.unpack_from("<4s6I", d, o)
        key = d[o + 0x20:o + 0x30].hex()
        usize = struct.unpack_from("<I", d, o + 0x34)[0]
        data = d[o + 0x38:o + 0x38 + size]
        payload = None
        if data[20:24] == b"4ZLP":
            payload = lz4_block(data[28:], struct.unpack_from("<I", data, 24)[0])
        stable = payload if payload is None or payload[:4] == b"\x7fELF" else (
            payload[:32] + bytes(8) + payload[40:])
        yield {"file": Path(path).name, "ordinal": ordinal, "key": key, "usize": usize,
               "payload": payload, "digest": hashlib.sha256(payload or data).hexdigest()[:16],
               "stable": hashlib.sha256(stable or data).hexdigest()[:16]}


def collect(paths):
    """{key: [entries]} of the archives in the given directories or files."""
    files = []
    for p in map(Path, paths):
        files += sorted(p.glob("*.parc")) if p.is_dir() else [p]
    out = {}
    for f in files:
        for e in entries(f):
            out.setdefault(e["key"], []).append(e)
    return out


def kind(payload):
    if payload is None:
        return "?"
    if payload[:4] == b"\x7fELF":
        return "elf"
    return payload[:4].hex()


def seed(src, dst, prefix, misses):
    """Copies SRC's archives to the new directory DST renamed to `prefix`, flipping the last key
    byte of every entry whose key starts with one of `misses`."""
    if not re.fullmatch(r"[0-9A-F]{16}", prefix):
        raise SystemExit(f"prefix {prefix}: expected 16 upper-case hex digits")
    dst = Path(dst)
    if dst.exists() and any(dst.iterdir()):
        raise SystemExit(f"{dst} is not empty")
    dst.mkdir(parents=True, exist_ok=True)
    changed = 0
    for f in sorted(Path(src).glob("*.parc")):
        m = SUFFIX.match(f.name)
        if not m:
            raise SystemExit(f"{f.name}: not an archive name")
        d = bytearray(f.read_bytes())
        for o in offsets(d):
            key = bytes(d[o + 0x20:o + 0x30]).hex()
            if any(key.startswith(k) for k in misses):
                d[o + 0x2f] ^= 0xFF
                changed += 1
                print(f"{f.name}: entry {key} made to miss")
        (dst / (prefix + m.group(1))).write_bytes(bytes(d))
    print(f"{dst}: {len(list(dst.iterdir()))} archives, {changed} entries made to miss")


def main():
    cmd, rest = sys.argv[1], sys.argv[2:]
    if cmd == "list":
        es = collect(rest)
        for k, v in sorted(es.items(), key=lambda kv: (kv[1][0]["file"], kv[1][0]["ordinal"])):
            for e in v:
                print(e["file"], e["ordinal"], k, e["usize"], kind(e["payload"]), e["digest"])
        print(len(es), "keys")
    elif cmd == "diff":
        a, b = collect([rest[0]]), collect([rest[1]])
        both = sorted(set(a) & set(b))
        differ = [k for k in both if {e["stable"] for e in a[k]} != {e["stable"] for e in b[k]}]
        print(f"A {len(a)} keys, B {len(b)} keys, both {len(both)}, differing {len(differ)}, "
              f"only A {len(set(a) - set(b))}, only B {len(set(b) - set(a))}")
        def show(es):
            return [(e["file"], e["ordinal"], kind(e["payload"]), e["digest"]) for e in es]
        for k in differ:
            print("differ", k, show(a[k]), show(b[k]))
    elif cmd == "dump":
        e = collect([rest[0]])[rest[1]][-1]
        Path(rest[2]).write_bytes(e["payload"])
        print(rest[2], len(e["payload"]), kind(e["payload"]))
    elif cmd == "seed":
        seed(rest[0], rest[1], rest[2], rest[3:])
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
