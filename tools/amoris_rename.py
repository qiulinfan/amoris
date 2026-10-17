#!/usr/bin/env python3
"""Renames the engine's old identifier `pocket` to `amoris` (the owner's decision of 2026-10-16,
charter 5.1), in this checkout or in any exported tree.

The rules are fixed and idempotent: a tree they have renamed is left as it is. Benchmarks use
this script to build historical commits with the current identifiers. `--tree` prints the
regeneration steps for an exported tree. Amoris has adopted the rename; no Pioneer-specific
synchronization policy is needed here.

    python tools/amoris_rename.py --git [--allow-dirty]    # this checkout: git mv, then rewrite
    python tools/amoris_rename.py --tree DIR               # a plain directory (export, old revision)
    python tools/amoris_rename.py --check [--tree DIR]     # dry run; exit 1 if anything would change
    python tools/amoris_rename.py --residual [--tree DIR]  # every `pocket` left, with the rule keeping it

Within a text file, in this order:

1. Protected spans keep their bytes: names that are not this engine's (another repository's,
   the old C++ master's, a superseded draft type), links and git object names pinned to a commit,
   the dated physics fixed-shape key context (a hash domain the physics cache and the world hash
   cover), verbatim recorded output, files saved under `out/` and the files recorded runs wrote,
   toolchain locations recorded in spike tables, the statements of the rename itself, and the
   bounded old-name keeps of `tools/eval/debug_eval.py` and `.gitignore`.
2. The two composite environment names: `AMORIS_POCKET_BINARY` becomes `AMORIS_BINARY` and
   `AMORIS_TEST_POCKET` becomes `AMORIS_TEST_BINARY`.
3. "Pocket Canonical Encoding" becomes "Portable Canonical Encoding" (the acronym PCE stays), and
   the article "a" before the name becomes "an" ("a `pocket` export", "a POCKET_TSC").
4. Every other `pocket`, `Pocket` or `POCKET` not preceded by an ASCII letter or digit becomes
   `amoris`, `Amoris` or `AMORIS` (so `aipocket` stays; compounds such as `POCKET_BACKEND`,
   `pocket_sim`, `.pocket-tools` and `pocketSamples` change).

Paths take rule 4 per component. Binary files (a NUL in their first 8,000 bytes, as git decides)
are never rewritten, though their paths are. Text is strict UTF-8 and is written back byte for
byte apart from the replacements. The rules themselves (this file and its test), the QuickJS-ng
sources, every QuickJS-ng patch except the P4/P7 build glue and recorded evidence
(`docs/evidence/`, `site/content/evidence/`) are never rewritten. Standard library only.
"""
import argparse
import fnmatch
import os
import re
import subprocess
import sys
import unicodedata
from pathlib import Path

LIT = re.escape

