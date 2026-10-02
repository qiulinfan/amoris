"""Edits to wrapped prose: find a passage whatever its line breaks (every run of whitespace in the
old text matches any run in the file) and put text in its place or after it. Re-wrap afterwards
with wrap_docs.py.

    import sys; sys.path.insert(0, "tools/scripts/dev"); from prose import replace, append_after
    replace("docs/x.md", "old words", "new words")
"""
import re


def _find(text, old):
    pattern = r"\s+".join(re.escape(w) for w in old.split())
    found = list(re.finditer(pattern, text))
    if len(found) != 1:
        raise AssertionError(f"expected one match, found {len(found)}: {old[:80]!r}")
    return found[0]


def replace(path, old, new):
    text = open(path).read()
    m = _find(text, old)
    open(path, "w").write(text[:m.start()] + new + text[m.end():])
    print("ok", path)


def append_after(path, old, new):
    text = open(path).read()
    m = _find(text, old)
    open(path, "w").write(text[:m.end()] + new + text[m.end():])
    print("ok", path)
