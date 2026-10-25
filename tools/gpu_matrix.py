#!/usr/bin/env python3
"""Runs the renderer's GPU tests on several adapters and backends and prints a test x configuration
table in which skips show (docs/bench/gpu-matrix.md).

`cargo xtask check` runs `amoris-render`'s tests once, on the default adapter and backend. This runs
the same test binaries (`cargo test -p amoris-render`, release by default, ignored tests included as
in `cargo xtask check`) once per configuration, each named `ADAPTER-BACKEND[-minimal]`:

- ADAPTER is `AMORIS_ADAPTER` (an index or part of a name, `default` for none),
- BACKEND is `AMORIS_BACKEND` (`dx12`, `vulkan`, `metal`), or `auto` to leave it unset as `cargo
  xtask check` does (the platform's default backend; tests that compare backends then run on every
  native backend, `BackendChoice::under_test`),
- `-minimal` adds `AMORIS_GPU_MINIMAL=features,limits,timestamps` (WebGPU's baseline,
  docs/spec/webgpu-baseline.md).

A passing test that printed that it skipped all or part of its checks for something the device
lacks (no GPU, no timestamps, no ray queries, no shader-f16, ...) is `skip`, with its lines in the
notes. Every binary runs with `--show-output`, so the output of passing tests is kept. A
configuration whose adapter-naming test opened no adapter, or one of whose tests found no GPU, is
an error: a mistyped adapter must not pass as a run of skips.

    python tools/gpu_matrix.py --build-only                       # outside the bench lock
    python tools/gpu_matrix.py --configs 5060-dx12,780m-vulkan --json out/gpu-matrix/a.json
    python tools/gpu_matrix.py --merge out/gpu-matrix/a.json out/gpu-matrix/b.json [--markdown]

Without `--configs`, every native backend of this platform on the default adapter. `--configs
machine` builds the configurations from this machine's adapters (`machine_configs`): every native
backend of every adapter but software ones, `auto` on every adapter but the default one (`cargo
xtask check`'s `test` step runs that configuration; `--default-auto` adds it as `default-auto`, for
a check whose `test` did not run it), and the minimal device on the default adapter with the
platform's default backend. `--workspace` builds the binaries as `cargo test --workspace`
does, so the run reuses the binaries of `cargo xtask check`'s `test` step (the workspace's other
crates add features to the renderer's dependencies, and `-p amoris-render` alone builds them again
without). `--bins lib,water` limits the binaries, `--filter` passes a test-name filter to
each, `--debug` uses the debug profile. A binary still running after `--timeout` seconds is killed;
its tests that libtest reported running for over a minute are `hang`, others without a result
`crash`. Logs go to `out/gpu-matrix/<config>/<binary>.log`. `--json` records the commit with
`-dirty` when the working tree had changes, the test binaries' build times, the inherited
`AMORIS_*` and `WGPU_*` variables (`WGPU_DEBUG=1`, for instance), what made the run fail
(`problems`) and, with `--configs machine`, the adapters found. `--merge` names a configuration
that occurs in several files `CONFIG#N`, N the file's position. The exit status is 1 when a test
failed, hung or crashed, or a configuration found no GPU; 3 when `--configs machine` found no
adapter but software ones (nothing ran).

AMD's Vulkan driver keeps the pipelines it compiled in an archive per executable
(`%LOCALAPPDATA%/AMD/VkCache`) and served a wrong one to every later run of a test binary
(docs/bench/gpu-matrix.md 9). Each run therefore removes `target/<profile>/gpu-matrix-vkcache/` and
gives every configuration's every binary an empty directory of its own there,
`gpu-matrix-vkcache/<config>/<binary>/`, through the driver's `AMD_VK_PIPELINE_CACHE_PATH` (for the
adapter-naming run, the listing and the tests alike): no configuration or run inherits an archive.
A directory that cannot be emptied stops the run; `--json` records the directory as
`driver_cache`. Other drivers ignore the variable. When a binary's test failed, hung or crashed
and its archive holds a compute program compiled for registers the hardware does not set, or a
pipeline compiled to more than one program (the driver's wrong compiles, `tools/amd_vkcache.py
unset` and `twins`, docs/bench/gpu-matrix.md 12), the binary's record says so as `driver` and every
such test's message ends with it, as context: the archive is the whole binary's, so whether the
failing test uses that pipeline is left to whoever reads it.
"""
import argparse
import importlib.util
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "crates" / "amoris-render"
MINIMAL = "features,limits,timestamps"
# The test that names the adapter a configuration opened (gpu.rs).
ADAPTER_TEST = "gpu::tests::the_adapter_under_test_is_named"
ADAPTER_PREFIX = "adapter under test: "
# The test that lists the machine's adapters per native backend (gpu.rs), and its lines.
LIST_TEST = "gpu::tests::the_machine_adapters_are_listed"
LISTED = re.compile(r"^machine adapter: (\w+) (\d+) (\w+) (.+)$")
LISTED_DEFAULT = re.compile(r"^machine default adapter: (\w+) (.+)$")
# Device types of software rasterizers (WARP's Microsoft Basic Render Driver, llvmpipe), which
# `--configs machine` leaves out.
SOFTWARE = {"Cpu"}
# Lines of a passing test that say it left out work for want of a device capability.
SKIP_REASONS = re.compile(
    r"no GPU|timestamps|ray quer|EXPERIMENTAL_RAY_QUERY|INDIRECT_FIRST_INSTANCE|multi-draw|"
    r"AMORIS_SPLAT_RASTER|shader-f16"
)
# A note that says the test found no device at all (not "no GPU timestamps").
NO_GPU = re.compile(r"no GPU\b(?! timestamps)")
SKIP_WORDS = re.compile(r"\bskipped\b|\bnot checked\b")
RESULT = re.compile(r"^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED|ignored)(?:, .*)?$")
SLOW = re.compile(r"^test (\S+) has been running for over \d+ seconds$")
BLOCK = re.compile(r"^---- (\S+) std(out|err) ----$")
ENV_KEYS = ("AMORIS_BACKEND", "AMORIS_ADAPTER", "AMORIS_GPU_MINIMAL")
# Where AMD's Vulkan driver keeps its pipeline archive (docs/bench/gpu-matrix.md 9), and the
# directory beside the profile's `deps/` that holds one per configuration and binary.
DRIVER_CACHE = "AMD_VK_PIPELINE_CACHE_PATH"
CACHE_DIR = "gpu-matrix-vkcache"


