#!/usr/bin/env python3
"""Whether the depth prepass's and occlusion culling's auto modes decide right in Chrome
(docs/bench/prepass.md 6): runs viewport pages in headless Chrome through tools/web_bench.mjs,
polls them every 200 ms (each frame's stats, and the auto modes' state from the viewport's
`auto_stats`, crates/amoris-viewport/src/web.rs), and records every frame's prepass and occlusion
choice, the decisions, the frames they took, the readbacks' lag and the timestamps' resolution;
`native` runs many_cubes headless on the same scenes for comparison.

Build the viewport package from this tree first (`tools/build_viewport.sh`) and
`cargo build --release -p amoris-render --example many_cubes`. From the repository root:

    python tools/web_auto.py run --out out/webauto/runs.jsonl \\
        [--pages dense,sphere,occluders] [--gpus nvidia,780m] [--loops uncapped,vsync] \\
        [--chrome unsafe,default] [--modes auto] [--seconds 30] [--label NAME] \\
        [--canvas 1254x564] [--web DIR]
    python tools/web_auto.py native --out out/webauto/native.jsonl [--gpus nvidia,780m] \\
        [--backends dx12] [--frames 600]
    python tools/web_auto.py table [--before B.jsonl] [--after A.jsonl] [--truth T.jsonl] \
        [--native N.jsonl]

`run` serves this tree's `web/` (or `--web DIR`, another build's) itself on a free port. Each
run's raw polls go beside `--out` (`<out stem>/<run>.polls.json`); one summary line per run, with
Chrome's version and the canvas size, is appended to `--out`. `--gpus nvidia` passes
`--force_high_performance_gpu` (the RTX 5060), `780m` leaves Chrome's choice (the Radeon 780M on
this laptop); `--loops vsync` keeps Chrome's vsync (`VSYNC=1`), `uncapped` runs the page's frame
loop as fast as Chrome lets it (the benchmarks' setup); `--chrome default` leaves out
`--enable-unsafe-webgpu` (Chrome's own timestamp quantization). `--canvas` holds the page's
viewport at one size in every configuration (`VIEWPORT=`; in the same window a default Chrome's
page is 56 pixels taller than an unsafe one's). `--modes` lists what each run sets on the page:
`auto` nothing, `on` and `off` the prepass (fixed-mode runs, whose GPU times say which way is
cheaper in that browser), anything else the page's own parameters (`occlusion=on&prepass=off`).
"""

import argparse
import http.server
import json
import os
import socketserver
import statistics
import subprocess
import sys
import threading
import time
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

PAGES = {
    "dense": "viewport/?demo=cubes&dense",
    "sphere": "viewport/?demo=cubes",
    "occluders": "viewport/?demo=occluders",
}
OPAQUE = ("depth prepass", "opaque early", "opaque+sky")

# Every frame drawn since the last poll ([prepass, occlusion, frame_ms]; the page keeps its last
# 600 frames' stats, each marked once taken), the latest frame's stats and the auto modes' state
# (`auto_stats`; a build from before it carried the state in every frame's stats).
POLL_EXPR = r"""(() => {
  const s = window.amorisSamples || [];
  const code = (l) => (l === "auto-on" || l === "on" ? 1 : 0);
  const frames = [];
  for (const x of s) {
    if (x.__taken) continue;
    x.__taken = 1;
    frames.push([code(x.prepass), code(x.occlusion), +(x.frame_ms || 0).toFixed(3)]);
  }
  const vp = window.amorisViewport;
  const st = window.amorisStats;
  const auto = vp && vp.raw.auto_stats ? JSON.parse(vp.raw.auto_stats()) : st || {};
  return { t: Math.round(performance.now()), frames, window: s.length,
    stats: st ? { prepass_auto: auto.prepass_auto, occlusion_auto: auto.occlusion_auto,
      quantum_us: auto.timestamp_quantum_us, gpu_ms: st.gpu_ms, passes: st.passes,
      instances: st.instances, backend: st.backend, antialiasing: st.antialiasing,
      prepass: st.prepass, occlusion: st.occlusion } : null };
})()"""

