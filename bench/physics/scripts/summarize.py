#!/usr/bin/env python3
"""Tables for docs/bench/physics.md from docs/bench/physics/bench.jsonl (scripts/bench.sh).

For every (run, scene) the repetition with the lowest mean step time is kept (the machine is shared,
so the fastest repetition is the least disturbed one); the spread of the means across repetitions is
printed beside it.

    python3 bench/physics/scripts/summarize.py docs/bench/physics/bench.jsonl > docs/bench/physics/summary.md
"""

import json
import sys
from collections import defaultdict

SCENES = ["Pyramid", "Pyramid30", "ConvexVsMesh", "Raycast", "RagdollNoSleep", "Ragdoll"]
# WebAssembly runs: repetitions whose runs all started below this 1-minute load are also paired apart.
QUIET_LOAD = 10


def load(path):
    runs = defaultdict(list)
    for line in open(path):
        line = line.strip()
        if line:
            d = json.loads(line)
            runs[(d["run"], d["scene"])].append(d)
    best = {}
    for key, ds in runs.items():
        b = min(ds, key=lambda d: d["mean_ms"])
        b = dict(b)
        b["spread"] = (min(d["mean_ms"] for d in ds), max(d["mean_ms"] for d in ds))
        b["reps"] = len(ds)
        if "ray_mean_ms" in b:
            b["ray_best"] = min(d["ray_mean_ms"] for d in ds)
        cpu = [d for d in ds if "cpu_mean_ms" in d]
        if cpu:
            c = min(cpu, key=lambda d: d["cpu_mean_ms"])
            b["cpu_best"] = c["cpu_mean_ms"]
            b["cpu_reps"] = len(cpu)
        best[key] = b
    return best


def cell(b, field="mean_ms", p95="p95_ms"):
    if b is None:
        return "–"
    return f"{b[field]:.2f} ({b[p95]:.2f})"


def table(best, title, cols, scenes=SCENES):
    print(f"### {title}\n")
    print("Mean ms per step (p95 in parentheses), fastest of the repetitions.\n")
    print("| Scene | " + " | ".join(label for label, _ in cols) + " |")
    print("|---|" + "---|" * len(cols))
    for s in scenes:
        row = [cell(best.get((run, s))) for _, run in cols]
        print(f"| {s} | " + " | ".join(row) + " |")
    print()


def paired(path):
    """Rapier / Jolt ratios from runs of the same repetition and scene, which ran within a minute or two
    of each other and so under similar background load; the median over repetitions."""
    import statistics
    by = defaultdict(dict)
    for line in open(path):
        if line.strip():
            d = json.loads(line)
            by[(d["scene"], d["rep"])][d["run"]] = d["mean_ms"]
            if d["scene"] == "Raycast" and "ray_mean_ms" in d:
                by[("Raycast: the 10,000 rays", d["rep"])][d["run"]] = d["ray_mean_ms"]
    pairs = [("rapier-det-t1", "jolt-det-t1", "Rapier det. / Jolt det., 1 thread"),
             ("rapier-default-t1", "jolt-nondet-t1", "Rapier default / Jolt, 1 thread"),
             ("rapier-det-parallel-t4", "jolt-det-t4", "Rapier det.+parallel / Jolt det., 4 threads"),
             ("rapier-parallel-t4", "jolt-nondet-t4", "Rapier parallel / Jolt, 4 threads"),
             ("jolt-det-t1", "jolt-nondet-t1", "Jolt det. / Jolt, 1 thread"),
             ("rapier-det-t1", "rapier-default-t1", "Rapier det. / Rapier default, 1 thread")]
    print("### Paired ratios of step time (median over repetitions, range; above 1 means the first is slower)\n")
    print("| Scene | " + " | ".join(label for _, _, label in pairs) + " |")
    print("|---|" + "---|" * len(pairs))
    for sc in SCENES[:4] + ["Raycast: the 10,000 rays"] + SCENES[4:]:
        cells = []
        for a, b, _ in pairs:
            rs = [runs[a] / runs[b] for (scene, _), runs in by.items() if scene == sc and a in runs and b in runs]
            cells.append(f"{statistics.median(rs):.2f} (n={len(rs)}, {min(rs):.2f}–{max(rs):.2f})" if rs else "–")
        print(f"| {sc} | " + " | ".join(cells) + " |")
    print()


