"""tools/amd_vkcache.py on archives built here in the layout it reads: no driver or GPU is used.

    python -m unittest tools/tests/test_amd_vkcache.py
"""
import importlib.util
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("amd_vkcache", TOOLS / "amd_vkcache.py")
amd_vkcache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(amd_vkcache)

ENDPGM, CODE_END, NOP = 0xBFB00000, 0xBF9F0000, 0xBF800000


def elf(note, words):
    """An ELF64 with a `.text` of `words` and a `.note` of `note` (sections as the driver's)."""
    text = struct.pack(f"<{len(words)}I", *words)
    names = b"\0.text\0.note\0.shstrtab\0"
    body = bytearray(64)
    offsets = []
    for data in (text, note, names):
        offsets.append(len(body))
        body += data
    shoff = len(body)
    heads = [(0, 0, 0, 0)]
    for name, off, data in ((1, offsets[0], text), (7, offsets[1], note),
                            (13, offsets[2], names)):
        heads.append((name, 1, off, len(data)))
    for name, kind, off, size in heads:
        body += struct.pack("<IIQQQQIIQQ", name, kind, 0, 0, off, size, 0, 0, 1, 0)
    body[0:4] = b"\x7fELF"
    struct.pack_into("<Q", body, 0x28, shoff)
    struct.pack_into("<HHH", body, 0x3A, 64, len(heads), 3)
    return bytes(body)


def lz4_literals(b):
    """One LZ4 block holding `b` as literals only."""
    if len(b) < 15:
        return bytes([len(b) << 4]) + b
    out, rest = bytearray([0xF0]), len(b) - 15
    while rest >= 255:
        out.append(255)
        rest -= 255
    out.append(rest)
    return bytes(out) + b


def archive(path, entries):
    """Writes a PAL archive file of (key bytes, payload) entries in the chained layout
    amd_vkcache.py reads (its module doc)."""
    d = bytearray(0x34)
    struct.pack_into("<I", d, 0x18, 0x34)
    for i, (key, payload) in enumerate(entries):
        data = bytes(20) + b"4ZLP" + struct.pack("<I", len(payload)) + lz4_literals(payload)
        o = len(d)
        nxt = o + 0x38 + len(data) if i + 1 < len(entries) else 0
        entry = bytearray(0x38)
        struct.pack_into("<4s6I", entry, 0, b"NTRY", i, nxt, len(data), o + 0x38, 0, 0)
        entry[0x20:0x30] = key
        struct.pack_into("<I", entry, 0x34, len(payload))
        d += entry + data
    Path(path).write_bytes(bytes(d))


def note(**fields):
    """PAL metadata as far as amd_vkcache.py reads it: msgpack strings, each followed by its
    value (a fixint, a boolean or a short string)."""
    out = bytearray(b"\x80")
    for k, v in fields.items():
        key = ("." + k).encode()
        out += bytes([0xA0 | len(key)]) + key
        if isinstance(v, bool):
            out.append(0xC3 if v else 0xC2)
        elif isinstance(v, int):
            out.append(v)
        else:
            out += bytes([0xA0 | len(v)]) + v.encode()
    return bytes(out)


# The skinning pipeline's metadata (2 user SGPRs, the id's x and y, the group size; section 12),
# and the start of its clean and its wrong program: the clean one takes the id's x and y from s2
# and s3, the wrong one from s5 and s6 after `s_getpc_b64 s[2:3]` has overwritten s2 and s3.
SKINNING = note(type="Cs", user_sgprs=2, tgid_x_en=True, tgid_y_en=True, tgid_z_en=False,
                tg_size_en=True, scratch_en=False)
RIGHT = [0xBE844700, 0xBE800001, 0xBE810005, 0x9603FF03, 0x0000FFFF, 0x81020203, ENDPGM]
WRONG = [0xBE824700, 0xBE800001, 0xBE810003, 0x9602FF06, 0x0000FFFF, 0x81020502, ENDPGM]