def native_backends():
    """This platform's native backends, its default (`auto`) first."""
    if sys.platform == "win32":
        return ["dx12", "vulkan"]
    if sys.platform == "darwin":
        return ["metal"]
    return ["vulkan"]


def parse_config(name):
    """`ADAPTER-BACKEND[-minimal]` -> the environment it sets (`auto`: no `AMORIS_BACKEND`)."""
    parts = name.split("-")
    if len(parts) not in (2, 3) or (len(parts) == 3 and parts[2] != "minimal"):
        raise SystemExit(f"configuration '{name}': expected ADAPTER-BACKEND[-minimal]")
    adapter, backend = parts[0], parts[1]
    if backend not in ("dx12", "vulkan", "metal", "auto"):
        raise SystemExit(
            f"configuration '{name}': backend '{backend}' is not dx12, vulkan, metal or auto")
    env = {} if backend == "auto" else {"AMORIS_BACKEND": backend}
    if adapter != "default":
        env["AMORIS_ADAPTER"] = adapter
    if len(parts) == 3:
        env["AMORIS_GPU_MINIMAL"] = MINIMAL
    return env


def build(debug, workspace=False):
    """Builds the test binaries (with `workspace`, every crate's, as `cargo test --workspace` does);
    returns {binary name: executable} for amoris-render's."""
    cmd = ["cargo", "test", *(["--workspace"] if workspace else ["-p", "amoris-render"]), "--locked",
           "--no-run", "--message-format=json"]
    if not debug:
        cmd.insert(2, "--release")
    p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, encoding="utf-8",
                       errors="replace")
    if p.returncode != 0:
        sys.stderr.write(p.stderr[-4000:])
        raise SystemExit(f"{' '.join(cmd)} failed (exit {p.returncode})")
    bins = {}
    for line in p.stdout.splitlines():
        if not line.startswith("{"):
            continue
        m = json.loads(line)
        if m.get("reason") != "compiler-artifact" or not m.get("executable"):
            continue
        if m["package_id"].find("amoris-render") < 0 or not m["profile"]["test"]:
            continue
        target = m["target"]
        name = "lib" if "lib" in target["kind"] else target["name"]
        bins[name] = m["executable"]
    return bins