ADAPTER_EVAL = r"""(async () => {
  const a = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  const c = document.querySelector("canvas");
  return { adapter: a ? `${a.info.vendor} ${a.info.architecture} ${a.info.description || ""}`.trim() : null,
    timestamps: a ? a.features.has("timestamp-query") : null, canvas: c ? [c.width, c.height] : null };
})()"""


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def serve(directory):
    handler = partial(Quiet, directory=str(directory))
    httpd = socketserver.ThreadingTCPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def conditions(path, label):
    subprocess.run([sys.executable, str(ROOT / "tools" / "machine_conditions.py"), str(path),
                    "--label", label, "--cpu-seconds", "2"], cwd=ROOT, capture_output=True)


def opaque_ms(passes):
    v = [p["ms"] for p in passes or [] if p["pass"] in OPAQUE]
    return sum(v) if v else None


def runs_of(labels):
    """[(value, first index, length)] of consecutive equal values."""
    out = []
    for i, v in enumerate(labels):
        if out and out[-1][0] == v:
            out[-1][2] += 1
        else:
            out.append([v, i, 1])
    return out


def summarize(polls):
    seq = [f for p in polls if p for f in p.get("frames", [])]
    # Polls that found all 600 frames the page keeps untaken may have missed older ones.
    full = sum(1 for p in polls if p and len(p.get("frames") or []) >= 600)
    states = [p["stats"] for p in polls if p and p.get("stats")]
    last = states[-1] if states else {}
    pa, oa = last.get("prepass_auto") or {}, last.get("occlusion_auto") or {}
    # Decisions as the polls saw them: (frame, kind, on) whenever a kind's decided/on changed.
    history, prev = [], {}
    for s in states:
        a = s.get("prepass_auto") or {}
        for kind in ("one", "two"):
            k = a.get(kind) or {}
            key = (k.get("decided"), k.get("on"), k.get("decisions"), k.get("timeouts"))
            if prev.get(kind) != key:
                if kind in prev:
                    history.append({"frame": a.get("frame"), "kind": kind, "on": k.get("on"),
                                    "decisions": k.get("decisions"),
                                    "timeouts": k.get("timeouts")})
                prev[kind] = key
    lags = [s["prepass_auto"]["lag"] for s in states
            if (s.get("prepass_auto") or {}).get("lag") is not None]
    olags = [s["occlusion_auto"]["lag"] for s in states
             if (s.get("occlusion_auto") or {}).get("lag") is not None]
    opaque = [o for o in (opaque_ms(s.get("passes")) for s in states) if o is not None]
    gpu = [s["gpu_ms"] for s in states if s.get("gpu_ms")]
    pre = [f[0] for f in seq]
    occ = [f[1] for f in seq]
    half = len(seq) // 2

    def kind(k):
        c = pa.get(k) or {}
        first, dec = c.get("first_probe"), c.get("first_decision")
        return {"on": c.get("on"), "decided": c.get("decided"), "decisions": c.get("decisions"),
                "timeouts": c.get("timeouts"), "first_probe": first, "first_decision": dec,
                "frames_to_decision": dec - first if dec is not None and first is not None else None,
                "probe": c.get("probe")}

    # Values of the opaque passes' readings: with quantized timestamps all are multiples of it.
    values = sorted({round(p["ms"], 3) for s in states for p in s.get("passes") or []
                     if p["pass"] in OPAQUE})
    return {
        "frames": len(seq), "frames_drawn": (pa.get("frame") or 0) + 1, "full_polls": full,
        "frame_ms_p50": statistics.median([f[2] for f in seq]) if seq else None,
        "prepass_frames_on": sum(pre), "prepass_frames_on_late_half": sum(pre[half:]),
        "prepass_runs": len(runs_of(pre)),
        "occlusion_frames_on": sum(occ), "occlusion_frames_on_late_half": sum(occ[half:]),
        "occlusion_runs": len(runs_of(occ)),
        "prepass": {"one": kind("one"), "two": kind("two")},
        "prepass_history": history,
        "occlusion": {k: oa.get(k) for k in ("on", "probes", "paid", "failed", "timeouts", "offs",
                                             "first_decision", "first_on", "brief")},
        "lag_p50": statistics.median(lags) if lags else None, "lag_max": pa.get("max_lag"),
        "occlusion_lag_p50": statistics.median(olags) if olags else None,
        "occlusion_lag_max": oa.get("max_lag"),
        "quantum_us": last.get("quantum_us"),
        "opaque_ms_p50": statistics.median(opaque) if opaque else None,
        "opaque_ms_min": min(opaque) if opaque else None,
        "gpu_ms_p50": statistics.median(gpu) if gpu else None,
        "opaque_values": values[:40], "opaque_values_count": len(values),
        "backend": last.get("backend"), "antialiasing": last.get("antialiasing"),
        "instances": last.get("instances"),
    }


