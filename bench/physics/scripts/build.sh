#!/bin/sh
# Builds everything docs/bench/physics.md measures. Third-party checkouts (read-only references;
# nothing is written into them):
#   git clone --depth 1 --branch v5.6.0 https://github.com/jrouwe/JoltPhysics.git ~/Reference/JoltPhysics-v5.6.0
#   git clone --depth 1 https://github.com/amerkoleci/joltc.git ~/Reference/joltc-amerkoleci
# Needs CMake, Ninja, Apple clang, and for WebAssembly brew install llvm lld wasi-libc wasi-runtimes
# binaryen. T is the target directory for all build outputs (default target/physics-bench, or
# $CARGO_TARGET_DIR/physics-bench); builds use 3 jobs.
#
# Expected warnings: every cargo command here prints "patch `rquickjs-sys ...` was not used in the
# crate graph", because the bench packages sit under the repository and inherit its
# .cargo/config.toml, whose [patch.crates-io] only the engine's crate graph uses. It is harmless.
set -e
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
J=${J:-$HOME/Reference/JoltPhysics-v5.6.0}
HERE=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_BUILD_JOBS=3 NUM_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3
mkdir -p "$T" "$T/wasm"
# wasm-opt as tools/build_web.sh runs it on the engine's browser module.
WASM_OPT="wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals"

# Jolt's own PerformanceTest, Distribution, with and without CROSS_PLATFORM_DETERMINISTIC. It finds
# its Assets/ from the working directory, so scripts/perftest.sh runs it from $J.
for v in nondet det; do
  d=OFF; [ $v = det ] && d=ON
  cmake -S "$J/Build" -B "$T/jolt-perftest-$v" -G Ninja -DCMAKE_BUILD_TYPE=Distribution -DCMAKE_CXX_COMPILER=clang++ \
    -DCROSS_PLATFORM_DETERMINISTIC=$d -DTARGET_UNIT_TESTS=OFF -DTARGET_HELLO_WORLD=OFF -DTARGET_SAMPLES=OFF \
    -DTARGET_VIEWER=OFF -DTARGET_PERFORMANCE_TEST=ON -DJPH_USE_MTL=OFF -DJPH_USE_VK=OFF -DJPH_USE_DX12=OFF -DENABLE_INSTALL=OFF
  cmake --build "$T/jolt-perftest-$v" -j 3 --target PerformanceTest
done

# jolt_bench (bench/physics/jolt), both configurations; then the Ragdoll scene export for Rapier,
# once (other runs may be reading it).
for v in nondet det; do
  d=OFF; [ $v = det ] && d=ON
  cmake -S "$HERE/jolt" -B "$T/jolt-$v" -G Ninja -DCMAKE_BUILD_TYPE=Distribution -DCMAKE_CXX_COMPILER=clang++ \
    -DJOLT_ROOT="$J" -DCROSS_PLATFORM_DETERMINISTIC=$d
  cmake --build "$T/jolt-$v" -j 3
done
mkdir -p "$T/data"
if [ ! -s "$T/data/ragdoll.txt" ] || [ ! -s "$T/data/ragdoll.txt.tri" ]; then
  "$T/jolt-nondet/jolt_bench" --scene Ragdoll --export "$T/data/ragdoll.txt.new"
  mv "$T/data/ragdoll.txt.new.tri" "$T/data/ragdoll.txt.tri"
  mv "$T/data/ragdoll.txt.new" "$T/data/ragdoll.txt"
fi
# jolt_bench as a wasm32-wasip1 module (deterministic, scalar: no Emscripten, so no wasm SIMD). Jolt
# itself without LTO, the harness file with thin LTO; clang runs wasm-opt -O3 on the module.
cmake -S "$HERE/jolt" -B "$T/jolt-wasi-det" -G Ninja -DCMAKE_BUILD_TYPE=Distribution -DJOLT_ROOT="$J" \
  -DCMAKE_TOOLCHAIN_FILE="$HERE/jolt/wasi/wasm32-wasip1.cmake" -DCROSS_PLATFORM_DETERMINISTIC=ON \
  -DINTERPROCEDURAL_OPTIMIZATION=OFF -DUSE_WASM_SIMD=OFF
cmake --build "$T/jolt-wasi-det" -j 3

# The Rapier benchmark in every configuration (each in its own target directory).
cd "$HERE/rapier"
CARGO_TARGET_DIR=$T/rapier-det cargo build --release --features det
CARGO_TARGET_DIR=$T/rapier-det cargo build --profile release-lto --features det
CARGO_TARGET_DIR=$T/rapier-default cargo build --release
CARGO_TARGET_DIR=$T/rapier-simd8 cargo build --release --features simd8
CARGO_TARGET_DIR=$T/rapier-parallel cargo build --release --features parallel
CARGO_TARGET_DIR=$T/rapier-parallel-simd8 cargo build --release --features parallel,simd8
CARGO_TARGET_DIR=$T/rapier-det-parallel cargo build --release --features det,parallel
# The shipped configuration with serde snapshots, for scripts/fork.sh.
CARGO_TARGET_DIR=$T/rapier-det-serde cargo build --release --features det,serde
# det + simd8 must fail to compile; keep the message.
mkdir -p "$T/det"
CARGO_TARGET_DIR=$T/rapier-det-simd8 cargo build --release --features det,simd8 2>&1 | grep -E '^error' -A6 | head -12 > "$T/det/det-simd8-build.txt" || true
# wasm32-wasip1 builds (WASI, for the benchmark's file reading and output; the engine ships
# wasm32-unknown-unknown, the same LLVM backend). Release profile, with and without simd128, for the
# determinism checks; the det. build again in the engine's web profile (fat LTO, one codegen unit,
# panic=abort) followed by wasm-opt -O3, as tools/build_web.sh ships it, for the timings.
CARGO_TARGET_DIR=$T/rapier-det cargo build --release --features det --target wasm32-wasip1
RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_DIR=$T/rapier-det-simd128 cargo build --release --features det --target wasm32-wasip1
CARGO_TARGET_DIR=$T/rapier-default cargo build --release --target wasm32-wasip1
RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_DIR=$T/rapier-default-simd128 cargo build --release --target wasm32-wasip1
CARGO_TARGET_DIR=$T/rapier-det cargo build --profile web --features det --target wasm32-wasip1
RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_DIR=$T/rapier-det-simd128 cargo build --profile web --features det --target wasm32-wasip1
$WASM_OPT "$T/rapier-det/wasm32-wasip1/web/physics-bench.wasm" -o "$T/wasm/rapier-det.wasm"
$WASM_OPT --enable-simd "$T/rapier-det-simd128/wasm32-wasip1/web/physics-bench.wasm" -o "$T/wasm/rapier-det-simd128.wasm"

# The Rust-calls-Jolt prototype (joltc), both configurations.
cd "$HERE/jolt-ffi"
CARGO_TARGET_DIR=$T/jolt-ffi cargo build --release
CARGO_TARGET_DIR=$T/jolt-ffi-det cargo build --release --features det
# The same library as a Rust wasm32-unknown-unknown module (Amoris's browser target), deterministic,
# in the web profile and through wasm-opt like the engine's module (but with plain exports, not
# wasm-bindgen).
CARGO_TARGET_DIR=$T/jolt-ffi-wasm cargo rustc --lib --profile web --target wasm32-unknown-unknown --features det --crate-type cdylib
$WASM_OPT "$T/jolt-ffi-wasm/wasm32-unknown-unknown/web/jolt_ffi_proto.wasm" -o "$T/wasm/jolt-ffi-det.wasm"