def cache_root(bins):
    """`gpu-matrix-vkcache/` beside the test binaries' `deps/`."""
    return Path(next(iter(bins.values()))).parent.parent / CACHE_DIR


def driver_caches(bins, configs):
    """{(configuration, binary): directory}: an empty directory per configuration and test binary
    for AMD's Vulkan driver to keep its pipeline archive in. What an earlier run left is removed
    first; a directory that cannot be emptied (an archive a process still holds) is an error."""
    root = cache_root(bins)
    try:
        shutil.rmtree(root)
    except FileNotFoundError:
        pass
    except OSError as e:
        raise SystemExit(f"cannot empty {root} for the driver's pipeline archives: {e}")
    caches = {(c, b): root / c / b for c in configs for b in bins}
    for d in caches.values():
        d.mkdir(parents=True, exist_ok=True)
        if any(d.iterdir()):
            raise SystemExit(f"{d} is not empty: the driver would inherit an archive")
    return caches


def driver_faults(cache):
    """What a binary's driver archive holds that may explain a failure (docs/bench/gpu-matrix.md
    12): the compute programs the driver compiled for registers the hardware does not set
    (`tools/amd_vkcache.py unset`), and the pipelines it compiled to more than one program in that
    run (`twins`), as a sentence; None when there are none or no archive. The archive is the whole
    binary's, with no link from a test to the pipelines it used, so the sentence is context, not a
    cause: a test that fails for a reason of its own beside an unrelated wrong program must still
    read as a failure to look into. An archive it cannot read (a process that died while writing
    it) is reported on the console, not raised: the diagnosis must not stop the matrix."""
    files = sorted(Path(cache).glob("*.parc"))
    if not files:
        return None
    spec = importlib.util.spec_from_file_location("amd_vkcache",
                                                  Path(__file__).with_name("amd_vkcache.py"))
    vkcache = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(vkcache)
    try:
        _, unset = vkcache.unset(files)
        _, twins = vkcache.twins(files)
    except (OSError, ValueError, IndexError, struct.error) as e:
        print(f"note: cannot read the driver's archive in {cache}: {e}", flush=True)
        return None
    found = []
    if unset:
        found.append(f"{len(unset)} compute program(s) compiled for registers the hardware does "
                     f"not set (python tools/amd_vkcache.py unset {cache})")
    if twins:
        found.append(f"{len(twins)} pipeline(s) compiled to more than one program "
                     f"(python tools/amd_vkcache.py twins {cache})")
    if not found:
        return None
    return (f"this run's driver archive of the binary holds {'; and '.join(found)}; whether the "
            f"failing test uses them is not checked; docs/bench/gpu-matrix.md 12")


def parse_adapters(text):
    """The adapter-listing test's output -> ([{backend, index, type, name}], {backend: the name of
    the adapter it opens without AMORIS_ADAPTER})."""
    adapters, defaults = [], {}
    for raw in text.splitlines():
        line = raw.strip()
        m = LISTED.match(line)
        if m:
            adapters.append({"backend": m.group(1), "index": int(m.group(2)), "type": m.group(3),
                             "name": m.group(4).strip()})
            continue
        m = LISTED_DEFAULT.match(line)
        if m:
            defaults[m.group(1)] = m.group(2).strip()
    return adapters, defaults


def adapter_token(name, others):
    """A short `AMORIS_ADAPTER` value for the adapter `name`: a word of it (a model number first)
    that is part of none of the `others` (the machine's other adapters' names), in lower case, so
    it picks this adapter on every backend; None when there is no such word."""
    def unique(word):
        w = word.lower()
        # A small number would be read as an index (gpu.rs `match_adapter`).
        if w.isdigit() and int(w) < 64:
            return False
        return all(w not in o.lower() for o in others)
    words = [w for w in re.split(r"[^A-Za-z0-9]+", name) if w and unique(w)]
    numbered = [w for w in words if any(c.isdigit() for c in w)]
    chosen = numbered or words
    return chosen[0].lower() if chosen else None