def run(args):
    out = Path(args.out)
    raw = out.with_suffix("")
    raw.mkdir(parents=True, exist_ok=True)
    httpd = serve(Path(args.web) if args.web else ROOT / "web")
    base = f"http://127.0.0.1:{httpd.server_address[1]}"
    try:
        for gpu in args.gpus.split(","):
            conditions(raw / f"conditions-{args.label}-{gpu}-{int(time.time())}.json",
                       f"web_auto {args.label} {gpu}")
            for loop in args.loops.split(","):
                for chrome in args.chrome.split(","):
                    for page in args.pages.split(","):
                        for mode in args.modes.split(","):
                            one(args, base, raw, out, gpu, loop, chrome, page, mode)
    finally:
        httpd.shutdown()


def one(args, base, raw, out, gpu, loop, chrome, page, mode):
    tag = mode.replace("=", "-").replace("&", "+")
    name = f"{args.label}-{page}-{gpu}-{loop}-{chrome}-{tag}"
    url = f"{base}/{PAGES[page]}"
    if mode in ("on", "off"):
        url += f"&prepass={mode}"
    elif mode != "auto":
        url += f"&{mode}"
    polls = raw / f"{name}.polls.json"
    env = dict(os.environ, POLL="200", POLL_EXPR=POLL_EXPR, POLL_OUT=str(polls),
               EVAL=ADAPTER_EVAL)
    env.pop("VIEWPORT", None)
    if args.canvas:
        env["VIEWPORT"] = args.canvas
    env.pop("VSYNC", None)
    env.pop("CHROME_FLAGS", None)
    if gpu == "nvidia":
        env["CHROME_FLAGS"] = "--force_high_performance_gpu"
    if loop == "vsync":
        env["VSYNC"] = "1"
    env["UNSAFE_WEBGPU"] = "0" if chrome == "default" else "1"
    t0 = time.time()
    p = subprocess.run(["node", str(ROOT / "tools" / "web_bench.mjs"), url, str(args.seconds),
                        "1280", "720"], cwd=ROOT, env=env, capture_output=True, text=True,
                       timeout=args.seconds + 180)
    info, browser = None, None
    for line in p.stdout.splitlines():
        if line.startswith("eval:"):
            info = json.loads(line[5:])
        elif line.startswith("browser:"):
            browser = line[8:].strip()
    try:
        data = json.loads(polls.read_text())
    except (OSError, ValueError):
        data = []
    row = {"name": name, "page": page, "gpu": gpu, "loop": loop, "chrome": chrome, "mode": mode,
           "seconds": args.seconds, "started": time.strftime("%Y-%m-%d %H:%M:%S",
                                                             time.localtime(t0)),
           "url": url.split("/", 3)[-1], "browser": browser, "adapter": info,
           "canvas": (info or {}).get("canvas"), **summarize(data)}
    if not data:
        row["error"] = (p.stdout[-600:] + p.stderr[-600:]).strip()
    with open(out, "a", encoding="utf-8") as f:
        f.write(json.dumps(row) + "\n")
    print(f"{name}: frames {row['frames']}, prepass one {row['prepass']['one']}, "
          f"occlusion {row['occlusion']}, lag {row['lag_p50']}, quantum {row['quantum_us']}",
          flush=True)