def ffi_pairs(path):
    """Rust-through-joltc against the C++ harness: the two `--rays` runs of a repetition ran back to
    back on the same scene, so their step and ray times pair up."""
    import statistics
    by = defaultdict(dict)
    for line in open(path):
        if line.strip():
            d = json.loads(line)
            if d["scene"] == "Pyramid" and d["run"] in ("joltc-t1-rays", "jolt-nondet-t1-rays"):
                by[d["rep"]][d["run"]] = d
    step = [r["joltc-t1-rays"]["mean_ms"] / r["jolt-nondet-t1-rays"]["mean_ms"] for r in by.values() if len(r) == 2]
    rays = [r["joltc-t1-rays"]["ray_mean_ms"] / r["jolt-nondet-t1-rays"]["ray_mean_ms"] for r in by.values() if len(r) == 2]
    if step:
        print(f"Paired, back to back (n={len(step)}): Rust-via-joltc step time / C++ step time, median "
              f"{statistics.median(step):.3f} (range {min(step):.3f}–{max(step):.3f}); 10,000 rays through "
              f"the C ABI / in C++, median {statistics.median(rays):.3f} (range {min(rays):.3f}–{max(rays):.3f}).\n")


def perftest(raw_dir):
    """Jolt's PerformanceTest (scripts/perftest.sh): ms per step by thread count, the fastest run of
    each thread count across the sessions' files (jolt-perftest-<config>-<scene>-<session>.txt)."""
    import glob
    import os
    rows, sessions, counts = {}, set(), defaultdict(int)
    for v in ("nondet", "det"):
        for sc in ("Pyramid", "ConvexVsMesh", "Ragdoll"):
            for f in sorted(glob.glob(os.path.join(raw_dir, f"jolt-perftest-{v}-{sc}-*.txt"))):
                sessions.add(f.rsplit("-", 1)[1])
                for line in open(f):
                    parts = [p.strip() for p in line.split(",")]
                    if len(parts) == 4 and parts[0] == "Discrete":
                        ms = 1000.0 / float(parts[2])
                        r = rows.setdefault((v, sc), {})
                        r[int(parts[1])] = min(ms, r.get(int(parts[1]), ms))
                        counts[(v, sc, int(parts[1]))] += 1
    if not rows:
        return
    threads = sorted({t for r in rows.values() for t in r})
    print("### Jolt's PerformanceTest (v5.6.0, Discrete, 500 steps, sleeping allowed)\n")
    print(f"Mean ms per step from its steps-per-second line, the fastest of {len(sessions)} sessions' runs per "
          "thread count.\n")
    print("| Build, scene | " + " | ".join(f"{t} thr." for t in threads) + " |")
    print("|---|" + "---|" * len(threads))
    for (v, sc), r in rows.items():
        label = ("Jolt det., " if v == "det" else "Jolt, ") + sc
        print(f"| {label} | " + " | ".join(f"{r[t]:.2f}" if t in r else "–" for t in threads) + " |")
    print()
    for (v, sc), r in rows.items():
        short = [t for t in threads if counts[(v, sc, t)] < len(sessions)]
        if short:
            label = ("Jolt det., " if v == "det" else "Jolt, ") + sc
            print(f"{label}: {min(short)} to {max(short)} threads come from fewer sessions "
                  f"({', '.join(str(counts[(v, sc, t)]) for t in short)}): a session's file stops early.\n")


