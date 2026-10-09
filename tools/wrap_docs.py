#!/usr/bin/env python3
"""Hard-wrap the prose of Markdown docs at 100 display columns (AGENTS.md, rule 7), so a search
that matches a word returns a line, not a paragraph: agents grep the docs, and a 3000-character line
came back whole for every match in it.

Only a paragraph or list item that has a line wider than 100 columns is refilled (list items with a
hanging indent), so an edit never reflows prose that already fits. Headings, tables, code blocks,
HTML, quotes, link definitions and blank lines are left as they are; nothing is broken inside a
`code span`, so commands stay whole; hard line breaks (`<br>`, two trailing spaces, a trailing
backslash) are kept. Width counts East Asian wide characters as two columns, and Chinese prose,
which has no spaces, breaks between two CJK characters (never before closing or after opening
punctuation), joined back without a space, so refilling it twice gives the same text. Rendering is
unchanged.

    python tools/wrap_docs.py docs/spec/checks.md       # refill what is too wide, in place
    python tools/wrap_docs.py --check docs/*.md         # list the files that need it, no writes
"""
import re
import sys
import unicodedata

WIDTH = 100
LIST = re.compile(r"^(\s*)([-*+]|\d+[.)])\s+")
# A word that would turn the line it starts into a list item, a heading, a quote or a heading's
# underline.
MARKER = re.compile(r"^(>.*|[-*+]|#+|\d+[.)]|=+|-+)$")
# A link reference definition, `[label]: target`.
LINK_DEF = re.compile(r"^\s{0,3}\[[^\]]+\]:\s")
HARD_BREAK = re.compile(r"(<br\s*/?>|\\)$")
# CJK punctuation a line may not start with, and the opening punctuation it may not end with.
NO_BREAK_BEFORE = set("，。、；：！？）」』》〉】〕｝］’”…—·％")
NO_BREAK_AFTER = set("（「『《〈【〔｛［‘“")


def wide(ch):
    return unicodedata.east_asian_width(ch) in "WF"


def width(text):
    return sum(2 if wide(ch) else 1 for ch in text)


def segments(text):
    """The paragraph as `(separator, piece)` pairs: a piece is a word, a `code span` with what
    touches it, or one CJK character with the punctuation that may not be split from it. A line may
    break before any piece, dropping its separator (" " between words, "" inside Chinese)."""
    out, cur, sep, in_code = [], "", "", False
    for i, ch in enumerate(text):
        if ch == "`":
            in_code = not in_code
        if ch == " " and not in_code:
            if cur:
                out.append((sep, cur))
            cur, sep = "", " "
            continue
        prev = text[i - 1] if i else ""
        if (cur and not in_code and wide(prev) and wide(ch)
                and ch not in NO_BREAK_BEFORE and prev not in NO_BREAK_AFTER):
            out.append((sep, cur))
            cur, sep = "", ""
        cur += ch
    if cur:
        out.append((sep, cur))
    return out


def join(lines):
    """A block's lines as one text: a newline between two CJK characters was only a wrap."""
    text = ""
    for s in lines:
        if text and not (wide(text[-1]) and wide(s[0])):
            text += " "
        text += s
    return text


def fill(text, first, rest):
    lines, line, empty = [], first, True
    for sep, piece in segments(text):
        # A space between two CJK characters is the author's, which `join` would not restore.
        kept = sep == " " and line != "" and wide(line[-1]) and wide(piece[0])
        too_wide = width(line) + len(sep) + width(piece) > WIDTH
        if not empty and too_wide and not kept and not MARKER.match(piece):
            lines.append(line)
            line = rest + piece
        else:
            line += ("" if empty else sep) + piece
        empty = False
    lines.append(line)
    return lines


def rewrap(src):
    out = []
    block = None     # [first prefix, rest prefix, stripped lines, original lines]
    fence = False

    def flush(hard=""):
        nonlocal block
        if block:
            if any(width(raw.rstrip()) > WIDTH for raw in block[3]):
                filled = fill(join(block[2]), block[0], block[1])
                filled[-1] += hard
                out.extend(filled)
            else:
                out.extend(block[3])
        block = None

    for raw in src.split("\n"):
        line = raw.rstrip()
        s = line.strip()
        if s.startswith("```") or s.startswith("~~~"):
            flush()
            fence = not fence
            out.append(raw)
            continue
        if (fence or not s or s[0] in "#|<>" or LINK_DEF.match(line)
                or (line.startswith("    ") and block is None)):
            flush()
            out.append(raw)
            continue
        m = LIST.match(line)
        if m:
            flush()
            marker = line[:m.end()]
            block = [marker, " " * len(marker), [line[m.end():].strip()], [raw]]
        elif block is not None and (block[1] == "" or line.startswith(block[1])):
            block[2].append(s)   # a paragraph's next line, or a list item's continuation
            block[3].append(raw)
        else:
            flush()
            block = ["", "", [s], [raw]]
        # A hard line break ends the run of lines refilled together; two spaces are kept as such.
        if HARD_BREAK.search(line):
            flush()
        elif raw.endswith("  "):
            flush(raw[len(line):])
    flush()
    return "\n".join(out)


def selftest():
    long_en = " ".join(["word"] * 40)
    cases = {
        # Prose that fits is left alone, however it was wrapped.
        "short\nlines stay\n": "short\nlines stay\n",
        # Too wide: refilled, the code span kept whole.
        long_en + " `a b c`\n": None,
        # Chinese: 61 characters are 122 columns; broken between characters, never before `，`.
        "中" * 50 + "，" + "文" * 10 + "\n": "中" * 49 + "\n" + "中，" + "文" * 10 + "\n",
        # A hard break ends the refilled run and keeps its two spaces.
        long_en + "  \nnext\n": None,
        # Tables, headings and code are never touched.
        "| " + long_en + " |\n# " + long_en + "\n```\n" + long_en + "\n```\n": None,
    }
    for src, want in cases.items():
        got = rewrap(src)
        assert rewrap(got) == got, f"not idempotent: {src!r}"
        if want is not None:
            assert got == want, f"{src!r} gave {got!r}"
        assert "".join(got.split()) == "".join(src.split()), f"text changed: {src!r}"
        if src.startswith("|"):
            assert got == src
        for line in got.split("\n"):
            if not line.startswith(("|", "#")) and line != long_en:
                assert width(line.rstrip()) <= WIDTH or " " not in line.strip(), line
    assert rewrap(long_en + "  \nnext\n").split("\n")[1].endswith("  ")
    print("selftest passed")


def main():
    args = sys.argv[1:]
    if args == ["--selftest"]:
        selftest()
        return
    check = "--check" in args
    files = [a for a in args if a != "--check"]
    changed = []
    for f in files:
        with open(f, encoding="utf-8", newline="") as fh:
            src = fh.read().replace("\r\n", "\n")
        new = rewrap(src)
        if new != src:
            changed.append(f)
            if not check:
                with open(f, "w", encoding="utf-8", newline="\n") as fh:   # LF on Windows too
                    fh.write(new)
    print(("would change" if check else "wrapped"), len(changed), "of", len(files))
    for f in changed:
        print(" ", f)


if __name__ == "__main__":
    main()
