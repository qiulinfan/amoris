"""Fetch LLVM's Windows release (clang for the MSVC ABI, lld, llvm-lib, llvm-rc) and unpack it to
~/.pocket-tools/llvm-<version>, where pocket finds it (tools/pocket/src/toolchain.rs). No installer
and no administrator rights; docs/development.md, Windows.

    python tools/scripts/dev/fetch_llvm.py [version]     # default 23.1.2
"""
import os
import shutil
import sys
import tarfile
import urllib.request
from compression import zstd   # Python 3.14

version = sys.argv[1] if len(sys.argv) > 1 else "23.1.2"
name = f"clang+llvm-{version}-x86_64-pc-windows-msvc"
url = f"https://github.com/llvm/llvm-project/releases/download/llvmorg-{version}/{name.replace('+', '%2B')}.tar.zst"
tools = os.path.join(os.path.expanduser("~"), ".pocket-tools")
dest = os.path.join(tools, f"llvm-{version}")
if os.path.isfile(os.path.join(dest, "bin", "clang++.exe")):
    print("already there:", dest)
    sys.exit(0)
os.makedirs(tools, exist_ok=True)
archive = os.path.join(tools, name + ".tar.zst")
if not os.path.isfile(archive):
    print("fetching", url)
    with urllib.request.urlopen(url) as r, open(archive + ".part", "wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    os.replace(archive + ".part", archive)
print("unpacking to", dest)
# The archive's frames need a 2 GiB window, above the decoder's default limit.
with zstd.open(archive, "rb", options={zstd.DecompressionParameter.window_log_max: 31}) as f:
    with tarfile.open(fileobj=f, mode="r|") as t:
        t.extractall(tools, filter="data")
os.replace(os.path.join(tools, name), dest)
os.remove(archive)
print("ok:", os.path.join(dest, "bin", "clang++.exe"))