# Step 1: protected spans, in this order. (id, pattern)
GLOBAL = [
    ("G1", LIT("PocketEngine")),  # another repository (AGENTS.md rule 5)
    ("G2", LIT("PocketError")),  # a superseded draft type name, recorded as such
    ("G3", r"https://github\.com/[^\s)>\]\"'`]*/(?:blob|tree|commit)/[0-9a-f]{7,40}"
           r"(?:/[^\s)>\]\"'`]*)?"),  # links pinned to a commit
    ("G6", r"(?<![0-9A-Za-z])[0-9a-f]{7,40}:[A-Za-z._][^\s)>\]\"'`]*/[^\s)>\]\"'`]*"),
    # git object names, `<commit>:<path>` (a file as it was at that commit)
    ("G4", LIT("Amoris 2026-10-15 pocket-physics fixed shape v1")),  # dated hash domain
    ("G5", r"`(?:pocket|Pocket|POCKET)[^`\n]*`\s*(?:改为|to|->|→)\s*`(?:amoris|Amoris|AMORIS)[^`\n]*`"),
    # statements of the rename itself
]
MARKDOWN = [  # files ending in .md
    ("M1", LIT("Pocket UI")),  # the old C++ master's UI library
    ("M2", LIT("master 的 `pocket` 工具")),  # the old master's build tool
    ("M3", LIT("`pocket` 构建工具")),  # the same
    ("M4", LIT("include/pocket/")),  # old master C++ paths
    ("M5", LIT("pocket::repro")),  # old master namespace
    ("M6", LIT("Master's suite (`pocket test`)")),  # old master command
    ("M7", LIT("`pocket_scenario`")),  # old master tool
    ("M8", LIT("tools/pocket/src/")),  # old master path
    ("M9", LIT("POCKET_EVAL_RUNTIME")),  # old master variable
    ("M10", LIT("(pocket:53:23)")),  # verbatim recorded stack
    ("M11", LIT("`pocket panicked: ... dlmalloc")),  # verbatim recorded panic
    ("M12", LIT('[Device "pocket"]')),  # verbatim recorded wgpu error
    ("M13", r"out/serveloop/bin/pocket-[\w.-]+\.exe"),  # saved evidence binaries
    ("M14", LIT("`pocket-new-wave2.exe`")),  # the same
    ("M15", LIT("`pocket-version.json`")),  # the file recorded runs wrote under `out/` (gizmo.md 6)
]
SCOPED = [  # (id, path globs, pattern)
    ("T1", ["docs/spikes/*.md", "docs/bench/web.md"], LIT(".pocket-tools")),
    # toolchain locations recorded when each spike ran
    ("E1", ["tools/eval/debug_eval.py"], LIT("(?:pocket|amoris)")),
    ("E1", ["tools/eval/debug_eval.py"], LIT('("pocket_", "amoris_")')),
    ("E1", ["tools/eval/debug_eval.py"], LIT('("pocket_scripts", "amoris_scripts")')),
    # transcripts recorded before the rename are re-read by `--report`
    ("E2", [".gitignore"], r"(?m)^# Until no build from before the 2026-10-16 rename runs in this "
                           r"checkout: such hosts' state\.\r?\n\.pocket/$"),
    # the stopgap only, with its comment, so the current `.pocket/` line still renames
]
# Step 2: composite environment names (exact strings), before rule 4 would double the prefix.
COMPOSITE = [("AMORIS_POCKET_BINARY", "AMORIS_BINARY"), ("AMORIS_TEST_POCKET", "AMORIS_TEST_BINARY")]
# Step 3: the canonical encoding's long name, which may break across lines; the article before the
# name ("an amoris-runtime example"), never a Rust lifetime (`&'a pocket_x`).
PCE = re.compile(r"Pocket(\s+)Canonical(\s+)Encoding")
ARTICLE = re.compile(r"(?<![\w'&])([Aa])(\s+[`\"(*\[]*)(?=pocket|Pocket|POCKET)")
# Step 4: the generic, case-preserving substitution.
GENERIC = re.compile(r"(?<![A-Za-z0-9])(pocket|Pocket|POCKET)")
CASES = {"pocket": "amoris", "Pocket": "Amoris", "POCKET": "AMORIS"}
# What may remain after the rules outside protected spans: only letter- or digit-preceded forms.
LEFT = re.compile(r"(?<![A-Za-z0-9])pocket", re.IGNORECASE)
ANY = re.compile(r"pocket", re.IGNORECASE)

# Files whose content is never rewritten (the rules themselves; the owner's QuickJS-ng patches;
# recorded evidence, which keeps the names its runs used).
RULE_FILES = {"tools/amoris_rename.py", "tools/tests/test_amoris_rename.py"}
KEPT_GLOBS = ["third_party/patches/*.diff", "third_party/rquickjs-sys-0.14.0/quickjs/*",
              "docs/evidence/*", "site/content/evidence/*"]
KEPT_EXCEPT = {"third_party/patches/p4-p7-build.diff"}
SKIP_DIRS = {".git", "target", "out", "node_modules", ".claude"}