def main():
    best = load(sys.argv[1])
    loads = [b["load1"] for b in best.values()]
    print(f"Runs: {len(best)} configurations x scenes; 1-minute load average during the kept runs "
          f"{min(loads):.1f} to {max(loads):.1f}.\n")

    table(best, "Single thread", [
        ("Jolt", "jolt-nondet-t1"),
        ("Jolt det.", "jolt-det-t1"),
        ("Rapier det. (shipped)", "rapier-det-t1"),
        ("Rapier det. + LTO", "rapier-det-lto-t1"),
        ("Rapier default", "rapier-default-t1"),
        ("Rapier simd8", "rapier-simd8-t1"),
        ("Rapier parallel, 1 thread", "rapier-parallel-t1"),
    ])
    table(best, "Multithreaded (M5: 4 performance + 6 efficiency cores)", [
        ("Jolt t4", "jolt-nondet-t4"),
        ("Jolt t10", "jolt-nondet-t10"),
        ("Jolt det. t4", "jolt-det-t4"),
        ("Jolt det. t10", "jolt-det-t10"),
        ("Rapier parallel t4", "rapier-parallel-t4"),
        ("Rapier parallel t10", "rapier-parallel-t10"),
        ("Rapier par.+simd8 t4", "rapier-parallel-simd8-t4"),
        ("Rapier det.+parallel t4", "rapier-det-parallel-t4"),
        ("Rapier det.+parallel t10", "rapier-det-parallel-t10"),
    ])

    paired(sys.argv[1])

    print("### Totals for the run (ms for all steps), single thread\n")
    print("| Scene | Steps | Jolt | Jolt det. | Rapier det. | Rapier default |")
    print("|---|---|---|---|---|---|")
    for s in SCENES:
        r = [best.get((k, s)) for k in ("jolt-nondet-t1", "jolt-det-t1", "rapier-det-t1", "rapier-default-t1")]
        steps = next((x["steps"] for x in r if x), 0)
        print(f"| {s} | {steps} | " + " | ".join(f"{x['total_ms']:.0f}" if x else "–" for x in r) + " |")
    print()

    print("### Ray casts: 10,000 closest-hit rays per step, single thread (Raycast scene)\n")
    print("| Configuration | Mean ms per 10,000 rays (p95) | Hits per step |")
    print("|---|---|---|")
    for label, run in [("Jolt", "jolt-nondet-t1"), ("Jolt det.", "jolt-det-t1"), ("Rapier det.", "rapier-det-t1"),
                       ("Rapier det. + LTO", "rapier-det-lto-t1"), ("Rapier default", "rapier-default-t1"),
                       ("Rapier simd8", "rapier-simd8-t1")]:
        b = best.get((run, "Raycast"))
        if b:
            print(f"| {label} | {b['ray_mean_ms']:.2f} ({b['ray_p95_ms']:.2f}) | {b['ray_hits'] / b['steps']:.0f} |")
    print()

    print("### Iteration-count sensitivity (Pyramid scenes, single thread)\n")
    print("| Configuration | Pyramid | Pyramid30 | top box y at the end (Pyramid / Pyramid30) |")
    print("|---|---|---|---|")
    for label, run in [("Jolt, 10 velocity + 2 position steps (default)", "jolt-nondet-t1"),
                       ("Jolt, 4 velocity + 1 position step", "jolt-nondet-t1-vel4pos1"),
                       ("Rapier det., 4 substeps (default)", "rapier-det-t1"),
                       ("Rapier det., 10 substeps", "rapier-det-t1-iters10")]:
        a, b = best.get((run, "Pyramid")), best.get((run, "Pyramid30"))
        ys = " / ".join(f"{x['top_y']:.2f}" if x else "–" for x in (a, b))
        print(f"| {label} | {cell(a)} | {cell(b)} | {ys} |")
    print()

    print("### Outcome check (where the dynamic bodies ended, single thread)\n")
    print("| Scene | Jolt top y / mean y | Rapier det. top y / mean y | Jolt active at end | Rapier active at end |")
    print("|---|---|---|---|---|")
    for s in SCENES:
        j, r = best.get(("jolt-nondet-t1", s)), best.get(("rapier-det-t1", s))
        if j and r:
            print(f"| {s} | {j['top_y']:.2f} / {j['mean_y']:.2f} | {r['top_y']:.2f} / {r['mean_y']:.2f} | "
                  f"{j['active_at_end']} | {r['active_at_end']} |")
    print()

    print("### Rust calling Jolt through joltc (Pyramid, single thread)\n")
    print("| Configuration | Step mean ms (p95) | Pose read-back of 1,241 bodies | 10,000 rays mean ms | End hash |")
    print("|---|---|---|---|---|")
    for label, run in [("C++ jolt_bench", "jolt-nondet-t1"), ("Rust via joltc", "joltc-t1"),
                       ("Rust via joltc, --sync", "joltc-t1-sync"), ("C++ jolt_bench --rays", "jolt-nondet-t1-rays"),
                       ("Rust via joltc, --rays", "joltc-t1-rays"), ("C++ jolt_bench det.", "jolt-det-t1"),
                       ("Rust via joltc det.", "joltc-det-t1"), ("C++ jolt_bench det. --rays", "jolt-det-t1-rays"),
                       ("Rust via joltc det., --rays", "joltc-det-t1-rays")]:
        b = best.get((run, "Pyramid"))
        if not b:
            continue
        sync = f"{b['sync_mean_ms'] * 1000:.1f} µs ({b['sync_ns_per_body']:.1f} ns/body)" if "sync_mean_ms" in b else "–"
        rays = f"{b['ray_mean_ms']:.3f}" if "ray_mean_ms" in b else "–"
        print(f"| {label} | {cell(b)} | {sync} | {rays} | `{b['hash']}` |")
    print()


