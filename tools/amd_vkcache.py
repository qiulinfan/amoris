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
    python tools/amd_vkcache.py unset DIR_OR_FILES...    compute programs whose start reads SGPRs
                                                         past those the hardware sets for them
                                                         (user SGPRs, workgroup id, group size,
                                                         by the PAL metadata); exit status 1 when
                                                         there are any
    python tools/amd_vkcache.py twins DIR_OR_FILES...    pipelines compiled to more than one
                                                         program: ELF entries with the same PAL
                                                         metadata (`.note`) and a `.text` that
                                                         differs other than in padding, under any
                                                         keys; exit status 1 when there are any

Status 1 is kept for what `unset` and `twins` find: a missing path, an archive that cannot be read
(a process that died while writing it) or a wrong argument ends any command with status 2 and one
line on stderr, so a script never counts it as a finding.

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


def elf_sections(b):
    """An ELF64 payload's sections by name."""
    shoff = struct.unpack_from("<Q", b, 0x28)[0]
    size, count, names = struct.unpack_from("<HHH", b, 0x3A)
    heads = [struct.unpack_from("<IIQQQQIIQQ", b, shoff + i * size) for i in range(count)]
    base = heads[names][4]
    return {b[base + h[0]:b.index(0, base + h[0])].decode(): b[h[4]:h[4] + h[5]] for h in heads}


# s_endpgm, and the s_code_end and s_nop words a compiler pads with after it (gfx11).
ENDPGM, PADDING = 0xBFB00000, (0xBF9F0000, 0xBF800000)