class UnsetTests(unittest.TestCase):
    def test_a_program_that_reads_registers_the_hardware_does_not_set_is_found(self):
        self.assertEqual(amd_vkcache.system_sgprs(SKINNING), 5)
        self.assertEqual(amd_vkcache.reads_before_writes(struct.pack("<7I", *RIGHT)), [1, 3, 2])
        self.assertEqual(amd_vkcache.reads_before_writes(struct.pack("<7I", *WRONG)), [1, 6, 5])
        with tempfile.TemporaryDirectory() as d:
            archive(Path(d) / "S.parc", [(bytes(16), elf(SKINNING, RIGHT))])
            self.assertEqual(amd_vkcache.unset([d]), (1, {}))
            # Alone, as one device's archive holds it: no clean twin is needed.
            archive(Path(d) / "S_1.parc", [(b"\1" * 16, elf(SKINNING, WRONG))])
            count, found = amd_vkcache.unset([d])
        self.assertEqual(count, 2)
        self.assertEqual(found, {("S_1.parc", 0, "01" * 16): (5, [6, 5])})

    def test_graphics_pipelines_and_the_registers_past_a_branch_are_left_out(self):
        graphics = note(type="Ngg", user_sgprs=2)
        late = [0x7E000280, 0xBFA00001, 0x9602FF06, 0x0000FFFF, ENDPGM]  # s_branch, then s6
        with tempfile.TemporaryDirectory() as d:
            archive(Path(d) / "G.parc", [(bytes(16), elf(graphics, WRONG)),
                                         (b"\1" * 16, elf(SKINNING, late))])
            self.assertEqual(amd_vkcache.unset([d]), (1, {}))


class TwinsTests(unittest.TestCase):
    def test_a_pipeline_compiled_to_two_programs_is_found_under_any_key(self):
        note = b"pipeline A: 2 user SGPRs"
        right = [0xBE844700, 0x9603FF03, 0x0000FFFF, ENDPGM, CODE_END, CODE_END]
        wrong = [0xBE824700, 0x9602FF06, 0x0000FFFF, ENDPGM, CODE_END, CODE_END]
        with tempfile.TemporaryDirectory() as d:
            archive(Path(d) / "A.parc", [(bytes(16), elf(note, wrong)),
                                         (b"\1" * 16, elf(b"pipeline B", [ENDPGM]))])
            archive(Path(d) / "A_1.parc", [(b"\2" * 16, elf(note, right))])
            count, found = amd_vkcache.twins([d])
        self.assertEqual(count, 2)
        self.assertEqual(len(found), 1)
        (codes,) = found.values()
        self.assertEqual(sorted(len(w) for w in codes.values()), [1, 1])
        where = sorted(w[0][0] for w in codes.values())
        self.assertEqual(where, ["A.parc", "A_1.parc"])

    def test_padding_after_the_end_of_a_program_is_not_a_second_program(self):
        note = b"pipeline C"
        a = [0x7E000280, ENDPGM, CODE_END, CODE_END, 0x7E020280, ENDPGM]
        b = [0x7E000280, ENDPGM, NOP, 0x7E020280, ENDPGM, CODE_END, CODE_END, CODE_END]
        with tempfile.TemporaryDirectory() as d:
            archive(Path(d) / "C.parc", [(bytes(16), elf(note, a)), (b"\1" * 16, elf(note, b))])
            self.assertEqual(amd_vkcache.twins([d]), (1, {}))
        # Padding within a program (a hazard's s_nop before the end) is code.
        self.assertNotEqual(amd_vkcache.code(struct.pack("<3I", 0x7E000280, NOP, ENDPGM)),
                            amd_vkcache.code(struct.pack("<2I", 0x7E000280, ENDPGM)))


class ExitStatusTests(unittest.TestCase):
    def test_a_finding_and_an_archive_that_cannot_be_read_exit_differently(self):
        def cli(*args):
            return subprocess.run([sys.executable, str(TOOLS / "amd_vkcache.py"), *args],
                                  capture_output=True, text=True)

        with tempfile.TemporaryDirectory() as d:
            clean, wrong, cut = (Path(d) / n for n in ("clean", "wrong", "cut"))
            for p in (clean, wrong, cut):
                p.mkdir()
            archive(clean / "S.parc", [(bytes(16), elf(SKINNING, RIGHT))])
            archive(wrong / "S.parc", [(bytes(16), elf(SKINNING, WRONG))])
            (cut / "S.parc").write_bytes(b"PAL")  # a process that died while writing it
            for cmd in ("unset", "twins"):
                self.assertEqual(cli(cmd, str(clean)).returncode, 0, cmd)
                for bad in (cut, Path(d) / "missing"):
                    r = cli(cmd, str(bad))
                    self.assertEqual(r.returncode, 2, (cmd, bad, r.stderr))
                    self.assertEqual(len(r.stderr.strip().splitlines()), 1, r.stderr)
                    self.assertEqual(r.stdout, "")
            self.assertEqual(cli("unset", str(wrong)).returncode, 1)
            self.assertEqual(cli("twins", str(clean), str(wrong)).returncode, 1)
            self.assertEqual(cli("seed", str(clean), str(Path(d) / "x"), "bad").returncode, 2)


if __name__ == "__main__":
    unittest.main()
