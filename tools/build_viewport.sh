#!/bin/sh
# Builds the browser viewport (crates/pocket-viewport) into web/viewport/pkg: cargo for
# wasm32-unknown-unknown, wasm-bindgen for the JS glue, wasm-opt for size and speed (skipped when
# it is not installed). The viewport's importer makes levels of detail with meshoptimizer's C++
# (docs/spec/lod.md), which needs clang with the wasm32 backend and llvm-ar.
set -e
cd "$(dirname "$0")/.."
# rustup's env file exists on macOS and Linux, not on Windows; `.` of a missing file would end a
# POSIX shell even behind `|| true`.
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
# The C and C++ compilers for wasm32, unless the caller named them: POCKET_LLVM, else the pinned
# ~/.pocket-tools/llvm-23.1.2, else Homebrew's LLVM (as rquickjs-sys's build script chooses for
# QuickJS-ng). meshoptimizer's build script also runs `llvm-ar` from PATH on a Windows host.
if [ -z "${CC_wasm32_unknown_unknown:-}" ]; then
  for llvm in "${POCKET_LLVM:-}" "$HOME/.pocket-tools/llvm-23.1.2" /opt/homebrew/opt/llvm /usr/local/opt/llvm; do
    [ -n "$llvm" ] || continue
    exe=""
    [ -f "$llvm/bin/clang.exe" ] && exe=".exe"
    if [ -f "$llvm/bin/clang$exe" ]; then
      export CC_wasm32_unknown_unknown="$llvm/bin/clang$exe"
      export CXX_wasm32_unknown_unknown="$llvm/bin/clang++$exe"
      export AR_wasm32_unknown_unknown="$llvm/bin/llvm-ar$exe"
      export PATH="$llvm/bin:$PATH"
      break
    fi
  done
fi
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
