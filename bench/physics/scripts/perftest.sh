#!/bin/sh
# Jolt's own PerformanceTest (v5.6.0, Distribution) on this machine: Discrete motion quality, every
# thread count from 1 to the core count, 500 steps, with and without CROSS_PLATFORM_DETERMINISTIC.
# Raw output goes to $RAW/jolt-perftest-<config>-<scene>-<SESSION>.txt (SESSION defaults to 1);
# scripts/summarize.py keeps the fastest run of each thread count across sessions.
J=${J:-$HOME/Reference/JoltPhysics-v5.6.0}
RAW=${RAW:-$(cd "$(dirname "$0")/../../.." && pwd)/docs/bench/physics}
SESSION=${SESSION:-1}
mkdir -p "$RAW"
for v in nondet det; do
  for s in Pyramid ConvexVsMesh Ragdoll; do
    f=$RAW/jolt-perftest-$v-$s-$SESSION.txt
    { echo "# $(date '+%Y-%m-%d %H:%M') load $(sysctl -n vm.loadavg)"; \
      cd "$J/Build/bench-$v" && perl -e 'alarm 1800; exec @ARGV' ./PerformanceTest -q=Discrete -s=$s; } > "$f" 2>&1
    echo "$v $s: $(tail -1 "$f")"
  done
done