def native(args):
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    exe = Path(args.exe) if args.exe else (
        ROOT / "target" / "release" / "examples" / ("many_cubes.exe" if os.name == "nt"
                                                     else "many_cubes"))
    for gpu in args.gpus.split(","):
        for backend in args.backends.split(","):
            conditions(out.parent / f"native-conditions-{gpu}-{backend}-{int(time.time())}.json",
                       f"web_auto native {gpu} {backend}")
            for page in ("dense", "sphere"):
                env = dict(os.environ, AMORIS_AA="msaa", AMORIS_GTAO="off",
                           AMORIS_BACKEND=backend)
                env.pop("AMORIS_ADAPTER", None)
                if gpu == "780m":
                    env["AMORIS_ADAPTER"] = "780m"
                cmd = [str(exe), "--headless-bench", str(args.frames), "--size", args.size,
                       "--settle", "5"] + (["--dense"] if page == "dense" else [])
                p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True,
                                   timeout=1800)
                rep = None
                for line in p.stdout.splitlines():
                    if line.startswith("{"):
                        rep = json.loads(line)
                row = {"name": f"{args.label}-{page}-{gpu}-{backend}", "page": page, "gpu": gpu,
                       "loop": "native", "chrome": backend, "mode": "auto",
                       "started": time.strftime("%Y-%m-%d %H:%M:%S"), "report": rep}
                if rep is None:
                    row["error"] = (p.stdout[-600:] + p.stderr[-600:]).strip()
                with open(out, "a", encoding="utf-8") as f:
                    f.write(json.dumps(row) + "\n")
                print(row["name"], json.dumps((rep or {}).get("prepass_auto")),
                      json.dumps((rep or {}).get("occlusion_auto")), flush=True)


def rows_of(paths, latest=True):
    """The rows of one or more (comma-separated) files; with `latest`, a later row replaces an
    earlier one with the same page, GPU, loop, Chrome and mode."""
    rows = {}
    for path in paths.split(","):
        for line in Path(path).read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                k = (r["page"], r["gpu"], r["loop"], r["chrome"], r["mode"])
                rows[k if latest else len(rows)] = r
    return list(rows.values())


def kind_of(r):
    """The kind of frame most of a run's frames were: two opaque phases where occlusion culling ran."""
    if "report" in r:
        frames = (r["report"] or {}).get("occlusion_frames") or {}
        on = sum(n for m, n in frames.items() if m in ("on", "auto-on"))
        return "two" if on * 2 > sum(frames.values()) else "one"
    return "two" if r["occlusion_frames_on"] * 2 > r["frames"] else "one"


def decision(c):
    if not c:
        return "-"
    if c.get("decided") is None:
        # A build without the auto state (master's viewport before `auto_stats`).
        return "not reported"
    t = f", {c['timeouts']} timeouts" if c.get("timeouts") else ""
    if not c.get("decided"):
        return f"none{t}"
    ftd = c.get("frames_to_decision")
    if ftd is None and c.get("first_decision") is not None:
        ftd = c["first_decision"] - c["first_probe"]
    return f"{'on' if c['on'] else 'off'} at {ftd}{t}"


def setup(rows):
    """The canvases and Chrome versions a table's rows ran with."""
    canvas = sorted({"x".join(map(str, r.get("canvas") or (r.get("adapter") or {}).get("canvas")
                                  or ["?"])) for r in rows})
    browsers = sorted({r.get("browser") or "not recorded" for r in rows})
    return f"Canvas {', '.join(canvas)}; {', '.join(browsers)}."


