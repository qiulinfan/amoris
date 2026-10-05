#!/usr/bin/env python3
"""Hard-wrap the prose of Markdown docs at 100 columns (AGENTS.md, rule 6), so a search that matches a
word returns a line, not a paragraph: agents grep the docs, and a 3000-character line came back
whole for every match in it. Paragraphs and list items are wrapped (list items with a hanging
indent); headings, tables, code blocks, HTML and blank lines are left as they are, and nothing is
broken inside a `code span`, so commands stay whole. Rendering is unchanged.

    python3 tools/wrap_docs.py docs/*.md docs/spec/*.md
    python3 tools/wrap_docs.py --check docs/*.md   # files that need it, no writes
"""
import re
import sys

WIDTH = 100
LIST = re.compile(r"^(\s*)([-*+]|\d+[.)])\s+")
# A word that would turn the line it starts into a list item, a heading or a quote.
MARKER = re.compile(r"^([-*+]|#+|>|\d+[.)])$")


def tokens(text):
    """Words, keeping a `code span` (with what touches it) as one word."""
    out, cur, in_code = [], "", False
    for ch in text:
        if ch == "`":
            in_code = not in_code
        if ch == " " and not in_code:
            if cur:
                out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur:
        out.append(cur)
    return out


def wrap(text, first, rest):
    lines, line = [], first
    empty = True
    for w in tokens(text):
        if not empty and len(line) + 1 + len(w) > WIDTH and not MARKER.match(w):
            lines.append(line)
            line, empty = rest + w, False
        else:
            line += ("" if empty else " ") + w
            empty = False
    lines.append(line)
    return lines


def rewrap(src):
    out = []
    block = None     # (first prefix, rest prefix, words)
    fence = False

    def flush():
        nonlocal block
        if block:
            out.extend(wrap(" ".join(block[2]), block[0], block[1]))
        block = None

    for raw in src.split("\n"):
        line = raw.rstrip()
        s = line.strip()
        if s.startswith("```") or s.startswith("~~~"):
            flush()
            fence = not fence
            out.append(line)
            continue
        if fence or not s or s.startswith("#") or s.startswith("|") or s.startswith("<") or s.startswith(">") or line.startswith("    ") and block is None:
            flush()
            out.append(line)
            continue
        m = LIST.match(line)
        if m:
            flush()
            marker = line[:m.end()]
            block = (marker, " " * len(marker), [line[m.end():].strip()])
            continue
        if block is not None and line.startswith(block[1]) and block[1].strip() == "" and block[1]:
            block[2].append(s)   # a list item's continuation
            continue
        if block is not None and block[1] == "":
            block[2].append(s)   # a paragraph's next line
            continue
        flush()
        block = ("", "", [s])
    flush()
    return "\n".join(out)


def main():
    args = sys.argv[1:]
    check = "--check" in args
    files = [a for a in args if a != "--check"]
    changed = []
    for f in files:
        src = open(f, encoding="utf-8").read()
        new = rewrap(src)
        if new != src:
            changed.append(f)
            if not check:
                open(f, "w", encoding="utf-8", newline="\n").write(new)   # LF on Windows too
    print(("would change" if check else "wrapped"), len(changed), "of", len(files))
    for f in changed:
        print(" ", f)


if __name__ == "__main__":
    main()
