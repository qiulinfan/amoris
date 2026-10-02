#!/usr/bin/env python3
"""Large test files on Google Drive (AGENTS.md, Test data): a file over 20 MiB, typically an art
asset such as a high-poly Blender model, is kept out of git. Each one is pinned in tests/data.json by
its path, SHA-256 and size, stored once on the owner's Google Drive as
pocket-data/blobs/<sha256><ext>, listed in the block this script keeps at the end of .gitignore, and
fetched back into its place on any machine. Everything smaller stays in git. Transfers go through
rclone (any OS); blobs are cached in .pocket/data/blobs and checked against their hash.

    python3 tools/scripts/data.py status [--json]           exit 2: git would take a file over the limit
                                                            (or still holds a pinned one); 1: a pinned file
                                                            is missing or changed here; 0: all in order
    python3 tools/scripts/data.py fetch [--force] [PATH...] put the pinned files (or those named) in place
    python3 tools/scripts/data.py add PATH...               upload and pin files over 20 MiB (a directory:
                                                            those in it), ignore them and take them out of git
    python3 tools/scripts/data.py drop PATH...              unpin files (the local copy and the blob stay)
    python3 tools/scripts/data.py verify                    check that Drive holds every pinned blob

POCKET_DATA_REMOTE is the rclone place of the store (default pocketdata:pocket-data); RCLONE the rclone
binary. Each machine is set up once, with the owner's own OAuth client, the same on every machine
(under drive.file a blob is reachable only through the client that created it):
    rclone config create pocketdata drive scope=drive.file client_id=<id> client_secret=<secret>
"""
import contextlib
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tests" / "data.json"
DATA = ROOT / ".pocket" / "data"
CACHE = DATA / "blobs"
GITIGNORE = ROOT / ".gitignore"
LIMIT = 20 << 20  # files larger than this go to Drive
MIB = 1 << 20
BEGIN = "# BEGIN pocket data: large test files on Google Drive, written by tools/scripts/data.py (AGENTS.md, Test data)"
END = "# END pocket data"
SETUP = ("rclone config create pocketdata drive scope=drive.file client_id=<id> client_secret=<secret> "
         "(AGENTS.md, Test data)")


def remote():
    return os.environ.get("POCKET_DATA_REMOTE", "pocketdata:pocket-data").rstrip("/")


def read(path):
    return path.read_text(encoding="utf-8") if path.exists() else ""


