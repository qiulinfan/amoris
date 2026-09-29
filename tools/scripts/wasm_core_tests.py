#!/usr/bin/env python3
"""core_tests compiled to WebAssembly and run under node: the reproducible math's pinned hash
(`[repro]`) is then checked on the web build's libm and compiler as well as the native one, which is
what lets a browser and a native peer play one lockstep game (docs/design/networking.md).

Needs the Emscripten SDK (POCKET_EMSDK, EMSDK or ~/.pocket-tools/emsdk, as `pocket` finds it) and node:
    python3 tools/scripts/wasm_core_tests.py [catch2 arguments, "[repro]" by default]
"""
import glob
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "build", "wasm-core-tests")


def emsdk():
    for c in [os.environ.get("POCKET_EMSDK"), os.environ.get("EMSDK"), os.path.expanduser("~/.pocket-tools/emsdk")]:
        if c and os.path.isfile(os.path.join(c, "upstream", "emscripten", "em++")):
            return c
    sys.exit("no Emscripten SDK: set POCKET_EMSDK or EMSDK, or install it at ~/.pocket-tools/emsdk")


def main():
    sdk = emsdk()
    os.makedirs(OUT, exist_ok=True)
    em = os.path.join(sdk, "upstream", "emscripten", "em++")
    env = dict(os.environ, EMSDK=sdk, EM_CONFIG=os.path.join(sdk, ".emscripten"))
    sources = [os.path.join(ROOT, "tests", "core_tests", "core_tests.cpp"), os.path.join(ROOT, "third_party", "catch2", "src", "catch_amalgamated.cpp")]
    sources += sorted(glob.glob(os.path.join(ROOT, "engine", "core", "src", "*.cpp")))
    js = os.path.join(OUT, "core_tests.js")
    # The same floating-point flags as pocket.toml's wasm configs.
    subprocess.run([em, "-std=c++2c", "-O2", "-ffp-contract=off", "-fexceptions",
                    "-I" + os.path.join(ROOT, "engine", "core", "include"), "-I" + os.path.join(ROOT, "third_party", "json", "include"),
                    "-I" + os.path.join(ROOT, "third_party", "catch2", "include"), "-I" + os.path.join(ROOT, "third_party", "catch2", "src"),
                    *sources, "-sNODERAWFS=1", "-sALLOW_MEMORY_GROWTH=1", "-o", js], check=True, env=env)
    args = sys.argv[1:] or ["[repro]"]
    sys.exit(subprocess.run(["node", js, *args], cwd=ROOT).returncode)


if __name__ == "__main__":
    main()
