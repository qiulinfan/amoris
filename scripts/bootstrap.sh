#!/bin/sh
# Bootstrap a fresh checkout: build the `pocket` tool and install dependencies.
# Requires rustup (cargo), cmake and ninja on PATH, and a C++ toolchain: the Xcode Command Line
# Tools on macOS, clang and lld on Linux, and on Windows (Git Bash) Visual Studio's C++ workload
# with a Windows SDK plus LLVM (on PATH or unpacked under ~/.pocket-tools/llvm-<version>);
# docs/development.md, Setting up a machine.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Library/Developer/CommandLineTools ]; then
    export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
exe=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) exe=".exe" ;; esac
cargo build --release --manifest-path tools/pocket/Cargo.toml
mkdir -p .pocket
# Removed before the copy: macOS kills a signed binary overwritten in place, and Windows cannot
# overwrite one that is running (moved aside, it can be replaced).
if [ -n "$exe" ] && [ -f ".pocket/pocket$exe" ]; then mv -f ".pocket/pocket$exe" ".pocket/pocket.old$exe" 2>/dev/null || true; fi
rm -f ".pocket/pocket$exe" ".pocket/pocket.old$exe" 2>/dev/null || true
cp "tools/pocket/target/release/pocket$exe" ".pocket/pocket$exe"
./.pocket/pocket setup
./.pocket/pocket doctor
echo "ok: use ./.pocket/pocket build | run hello | test"