def write(path, text):
    """Replaces a file whole, so an interrupted run leaves the old one."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".writing")
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    os.replace(tmp, path)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(MIB), b""):
            h.update(chunk)
    return h.hexdigest()


def copy_hashing(src):
    """Copies src into the cache under a temporary name while hashing it: the path, SHA-256 and size of
    exactly the bytes copied, so a file saved meanwhile cannot pin one version and upload another."""
    CACHE.mkdir(parents=True, exist_ok=True)
    h, n = hashlib.sha256(), 0
    fd, tmp = tempfile.mkstemp(dir=CACHE, suffix=".part")
    try:
        with os.fdopen(fd, "wb") as out, open(src, "rb") as f:
            for chunk in iter(lambda: f.read(MIB), b""):
                h.update(chunk)
                out.write(chunk)
                n += len(chunk)
    except BaseException:
        os.unlink(tmp)
        raise
    return Path(tmp), h.hexdigest(), n


def blob_name(rel, digest):
    ext = Path(rel).suffix.lower()
    return digest + (ext if 1 < len(ext) <= 8 and ext[1:].isalnum() else "")


def load():
    text = read(MANIFEST)
    files = json.loads(text)["files"] if text else {}
    for rel in files:
        if not rel or Path(rel).is_absolute() or ".." in Path(rel).parts:
            sys.exit(f"tests/data.json: {rel!r} is not a path inside the repository")
    return files


def manifest_text(files):
    doc = {"about": "Large test files kept on Google Drive; see tools/scripts/data.py and AGENTS.md (Test data).",
           "files": {k: files[k] for k in sorted(files)}}
    return json.dumps(doc, indent=1, ensure_ascii=False) + "\n"


def ignore_line(rel):
    line = "".join("\\" + c if c in "*?[\\" else c for c in rel)
    if line.endswith(" "):
        line = line[:-1] + "\\ "
    return "/" + line


def ignores_text(files):
    lines = read(GITIGNORE).splitlines()
    if BEGIN in lines:
        start = lines.index(BEGIN)
        if END not in lines[start:]:
            sys.exit(f".gitignore has the pocket data block's first line but not '{END}' after it: "
                     "restore that line, then run again")
        end = lines.index(END, start)
        lines = lines[:start] + lines[end + 1:]
        while lines and not lines[-1].strip():
            lines.pop()
    if files:
        lines += ["", BEGIN] + [ignore_line(rel) for rel in sorted(files)] + [END]
    return "\n".join(lines) + "\n"


def save(files):
    manifest, ignores = manifest_text(files), ignores_text(files)  # both checked before either is written
    write(MANIFEST, manifest)
    write(GITIGNORE, ignores)
    git("add", "--", "tests/data.json", ".gitignore")


def git(*args):
    return subprocess.run(["git", "--literal-pathspecs", "-C", str(ROOT), *args],
                          capture_output=True, encoding="utf-8", errors="replace")


def git_paths(*args):
    return [unicodedata.normalize("NFC", p) for p in git("ls-files", "-z", *args).stdout.split("\0") if p]


@contextlib.contextmanager
def locked():
    """One add or drop at a time, so two sessions do not each save a manifest without the other's pin."""
    DATA.mkdir(parents=True, exist_ok=True)
    lock = DATA / "lock"
    try:
        fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError:
        sys.exit(f"another data.py is changing the pins ({lock.relative_to(ROOT)} exists; delete it if none runs)")
    try:
        os.write(fd, str(os.getpid()).encode())
        yield
    finally:
        os.close(fd)
        lock.unlink(missing_ok=True)


def rel_of(arg):
    """The path inside the repository as git spells it: the case on disk, composed Unicode."""
    try:
        rel = Path(arg).resolve().relative_to(ROOT)
    except ValueError:
        sys.exit(f"{arg}: not inside the repository {ROOT}")
    parts, cur = [], ROOT
    for name in rel.parts:
        if cur.is_dir():
            names = os.listdir(cur)
            if name not in names:
                fold = unicodedata.normalize("NFC", name).casefold()
                same = [n for n in names if unicodedata.normalize("NFC", n).casefold() == fold]
                if len(same) == 1:
                    name = same[0]
        parts.append(name)
        cur = cur / name
    out = unicodedata.normalize("NFC", "/".join(parts))
    if out.split("/")[0] in (".git", ".pocket"):
        sys.exit(f"{arg}: not a place for test files")
    return out


def rclone(*args, input=None, ok=(0,)):
    exe = os.environ.get("RCLONE") or shutil.which("rclone")
    if not exe:
        sys.exit("rclone is not installed: brew install rclone (macOS), winget install Rclone.Rclone (Windows), "
                 "or rclone.org/install (Linux); then set it up: " + SETUP)
    r = subprocess.run([exe, *args], capture_output=True, input=input, encoding="utf-8", errors="replace")
    if r.returncode not in ok:
        msg = r.stderr.strip().splitlines()[-3:] or [f"exit {r.returncode}"]
        hint = ""
        if "didn't find section in config file" in r.stderr:
            hint = f"\nThe rclone remote of {remote()} is not set up on this machine: " + SETUP
        sys.exit(f"rclone {args[0]} failed:\n  " + "\n  ".join(msg) + hint)
    for line in r.stderr.splitlines():  # rclone's notices (a retiring client, say) reach the person too
        if "NOTICE" in line:
            print("rclone: " + line.split(": ", 1)[-1], file=sys.stderr)
    return r


def transfer(names, src, dst):
    """Copies the named files from src to dst; one that dst holds already with the same hash is skipped."""
    if names:
        rclone("copy", str(src), str(dst), "--files-from", "-", "--no-traverse", "--checksum",
               input="\n".join(sorted(set(names))) + "\n")


