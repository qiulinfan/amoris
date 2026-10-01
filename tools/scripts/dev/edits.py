#!/usr/bin/env python3
"""Exact text replacement for scripted edits (AGENTS.md, Development helpers): each old string must
occur exactly once in the file, or nothing is written and the call fails, so an edit never lands
in the wrong place.

    import sys; sys.path.insert(0, "tools/scripts/dev")
    from edits import patch
    patch("engine/ui/src/font.cpp", [("old text", "new text"), ...])
"""


def patch(path, pairs):
    with open(path) as f:
        text = f.read()
    for old, new in pairs:
        n = text.count(old)
        if n != 1:
            raise AssertionError(f"{path}: expected one match, found {n}: {old[:80]!r}")
        text = text.replace(old, new)
    with open(path, "w") as f:
        f.write(text)
    print("ok", path)