REGENERATE = """\
Regenerate, in order (AGENTS.md, Building; AMORIS_LLVM set, CARGO_NET_OFFLINE=true):
 1. python third_party/vendor.py --offline, then python third_party/vendor.py --check --offline
 2. cargo metadata --offline --format-version 1 >/dev/null   (Cargo.lock: package names only)
 3. cargo fmt --all   (use order); rustfmt --edition 2024 tools/splats/web_harness/src/lib.rs
 4. cargo xtask gen   (the clippy copies)
 5. cargo test --release -p amoris-script --features transpile --test prelude   (no bless)
 6. cargo test --release -p amoris-render --test neural_mlp   (no bless)
 7. bash tools/build_viewport.sh   (the render feed's FORMAT hashes frame.rs and visual.rs)
 8. (cd tools/neural/splatfit && uv lock --offline)
 9. the gi-room probes' scene_signature: cargo run --release -p amoris-render --example bake_gi
    (samples/gi-room/lighting/probes.json's recorded settings), copying only the signature
10. python tools/wrap_docs.py on each docs file whose line widths changed (listed above)"""


def is_kept_file(rel):
    if rel in RULE_FILES:
        return True
    if rel in KEPT_EXCEPT:
        return False
    return any(fnmatch.fnmatchcase(rel, g) for g in KEPT_GLOBS)


def is_binary(data):
    return b"\0" in data[:8000]


def protections(rel):
    """The step-1 patterns that apply to the file at `rel` (a `/` path), in order."""
    out = list(GLOBAL)
    if rel.endswith(".md"):
        out += MARKDOWN
    for rid, globs, pattern in SCOPED:
        if any(fnmatch.fnmatchcase(rel, g) for g in globs):
            out.append((rid, pattern))
    return out


def protected_spans(text, rel):
    """The protected spans as sorted, disjoint `(start, end, id)` in `text`. Each pattern runs on
    the text with the earlier spans masked (as the placeholders of the specification do)."""
    spans = []
    masked = text
    for rid, pattern in protections(rel):
        for m in re.finditer(pattern, masked):
            if m.end() > m.start():
                spans.append((m.start(), m.end(), rid))
        for s, e, _ in spans:
            masked = masked[:s] + "\0" * (e - s) + masked[e:]
    spans.sort()
    merged = []
    for s, e, rid in spans:
        if merged and s < merged[-1][1]:
            ps, pe, pid = merged[-1]
            merged[-1] = (ps, max(pe, e), pid)
        else:
            merged.append((s, e, rid))
    return merged


class Counts:
    def __init__(self):
        self.generic = 0
        self.pce = 0
        self.article = 0
        self.composite = 0
        self.kept = {}

    def keep(self, rid, n=1):
        if n:
            self.kept[rid] = self.kept.get(rid, 0) + n

    def add(self, other):
        self.generic += other.generic
        self.pce += other.pce
        self.article += other.article
        self.composite += other.composite
        for k, v in other.kept.items():
            self.keep(k, v)


def rename_free(segment, counts):
    """Steps 2 to 4 on text outside every protected span."""
    for old, new in COMPOSITE:
        counts.composite += segment.count(old)
        segment = segment.replace(old, new)
    segment, n = PCE.subn(r"Portable\1Canonical\2Encoding", segment)
    counts.pce += n
    segment, n = ARTICLE.subn(r"\1n\2", segment)
    counts.article += n
    segment, n = GENERIC.subn(lambda m: CASES[m.group(1)], segment)
    counts.generic += n
    return segment


def rename_text(text, rel):
    """The renamed text and its `Counts`. Raises ValueError if a `pocket` would remain unprotected
    (a casing the rules do not know) or if the text holds a NUL."""
    if "\0" in text:
        raise ValueError(f"{rel}: holds a NUL byte, which the rules reserve")
    counts = Counts()
    out, masked, pos = [], [], 0
    for s, e, rid in protected_spans(text, rel):
        free = rename_free(text[pos:s], counts)
        out += [free, text[s:e]]
        masked += [free, "\0" * (e - s)]
        counts.keep(rid, len(ANY.findall(text[s:e])))
        pos = e
    free = rename_free(text[pos:], counts)
    out.append(free)
    masked.append(free)
    masked = "".join(masked)
    left = LEFT.search(masked)
    if left:
        line = masked.count("\n", 0, left.start()) + 1
        raise ValueError(f"{rel}:{line}: {left.group(0)!r} is a casing the rules do not rename")
    counts.keep("letter-guard", len(ANY.findall(masked)))
    return "".join(out), counts