def table(args):
    """Markdown tables: the fixed modes (`--truth`: the prepass's opaque passes, occlusion
    culling's GPU frame), decisions before and after (`--before`, `--after`, each one or more
    files), the native runs (`--native`)."""
    key = lambda r: (r["page"], r["gpu"], r["loop"], r["chrome"])
    after = {key(r): r for r in rows_of(args.after) if r["mode"] == "auto"} if args.after else {}
    if args.truth:
        truth = rows_of(args.truth)
        t = {}
        for r in truth:
            t.setdefault((r["page"], r["gpu"], r["loop"]), {})[r["mode"]] = r
        pick = lambda m, mode: next((r for k, r in m.items() if k == mode or mode in k.split("&")),
                                    None)
        rows = [(k, pick(m, "on"), pick(m, "off")) for k, m in t.items()]
        rows = [(k, on, off) for k, on, off in rows if on and off]
        if rows:
            print("| Page | GPU | Loop | Opaque ms, prepass on | Off | Cheaper |")
            print("|---|---|---|---|---|---|")
            for (page, gpu, loop), on, off in rows:
                on, off = on["opaque_ms_p50"], off["opaque_ms_p50"]
                print(f"| {page} | {gpu} | {loop} | {on:.3f} | {off:.3f} | "
                      f"{'on' if on < off * 0.95 else 'off'} ({(on / off - 1) * 100:+.0f}%) |")
            print()
        rows = [(k, pick(m, "occlusion=on"), pick(m, "occlusion=off")) for k, m in t.items()]
        rows = [(k, on, off) for k, on, off in rows if on and off]
        if rows:
            print("| Page | GPU | Loop | GPU ms, occlusion on | Off | Cheaper | Auto, unsafe: "
                  "frames on, last half | Default |")
            print("|---|---|---|---|---|---|---|---|")
            for (page, gpu, loop), on, off in rows:
                cells = []
                for chrome in ("unsafe", "default"):
                    a = after.get((page, gpu, loop, chrome))
                    cells.append(f"{'on' if a['occlusion'].get('on') else 'off'}, "
                                 f"{200 * a['occlusion_frames_on_late_half'] / max(a['frames'], 1):.0f}%"
                                 if a else "-")
                on, off = on["gpu_ms_p50"], off["gpu_ms_p50"]
                print(f"| {page} | {gpu} | {loop} | {on:.3f} | {off:.3f} | "
                      f"{'on' if on < off else 'off'} ({(on / off - 1) * 100:+.0f}%) | "
                      f"{cells[0]} | {cells[1]} |")
            print()
        print(setup(truth))
        print()
    if args.before or args.after:
        before = ({key(r): r for r in rows_of(args.before) if r["mode"] == "auto"}
                  if args.before else {})
        print("| Page | GPU | Loop | Chrome | Lag p50 (max) | Quantum, us | Kind | Before: decision "
              "at frames; prepass frames | After |")
        print("|---|---|---|---|---|---|---|---|---|")
        for k in dict.fromkeys(list(before) + list(after)):
            b, a = before.get(k), after.get(k)
            r = a or b
            kind = kind_of(r)
            cell = lambda x: (f"{decision(x['prepass'][kind])}; "
                              f"{100 * x['prepass_frames_on'] / max(x['frames'], 1):.0f}%"
                              if x else "-")
            lag = f"{r['lag_p50']:.0f} ({r['lag_max']})" if r.get("lag_p50") is not None else "-"
            print(f"| {k[0]} | {k[1]} | {k[2]} | {k[3]} | {lag} | {r['quantum_us']} | {kind} | "
                  f"{cell(b)} | {cell(a)} |")
        print()
        if before:
            print("Before: " + setup(before.values()))
        if after:
            print("After: " + setup(after.values()))
        print()
    if args.native:
        print("| Run | Kind | Prepass: decision at frames | Lag (max) | Occlusion: first decision "
              "| Prepass frames |")
        print("|---|---|---|---|---|---|")
        for r in rows_of(args.native, latest=False):
            rep = r.get("report") or {}
            pa, oa = rep.get("prepass_auto") or {}, rep.get("occlusion_auto") or {}
            kind = kind_of(r)
            print(f"| {r['name']} | {kind} | {decision(pa.get(kind))} | {pa.get('lag')} "
                  f"({pa.get('max_lag')}) | {'on' if oa.get('first_on') is not None else 'off'} "
                  f"at {oa.get('first_decision')} | {rep.get('prepass_frames')} |")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--out", required=True)
    r.add_argument("--pages", default="dense,sphere,occluders")
    r.add_argument("--gpus", default="nvidia,780m")
    r.add_argument("--loops", default="uncapped,vsync")
    r.add_argument("--chrome", default="unsafe,default")
    r.add_argument("--modes", default="auto")
    r.add_argument("--seconds", type=int, default=30)
    r.add_argument("--label", default="run")
    # The viewport page's canvas in a 1280x720 headless Chrome window with --enable-unsafe-webgpu.
    r.add_argument("--canvas", default="1254x564", help="WxH, or '' for the window's own")
    r.add_argument("--web", help="the web/ directory to serve (default: this tree's)")
    n = sub.add_parser("native")
    n.add_argument("--out", required=True)
    n.add_argument("--gpus", default="nvidia,780m")
    n.add_argument("--backends", default="dx12")
    n.add_argument("--frames", type=int, default=600)
    # The viewport page's canvas in a 1280x720 headless Chrome window.
    n.add_argument("--size", default="1254x564")
    n.add_argument("--exe", help="many_cubes to run (default: this tree's release build)")
    n.add_argument("--label", default="native")
    t = sub.add_parser("table")
    t.add_argument("--before")
    t.add_argument("--after")
    t.add_argument("--truth")
    t.add_argument("--native")
    a = ap.parse_args()
    {"run": run, "native": native, "table": table}[a.cmd](a)


if __name__ == "__main__":
    main()
