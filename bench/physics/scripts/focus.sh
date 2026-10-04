#!/bin/sh
# Extra repetitions of the configurations the recommendation rests on (docs/bench/physics.md), for a
# machine shared with other builds: the fastest repetition of a configuration is the least disturbed
# one, and more repetitions make an undisturbed one likelier. Appends to $RAW/bench.jsonl in
# bench.sh's format, repetitions FIRST_REP.. (default 4). RAW defaults to $T/raw, as in bench.sh.
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
RAW=${RAW:-$T/raw}
REPS=${REPS:-5}
STEPS=${STEPS:-500}
mkdir -p "$RAW"
OUTF=$RAW/bench.jsonl

run() { # tag command...
  tag=$1; shift
  load=$(sysctl -n vm.loadavg | awk '{print $2}')
  line=$(perl -e 'alarm 1800; exec @ARGV' "$@")
  [ -n "$line" ] || { echo "FAILED: $tag $*" >&2; return; }
  echo "$line" | sed "s/^{/{\"run\":\"$tag\",\"rep\":$rep,\"load1\":$load,/" >> "$OUTF"
  echo "$tag rep $rep: $(echo "$line" | sed 's/.*"scene":"\([A-Za-z0-9]*\)".*"mean_ms":\([0-9.]*\).*/\1 \2/') ms"
}

rep=${FIRST_REP:-4}
last=$((rep + REPS - 1))
while [ $rep -le $last ]; do
  # RagdollNoSleep: the Ragdoll scene with sleeping off in both engines (Jolt lets about two thirds
  # of the ragdolls sleep within 500 steps, Rapier almost none, so the sleeping scene compares
  # different amounts of work).
  for s in Ragdoll:--no-sleep ConvexVsMesh: Raycast: Pyramid: Pyramid30:; do
    scene=${s%%:*}; extra=${s#*:}
    for v in nondet det; do
      for th in 1 4; do
        run "jolt-$v-t$th" "$T/jolt-$v/jolt_bench" --scene $scene --threads $th --steps $STEPS $extra
      done
    done
    run "rapier-det-t1" "$T/rapier-det/release/physics-bench" --scene $scene --steps $STEPS --data "$T/data" $extra
    run "rapier-default-t1" "$T/rapier-default/release/physics-bench" --scene $scene --steps $STEPS --data "$T/data" $extra
    run "rapier-det-parallel-t4" "$T/rapier-det-parallel/release/physics-bench" --scene $scene --threads 4 --steps $STEPS --data "$T/data" $extra
    run "rapier-parallel-t4" "$T/rapier-parallel/release/physics-bench" --scene $scene --threads 4 --steps $STEPS --data "$T/data" $extra
  done
  for v in "" -det; do
    run "joltc$v-t1" "$T/jolt-ffi$v/release/jolt-ffi-proto" --steps $STEPS
  done
  run "jolt-nondet-t1-rays" "$T/jolt-nondet/jolt_bench" --scene Pyramid --threads 1 --steps $STEPS --rays
  run "joltc-t1-rays" "$T/jolt-ffi/release/jolt-ffi-proto" --steps $STEPS --rays
  rep=$((rep + 1))
done
