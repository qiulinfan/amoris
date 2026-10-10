#!/bin/sh
# The Ragdoll scene with Rapier's multibody joints instead of impulse joints (docs/bench/physics.md,
# "Differences that remain"): 60 steps of the deterministic build in four variants of the emulated
# swing-twist joint (limits and motors, limits only, motors only, neither), run side by side. Each
# variant's per-step times (stderr), its JSON line or its panic go to $RAW/multibody.txt (RAW
# defaults to $T/raw; results stay in ignored local output).
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
T=${T:-${CARGO_TARGET_DIR:-$REPO/target}/physics-bench}
RAW=${RAW:-$T/raw}
STEPS=${STEPS:-60}
mkdir -p "$RAW" "$T/multibody"
B="$T/rapier-det/release/physics-bench"
for v in limits-motors: limits:--no-joint-motors motors:--no-joint-limits free:--no-joint-motors,--no-joint-limits; do
  name=${v%%:*}; flags=$(echo "${v#*:}" | tr ',' ' ')
  ( perl -e 'alarm 3000; exec @ARGV' "$B" --scene Ragdoll --steps $STEPS --multibody $flags --data "$T/data" \
      > "$T/multibody/$name.json" 2> "$T/multibody/$name.err"
    echo "exit status $?" >> "$T/multibody/$name.err" ) &
done
load=$(sysctl -n vm.loadavg)
wait
{
  echo "# $(date '+%Y-%m-%d %H:%M'), load at the start $load; the four variants ran side by side."
  for name in limits-motors limits motors free; do
    echo
    echo "## $name"
    cat "$T/multibody/$name.err" "$T/multibody/$name.json"
  done
} > "$RAW/multibody.txt"
grep -E '^## |^exit|panicked|nonfinite' "$RAW/multibody.txt"
