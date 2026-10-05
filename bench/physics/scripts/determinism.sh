#!/bin/sh
# Determinism checks behind docs/bench/physics.md ("Determinism"). Prints a report on stdout.
#
# Rapier: per-step body-hash chains (FNV-1a over every body's position and rotation bits after each
# step) compared between native aarch64 and wasm32-wasip1 under Node, for each build configuration
# (the det. build also in the web profile through wasm-opt, as tools/build_web.sh ships it), and across rayon thread counts for `det,parallel`. Jolt: PerformanceTest against the hashes Jolt's
# CI records for its cross-platform-deterministic build (.github/workflows/determinism_check.yml),
# and jolt_bench's end hash across thread counts.
#
# Needs the builds of bench/physics/scripts/build.sh. T is the target directory.
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
J=${J:-$HOME/Reference/JoltPhysics-v5.6.0}
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$T/det
mkdir -p "$OUT"
watchdog() { perl -e 'alarm 1800; exec @ARGV' "$@"; }

chain() { # name scene steps command...
  name=$1; scene=$2; steps=$3; shift 3
  watchdog "$@" --scene "$scene" --steps "$steps" --data "$T/data" --hash-chain "$OUT/$scene-$name.txt" > "$OUT/$scene-$name.json"
}
compare() { # scene reference other
  if cmp -s "$OUT/$1-$2.txt" "$OUT/$1-$3.txt"; then
    echo "  $1: $3 == $2 (all $(wc -l < "$OUT/$1-$2.txt" | tr -d ' ') steps)"
  else
    first=$(cmp "$OUT/$1-$2.txt" "$OUT/$1-$3.txt" | awk '{print $NF}')
    echo "  $1: $3 != $2 (first differing step: line $first)"
  fi
}

echo "== Rapier 0.36.0 native aarch64 vs wasm32-wasip1 (Node $(node --version))"
for scene in Pyramid:300 ConvexVsMesh:300 Ragdoll:100; do
  s=${scene%%:*}; n=${scene#*:}
  chain det-native "$s" "$n" "$T/rapier-det/release/physics-bench"
  chain det-wasm "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-det/wasm32-wasip1/release/physics-bench.wasm"
  chain det-wasm-simd128 "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-det-simd128/wasm32-wasip1/release/physics-bench.wasm"
  chain det-wasm-web "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/wasm/rapier-det.wasm"
  chain det-wasm-web-simd128 "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/wasm/rapier-det-simd128.wasm"
  chain det-lto "$s" "$n" "$T/rapier-det/release-lto/physics-bench"
  chain default-native "$s" "$n" "$T/rapier-default/release/physics-bench"
  chain default-wasm "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-default/wasm32-wasip1/release/physics-bench.wasm"
  chain default-wasm-simd128 "$s" "$n" node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-default-simd128/wasm32-wasip1/release/physics-bench.wasm"
  chain simd8-native "$s" "$n" "$T/rapier-simd8/release/physics-bench"
  for th in 1 4 10; do
    chain det-parallel-t$th "$s" "$n" "$T/rapier-det-parallel/release/physics-bench" --threads $th
  done
  echo " enhanced-determinism:"
  compare "$s" det-native det-wasm
  compare "$s" det-native det-wasm-simd128
  compare "$s" det-native det-wasm-web
  compare "$s" det-native det-wasm-web-simd128
  compare "$s" det-native det-lto
  echo " enhanced-determinism + parallel:"
  for th in 1 4 10; do compare "$s" det-native det-parallel-t$th; done
  echo " without enhanced-determinism:"
  compare "$s" default-native default-wasm
  compare "$s" default-native default-wasm-simd128
  compare "$s" default-wasm default-wasm-simd128
  compare "$s" default-native simd8-native
  compare "$s" det-native default-native
done
echo " enhanced-determinism + simd8:"
echo "  $(head -1 "$OUT/det-simd8-build.txt" 2>/dev/null || echo 'not built')"

echo
echo "== Jolt v5.6.0 PerformanceTest vs Jolt CI's cross-platform hashes (-q=LinearCast, 500 steps)"
ci() { grep "^  $1:" "$J/.github/workflows/determinism_check.yml" | head -1 | sed "s/.*'\(.*\)'/\1/"; }
for v in det nondet; do
  for th in max 1; do
    for s in ConvexVsMesh:CONVEX_VS_MESH_HASH Ragdoll:RAGDOLL_HASH Pyramid:PYRAMID_HASH HighSpeed:HIGH_SPEED_HASH; do
      scene=${s%%:*}; key=${s#*:}; want=$(ci $key)
      got=$(cd "$J" && watchdog "$T/jolt-perftest-$v/PerformanceTest" -q=LinearCast -t=$th -s=$scene | tail -1 | awk -F', ' '{print $4}')
      [ "$got" = "$want" ] && r=match || r=differs
      echo "  CROSS_PLATFORM_DETERMINISTIC=$v threads=$th $scene: $got (CI $want) $r"
    done
  done
done

echo
echo "== Jolt v5.6.0 jolt_bench end hash across thread counts (Discrete, 300 steps)"
for v in det nondet; do
  for s in Pyramid ConvexVsMesh Ragdoll; do
    line="  $v $s:"
    for th in 1 4 10; do
      h=$(watchdog "$T/jolt-$v/jolt_bench" --scene $s --threads $th --steps 300 | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
      line="$line t$th=$h"
    done
    echo "$line"
  done
done

echo
echo "== Jolt v5.6.0 CROSS_PLATFORM_DETERMINISTIC: native aarch64 vs wasm32-wasip1 under Node (end hash, 300 steps)"
for s in Pyramid ConvexVsMesh Ragdoll; do
  n=$(watchdog "$T/jolt-det/jolt_bench" --scene $s --steps 300 | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
  w=$(watchdog node --no-warnings "$HERE/run_wasi.mjs" "$T/jolt-wasi-det/jolt_bench" --scene $s --steps 300 2>/dev/null | tail -1 | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
  [ "$n" = "$w" ] && r=identical || r=differs
  echo "  $s: native $n, wasm32 $w: $r"
done

echo
echo "== Jolt v5.6.0 CROSS_PLATFORM_DETERMINISTIC inside a Rust wasm32-unknown-unknown module (jolt-ffi through joltc, Node $(node --version), no WASI runtime), Pyramid end hash"
for n in 300 500; do
  c=$(watchdog "$T/jolt-det/jolt_bench" --scene Pyramid --steps $n | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
  f=$(watchdog "$T/jolt-ffi-det/release/jolt-ffi-proto" --steps $n | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
  w=$(watchdog node --no-warnings "$HERE/run_jolt_wasm.mjs" "$T/wasm/jolt-ffi-det.wasm" --steps $n | sed 's/.*"hash":"\([0-9a-fx]*\)".*/\1/')
  [ "$c" = "$w" ] && [ "$f" = "$w" ] && r=identical || r=differs
  echo "  $n steps: C++ native $c, Rust native via joltc $f, Rust wasm32-unknown-unknown via joltc $w: $r"
done