def machine_configs(adapters, defaults, backends, default_auto=False):
    """The configurations of `--configs machine` from `parse_adapters`'s output; `backends` is the
    platform's native backends, its default first. Returns (configurations, what was left out and
    why). Adapters are told apart by name (an adapter on two backends is one adapter); one without
    a word of its own is named by its index on each backend and has no `auto` configuration. The
    default adapter's `auto` is `cargo xtask check`'s `test` run, left out unless `default_auto`,
    which adds it as `default-auto` (no `AMORIS_ADAPTER` either, exactly as `test` runs)."""
    auto = backends[0]
    hardware = [a for a in adapters if a["type"] not in SOFTWARE and a["backend"] in backends]
    left_out = [f"{a['name']} ({a['backend']}): a software adapter" for a in adapters
                if a["type"] in SOFTWARE]
    names = []
    for a in hardware:
        if a["name"] not in names:
            names.append(a["name"])
    default = defaults.get(auto)
    if default in names:
        names.remove(default)
        names.insert(0, default)

    def on(name, backend):
        return next((a for a in hardware if a["name"] == name and a["backend"] == backend), None)

    every = [a["name"] for a in adapters]
    tokens = {n: adapter_token(n, [o for o in every if o != n]) for n in names}
    configs = []
    for n in names:
        for b in backends:
            a = on(n, b)
            if a:
                configs.append(f"{tokens[n] or a['index']}-{b}")
    for n in names:
        if n == default and default_auto:
            configs.append("default-auto")
        elif n == default:
            left_out.append(f"{tokens[n] or n} auto: cargo xtask check's test step runs it")
        elif tokens[n] is None:
            left_out.append(f"{n} auto: no word of its name names it alone")
        elif all(on(n, b) for b in backends):
            configs.append(f"{tokens[n]}-auto")
        else:
            left_out.append(f"{tokens[n]} auto: not on every native backend")
    if names:
        a = on(names[0], auto) or next(on(names[0], b) for b in backends if on(names[0], b))
        configs.append(f"{tokens[names[0]] or a['index']}-{a['backend']}-minimal")
    return configs, left_out


def list_adapters(bins, timeout):
    """Runs the adapter-listing test with no `AMORIS_*` device variable set."""
    if "lib" not in bins:
        raise SystemExit("--configs machine: no lib test binary to list the adapters with")
    env = {k: v for k, v in os.environ.items() if k not in ENV_KEYS}
    # The listing opens devices too: give it an empty pipeline archive of its own (the module
    # doc); `driver_caches` removes the whole root before the configurations run.
    listing = cache_root(bins) / "adapters"
    shutil.rmtree(listing, ignore_errors=True)
    listing.mkdir(parents=True, exist_ok=True)
    env[DRIVER_CACHE] = str(listing)
    p = subprocess.run([bins["lib"], LIST_TEST, "--exact", "--show-output"], cwd=CRATE, env=env,
                       capture_output=True, text=True, encoding="utf-8", errors="replace",
                       timeout=timeout)
    text = p.stdout + p.stderr
    if p.returncode != 0 or f"test {LIST_TEST} ... ok" not in text:
        sys.stderr.write(text[-4000:])
        raise SystemExit(f"--configs machine: {LIST_TEST} failed (exit {p.returncode})")
    return parse_adapters(text)


def list_tests(exe, env, ignored, timeout):
    """Names of the binary's tests (with `--include-ignored` semantics when `ignored`)."""
    p = subprocess.run([exe, "--list"], cwd=CRATE, env=env, capture_output=True, text=True,
                       encoding="utf-8", errors="replace", timeout=timeout)
    names = [l[: -len(": test")] for l in p.stdout.splitlines() if l.endswith(": test")]
    if not ignored:
        q = subprocess.run([exe, "--list", "--ignored"], cwd=CRATE, env=env, capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=timeout)
        skipped = {l[: -len(": test")] for l in q.stdout.splitlines() if l.endswith(": test")}
        names = [n for n in names if n not in skipped]
    return names


def parse_output(text):
    """libtest's output with `--show-output` -> ({test: status}, {test: captured lines}). A test
    libtest reported as running for over a minute and that has no result is `hung`."""
    status, output = {}, {}
    current = None
    for raw in text.splitlines():
        line = raw.rstrip("\r")
        m = BLOCK.match(line)
        if m:
            current = m.group(1)
            output.setdefault(current, [])
            continue
        if current is not None:
            if line in ("successes:", "failures:") or line.startswith("test result:"):
                current = None
            else:
                output[current].append(line)
                continue
        m = RESULT.match(line)
        if m:
            status[m.group(1)] = m.group(2)
            continue
        m = SLOW.match(line)
        if m:
            status.setdefault(m.group(1), "hung")
    return status, output