def state(rel, entry):
    p = ROOT / rel
    if not p.is_file():
        return "missing"
    if p.stat().st_size != entry["bytes"] or sha256(p) != entry["sha256"]:
        return "changed"
    return "ok"


def cmd_status(args):
    files = load()
    index = set(git_paths("--cached"))
    rows = [{"path": rel, "state": state(rel, e), "bytes": e["bytes"]} for rel, e in files.items()]
    still = [rel for rel in files if rel in index]
    big = []
    for rel in git_paths("--cached", "--others", "--exclude-standard"):
        p = ROOT / rel
        if rel not in files and p.is_file() and p.stat().st_size > LIMIT:
            big.append({"path": rel, "bytes": p.stat().st_size})
    if "--json" in args:
        print(json.dumps({"pinned": rows, "still_in_git": still, "too_big_for_git": big}, indent=1, ensure_ascii=False))
    else:
        counts = {s: sum(r["state"] == s for r in rows) for s in ("ok", "missing", "changed")}
        print(f"{len(rows)} pinned: {counts['ok']} in place, {counts['missing']} missing, {counts['changed']} changed")
        for r in rows:
            p = ROOT / r["path"]
            if r["state"] == "missing":
                hint = "data.py fetch; or, if it was moved or deleted, data.py drop it (and add its new path)"
            elif r["state"] == "changed" and p.stat().st_size <= LIMIT:
                hint = "now 20 MiB or less: data.py drop it, then git add it"
            elif r["state"] == "changed":
                hint = "data.py add pins this version; fetch --force takes the pinned one back"
            else:
                continue
            print(f"  {r['state']:<8} {r['path']}   ({hint})")
        for rel in still:
            print(f"  in git   {rel}   (pinned yet in git's index: git rm --cached it, or data.py add it again)")
        for b in big:
            print(f"  too big  {b['path']}   ({b['bytes'] / MIB:.1f} MiB: data.py add)")
    if still or big:
        sys.exit(2)
    if any(r["state"] != "ok" for r in rows):
        sys.exit(1)


def cmd_fetch(args):
    force = "--force" in args
    files = load()
    wanted = [rel_of(a) for a in args if not a.startswith("--")] or sorted(files)
    for rel in wanted:
        if rel not in files:
            sys.exit(f"{rel} is not pinned in tests/data.json")
    states = {rel: state(rel, files[rel]) for rel in wanted}
    todo = [rel for rel in wanted if states[rel] == "missing" or (force and states[rel] == "changed")]
    need = []
    for rel in todo:
        e = files[rel]
        blob = CACHE / blob_name(rel, e["sha256"])
        if not (blob.is_file() and blob.stat().st_size == e["bytes"]):
            need.append(blob.name)
    CACHE.mkdir(parents=True, exist_ok=True)
    transfer(need, remote() + "/blobs", CACHE)
    failed, saved = [], []
    for rel in todo:
        e = files[rel]
        blob = CACHE / blob_name(rel, e["sha256"])
        if not blob.is_file() or sha256(blob) != e["sha256"]:
            blob.unlink(missing_ok=True)
            failed.append(rel)
            continue
        dest = ROOT / rel
        if states[rel] == "changed":  # the edit made here is kept in the cache under its own hash
            tmp, digest, _ = copy_hashing(dest)
            kept = CACHE / blob_name(rel, digest)
            os.replace(tmp, kept)
            saved.append((rel, kept))
        dest.parent.mkdir(parents=True, exist_ok=True)
        tmp, _, _ = copy_hashing(blob)
        try:
            os.replace(tmp, dest)
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
    left = [rel for rel in wanted if rel not in todo and states[rel] == "changed"]
    print(f"{len(todo) - len(failed)} put in place ({len(need)} asked of Drive), "
          f"{len(wanted) - len(todo) - len(left)} already in place")
    for rel in left:
        print(f"  changed here, left alone: {rel} (fetch --force replaces it; add pins it)")
    for rel, kept in saved:
        print(f"  the edit replaced at {rel} is kept at {kept.relative_to(ROOT)}")
    for rel in failed:
        print(f"  not on {remote()} (or not matching its hash): {rel}")
    if failed:
        sys.exit("Some blobs could not be fetched: check the Google account, POCKET_DATA_REMOTE and that this "
                 "machine uses the same OAuth client as the one that added them (AGENTS.md, Test data)")


