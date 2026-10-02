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


# UTF-8 with LF on every host (Windows' default would be its code page and CRLF).
def _read(path):
    return open(path, encoding="utf-8").read()


def _write(path, text):
    open(path, "w", encoding="utf-8", newline="\n").write(text)


def replace(path, old, new):
    text = _read(path)
    m = _find(text, old)
    _write(path, text[:m.start()] + new + text[m.end():])
    print("ok", path)


def append_after(path, old, new):
    text = _read(path)
    m = _find(text, old)
    _write(path, text[:m.end()] + new + text[m.end():])
    print("ok", path)
