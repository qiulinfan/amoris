# Same-machine physics comparison

Status: benchmark tools adopted from `qiulinfan/amoris-pioneer` at
`7cb80e0ee0d2f3f43a98330da368aaeb83c30ebe` (2026-10-16). The source's R1/Windows
measurements remain private pending the owner's clearance to publish Unity comparisons. This
document contains methods, not those measurements. The harness has not been run on this Mac.

## Workload and configurations

`bench/physics/scripts/r1_bench.py` builds and interleaves the standalone Rapier benchmark and
Unity players running the same scene constructors: Pyramid, Pyramid30, ConvexVsMesh and Raycast.
It compares Rapier's shipped deterministic configuration, defaults, and deterministic parallel
builds; Unity players use PGS and TGS. Thread counts are 1, 8 and 16. Optional x86-64-v2/v3 builds
separate instruction-set effects from the shipped baseline. Unity is a benchmark dependency only;
the engine continues to use Rapier.

The Unity fixture targets editor 6000.3.13f1 and its built-in physics module. A licensed editor is
required to build or run its players. `UNITY_EDITOR` selects the installed editor. Nothing in the
ordinary engine checks downloads or starts Unity. `unity-compile` only checks compilation using
an installed editor's compiler and assemblies; it does not establish runtime correctness.

## Verify before timing

```sh
python bench/physics/scripts/r1_bench.py build --isa --unity
python bench/physics/scripts/r1_bench.py check --out out/physics-r1/check --unity
```

The deterministic Rapier configurations must reproduce the recorded M5 simulation outcomes.
`check --unity` additionally verifies the fixtures' expected end states and writes the players'
settings and mass-property probes to ignored output. The fixed end-state digests are regression
fixtures, not performance results or file-verification inventories. Check dimensions, positions,
mass, inertia, centre of mass, friction, restitution, damping, limits, contact distance, iteration
counts, body counts and ray construction before interpreting a timing comparison.

The two engines do different work: solvers and iteration counts, friction models, sleeping rules,
terrain-edge handling and determinism settings differ. Unity also writes back Transforms. Raycast
timings include the managed-to-native call for each Unity query. Equal inputs do not establish
equal solver effort or identical outcomes. Do not attribute a difference to determinism,
instruction sets, allocation or sleeping without a control that separates it.

## Quiet protocol

Build first; stop other builds and benchmarks, wait out startup load and hold the machine's bench
lock for each round. Record machine conditions inside the lock. `run` rotates configurations
through a balanced Williams design, records their order and predecessor, and gives up the lock
between rounds. For an odd number of configurations, use the mirrored rounds for a complete
balanced design. Inspect whole-machine and other-process CPU use before selecting quiet pairs.

```sh
python bench/physics/scripts/r1_bench.py run --out out/physics-r1/run --rounds 18 --unity
python bench/physics/scripts/r1_bench.py summarize out/physics-r1/run --max-other-cpu 9
```

Report medians and paired ratios across rounds, with outcome checks, hardware, software versions,
power plan, load and limitations. Do not combine runs from different machines or compare best
runs against medians. `bench/physics/scripts/win_sample.py` samples Windows stacks without
elevation when the cost's location needs investigating. Raw runs and generated reports stay in
ignored `out/` or separately authorized results storage; do not publish restricted Unity results.

This integration makes no PhysX speed claim. The older M5 Rapier/Jolt study and its caveats remain
in [physics.md](physics.md); it does not substitute for a same-machine PhysX comparison.