def skip_notes(lines):
    return sorted({l.strip() for l in lines if SKIP_WORDS.search(l) and SKIP_REASONS.search(l)})


def panic_message(lines):
    for i, l in enumerate(lines):
        if "panicked at" in l:
            rest = [x.strip() for x in lines[i + 1:i + 4] if x.strip()]
            return rest[0] if rest else l.strip()
    return ""


def classify(state, lines):
    """One cell: (status, notes, message)."""
    if state == "FAILED":
        return "FAIL", skip_notes(lines), panic_message(lines)
    if state == "ignored":
        return "ign", [], ""
    if state == "hung":
        return "hang", [], "no result after a minute or more"
    if state == "ok":
        notes = skip_notes(lines)
        return ("skip" if notes else "pass"), notes, ""
    return "crash", [], ""


def run_config(name, bins, args, out_dir, caches):
    base = {k: v for k, v in os.environ.items() if k not in ENV_KEYS}
    base.update(parse_config(name))
    log_dir = out_dir / name
    log_dir.mkdir(parents=True, exist_ok=True)
    cells, adapter, binaries = {}, None, {}
    if "lib" in bins:
        env = dict(base, **{DRIVER_CACHE: str(caches[(name, "lib")])})
        try:
            p = subprocess.run([bins["lib"], ADAPTER_TEST, "--exact", "--show-output"], cwd=CRATE,
                               env=env, capture_output=True, text=True, encoding="utf-8",
                               errors="replace", timeout=args.timeout)
            for l in (p.stdout + p.stderr).splitlines():
                if l.startswith(ADAPTER_PREFIX):
                    adapter = l[len(ADAPTER_PREFIX):].strip()
        except subprocess.TimeoutExpired:
            print(f"{name}: opening the adapter took over {args.timeout:.0f}s", flush=True)
    for b in args.bins or sorted(bins):
        if b not in bins:
            raise SystemExit(f"no test binary '{b}'; have {', '.join(sorted(bins))}")
        exe = bins[b]
        env = dict(base, **{DRIVER_CACHE: str(caches[(name, b)])})
        try:
            names = list_tests(exe, env, not args.no_ignored, args.timeout)
        except subprocess.TimeoutExpired:
            print(f"{name} {b}: listing its tests took over {args.timeout:.0f}s", flush=True)
            binaries[b] = {"exit": "timeout", "seconds": args.timeout, "tests": 0}
            continue
        if args.filter:
            names = [n for n in names if args.filter in n]
        if not names:
            continue
        cmd = [exe, "--show-output"]
        if not args.no_ignored:
            cmd.append("--include-ignored")
        if args.filter:
            cmd.append(args.filter)
        if args.test_threads:
            cmd.append(f"--test-threads={args.test_threads}")
        t = time.time()
        try:
            p = subprocess.run(cmd, cwd=CRATE, env=env, capture_output=True, text=True,
                               encoding="utf-8", errors="replace", timeout=args.timeout)
            code, text = p.returncode, p.stdout + "\n" + p.stderr
        except subprocess.TimeoutExpired as e:
            code = "timeout"
            text = "\n".join(x if isinstance(x, str) else x.decode("utf-8", "replace")
                             for x in (e.stdout or "", e.stderr or ""))
        seconds = time.time() - t
        (log_dir / f"{b}.log").write_text(text, encoding="utf-8")
        status, output = parse_output(text)
        binaries[b] = {"exit": code, "seconds": round(seconds, 1), "tests": len(names)}
        for n in names:
            cell = classify(status.get(n), output.get(n, []))
            if cell[0] == "crash":
                cell = ("crash", [], f"no result (exit {code})")
            cells[f"{b} {n}"] = {"status": cell[0], "notes": cell[1], "message": cell[2]}
        counts = {s: sum(1 for k, c in cells.items() if k.startswith(b + " ") and c["status"] == s)
                  for s in ("pass", "skip", "FAIL", "hang", "ign", "crash")}
        print(f"{name} {b}: exit {code} in {seconds:.0f}s; "
              + ", ".join(f"{v} {k}" for k, v in counts.items() if v), flush=True)
        bad = [f"{b} {n}" for n in names
               if cells[f"{b} {n}"]["status"] in ("FAIL", "hang", "crash")]
        faults = driver_faults(caches[(name, b)]) if bad else None
        if faults:
            binaries[b]["driver"] = faults
            for t in bad:
                cells[t]["message"] = f"{cells[t]['message']} [{faults}]".lstrip()
            print(f"{name} {b}: {faults}", flush=True)
    return {"env": parse_config(name), "adapter": adapter, "binaries": binaries, "tests": cells}


