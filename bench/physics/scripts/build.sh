#!/bin/sh
# Builds everything docs/bench/physics.md measures. Third-party checkouts (read-only references):
#   git clone --depth 1 --branch v5.6.0 https://github.com/jrouwe/JoltPhysics.git ~/Reference/JoltPhysics-v5.6.0
#   git clone --depth 1 https://github.com/amerkoleci/joltc.git ~/Reference/joltc-amerkoleci
# T is the target directory for all build outputs (outside the repository); builds use 3 jobs.
set -e
T=${T:-/Users/qiulinfan/Desktop/aipocket2-wt/target-physics}
J=${J:-$HOME/Reference/JoltPhysics-v5.6.0}
HERE=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_BUILD_JOBS=3 NUM_JOBS=3 CMAKE_BUILD_PARALLEL_LEVEL=3
mkdir -p "$T"

# Jolt's own PerformanceTest, Distribution, with and without CROSS_PLATFORM_DETERMINISTIC.
for v in nondet det; do
  d=OFF; [ $v = det ] && d=ON
  cmake -S "$J/Build" -B "$J/Build/bench-$v" -G Ninja -DCMAKE_BUILD_TYPE=Distribution -DCMAKE_CXX_COMPILER=clang++ \
    -DCROSS_PLATFORM_DETERMINISTIC=$d -DTARGET_UNIT_TESTS=OFF -DTARGET_HELLO_WORLD=OFF -DTARGET_SAMPLES=OFF \
    -DTARGET_VIEWER=OFF -DTARGET_PERFORMANCE_TEST=ON -DJPH_USE_MTL=OFF -DJPH_USE_VK=OFF -DJPH_USE_DX12=OFF -DENABLE_INSTALL=OFF
  cmake --build "$J/Build/bench-$v" -j 3 --target PerformanceTest
done

# jolt_bench (bench/physics/jolt), both configurations; then the Ragdoll scene export for Rapier.
for v in nondet det; do
  d=OFF; [ $v = det ] && d=ON
  cmake -S "$HERE/jolt" -B "$T/jolt-$v" -G Ninja -DCMAKE_BUILD_TYPE=Distribution -DCMAKE_CXX_COMPILER=clang++ \
    -DJOLT_ROOT="$J" -DCROSS_PLATFORM_DETERMINISTIC=$d
  cmake --build "$T/jolt-$v" -j 3
done
mkdir -p "$T/data"
"$T/jolt-nondet/jolt_bench" --scene Ragdoll --export "$T/data/ragdoll.txt"
# jolt_bench as a wasm32-wasip1 module (deterministic, scalar: no Emscripten, so no wasm SIMD).
# Needs brew install llvm lld wasi-libc wasi-runtimes.
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
# wasm32-wasip1 builds for the native-vs-wasm comparison, with and without simd128.
CARGO_TARGET_DIR=$T/rapier-det cargo build --release --features det --target wasm32-wasip1
RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_DIR=$T/rapier-det-simd128 cargo build --release --features det --target wasm32-wasip1
CARGO_TARGET_DIR=$T/rapier-default cargo build --release --target wasm32-wasip1
RUSTFLAGS="-C target-feature=+simd128" CARGO_TARGET_DIR=$T/rapier-default-simd128 cargo build --release --target wasm32-wasip1

# The Rust-calls-Jolt prototype (joltc), both configurations.
cd "$HERE/jolt-ffi"
CARGO_TARGET_DIR=$T/jolt-ffi cargo build --release
CARGO_TARGET_DIR=$T/jolt-ffi-det cargo build --release --features det
# The same library as a Rust wasm32-unknown-unknown module (aipocket2's browser target), deterministic.
CARGO_TARGET_DIR=$T/jolt-ffi-wasm cargo rustc --lib --release --target wasm32-unknown-unknown --features det --crate-type cdylib
