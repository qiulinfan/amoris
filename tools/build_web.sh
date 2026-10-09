#!/bin/sh
# Builds the browser player into web/: the game module (crates/pocket-web, the worker's and the
# page's roles) into web/pkg, the viewport (tools/build_viewport.sh) into web/viewport/pkg, and the
# web package of each project given (default: samples/sailing samples/anim) into
# web/projects/<slug>/package.json beside the project's asset files, which the viewport fetches.
# Then: python3 -m http.server -d web 8090 and open http://127.0.0.1:8090/?package=projects/<slug>/package.json
set -e
cd "$(dirname "$0")/.."
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
# The C compiler is rquickjs-sys's build script's choice: POCKET_LLVM, else the pinned
# ~/.pocket-tools/llvm-23.1.2, else Homebrew's LLVM, so every build compiles QuickJS-ng the same way.
cargo build --profile web --target wasm32-unknown-unknown -p pocket-web
wasm-bindgen --target web --no-typescript --out-dir web/pkg \
  target/wasm32-unknown-unknown/web/pocket_web.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals web/pkg/pocket_web_bg.wasm -o web/pkg/pocket_web_bg.wasm
fi
[ "${SKIP_VIEWPORT:-}" = 1 ] || tools/build_viewport.sh
[ $# -gt 0 ] || set -- samples/sailing samples/anim
for project in "$@"; do
  slug=$(echo "$project" | tr -c 'A-Za-z0-9\n' '-' | sed 's/^-*//; s/-*$//')
  out="web/projects/$slug"
  mkdir -p "$out"
  cargo run -q -p pocket-web --example pack --release -- "$project" "$out/package.json"
  echo
  # The project's assets (everything but its sources and tooling) at the same relative paths.
  (cd "$project" && find . -type f \( -name '*.glb' -o -name '*.gltf' -o -name '*.bin' -o -name '*.png' \
    -o -name '*.jpg' -o -name '*.ktx2' -o -name '*.ply' -o -name '*.spz' -o -name '*.wav' -o -name '*.ogg' \
    -o -name '*.mp3' -o -name '*.flac' -o -name '*.ttf' \) -not -path './.pocket/*') |
    while read -r f; do mkdir -p "$out/$(dirname "$f")"; cp "$project/$f" "$out/$f"; done
done
