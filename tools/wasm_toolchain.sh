# Sourced by the web game and viewport builders, after changing to the repository root.
# Both need wasm C/C++ now: QuickJS-ng, and the renderer's meshoptimizer dependency.
if [ -z "${CC_wasm32_unknown_unknown:-}" ]; then
  for llvm in "${AMORIS_LLVM:-}" "$HOME/.amoris-tools/llvm-23.1.2" /opt/homebrew/opt/llvm /usr/local/opt/llvm; do
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