def wasm(raw_dir):
    """WebAssembly against native, deterministic builds, single thread (scripts/wasm.sh)."""
    import os
    import statistics
    f = os.path.join(raw_dir, "wasm.jsonl")
    if not os.path.exists(f):
        return
    best, by_rep, loads, rep_load = {}, defaultdict(dict), [], defaultdict(float)
    for line in open(f):
        if line.strip():
            d = json.loads(line)
            k = (d["run"], d["scene"])
            loads.append(d["load1"])
            by_rep[(d["scene"], d["rep"])][d["run"]] = d["mean_ms"]
            rep_load[(d["scene"], d["rep"])] = max(rep_load[(d["scene"], d["rep"])], d["load1"])
            if k not in best or d["mean_ms"] < best[k]["mean_ms"]:
                best[k] = d
    cols = [("Rapier det. native", "rapier-det-native"),
            ("Rapier det. wasm32, web profile + wasm-opt", "rapier-det-wasm-web"),
            ("Rapier det. wasm32 simd128, web profile + wasm-opt", "rapier-det-wasm-web-simd128"),
            ("Jolt det. native", "jolt-det-native"), ("Jolt det. wasm32 (scalar)", "jolt-det-wasm"),
            ("Jolt det. in a Rust wasm32-unknown-unknown module", "jolt-det-rust-wasm")]
    steps = next(iter(best.values()))["steps"]
    reps = len({r for (_, r) in by_rep})
    node = next((d["node"] for d in best.values() if "node" in d), "24")
    print(f"### WebAssembly (V8 in Node {node}), deterministic builds, single thread, {steps} steps\n")
    print(f"Mean ms per step (p95), fastest of {reps} repetitions (1-minute load {min(loads):.0f} to "
          f"{max(loads):.0f}); in brackets the ratio to the same engine's native run.\n")
    print("| Scene | " + " | ".join(label for label, _ in cols) + " |")
    print("|---|" + "---|" * len(cols))
    for sc in ("Pyramid", "ConvexVsMesh", "RagdollNoSleep", "Ragdoll"):
        row = []
        for _, run in cols:
            b = best.get((run, sc))
            if not b:
                row.append("–")
                continue
            native = best.get(("rapier-det-native" if run.startswith("rapier") else "jolt-det-native", sc))
            ratio = f" [{b['mean_ms'] / native['mean_ms']:.1f}x]" if native and native is not b else ""
            row.append(f"{b['mean_ms']:.2f} ({b['p95_ms']:.2f}){ratio}")
        print(f"| {sc} | " + " | ".join(row) + " |")
    print()
    # Same-repetition ratios: what the shipped web profile changes, and Rapier against Jolt in wasm.
    pairs = [("rapier-det-wasm-web", "rapier-det-wasm-release", "Rapier web profile + wasm-opt / release profile"),
             ("rapier-det-wasm-web", "jolt-det-wasm", "Rapier det. wasm32 / Jolt det. wasm32"),
             ("rapier-det-wasm-web-simd128", "jolt-det-wasm", "Rapier det. wasm32 simd128 / Jolt det. wasm32"),
             ("jolt-det-rust-wasm", "jolt-det-wasm", "Jolt in the Rust module / Jolt alone, wasm32")]
    for title, quiet in (("Paired within a repetition (median, range), all repetitions:", None),
                         (f"The same, only repetitions whose runs all started at a 1-minute load below {QUIET_LOAD}:",
                          QUIET_LOAD)):
        print(title + "\n")
        print("| Scene | " + " | ".join(label for _, _, label in pairs) + " |")
        print("|---|" + "---|" * len(pairs))
        for sc in ("Pyramid", "ConvexVsMesh", "RagdollNoSleep", "Ragdoll"):
            cells = []
            for a, b, _ in pairs:
                rs = [r[a] / r[b] for (scene, rp), r in by_rep.items()
                      if scene == sc and a in r and b in r and (quiet is None or rep_load[(scene, rp)] < quiet)]
                cells.append(f"{statistics.median(rs):.2f} (n={len(rs)}, {min(rs):.2f}–{max(rs):.2f})" if rs else "–")
            print(f"| {sc} | " + " | ".join(cells) + " |")
        print()
    hashes = defaultdict(set)
    for (run, sc), d in best.items():
        hashes[(sc, run.split("-")[0])].add(d["hash"])
    split = [f"{sc} ({e})" for (sc, e), h in hashes.items() if len(h) > 1]
    print("End hashes: " + ("every native and wasm run of an engine agrees on every scene." if not split
                            else "differ for " + ", ".join(split) + ".") + "\n")


