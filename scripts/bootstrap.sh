#!/bin/sh
# Bootstrap a fresh checkout on macOS: build the `pocket` tool and install dependencies.
# Requires: rustup (cargo), the Xcode Command Line Tools, cmake and ninja on PATH.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d /Library/Developer/CommandLineTools ]; then
    export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi
cargo build --release --manifest-path tools/pocket/Cargo.toml
mkdir -p .pocket
cp tools/pocket/target/release/pocket .pocket/pocket
./.pocket/pocket setup
./.pocket/pocket doctor
echo "ok: use ./.pocket/pocket build | run hello | test"