def rename_path(rel):
    return "/".join(GENERIC.sub(lambda m: CASES[m.group(1)], part) for part in rel.split("/"))


def width(line):
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in line)


def width_changes(old, new):
    """`(line, old width, new width)` for every line whose display width changed."""
    a, b = old.split("\n"), new.split("\n")
    if len(a) != len(b):
        return [(0, len(a), len(b))]
    return [(i + 1, width(x), width(y)) for i, (x, y) in enumerate(zip(a, b)) if x != y
            and width(x) != width(y)]


# --- trees -------------------------------------------------------------------------------------


def git(root, *args, **kw):
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, **kw)


def git_files(root):
    out = git(root, "ls-files", "-z").stdout.decode("utf-8")
    return [p for p in out.split("\0") if p]


def tree_files(root):
    files = []
    for d, dirs, names in os.walk(root):
        dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS)
        for n in sorted(names):
            files.append(Path(d, n).relative_to(root).as_posix())
    return files


def plan_paths(files):
    """`(moves, count)`: the `(old, new)` moves that rename every path in `files`, each renamed
    directory once from the top down, then the files whose own name changes; `count` is the number
    of paths that change. Refuses a move onto anything that exists."""
    cur, moves = list(files), []
    while True:
        dirs = set()
        for f in cur:
            parts = f.split("/")
            for i in range(len(parts) - 1):
                if rename_path(parts[i]) != parts[i]:
                    dirs.add("/".join(parts[: i + 1]))
                    break
        if not dirs:
            break
        step = []
        for d in sorted(dirs):
            parent, _, name = d.rpartition("/")
            nd = f"{parent}/{rename_path(name)}" if parent else rename_path(name)
            if any(f == nd or f.startswith(nd + "/") for f in cur):
                raise ValueError(f"renaming {d} would collide with the existing {nd}")
            step.append((d, nd))
        moves += step

        def moved(f):
            for d, nd in step:
                if f.startswith(d + "/"):
                    return nd + f[len(d):]
            return f

        cur = [moved(f) for f in cur]
    present = set(cur)
    for f in cur:
        new = rename_path(f)
        if new != f:
            if new in present:
                raise ValueError(f"renaming {f} would overwrite the existing {new}")
            moves.append((f, new))
    finals = [rename_path(f) for f in files]
    if len(set(finals)) != len(finals):
        raise ValueError("two paths would become the same path")
    return moves, sum(1 for f, n in zip(files, finals) if f != n)


def scan(root, files):
    """`(path, old text, new text, counts)` for every text file the rules change, the counts over
    every file, and the binary files that hold the name."""
    changes, total, binaries = [], Counts(), []
    for rel in files:
        path = Path(root, rel)
        if path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        if b"pocket" not in data.lower():
            continue
        if is_binary(data):
            binaries.append(rel)
            continue
        if is_kept_file(rel):
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError as e:
            raise ValueError(f"{rel}: not UTF-8 ({e}) and holds the name") from e
        new, counts = rename_text(text, rel)
        total.add(counts)
        if new != text:
            changes.append((rel, text, new, counts))
    return changes, total, binaries


def report(moves, nmoved, changes, total, binaries):
    print(f"paths renamed: {nmoved} ({len(moves)} moves)")
    print(f"files rewritten: {len(changes)}")
    print(f"replacements: generic {total.generic}, PCE {total.pce}, article {total.article}, "
          f"composite {total.composite}")
    kept = sum(total.kept.values())
    detail = ", ".join(f"{k} {v}" for k, v in sorted(total.kept.items()))
    print(f"kept: {kept} ({detail})")
    if binaries:
        print(f"binary files holding the name (never rewritten): {', '.join(binaries)}")
    widths = [(rel, w) for rel, old, new, _ in changes for w in width_changes(old, new)]
    if widths:
        print("line widths changed:")
        for rel, (line, a, b) in widths:
            print(f"  {rel}:{line}: {a} -> {b}")


