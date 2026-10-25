"""The rename rules of tools/amoris_rename.py (charter 5.1): every compound form, every protection,
binary and non-UTF-8 files, path mapping, collisions and idempotency. No checkout is changed."""
import contextlib
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("amoris_rename", TOOLS / "amoris_rename.py")
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)

OLD = "po" + "cket"  # spelled apart so this file's own text reads the same before and after
OLD_C = "Po" + "cket"
OLD_U = "PO" + "CKET"


def ren(text, rel="src/lib.rs"):
    return r.rename_text(text, rel)[0]


def quiet(f, *args):
    with contextlib.redirect_stdout(io.StringIO()):
        return f(*args)


class Forms(unittest.TestCase):
    def test_every_compound_form(self):
        cases = {
            f"{OLD_U}_BACKEND": "AMORIS_BACKEND",
            f"VITE_{OLD_U}_EXPOSE": "VITE_AMORIS_EXPOSE",
            f"PURE_{OLD_U}": "PURE_AMORIS",
            f"const {OLD_U} = 1": "const AMORIS = 1",
            f"{OLD}_sim::World": "amoris_sim::World",
            f"crates/{OLD}-sim": "crates/amoris-sim",
            f"{OLD_C}Mcp": "AmorisMcp",
            f"class {OLD_C} {{": "class Amoris {",
            f"__{OLD}_harden(__{OLD}EntityPart)": "__amoris_harden(__amorisEntityPart)",
            f"window.{OLD}Samples": "window.amorisSamples",
            f'import {{ x }} from "{OLD}";': 'import { x } from "amoris";',
            f"{OLD}:host": "amoris:host",
            f"{OLD}:///scripts/main.ts": "amoris:///scripts/main.ts",
            f"{OLD}-game": "amoris-game",
            f"~/.{OLD}-tools/llvm-23.1.2": "~/.amoris-tools/llvm-23.1.2",
            f"<p>/.{OLD}/types/{OLD}.d.ts": "<p>/.amoris/types/amoris.d.ts",
            f"{OLD}_viewport_bg.wasm": "amoris_viewport_bg.wasm",
            f"[{OLD} ERROR] {OLD} panicked": "[amoris ERROR] amoris panicked",
            f"application/x-{OLD}-asset": "application/x-amoris-asset",
            f'"{OLD}-scene" "{OLD}-web-package"': '"amoris-scene" "amoris-web-package"',
            f"cfg({OLD}_qjs_msvc) {OLD}_compiler": "cfg(amoris_qjs_msvc) amoris_compiler",
            f"{OLD_C}JoltBench {OLD}-splatfit": "AmorisJoltBench amoris-splatfit",
            f"crates\\\\{OLD}-render\\\\tests": "crates\\\\amoris-render\\\\tests",
        }
        for old, new in cases.items():
            with self.subTest(old=old):
                self.assertEqual(ren(old), new)

    def test_letter_or_digit_preceded_forms_stay(self):
        text = f"ai{OLD} and ai{OLD}2 and x{OLD} and 9{OLD}"
        self.assertEqual(ren(text), text)

    def test_composites_before_the_generic_rule(self):
        self.assertEqual(ren(f"AMORIS_{OLD_U}_BINARY"), "AMORIS_BINARY")
        self.assertEqual(ren(f"AMORIS_TEST_{OLD_U}=1"), "AMORIS_TEST_BINARY=1")
        _, counts = r.rename_text(f"AMORIS_{OLD_U}_BINARY {OLD_U}_X", "a.cjs")
        self.assertEqual((counts.composite, counts.generic), (1, 1))

    def test_pce_wording_across_a_line_break(self):
        self.assertEqual(ren(f"{OLD_C} Canonical Encoding"), "Portable Canonical Encoding")
        self.assertEqual(ren(f"the {OLD_C}\nCanonical  Encoding (PCE)", "x.md"),
                         "the Portable\nCanonical  Encoding (PCE)")

    def test_the_article_before_the_name(self):
        cases = {
            f"for a `{OLD}` export": "for an `amoris` export",
            f"(a {OLD}-runtime example": "(an amoris-runtime example",
            f"A {OLD_U}_TSC names it": "An AMORIS_TSC names it",
            f"through a\n`{OLD}:host` native": "through an\n`amoris:host` native",
            f"x: &'a {OLD}_assets::Mesh": "x: &'a amoris_assets::Mesh",  # a lifetime
            f"data {OLD}, a {OLD_C}Engine": f"data amoris, a {OLD_C}Engine",  # G1
            f"a `{OLD}` to `amoris` rename": f"a `{OLD}` to `amoris` rename",  # protected (G5)
        }
        for old, new in cases.items():
            with self.subTest(old=old):
                self.assertEqual(ren(old, "x.md"), new)
        _, counts = r.rename_text(f"a `{OLD}` and a {OLD}", "x.md")
        self.assertEqual((counts.article, counts.generic), (2, 2))

    def test_an_unknown_casing_is_refused(self):
        with self.assertRaises(ValueError):
            r.rename_text("POCKet", "a.rs")

    def test_a_nul_is_refused(self):
        with self.assertRaises(ValueError):
            r.rename_text(f"{OLD}\0", "a.rs")