def findings(results):
    """What makes a run fail, per configuration: `no_adapter` (none named), `no_gpu` (tests that
    printed they found none) and `failed` (tests that failed, hung or crashed)."""
    out = []
    for name, r in results.items():
        if not r.get("adapter"):
            out.append({"config": name, "kind": "no_adapter", "tests": []})
        no_gpu = sorted(t for t, c in r["tests"].items()
                        if any(NO_GPU.search(n) for n in c["notes"]))
        if no_gpu:
            out.append({"config": name, "kind": "no_gpu", "tests": no_gpu})
        bad = sorted(t for t, c in r["tests"].items() if c["status"] in ("FAIL", "hang", "crash"))
        if bad:
            out.append({"config": name, "kind": "failed", "tests": bad})
    return out


def problems(results):
    """`findings` as lines."""
    text = {"no_adapter": lambda f: "no adapter reported",
            "no_gpu": lambda f: f"{len(f['tests'])} tests found no GPU ({f['tests'][0]}, ...)",
            "failed": lambda f: f"{len(f['tests'])} tests failed, hung or crashed"}
    return [f"{f['config']}: {text[f['kind']](f)}" for f in findings(results)]


def merge(paths):
    """The configurations of several --json files; a name in more than one file becomes NAME#N."""
    runs = [json.loads(Path(p).read_text(encoding="utf-8"))["configs"] for p in paths]
    seen = {}
    for run in runs:
        for c in run:
            seen[c] = seen.get(c, 0) + 1
    results = {}
    for i, run in enumerate(runs, 1):
        for c, r in run.items():
            results[c if seen[c] == 1 else f"{c}#{i}"] = r
    return results


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def table(results, every=False, markdown=False):
    configs = list(results)
    tests = sorted({t for r in results.values() for t in r["tests"]})
    rows = []
    notes = []
    for t in tests:
        cells = [results[c]["tests"].get(t, {"status": "-", "notes": [], "message": ""})
                 for c in configs]
        if not every and all(c["status"] in ("pass", "-") for c in cells):
            continue
        row = []
        for c, cell in zip(configs, cells):
            s = cell["status"]
            if cell["notes"] or cell["message"]:
                notes.append((t, c, cell["message"] or "; ".join(cell["notes"])))
                s += f" [{len(notes)}]"
            row.append(s)
        rows.append((t, row))
    totals = []
    for c in configs:
        cells = results[c]["tests"].values()
        totals.append(", ".join(f"{sum(1 for x in cells if x['status'] == s)} {s}"
                                for s in ("pass", "skip", "FAIL", "hang", "ign", "crash")
                                if any(x["status"] == s for x in cells)))
    out = []
    if markdown:
        out.append("| Test | " + " | ".join(configs) + " |")
        out.append("|---" * (len(configs) + 1) + "|")
        for t, row in rows:
            out.append(f"| `{t}` | " + " | ".join(row) + " |")
        out.append("| totals | " + " | ".join(totals) + " |")
        out.append("")
        for i, (t, c, n) in enumerate(notes, 1):
            out.append(f"{i}. {c}, `{t}`: {n}")
        out.append("")
        for c in configs:
            out.append(f"- {c}: {results[c].get('adapter') or 'adapter not reported'}")
    else:
        width = max([len(t) for t, _ in rows] + [6])
        cw = [max(len(c), 10) for c in configs]
        out.append("test".ljust(width) + "  " + "  ".join(c.ljust(w) for c, w in zip(configs, cw)))
        for t, row in rows:
            out.append(t.ljust(width) + "  " + "  ".join(s.ljust(w) for s, w in zip(row, cw)))
        out.append("")
        for c, tot in zip(configs, totals):
            out.append(f"{c}: {tot}; {results[c].get('adapter') or 'adapter not reported'}")
        for i, (t, c, n) in enumerate(notes, 1):
            out.append(f"[{i}] {c} {t}: {n}")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--configs", help="comma-separated ADAPTER-BACKEND[-minimal] names "
                    "(BACKEND dx12, vulkan, metal or auto), or `machine`: this machine's adapters")
    ap.add_argument("--workspace", action="store_true",
                    help="build every crate's tests, as cargo xtask check's test step does")
    ap.add_argument("--default-auto", action="store_true",
                    help="with --configs machine, the default adapter's auto configuration too")
    ap.add_argument("--bins", help="comma-separated test binaries (lib, water, ...)")
    ap.add_argument("--filter", help="a test-name filter passed to every binary")
    ap.add_argument("--debug", action="store_true", help="the debug profile instead of release")
    ap.add_argument("--no-ignored", action="store_true", help="leave out #[ignore] tests")
    ap.add_argument("--test-threads", type=int, help="libtest's --test-threads")
    ap.add_argument("--timeout", type=float, default=300,
                    help="seconds per binary (the slowest takes about a minute)")
    ap.add_argument("--build-only", action="store_true", help="build the binaries and stop")
    ap.add_argument("--out", default=str(ROOT / "out" / "gpu-matrix"), help="log directory")
    ap.add_argument("--json", help="write the results here")
    ap.add_argument("--merge", nargs="+", help="print the table of earlier --json results")
    ap.add_argument("--all", action="store_true", help="list tests that pass everywhere too")
    ap.add_argument("--markdown", action="store_true", help="the table as Markdown")
    args = ap.parse_args()
    if args.merge:
        results = merge(args.merge)
        print(table(results, args.all, args.markdown))
        bad = problems(results)
        for p in bad:
            print(f"error: {p}")
        return 1 if bad else 0
    commit = git("describe", "--always", "--dirty", "--abbrev=12")
    bins = build(args.debug, args.workspace)
    if args.build_only:
        for b, exe in sorted(bins.items()):
            print(f"{b}: {exe}")
        return 0
    args.bins = args.bins.split(",") if args.bins else None
    machine = None
    if args.configs == "machine":
        adapters, defaults = list_adapters(bins, args.timeout)
        configs, left_out = machine_configs(adapters, defaults, native_backends(),
                                            args.default_auto)
        machine = {"adapters": adapters, "defaults": defaults, "configs": configs,
                   "left_out": left_out}
        for a in adapters:
            print(f"adapter: {a['backend']} {a['index']} {a['type']} {a['name']}", flush=True)
        for why in left_out:
            print(f"left out: {why}", flush=True)
        print(f"configurations: {', '.join(configs) or 'none'}", flush=True)
    else:
        configs = (args.configs.split(",") if args.configs
                   else [f"default-{b}" for b in native_backends()])
    for c in configs:
        parse_config(c)
    out_dir = Path(args.out)
    started = time.strftime("%Y-%m-%d %H:%M:%S")
    inherited = {k: v for k, v in sorted(os.environ.items())
                 if k.startswith(("AMORIS_", "WGPU_")) and k not in ENV_KEYS}

    def write(results):
        if not args.json:
            return
        Path(args.json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json).write_text(json.dumps({
            "started": started,
            "profile": "debug" if args.debug else "release",
            "commit": commit,
            "built": {b: time.strftime("%Y-%m-%d %H:%M:%S",
                                       time.localtime(os.path.getmtime(exe)))
                      for b, exe in sorted(bins.items())},
            "environment": inherited,
            "driver_cache": str(cache_root(bins)),
            **({"machine": machine} if machine is not None else {}),
            "problems": findings(results),
            "configs": results,
        }, indent=1), encoding="utf-8")

    if not configs:
        print("no GPU adapter on this machine (software adapters left out): nothing to run")
        write({})
        return 3
    if DRIVER_CACHE in os.environ:
        print(f"note: {DRIVER_CACHE} is replaced by a directory per configuration and binary",
              flush=True)
    caches = driver_caches(bins, configs)
    results = {c: run_config(c, bins, args, out_dir, caches) for c in configs}
    print()
    print(table(results, args.all, args.markdown))
    bad = problems(results)
    for p in bad:
        print(f"error: {p}")
    write(results)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
