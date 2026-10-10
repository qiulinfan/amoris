#!/bin/sh
# The timing matrix behind docs/bench/physics.md. Every run prints one JSON line; this script tags it
# with the repetition and the machine's 1-minute load average and appends it to $RAW/bench.jsonl.
# REPS repetitions (default 3) of every configuration, interleaved so that background load affects
# all configurations alike; scripts/summarize.py keeps the fastest repetition of each. APPEND=1 adds
# repetitions FIRST_REP.. to an existing bench.jsonl instead of starting it anew. RAW defaults to
# $T/raw; results stay in ignored local output (set RAW to choose another local directory).
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
J=${J:-$HOME/Reference/JoltPhysics-v5.6.0}
RAW=${RAW:-$T/raw}
REPS=${REPS:-3}
STEPS=${STEPS:-500}
mkdir -p "$RAW"
OUTF=$RAW/bench.jsonl
[ -n "$APPEND" ] || : > "$OUTF"

run() { # tag command...
  tag=$1; shift
  load=$(sysctl -n vm.loadavg | awk '{print $2}')
  line=$(perl -e 'alarm 1800; exec @ARGV' "$@")
  [ -n "$line" ] || { echo "FAILED: $tag $*" >&2; return; }
  echo "$line" | sed "s/^{/{\"run\":\"$tag\",\"rep\":$rep,\"load1\":$load,/" >> "$OUTF"
  echo "$tag rep $rep: $(echo "$line" | sed 's/.*"mean_ms":\([0-9.]*\).*/\1/') ms"
}

SCENES="Pyramid Pyramid30 ConvexVsMesh Raycast Ragdoll"
rep=${FIRST_REP:-1}
last=$((rep + REPS - 1))
while [ $rep -le $last ]; do
  for s in $SCENES; do
    for v in nondet det; do
      for th in 1 4 10; do
        run "jolt-$v-t$th" "$T/jolt-$v/jolt_bench" --scene $s --threads $th --steps $STEPS
      done
    done
    case $s in Pyramid*)
      # Jolt at Rapier-like iteration counts (4 velocity, 1 position; ConvexVsMesh's own setting).
      run "jolt-nondet-t1-vel4pos1" "$T/jolt-nondet/jolt_bench" --scene $s --threads 1 --steps $STEPS --vel 4 --pos 1 ;;
    esac
    run "rapier-det-t1" "$T/rapier-det/release/physics-bench" --scene $s --steps $STEPS --data "$T/data"
    run "rapier-det-lto-t1" "$T/rapier-det/release-lto/physics-bench" --scene $s --steps $STEPS --data "$T/data"
    run "rapier-default-t1" "$T/rapier-default/release/physics-bench" --scene $s --steps $STEPS --data "$T/data"
    run "rapier-simd8-t1" "$T/rapier-simd8/release/physics-bench" --scene $s --steps $STEPS --data "$T/data"
    for th in 1 4 10; do
      run "rapier-parallel-t$th" "$T/rapier-parallel/release/physics-bench" --scene $s --threads $th --steps $STEPS --data "$T/data"
      run "rapier-parallel-simd8-t$th" "$T/rapier-parallel-simd8/release/physics-bench" --scene $s --threads $th --steps $STEPS --data "$T/data"
      run "rapier-det-parallel-t$th" "$T/rapier-det-parallel/release/physics-bench" --scene $s --threads $th --steps $STEPS --data "$T/data"
    done
    case $s in Pyramid*)
      run "rapier-det-t1-iters10" "$T/rapier-det/release/physics-bench" --scene $s --steps $STEPS --iters 10 ;;
    esac
  done
  # Rust calling Jolt through joltc, against the C++ harness on the same scene and rays.
  for v in "" -det; do
    run "joltc$v-t1" "$T/jolt-ffi$v/release/jolt-ffi-proto" --steps $STEPS
    run "joltc$v-t1-sync" "$T/jolt-ffi$v/release/jolt-ffi-proto" --steps $STEPS --sync
    run "joltc$v-t1-rays" "$T/jolt-ffi$v/release/jolt-ffi-proto" --steps $STEPS --rays
  done
  run "jolt-nondet-t1-rays" "$T/jolt-nondet/jolt_bench" --scene Pyramid --threads 1 --steps $STEPS --rays
  run "jolt-det-t1-rays" "$T/jolt-det/jolt_bench" --scene Pyramid --threads 1 --steps $STEPS --rays
  rep=$((rep + 1))
done