def forks(raw_dir):
    """Snapshot, restore into a second world, step both (scripts/fork.sh): per scene and engine the
    state size, the median and range of the save, rebuild and restore times over the repetitions,
    and whether every repetition's fork ended with the original's hash."""
    import os
    import statistics
    f = os.path.join(raw_dir, "fork.jsonl")
    if not os.path.exists(f):
        return
    runs = defaultdict(list)
    for line in open(f):
        if line.strip():
            d = json.loads(line)
            runs[(d["scene"], d["engine"])].append(d)
    loads = [d["load1"] for ds in runs.values() for d in ds if "load1" in d]
    n = max(len(ds) for ds in runs.values())
    print(f"### Snapshots and forks, deterministic builds, single thread\n")
    print(f"Times in ms: median over {n} runs (fastest–slowest)"
          + (f", 1-minute load {min(loads):.0f} to {max(loads):.0f}" if loads else "") + ".\n")
    print("| Scene | Engine | Fork after / of steps | State bytes | Save ms | Rebuild ms | Restore ms | Fork's end hash = original's |")
    print("|---|---|---|---|---|---|---|---|")

    def ms(x):
        return f"{x:.0f}" if x >= 100 else f"{x:.1f}" if x >= 10 else f"{x:.2f}"

    def spread(ds, k):
        v = [d[k] for d in ds]
        return f"{ms(statistics.median(v))} ({ms(min(v))}–{ms(max(v))})"

    for (scene, engine), ds in runs.items():
        d = ds[0]
        label = "Jolt det." if engine == "jolt" else "Rapier det."
        sizes = {x["state_bytes"] for x in ds}
        size = f"{d['state_bytes'] / 1e6:.2f} MB" if len(sizes) == 1 else "varies"
        rebuild = spread(ds, "rebuild_ms") if "rebuild_ms" in d else "–"
        same = all(x["fork_hash"] == x["hash"] for x in ds) and len({x["hash"] for x in ds}) == 1
        print(f"| {scene} | {label} | {d['fork_at']} / {d['steps']} | {size} | {spread(ds, 'save_ms')} | "
              f"{rebuild} | {spread(ds, 'restore_ms')} | {'yes' if same else '**no**'} (`{d['hash']}`, {len(ds)} runs) |")
    print()


if __name__ == "__main__":
    main()
    ffi_pairs(sys.argv[1])
    import os
    perftest(os.path.dirname(os.path.abspath(sys.argv[1])))
    wasm(os.path.dirname(os.path.abspath(sys.argv[1])))
    forks(os.path.dirname(os.path.abspath(sys.argv[1])))
