#!/bin/sh
# Snapshot-and-fork checks behind docs/bench/physics.md ("Snapshots and forks"), deterministic
# builds, single thread: after FORK_AT steps each engine's whole simulation state is saved, restored
# into a second world, and both step on to STEPS; the fork must end with the original's hash.
# Rapier: serde through bincode 1.3.3 into a new PhysicsWorld, the way pocket-physics writes its
# Cache. Jolt: PhysicsSystem::SaveState / RestoreState into a fresh PhysicsSystem with the scene
# built again. REPS repetitions (default 5), since save and restore times vary with the machine's
# load; scripts/summarize.py gives their median and range. Writes tagged JSON lines to
# $RAW/fork.jsonl (RAW defaults to $T/raw; the committed results are docs/bench/physics/).
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
RAW=${RAW:-$T/raw}
STEPS=${STEPS:-300}
FORK_AT=${FORK_AT:-150}
REPS=${REPS:-5}
mkdir -p "$RAW"
OUTF=$RAW/fork.jsonl
: > "$OUTF"
rep=1
while [ $rep -le $REPS ]; do
  for s in Pyramid ConvexVsMesh Ragdoll; do
    for e in jolt rapier; do
      load=$(sysctl -n vm.loadavg | awk '{print $2}')
      if [ $e = jolt ]; then
        line=$(perl -e 'alarm 1800; exec @ARGV' "$T/jolt-det/jolt_bench" --scene $s --steps $STEPS --fork-at $FORK_AT)
      else
        line=$(perl -e 'alarm 1800; exec @ARGV' "$T/rapier-det-serde/release/physics-bench" --scene $s --steps $STEPS --fork-at $FORK_AT --data "$T/data")
      fi
      [ -n "$line" ] || { echo "FAILED: $e $s" >&2; continue; }
      echo "$line" | sed "s/^{/{\"rep\":$rep,\"load1\":$load,/" >> "$OUTF"
      f() { echo "$line" | sed -n "s/.*\"$1\":\"*\([^,}\"]*\).*/\1/p"; }
      echo "rep $rep $e $s: save $(f save_ms) ms, restore $(f restore_ms) ms, fork $(f fork_hash), original $(f hash)"
    done
  done
  rep=$((rep + 1))
done
