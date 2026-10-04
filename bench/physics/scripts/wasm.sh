#!/bin/sh
# WebAssembly timings behind docs/bench/physics.md: the deterministic builds of both engines as
# WebAssembly modules under Node (V8), single thread, against their native runs.
# - Rapier: wasm32-wasip1 under Node's WASI, built as tools/build_web.sh ships the engine (web
#   profile, then wasm-opt -O3), scalar and with simd128; plus the release-profile scalar module
#   without wasm-opt, to show what the profile changes.
# - Jolt: jolt_bench for wasm32-wasip1 (scalar: its wasm SIMD path needs Emscripten's SSE headers),
#   and for the Pyramid also Jolt inside a Rust wasm32-unknown-unknown module through joltc
#   (jolt-ffi in the web profile and through wasm-opt, run_jolt_wasm.mjs).
# Scenes: Pyramid, ConvexVsMesh, Ragdoll and RagdollNoSleep (sleeping off in both engines, the fair
# ragdoll comparison). Writes one tagged JSON line per run to $RAW/wasm.jsonl (RAW defaults to
# $T/raw; the committed results are docs/bench/physics/).
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
RAW=${RAW:-$T/raw}
HERE=$(cd "$(dirname "$0")" && pwd)
REPS=${REPS:-2}
STEPS=${STEPS:-300}
mkdir -p "$RAW"
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
  for s in Pyramid: ConvexVsMesh: Ragdoll: Ragdoll:--no-sleep; do
    scene=${s%%:*}; extra=${s#*:}
    run rapier-det-native "$T/rapier-det/release/physics-bench" --scene $scene --steps $STEPS --data "$T/data" $extra
    run rapier-det-wasm-web node --no-warnings "$HERE/run_wasi.mjs" "$T/wasm/rapier-det.wasm" --scene $scene --steps $STEPS --data "$T/data" $extra
    run rapier-det-wasm-web-simd128 node --no-warnings "$HERE/run_wasi.mjs" "$T/wasm/rapier-det-simd128.wasm" --scene $scene --steps $STEPS --data "$T/data" $extra
    run rapier-det-wasm-release node --no-warnings "$HERE/run_wasi.mjs" "$T/rapier-det/wasm32-wasip1/release/physics-bench.wasm" --scene $scene --steps $STEPS --data "$T/data" $extra
    run jolt-det-native "$T/jolt-det/jolt_bench" --scene $scene --steps $STEPS $extra
    run jolt-det-wasm node --no-warnings "$HERE/run_wasi.mjs" "$T/jolt-wasi-det/jolt_bench" --scene $scene --steps $STEPS $extra
    [ "$s" = Pyramid: ] && run jolt-det-rust-wasm node --no-warnings "$HERE/run_jolt_wasm.mjs" "$T/wasm/jolt-ffi-det.wasm" --steps $STEPS
  done
  rep=$((rep + 1))
done