class Protections(unittest.TestCase):
    def kept(self, text, rel):
        out, counts = r.rename_text(text, rel)
        self.assertEqual(out, text, rel)
        return counts

    def test_global(self):
        self.kept(f"{OLD_C}Engine", "a.rs")
        self.kept(f"{OLD_C}Error", "a.md")
        self.kept(f"https://github.com/o/r/blob/9a3e4b08/crates/{OLD}-render/src/lod.rs#L1", "a.md")
        self.kept(f"https://github.com/o/r/tree/9a3e4b0814959629/crates/{OLD}-sim", "a.md")
        self.kept(f'"Amoris 2026-10-15 {OLD}-physics fixed shape v1"', "src/shapes.rs")
        self.kept(f"`{OLD}` 改为 `amoris`，`{OLD_C}`\n改为 `Amoris`", "docs/charter.md")
        self.kept(f"(`{OLD}` to `amoris`, charter 5.1)", "AGENTS.md")
        self.kept(f"`{OLD_U}_X` -> `AMORIS_X`", "a.md")
        self.kept(f"`git show 3fdc5b83:crates/{OLD}-render/src/trace.rs`", "a.md")
        self.kept(f"git diff 9a3e4b0814959629:tools/{OLD}_x.py", "a.py")

    def test_a_path_after_a_colon_that_is_no_object_name_is_renamed(self):
        self.assertEqual(ren(f"at 17:30:crates/{OLD}-sim and src:crates/{OLD}-sim", "a.md"),
                         "at 17:30:crates/amoris-sim and src:crates/amoris-sim")

    def test_an_unpinned_link_is_renamed(self):
        self.assertEqual(ren(f"https://github.com/o/r/blob/master/crates/{OLD}-sim", "a.md"),
                         "https://github.com/o/r/blob/master/crates/amoris-sim")

    def test_markdown_only(self):
        md = [f"{OLD_C} UI", f"master 的 `{OLD}` 工具", f"`{OLD}` 构建工具", f"include/{OLD}/rng.h",
              f"{OLD}::repro", f"Master's suite (`{OLD} test`)", f"`{OLD}_scenario`",
              f"tools/{OLD}/src/main.cpp", f"{OLD_U}_EVAL_RUNTIME", f"at f ({OLD}:53:23)",
              f"`{OLD} panicked: ... dlmalloc`", f'[Device "{OLD}"]',
              f"out/serveloop/bin/{OLD}-base-6c0b.exe", f"`{OLD}-new-wave2.exe`",
              f"`{OLD}-version.json`"]
        for text in md:
            with self.subTest(text=text):
                self.kept(text, "docs/x.md")
                self.assertNotEqual(ren(text, "src/x.rs"), text)

    def test_scoped(self):
        tools = f"~/.{OLD}-tools/llvm-23.1.2"
        self.kept(tools, "docs/spikes/render.md")
        self.kept(tools, "docs/bench/web.md")
        self.assertEqual(ren(tools, "docs/spec/checks.md"), "~/.amoris-tools/llvm-23.1.2")
        line = f'APPLY = re.compile(r"(?:{OLD}|amoris)\\s+scripts")'
        self.kept(line, "tools/eval/debug_eval.py")
        self.kept(f'name.startswith(("{OLD}_", "amoris_"))', "tools/eval/debug_eval.py")
        self.kept(f'name in ("{OLD}_scripts", "amoris_scripts")', "tools/eval/debug_eval.py")
        self.assertNotEqual(ren(line, "tools/eval/other.py"), line)

    def test_gitignore_stopgap_keeps_only_the_commented_line(self):
        text = (f"# A running host's address in its project (amoris serve)\n.{OLD}/\n"
                "# Until no build from before the 2026-10-16 rename runs in this checkout: such "
                f"hosts' state.\n.{OLD}/\n")
        out = ren(text, ".gitignore")
        self.assertEqual(out, text.replace(f"serve)\n.{OLD}/", "serve)\n.amoris/"))
        self.assertEqual(ren(out, ".gitignore"), out)


