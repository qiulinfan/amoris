#!/bin/sh
# WebAssembly timings behind docs/bench/physics.md: the deterministic builds of both engines as
# wasm32-wasip1 modules under Node's WASI (V8), single thread, against their native runs.
# Rapier with and without simd128; Jolt scalar only (its wasm SIMD path needs Emscripten's SSE
# headers); for the Pyramid also Jolt inside a Rust wasm32-unknown-unknown module through joltc
# (jolt-ffi, run_jolt_wasm.mjs). Writes one tagged JSON line per run to $RAW/wasm.jsonl.
T=${T:-/Users/qiulinfan/Desktop/aipocket2-wt/target-physics}
RAW=${RAW:-$(cd "$(dirname "$0")/../../.." && pwd)/docs/bench/physics}
HERE=$(cd "$(dirname "$0")" && pwd)
REPS=${REPS:-2}
STEPS=${STEPS:-300}
OUTF=$RAW/wasm.jsonl
[ -n "$APPEND" ] || : > "$OUTF"   # APPEND=1 FIRST_REP=n adds repetitions n.. instead
run() { # tag command...
  tag=$1; shift
  load=$(sysctl -n vm.loadavg | awk '{print $2}')
  line=$(perl -e 'alarm 1800; exec @ARGV' "$@" 2>/dev/null | tail -1)
  [ -n "$line" ] || { echo "FAILED: $tag" >&2; return; }
  echo "$line" | sed "s/^{/{\"run\":\"$tag\",\"rep\":$rep,\"load1\":$load,/" >> "$OUTF"
  echo "$tag $(echo "$line" | sed 's/.*"scene":"\([A-Za-z0-9]*\)".*"mean_ms":\([0-9.]*\).*/\1 \2/') ms"
}
rep=${FIRST_REP:-1}
last=$((rep + REPS - 1))
while [ $rep -le $last ]; do
  for s in Pyramid ConvexVsMesh Ragdoll; do
    run rapier-det-native "$T/rapier-det/release/physics-bench" --scene $s --steps $STEPS --data "$T/data"
    run rapier-det-wasm node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-det/wasm32-wasip1/release/physics-bench.wasm" --scene $s --steps $STEPS --data "$T/data"
    run rapier-det-wasm-simd128 node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-det-simd128/wasm32-wasip1/release/physics-bench.wasm" --scene $s --steps $STEPS --data "$T/data"
    run jolt-det-native "$T/jolt-det/jolt_bench" --scene $s --steps $STEPS
    run jolt-det-wasm node --no-warnings "$HERE/run_wasi.mjs" "$T/jolt-wasi-det/jolt_bench" --scene $s --steps $STEPS
    [ $s = Pyramid ] && run jolt-det-rust-wasm node --no-warnings "$HERE/run_jolt_wasm.mjs" "$T/jolt-ffi-wasm/wasm32-unknown-unknown/release/jolt_ffi_proto.wasm" --steps $STEPS
  done
  rep=$((rep + 1))
done
