"""Builds third_party/rquickjs-sys-0.14.0/: rquickjs-sys 0.14.0 from crates.io with quickjs-ng PR
#1421 and the script host's patches P1 to P8 applied (docs/spec/architecture.md 7.5,
docs/spec/script-sandbox.md 4.2). `.cargo/config.toml`'s [patch.crates-io] points rquickjs at the
result, which is committed, so a fresh checkout builds without running this.

    python third_party/vendor.py           # rebuild the directory from the pins
    python third_party/vendor.py --check   # rebuild into a temporary directory and compare

The crate comes from cargo's cache or crates.io and every input is pinned by SHA-256 below; a
mismatch, a patch that does not apply cleanly or (with --check) any byte that differs from the
committed directory fails with exit 1. Standard library and git (as a plain patch tool) only.
Slice 1 keeps this script until `cargo xtask vendor` replaces it (script-sandbox.md 6).
"""

import hashlib
import io
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

CRATE = "rquickjs-sys"
VERSION = "0.14.0"
CRATE_SHA256 = "cee271d0eeba64f0915b846cb7ae02e16faf3dfdffdca91731101d9d30fe3423"

# (file under patches/, the directory inside the crate it applies to, SHA-256), in order.
PATCHES = [
    # quickjs-ng PR #1421 at head 0e7a5e08, as GitHub serves it (the debugger, charter 4.2.6).
    ("quickjs-ng-pr1421.diff", "quickjs",
     "b97233ca42edcd180f50df5a0b53bd33cc1264fa9aaf3b3680c42545fae8df66"),
    ("p1-interrupt-counter.diff", ".",
     "05b34022d1d3173c3db7d66db6806b7c5f208b95125dd7b7dcc68a1c0c091291"),
    ("p2-uncatchable-faults.diff", ".",
     "b1e0db37b41def39c1e53cb31c2a1ccbc7804f49a84722cc51466d2765b30dc8"),
    ("p3-constant-seeds.diff", ".",
     "77bd74c93a01f491d35e174deace6487a208690e7ae97aaeb2f41460fe9ec3b9"),
    ("p5-call-depth.diff", ".",
     "4ea9114b301cd971735ebb455a4208e060b128ac2905d9e9d263d894d2cdf53a"),
    ("p6-canonical-nan.diff", ".",
     "47f0f08139b5c0b38ddb2f099f776596b51e15320a678b378632414ca54cfca2"),
    ("p4-p7-build.diff", ".",
     "068b1d1be2119893259a3b7abff103f9d50666e92a2c55d3947ca847b905c4ed"),
    # JS_DiscardPendingJobs: a failed call's queued jobs are dropped (script-sandbox.md 6).
    ("p8-discard-jobs.diff", ".",
     "13cdf6064f4b4a2e2cb6749cc85b2121e02c9fe66e5d15121c22b67c5e5844be"),
]

HERE = Path(__file__).resolve().parent
DEST = HERE / f"{CRATE}-{VERSION}"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch_crate() -> bytes:
    cache = Path.home() / ".cargo" / "registry" / "cache"
    for hit in sorted(cache.glob(f"*/{CRATE}-{VERSION}.crate")):
        data = hit.read_bytes()
        if sha256(data) == CRATE_SHA256:
            return data
    url = f"https://static.crates.io/crates/{CRATE}/{CRATE}-{VERSION}.crate"
    print(f"downloading {url}")
    req = urllib.request.Request(url, headers={"User-Agent": "pocket3d-vendor"})
    with urllib.request.urlopen(req) as resp:
        return resp.read()


def apply(root: Path, patch: Path, directory: str) -> bool:
    # git apply as a plain patch tool: the ceiling stops it from finding an enclosing checkout
    # (whose root it would resolve paths against); autocrlf off keeps the crate's LF endings.
    env = dict(os.environ, GIT_CEILING_DIRECTORIES=str(root.parent))
    result = subprocess.run(
        ["git", "-c", "core.autocrlf=false", "apply", "--whitespace=nowarn", str(patch)],
        cwd=root / directory, env=env, capture_output=True, text=True,
    )
    if result.returncode != 0:
        sys.stdout.write(result.stdout + result.stderr)
    return result.returncode == 0


def build(into: Path) -> bool:
    """Extracts the pinned crate into `into` (which must not exist) and applies the patches."""
    data = fetch_crate()
    if sha256(data) != CRATE_SHA256:
        print(f"error: {CRATE}-{VERSION}.crate has SHA-256 {sha256(data)}, not {CRATE_SHA256}")
        return False
    with tempfile.TemporaryDirectory(dir=into.parent) as tmp:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
            tar.extractall(tmp, filter="data")
        (Path(tmp) / f"{CRATE}-{VERSION}").rename(into)
    for name, directory, pin in PATCHES:
        patch = HERE / "patches" / name
        got = sha256(patch.read_bytes())
        if got != pin:
            print(f"error: patches/{name} has SHA-256 {got}, not {pin}")
            return False
        if not apply(into, patch, directory):
            print(f"error: patches/{name} did not apply")
            return False
    return True


def files(root: Path) -> dict:
    return {p.relative_to(root).as_posix(): p.read_bytes()
            for p in sorted(root.rglob("*")) if p.is_file()}


def main() -> int:
    if "--check" in sys.argv:
        with tempfile.TemporaryDirectory() as tmp:
            fresh = Path(tmp) / DEST.name
            if not build(fresh):
                return 1
            want, have = files(fresh), files(DEST) if DEST.exists() else {}
        stale = sorted(k for k in want.keys() | have.keys() if want.get(k) != have.get(k))
        for k in stale[:20]:
            print(f"deps.vendor_stale: {DEST.name}/{k}")
        if stale:
            print(f"{len(stale)} files differ from what the pins and patches produce")
            return 1
        print(f"{DEST.name}: {len(want)} files as the pins and patches produce them")
        return 0
    if DEST.exists():
        shutil.rmtree(DEST)
    if not build(DEST):
        return 1
    # rquickjs-sys's build script watches environment variables only, and the archive's files
    # carry fixed timestamps: a fresh build.rs makes cargo run it again over the new sources.
    (DEST / "build.rs").touch()
    print(f"ready: {DEST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
