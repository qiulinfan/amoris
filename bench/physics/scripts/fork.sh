#!/bin/sh
# Snapshot-and-fork checks behind docs/bench/physics.md ("Snapshots and forks"), deterministic
# builds, single thread: after FORK_AT steps each engine's whole simulation state is saved, restored
# into a second world, and both step on to STEPS; the fork must end with the original's hash.
# Rapier: serde through bincode 1.3.3 into a new PhysicsWorld, the way pocket-physics writes its
# Cache. Jolt: PhysicsSystem::SaveState / RestoreState into a fresh PhysicsSystem with the scene
# built again. Writes the JSON lines to $RAW/fork.jsonl (scripts/summarize.py tabulates them).
T=${T:-/Users/qiulinfan/Desktop/aipocket2-wt/target-physics}
RAW=${RAW:-$(cd "$(dirname "$0")/../../.." && pwd)/docs/bench/physics}
STEPS=${STEPS:-300}
FORK_AT=${FORK_AT:-150}
OUTF=$RAW/fork.jsonl
: > "$OUTF"
for s in Pyramid ConvexVsMesh Ragdoll; do
  for e in jolt rapier; do
    if [ $e = jolt ]; then
      line=$(perl -e 'alarm 1800; exec @ARGV' "$T/jolt-det/jolt_bench" --scene $s --steps $STEPS --fork-at $FORK_AT)
    else
      line=$(perl -e 'alarm 1800; exec @ARGV' "$T/rapier-det-serde/release/physics-bench" --scene $s --steps $STEPS --fork-at $FORK_AT --data "$T/data")
    fi
    echo "$line" >> "$OUTF"
    echo "$e $s: $(echo "$line" | sed 's/.*"fork_hash":"\([0-9a-fx]*\)".*"hash":"\([0-9a-fx]*\)".*/fork \1, original \2/')"
  done
done