class Files(unittest.TestCase):
    def tree(self, files):
        d = Path(tempfile.mkdtemp())
        for rel, data in files.items():
            p = d / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)
        return d

    def test_binary_files_keep_their_bytes_and_move(self):
        blob = b"\0\1" + OLD.encode() + b"\0"
        d = self.tree({f"web/{OLD}_bg.wasm": blob, f"a/{OLD}.txt": f"{OLD}\r\n".encode()})
        quiet(r.apply, d)
        self.assertEqual((d / "web/amoris_bg.wasm").read_bytes(), blob)
        self.assertEqual((d / "a/amoris.txt").read_bytes(), b"amoris\r\n")  # no newline change
        self.assertFalse((d / f"web/{OLD}_bg.wasm").exists())

    def test_non_utf8_with_the_name_is_an_error(self):
        d = self.tree({"x.txt": b"\xff" + OLD.encode()})
        with self.assertRaises(ValueError):
            r.scan(d, ["x.txt"])

    def test_non_utf8_without_the_name_is_never_decoded(self):
        d = self.tree({"LICENSE.txt": b"\xe9t\xe9"})
        self.assertEqual(r.scan(d, ["LICENSE.txt"])[0], [])

    def test_kept_files(self):
        d = self.tree({"third_party/patches/p12-x.diff": OLD.encode(),
                       "third_party/patches/p4-p7-build.diff": f"+{OLD_U}_LLVM".encode(),
                       "third_party/rquickjs-sys-0.14.0/quickjs/quickjs.c": OLD.encode(),
                       "docs/evidence/debug-eval/a/transcript.jsonl": f"{OLD} scripts".encode(),
                       "site/content/evidence/agents/result.json": f'"from \\"{OLD}\\""'.encode(),
                       "tools/amoris_rename.py": OLD.encode()})
        changes, _, _ = r.scan(d, r.tree_files(d))
        self.assertEqual([c[0] for c in changes], ["third_party/patches/p4-p7-build.diff"])
        self.assertEqual(changes[0][2], "+AMORIS_LLVM")

    def test_paths(self):
        files = [f"crates/{OLD}-sim/src/lib.rs", f"crates/{OLD}-script/src/prelude/{OLD}.ts",
                 f"crates/{OLD}-a/{OLD}-b/{OLD}.rs", "README.md", f"web/{OLD}.js"]
        moves, n = r.plan_paths(files)
        self.assertEqual(n, 4)
        self.assertEqual(moves[:3], [(f"crates/{OLD}-a", "crates/amoris-a"),
                                     (f"crates/{OLD}-script", "crates/amoris-script"),
                                     (f"crates/{OLD}-sim", "crates/amoris-sim")])
        self.assertIn((f"crates/amoris-a/{OLD}-b", "crates/amoris-a/amoris-b"), moves)
        self.assertIn((f"crates/amoris-script/src/prelude/{OLD}.ts",
                       "crates/amoris-script/src/prelude/amoris.ts"), moves)
        d = self.tree({f: b"x" for f in files})
        quiet(r.apply, d)
        self.assertEqual(sorted(r.tree_files(d)), sorted(r.rename_path(f) for f in files))

    def test_collisions_are_refused(self):
        with self.assertRaises(ValueError):
            r.plan_paths([f"web/{OLD}.js", "web/amoris.js"])
        with self.assertRaises(ValueError):
            r.plan_paths([f"crates/{OLD}-sim/a.rs", "crates/amoris-sim/b.rs"])


class Idempotency(unittest.TestCase):
    SAMPLE = (f"use {OLD}_sim; {OLD_U}_LLVM; {OLD_C}Engine; ai{OLD}2; `{OLD}` to `amoris`;\n"
              f"{OLD_C}\nCanonical Encoding; AMORIS_{OLD_U}_BINARY; https://github.com/o/r/blob/"
              f"9a3e4b08/crates/{OLD}-x; Amoris 2026-10-15 {OLD}-physics fixed shape v1\n"
              f"a `{OLD}` export; 3fdc5b83:crates/{OLD}-render/src/trace.rs; "
              f"`{OLD}-version.json`\n")

    def test_twice_equals_once(self):
        for rel in ["a.rs", "a.md", "docs/spikes/a.md", "tools/eval/debug_eval.py", ".gitignore"]:
            once = ren(self.SAMPLE, rel)
            self.assertEqual(ren(once, rel), once, rel)
            self.assertEqual(r.rename_path(r.rename_path(f"{OLD}/{OLD}.ts")), "amoris/amoris.ts")

    def test_a_renamed_tree_checks_clean(self):
        d = Path(tempfile.mkdtemp())
        (d / f"{OLD}-x").mkdir()
        (d / f"{OLD}-x" / "a.md").write_text(self.SAMPLE, "utf-8", newline="\n")
        quiet(r.apply, d)
        self.assertEqual(quiet(r.check, d, "tree"), 0)


if __name__ == "__main__":
    unittest.main()
