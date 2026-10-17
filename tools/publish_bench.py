#!/usr/bin/env python3
"""The serve loop's work after each tick, part by part (docs/bench/profiler.md, "The serve loop"):
amoris-runtime's `publish_bench` example, `--runs` times in turn with every section encoded
(`--full`, `Game::snapshot`) and through the game thread's snapshot cache, paced like the real-time
game thread; then the median over the runs of each part's median, and of the snapshot's anatomy.

Build first: `cargo build --release -p amoris-runtime --features transpile --example
publish_bench`. From the repository root, on the shared laptop inside the bench lock:

    python tools/publish_bench.py --out out/serveloop/publish --runs 5

Raw JSON goes to --out (ignored) with the machine's conditions and summary.md. `--also
LABEL=path/to/publish_bench.exe` times another build in the same turns (its cached mode only).

`--build-base REV --into DIR` builds the base to compare with: this checkout's example compiled
against the runtime of commit REV (one without `SnapshotCache`, such as master before the serve
loop's changes), with the cache's lines swapped for `Game::snapshot`, so both builds run the same
harness. It extracts REV with `git archive` into DIR/src, builds there into DIR/target (its own
target directory) and writes DIR/publish_bench-REV.exe; then pass that to `--also`.
"""

import argparse
import io
import json
import os
import shutil
import statistics
import subprocess
import sys
import tarfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
PARTS = ("step", "streams", "present", "snapshot", "publish", "after_step")


def conditions(out: Path, label: str) -> None:
    script = REPO / "tools/machine_conditions.py"
    if os.name == "nt" and script.exists():
        subprocess.run([sys.executable, str(script), str(out / f"conditions-{label}.json"),
                        "--label", f"publish-{label}"], check=False, capture_output=True)


EXAMPLE = "crates/amoris-runtime/examples/publish_bench.rs"
# The harness's only uses of the cache, and what a runtime without one runs instead.
BASE_SWAPS = [
    ("    let mut cache = amoris_persist::SnapshotCache::new();\n", ""),
    ("cache.snapshot(g.world(), g.registry())", "g.snapshot()"),
    ('{ "full" } else { "cached" }', '{ "full" } else { "uncached" }'),
]
STANZA = """
[[example]]
name = "publish_bench"
required-features = ["transpile"]
"""


def build_base(rev: str, into: Path) -> Path:
    """This checkout's example against commit `rev`'s runtime, the cache swapped out."""
    sha = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--short=8", rev], check=True,
                         capture_output=True, text=True).stdout.strip()
    src = into / "src"
    if src.exists():
        shutil.rmtree(src)
    src.mkdir(parents=True)
    tar = subprocess.run(["git", "-C", str(REPO), "archive", "--format=tar", sha], check=True,
                         capture_output=True).stdout
    with tarfile.open(fileobj=io.BytesIO(tar)) as t:
        t.extractall(src, filter="data")
    # A revision from before the 2026-10-16 rename uses the old names: rename its tree as this
    # checkout was renamed (a no-op on a later revision).
    subprocess.run([sys.executable, str(REPO / "tools" / "amoris_rename.py"), "--tree", str(src)],
                   check=True)
    text = (REPO / EXAMPLE).read_text(encoding="utf-8")
    for old, new in BASE_SWAPS:
        if text.count(old) != 1:
            sys.exit(f"{EXAMPLE} no longer has exactly one {old.strip()!r}: update BASE_SWAPS")
        text = text.replace(old, new)
    (src / EXAMPLE).write_text(text, encoding="utf-8", newline="\n")
    manifest = src / "crates/amoris-runtime/Cargo.toml"
    if 'name = "publish_bench"' not in manifest.read_text(encoding="utf-8"):
        with manifest.open("a", encoding="utf-8", newline="\n") as f:
            f.write(STANZA)
    env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "5"))
    env.pop("CARGO_TARGET_DIR", None)
    subprocess.run(["cargo", "build", "--release", "-p", "amoris-runtime", "--features",
                    "transpile", "--example", "publish_bench", "--target-dir",
                    str(into / "target")], cwd=src, env=env, check=True)
    exe = into / f"publish_bench-{sha}{EXE}"
    shutil.copy2(into / f"target/release/examples/publish_bench{EXE}", exe)
    print(f"built {exe}")
    return exe


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", default="out/serveloop/publish")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--ticks", type=int, default=1800)
    ap.add_argument("--pace-ms", type=float, default=1000 / 60)
    ap.add_argument("--project", default="samples/sailing")
    ap.add_argument("--also", nargs="*", default=[], metavar="LABEL=EXE")
    ap.add_argument("--build-base", metavar="REV", help="build the base harness and stop")
    ap.add_argument("--into", help="--build-base's directory (sources, target, the .exe)")
    a = ap.parse_args()
    if a.build_base:
        if not a.into:
            sys.exit("--build-base takes --into DIR")
        build_base(a.build_base, Path(a.into).resolve())
        return
    out = Path(a.out) if Path(a.out).is_absolute() else REPO / a.out
    out.mkdir(parents=True, exist_ok=True)
    exe = REPO / f"target/release/examples/publish_bench{EXE}"
    if not exe.exists():
        sys.exit(f"{exe} is not built (see the docstring)")
    modes = [("full", exe, ["--full"]), ("cached", exe, [])]
    modes += [(x.split("=", 1)[0], Path(x.split("=", 1)[1]), []) for x in a.also]
    conditions(out, "start")
    reports = {}
    for i in range(a.runs):
        # Each mode leads in turn.
        turn = modes[i % len(modes):] + modes[:i % len(modes)]
        for label, path, extra in turn:
            dest = out / f"{label}-{i + 1}.json"
            subprocess.run([str(path), "--ticks", str(a.ticks), "--pace-ms", str(a.pace_ms),
                            "--repeat", "500", *extra, "--out", str(dest),
                            str(REPO / a.project)], check=True, stdout=subprocess.DEVNULL)
            reports.setdefault(label, []).append(json.loads(dest.read_text())["run"])
            print(f"run {i + 1} {label}: snapshot "
                  f"{reports[label][-1]['loop']['us']['snapshot']['median']} us", flush=True)
    conditions(out, "end")
    lines = [f"Paced every {a.pace_ms:.3f} ms, {a.ticks} ticks a run, {a.runs} runs: the median "
             "over the runs of each part's median (mean, p95) in us per tick", "",
             "| Mode | " + " | ".join(PARTS) + " |", "|---|" + "---|" * len(PARTS)]
    for label, runs in reports.items():
        cells = []
        for part in PARTS:
            med = statistics.median(r["loop"]["us"][part]["median"] for r in runs)
            mean = statistics.median(r["loop"]["us"][part]["mean"] for r in runs)
            p95 = statistics.median(r["loop"]["us"][part]["p95"] for r in runs)
            cells.append(f"{med:.1f} ({mean:.1f}, {p95:.1f})")
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    first = next(iter(reports.values()))[-1]
    lines += ["", "Sections changed per tick (ticks of the last run whose digest changed): " +
              ", ".join(f"{k} {v}" for k, v in first["loop"]["sections_changed_ticks"].items() if v),
              "", "Anatomy (back to back, last run): " + json.dumps(first["anatomy"]["us"])]
    table = "\n".join(lines)
    (out / "summary.md").write_text(table + "\n")
    print(table)


if __name__ == "__main__":
    main()
