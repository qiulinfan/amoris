#!/bin/sh
# Builds the browser viewport (crates/pocket-viewport) into web/viewport/pkg: cargo for
# wasm32-unknown-unknown, wasm-bindgen for the JS glue, wasm-opt for size and speed.
set -e
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env" 2>/dev/null || true
export POCKET_LLVM="${POCKET_LLVM:-/opt/homebrew/opt/llvm}"
cargo build --profile web --target wasm32-unknown-unknown -p pocket-viewport
OUT=web/viewport/pkg
mkdir -p "$OUT"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" \
  target/wasm32-unknown-unknown/web/pocket_viewport.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals "$OUT/pocket_viewport_bg.wasm" -o "$OUT/pocket_viewport_bg.wasm"
fi
ls -la "$OUT"
gzip -9 -c "$OUT/pocket_viewport_bg.wasm" | wc -c | awk '{printf "gzip: %.2f MB\n", $1/1048576}'