def apply(root, mode, allow_dirty):
    root = Path(root).resolve()
    if mode == "git":
        if not allow_dirty and git(root, "status", "--porcelain").stdout.strip():
            sys.exit("amoris_rename: the checkout has changes; commit them or pass --allow-dirty")
        files = git_files(root)
    else:
        files = tree_files(root)
    moves, nmoved = plan_paths(files)
    _, total, binaries = scan(root, files)  # validates every file before anything moves
    for old, new in moves:
        Path(root, new).parent.mkdir(parents=True, exist_ok=True)
        if mode == "git":
            git(root, "mv", old, new)
        else:
            os.rename(Path(root, old), Path(root, new))
    files = [rename_path(f) for f in files]
    changes, total, binaries = scan(root, files)
    for rel, _, new, _ in changes:
        Path(root, rel).write_bytes(new.encode("utf-8"))
    report(moves, nmoved, changes, total, binaries)
    print(REGENERATE)


def check(root, mode):
    root = Path(root).resolve()
    files = git_files(root) if mode == "git" else tree_files(root)
    moves, nmoved = plan_paths(files)
    changes, total, binaries = scan(root, files)
    report(moves, nmoved, changes, total, binaries)
    for old, new in moves:
        print(f"would move {old} -> {new}")
    for rel, _, _, c in changes:
        print(f"would rewrite {rel} (generic {c.generic}, PCE {c.pce}, composite {c.composite})")
    return 1 if moves or changes else 0


def residual(root, mode):
    """Every `pocket` left in the tree, with the rule that keeps it."""
    root = Path(root).resolve()
    files = git_files(root) if mode == "git" else tree_files(root)
    for rel in files:
        if rename_path(rel) != rel:
            print(f"{rel}: path WOULD CHANGE")
    for rel in files:
        path = Path(root, rel)
        if path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        if b"pocket" not in data.lower():
            continue
        if is_binary(data):
            print(f"{rel}: binary, {data.lower().count(b'pocket')} occurrence(s), never rewritten")
            continue
        text = data.decode("utf-8", errors="replace")
        why_file = None
        if rel in RULE_FILES:
            why_file = "rules file"
        elif is_kept_file(rel):
            why_file = "QuickJS-ng or recorded evidence, kept byte for byte"
        spans = [] if why_file else protected_spans(text, rel)
        lines = text.split("\n")
        for m in ANY.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            if why_file:
                why = why_file
            else:
                why = next((rid for s, e, rid in spans if s <= m.start() < e), None)
                if why is None:
                    prev = text[m.start() - 1] if m.start() else ""
                    why = "letter-guard" if prev.isascii() and prev.isalnum() else "WOULD CHANGE"
            src = lines[line - 1]
            print(f"{rel}:{line}: [{why}] {src.strip()[:160]}")


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")  # paths and lines hold CJK text
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    act = ap.add_mutually_exclusive_group()
    act.add_argument("--git", action="store_true", help="rename this checkout (git mv, rewrite)")
    act.add_argument("--check", action="store_true", help="dry run; exit 1 if anything would change")
    act.add_argument("--residual", action="store_true", help="list every remaining occurrence")
    ap.add_argument("--tree", metavar="DIR", help="rename a plain directory (or check it)")
    ap.add_argument("--allow-dirty", action="store_true", help="--git on a checkout with changes")
    args = ap.parse_args()
    repo = Path(__file__).resolve().parent.parent
    root, mode = (Path(args.tree), "tree") if args.tree else (repo, "git")
    try:
        if args.git:
            if args.tree:
                ap.error("--git renames this checkout; --tree DIR alone renames a directory")
            apply(repo, "git", args.allow_dirty)
        elif args.check:
            sys.exit(check(root, mode))
        elif args.residual:
            residual(root, mode)
        elif args.tree:
            apply(root, "tree", True)
        else:
            ap.error("give --git, --tree DIR, --check or --residual")
    except ValueError as e:
        sys.exit(f"amoris_rename: {e}")


if __name__ == "__main__":
    main()