def code(text):
    """A `.text` section's words without the padding after each s_endpgm, which differs between
    compiles of one pipeline (the layout of a graphics pipeline's stages) where the code does
    not."""
    out, pad = [], False
    for (w,) in struct.iter_unpack("<I", text[:len(text) // 4 * 4]):
        if pad and w in PADDING:
            continue
        pad = w == ENDPGM
        out.append(w)
    return struct.pack(f"<{len(out)}I", *out)


def twins(paths):
    """(pipelines, {metadata digest: {code digest: [(file, ordinal, key)]}}) of the pipelines whose
    ELF entries have the same PAL metadata (the `.note` section: the pipeline's hashes, registers,
    user SGPRs) and more than one code ([`code`]): one pipeline compiled to two programs
    (docs/bench/gpu-matrix.md 12). Keys are left out of it: one pipeline sits under different keys
    (section 9)."""
    groups = {}
    for k, es in collect(paths).items():
        for e in es:
            if kind(e["payload"]) != "elf":
                continue
            s = elf_sections(e["payload"])
            if ".note" not in s or ".text" not in s:
                continue
            note = hashlib.sha256(s[".note"]).hexdigest()[:16]
            program = hashlib.sha256(code(s[".text"])).hexdigest()[:16]
            groups.setdefault(note, {}).setdefault(program, []).append((e["file"], e["ordinal"], k))
    return len(groups), {n: c for n, c in groups.items() if len(c) > 1}


def meta(note, key):
    """A small value of the PAL metadata (msgpack in the `.note`): the fixint, boolean or short
    string after the string `key`, its first occurrence; None when absent."""
    k = key.encode()
    i = note.find(bytes([0xA0 | len(k)]) + k)
    if i < 0 or i + 1 + len(k) >= len(note):
        return None
    v = note[i + 1 + len(k)]
    if v <= 0x7F:
        return v
    if v in (0xC2, 0xC3):
        return v == 0xC3
    if 0xA0 <= v <= 0xBF:
        return note[i + 2 + len(k):i + 2 + len(k) + (v & 0x1F)].decode("ascii", "replace")
    return None


def system_sgprs(note):
    """How many SGPRs the hardware sets when it starts a wave of a compute pipeline: its user SGPRs,
    then the enabled x, y and z of the workgroup id, the group size and the scratch offset, in that
    order. None for other pipelines."""
    if meta(note, ".type") != "Cs" or meta(note, ".user_sgprs") is None:
        return None
    enables = (".tgid_x_en", ".tgid_y_en", ".tgid_z_en", ".tg_size_en", ".scratch_en")
    return meta(note, ".user_sgprs") + sum(1 for e in enables if meta(note, e))


# Operands that name a literal constant or a DPP word after the instruction.
EXTRA = (255, 250, 233, 234)


def walk(words):
    """A gfx11 program's instructions as (encoding, words), told apart by the encodings' fixed bits
    and lengths (a trailing literal included). Enough for the scalar start of a compute program;
    not a disassembler."""
    i = 0
    while i < len(words):
        w = words[i]
        nxt = words[i + 1] if i + 1 < len(words) else 0
        if w >> 23 == 0b101111101:
            c, n, extra = "SOP1", 1, (w & 0xFF) == 255
        elif w >> 23 == 0b101111110:
            c, n, extra = "SOPC", 1, 255 in (w & 0xFF, (w >> 8) & 0xFF)
        elif w >> 23 == 0b101111111:
            c, n, extra = "SOPP", 1, False
        elif w >> 28 == 0b1011:
            c, n, extra = "SOPK", 1, (w >> 23) & 0x1F == 19  # s_setreg_imm32_b32
        elif w >> 30 == 0b10:
            c, n, extra = "SOP2", 1, 255 in (w & 0xFF, (w >> 8) & 0xFF)
        elif w >> 26 == 0b111101:
            c, n, extra = "SMEM", 2, False
        elif w >> 25 == 0b0111111:
            c, n, extra = "VOP1", 1, (w & 0x1FF) in EXTRA
        elif w >> 25 == 0b0111110:
            c, n, extra = "VOPC", 1, (w & 0x1FF) in EXTRA
        elif w >> 31 == 0:
            # v_fmamk and v_fmaak (f32 and f16) carry a literal of their own.
            c, n = "VOP2", 1
            extra = (w & 0x1FF) in EXTRA or (w >> 25) & 0x3F in (0x2C, 0x2D, 0x37, 0x38)
        elif w >> 26 == 0b110101 or w >> 24 == 0b11001100:
            c, n = "VOP3", 2
            extra = any((nxt >> s) & 0x1FF in EXTRA for s in (0, 9, 18))
        elif w >> 26 == 0b110010:
            c, n = "VOPD", 2
            extra = (w & 0x1FF) == 255 or (nxt & 0x1FF) == 255 or (w >> 22) & 0xF in (1, 2)
        elif w >> 26 == 0b111000:
            c, n, extra = "MUBUF", 2, False
        else:
            c, n, extra = "other", 1 if w >> 24 == 0b11001110 else 2, False
        n += extra
        yield c, words[i:i + n]
        i += n


# SMEM loads' sizes in dwords by opcode (s_load_b32 to _b512, s_buffer_load_b32 to _b512).
SMEM_DWORDS = {0: 1, 1: 2, 2: 4, 3: 8, 4: 16, 8: 1, 9: 2, 10: 4, 11: 8, 12: 16}
# SOP1 opcodes with 64-bit operands among those that start a program: s_mov_b64, s_getpc_b64.
SOP1_PAIRS, GETPC = {1, 71}, 71


def reads_before_writes(text):
    """The SGPRs (s0 to s105) a gfx11 program reads before it writes them, in order, over its
    straight-line start: up to its first branch or s_endpgm. Approximate: scalar, vector and buffer
    operands; a VOP3 destination counts as written (v_cmp, v_readlane and carries write SGPRs
    there). Enough to see where a compute program takes the values the hardware set."""
    words = [w for (w,) in struct.iter_unpack("<I", text[:len(text) // 4 * 4])]
    written, read = set(), []

    def use(r, n=1):
        for k in range(r, r + n):
            if k < 106 and k not in written and k not in read:
                read.append(k)

    def put(r, n=1):
        written.update(range(r, r + n))

    for c, ws in walk(words):
        w = ws[0]
        if c == "SOPP" and (32 <= (w >> 16) & 0x7F <= 42 or w == ENDPGM):
            break
        if c == "SOP1":
            op = (w >> 8) & 0xFF
            pair = 2 if op in SOP1_PAIRS else 1
            if op != GETPC:
                use(w & 0xFF, pair)
            put((w >> 16) & 0x7F, pair)
        elif c in ("SOP2", "SOPC"):
            use(w & 0xFF)
            use((w >> 8) & 0xFF)
            if c == "SOP2":
                put((w >> 16) & 0x7F)
        elif c == "SOPK":
            put((w >> 16) & 0x7F)
        elif c == "SMEM":
            op = (w >> 18) & 0xFF
            use((w & 0x3F) * 2, 2 if op < 8 else 4)
            put((w >> 6) & 0x7F, SMEM_DWORDS.get(op, 1))
        elif c in ("VOP1", "VOP2", "VOPC"):
            use(w & 0x1FF)
            if c == "VOP1" and (w >> 9) & 0xFF == 2:  # v_readfirstlane_b32 writes an SGPR
                put((w >> 17) & 0xFF)
        elif c == "VOP3":
            for shift in (0, 9, 18):
                use((ws[1] >> shift) & 0x1FF)
            put(w & 0xFF)
            put((w >> 8) & 0x7F)
        elif c == "MUBUF":  # its descriptor and offset SGPRs
            use(((ws[1] >> 16) & 0x1F) * 4, 4)
            use((ws[1] >> 24) & 0xFF)
    return read


def unset(paths):
    """(compute programs, {(file, ordinal, key): (SGPRs set, the SGPRs past them read)}) of the
    compute programs whose start reads SGPRs past those the hardware sets for them
    ([`system_sgprs`]): programs compiled for another user-SGPR count than their metadata gives,
    the AMD Vulkan driver's wrong compiles of docs/bench/gpu-matrix.md 12."""
    count, found = 0, {}
    for k, es in collect(paths).items():
        for e in es:
            if kind(e["payload"]) != "elf":
                continue
            s = elf_sections(e["payload"])
            n = system_sgprs(s.get(".note", b""))
            if n is None or ".text" not in s:
                continue
            count += 1
            past = [r for r in reads_before_writes(s[".text"]) if r >= n]
            if past:
                found[(e["file"], e["ordinal"], k)] = (n, past)
    return count, found


def seed(src, dst, prefix, misses):
    """Copies SRC's archives to the new directory DST renamed to `prefix`, flipping the last key
    byte of every entry whose key starts with one of `misses`."""
    if not re.fullmatch(r"[0-9A-F]{16}", prefix):
        raise ValueError(f"prefix {prefix}: expected 16 upper-case hex digits")
    dst = Path(dst)
    if dst.exists() and any(dst.iterdir()):
        raise ValueError(f"{dst} is not empty")
    dst.mkdir(parents=True, exist_ok=True)
    changed = 0
    for f in sorted(Path(src).glob("*.parc")):
        m = SUFFIX.match(f.name)
        if not m:
            raise ValueError(f"{f.name}: not an archive name")
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
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    try:
        run(sys.argv[1], sys.argv[2:])
    except (OSError, ValueError, IndexError, KeyError, struct.error) as e:
        print(f"amd_vkcache.py {sys.argv[1]}: {type(e).__name__}: {e}", file=sys.stderr)
        sys.exit(2)


def run(cmd, rest):
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
    elif cmd == "unset":
        count, found = unset(rest)
        for (f, o, k), (n, past) in sorted(found.items()):
            print(f"{f} {o} {k}: sets s0 to s{n - 1}, reads", ", ".join(f"s{r}" for r in past))
        print(f"{count} compute programs, {len(found)} read SGPRs the hardware does not set")
        sys.exit(1 if found else 0)
    elif cmd == "twins":
        count, found = twins(rest)
        for note, codes in sorted(found.items()):
            print("pipeline", note, "compiled to", len(codes), "programs:")
            for program, where in sorted(codes.items()):
                print("  program", program, "in", ", ".join(f"{f} {o} {k}" for f, o, k in where))
        print(f"{count} pipelines, {len(found)} compiled to more than one program")
        sys.exit(1 if found else 0)
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