def expand(args):
    out = []
    for a in args:
        p = Path(a)
        if p.is_dir():
            d = rel_of(a) or "."
            for rel in git_paths("--cached", "--others", "--exclude-standard", "--", d):
                f = ROOT / rel
                if f.is_file() and f.stat().st_size > LIMIT:
                    out.append(rel)
        elif p.is_file():
            rel = rel_of(a)
            if p.stat().st_size <= LIMIT:
                sys.exit(f"{rel} is {p.stat().st_size / MIB:.1f} MiB: files of 20 MiB or less stay in git")
            out.append(rel)
        else:
            sys.exit(f"{a}: no such file")
    return list(dict.fromkeys(out))


def cmd_add(args):
    rels = expand(args)
    if not rels:
        sys.exit("add: no file over 20 MiB named")
    with locked():
        pinned = {}
        for rel in rels:
            tmp, digest, n = copy_hashing(ROOT / rel)
            os.replace(tmp, CACHE / blob_name(rel, digest))
            pinned[rel] = {"sha256": digest, "bytes": n}
        transfer([blob_name(rel, e["sha256"]) for rel, e in pinned.items()], CACHE, remote() + "/blobs")
        files = load()
        files.update(pinned)
        save(files)
        index = set(git_paths("--cached"))
        tracked = [rel for rel in pinned if rel in index]
        if tracked:
            r = git("rm", "--cached", "--quiet", "--", *tracked)
            if r.returncode != 0:
                sys.exit(r.stderr)
    print(f"{len(pinned)} pinned on {remote()} ({sum(e['bytes'] for e in pinned.values()) / MIB:.1f} MiB), "
          f"{len(tracked)} of them taken out of git; tests/data.json and .gitignore are staged: commit them "
          "with the rest of the change")


def cmd_drop(args):
    with locked():
        files = load()
        rels = [rel_of(a) for a in args]
        for rel in rels:
            if rel not in files:
                sys.exit(f"{rel} is not pinned")
            del files[rel]
        save(files)
    print(f"{len(rels)} unpinned and no longer ignored; their files stay here and on Drive "
          "(git add a file that should be in git now)")


def cmd_verify(args):
    files = load()
    if not files:
        print("nothing pinned")
        return
    r = rclone("lsjson", remote() + "/blobs", "--files-only", "--hash", "--hash-type", "sha256", ok=(0, 3))
    there = {f["Name"]: f for f in json.loads(r.stdout or "[]")} if r.returncode == 0 else {}
    bad = []
    for rel, e in files.items():
        f = there.get(blob_name(rel, e["sha256"]))
        digest = (f or {}).get("Hashes", {}).get("sha256")
        if not f or f["Size"] != e["bytes"] or (digest and digest != e["sha256"]):
            bad.append(rel)
    print(f"{len(files) - len(bad)} of {len(files)} pinned blobs are on {remote()}, matching their hash")
    if r.returncode == 3:
        print(f"  {remote()}/blobs does not exist there: the wrong Google account, POCKET_DATA_REMOTE or OAuth client?")
    for rel in bad:
        print(f"  missing or not matching: {rel}")
    if bad:
        sys.exit(1)


def main():
    commands = {"status": cmd_status, "fetch": cmd_fetch, "add": cmd_add, "drop": cmd_drop, "verify": cmd_verify}
    if len(sys.argv) < 2 or sys.argv[1] not in commands:
        sys.exit(__doc__)
    commands[sys.argv[1]](sys.argv[2:])


if __name__ == "__main__":
    main()
