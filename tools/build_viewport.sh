#!/bin/sh
# Builds the browser viewport (crates/pocket-viewport) into web/viewport/pkg: cargo for
# wasm32-unknown-unknown, wasm-bindgen for the JS glue, wasm-opt for size and speed (skipped when
# it is not installed). The viewport needs no C compiler (only the game module, pocket-web, does).
set -e
cd "$(dirname "$0")/.."
# rustup's env file exists on macOS and Linux, not on Windows; `.` of a missing file would end a
# POSIX shell even behind `|| true`.
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
# The C compiler is rquickjs-sys's build script's choice: POCKET_LLVM, else the pinned
# ~/.pocket-tools/llvm-23.1.2, else Homebrew's LLVM, so every build compiles QuickJS-ng the same way.
cargo build --profile web --target wasm32-unknown-unknown -p pocket-viewport
OUT=web/viewport/pkg
mkdir -p "$OUT"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" \
  target/wasm32-unknown-unknown/web/pocket_viewport.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals "$OUT/pocket_viewport_bg.wasm" -o "$OUT/pocket_viewport_bg.wasm"
else
  echo "wasm-opt not found: the package is not optimized"
fi
ls -la "$OUT"
gzip -9 -c "$OUT/pocket_viewport_bg.wasm" | wc -c | awk '{printf "gzip: %d bytes\n", $1}'
