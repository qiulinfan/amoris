#!/usr/bin/env python3
"""Measure how the engine loads and draws the OBJ stress files (generate.py makes them; the cases are
in cases.json). Each case runs in a fresh release runtime at 1280x720, headless and paused:

1. a camera is spawned (priority 100, look_at) and two frames drawn;
2. `world.spawn {Model: Transform, MeshRenderer {mesh}}` and one `step {render: "each"}` are timed:
   that is the load time (reading, parsing, tangents, upload and the first frame);
3. thirty more frames, then render.stats, a capture and assets.describe;
4. the model turned half a degree a frame for ten frames, so the shadow cascades are drawn again,
   then render.stats and perf once more; then each variant (world.set calls, five frames, a capture).

Memory: the runtime's resident size is sampled every 100 ms (ps), and on macOS the runtime runs under
`/usr/bin/time -l` for its maximum resident size and peak memory footprint. Results go to
build/objstress/runs/logs/<case>.json, pictures to build/objstress/runs/captures/. Do not measure while an
agent benchmark or a build runs: the timings in the 2026-10-02 assessment were taken beside one and
are noisy.

    python3 tools/scripts/dev/objstress/measure.py <case> ['<json spec>']   one case (cases.json, or the spec given)
    python3 tools/scripts/dev/objstress/measure.py all                      every case whose file exists
    python3 tools/scripts/dev/objstress/measure.py freeze assets/scan_5m.obj   whether the runtime answers while it loads
    python3 tools/scripts/dev/objstress/measure.py split assets/x.obj ...    assets.import alone, then spawn and the first frame
    python3 tools/scripts/dev/objstress/measure.py profile assets/x.obj 8   macOS `sample` of the main thread during a load

References for the same files: build/objstress/bin/fastobj_bench <file.obj> (a plain C reader, the
lower bound for parsing), and Blender's importer,
    <Blender> -b --factory-startup --python tools/scripts/dev/objstress/blender_import.py -- <file.obj>
    <Blender> -b --factory-startup --python tools/scripts/dev/objstress/blender_render.py -- <file.obj> <out.png> '<json: cam_pos, cam_at, up, scale, location>'
(<Blender> is /Applications/Blender.app/Contents/MacOS/Blender on macOS).
"""
import json
import os
import platform
import subprocess
import sys
import threading
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import runtime as rt  # noqa: E402

OUT = os.path.join(rt.ROOT, "build", "objstress")
PROJECT = OUT   # the project is the directory itself: runtime.py finds its bundle by the directory's name
LOGS = os.path.join(OUT, "runs", "logs")
CAPTURES = os.path.join(OUT, "runs", "captures")
CASES = json.load(open(os.path.join(HERE, "cases.json"), encoding="utf-8"))
CAMERA = {"Transform": {"position": {"x": 0.2, "y": 0.35, "z": 1.6}, "look_at": {"x": 0, "y": 0, "z": 0}}, "Camera": {"priority": 100}}


class Runtime(rt.Runtime):
    """The dev helper's runtime with a long rpc timeout: a 12M-triangle load blocks for half a minute."""

    def rpc(self, method, params=None):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}/rpc", data=body, headers={"Content-Type": "application/json"})
        answer = json.loads(urllib.request.urlopen(req, timeout=900).read())
        if "error" in answer:
            raise RuntimeError(f"{method}: {answer['error']}")
        return answer["result"]


def timed_exe(time_out):
    """A wrapper that runs the release runtime under /usr/bin/time (macOS -l, GNU -v)."""
    exe = os.path.join(rt.ROOT, "build", "release", "bin", "pocket_runtime")
    wrapper = os.path.join(OUT, "bin", "timed_runtime.sh")
    os.makedirs(os.path.dirname(wrapper), exist_ok=True)
    flag = "-l" if platform.system() == "Darwin" else "-v"
    with open(wrapper, "w", encoding="utf-8", newline="\n") as f:
        f.write(f'#!/bin/sh\nexec /usr/bin/time {flag} -o "{time_out}" "{exe}" "$@"\n')
    os.chmod(wrapper, 0o755)
    return wrapper


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True, encoding="utf-8", errors="replace").stdout.strip()
    return int(out) / 1024 if out else 0.0


def start(port, exe=None):
    return Runtime(PROJECT, port=port, size="1280x720", release=True, exe=exe)


def measure(name, spec):
    port = spec.get("port", 4791)
    res = {"case": name, "mesh": spec["mesh"]}
    res["file_mb"] = round(os.path.getsize(os.path.join(PROJECT, spec["mesh"])) / 1e6, 1)
    time_out = os.path.join(LOGS, name + ".time.txt")
    t_start = time.time()
    r = start(port, timed_exe(time_out))
    children = subprocess.run(["pgrep", "-P", str(r.proc.pid)], capture_output=True, text=True, encoding="utf-8", errors="replace").stdout.split()
    pid = int(children[0]) if children else r.proc.pid
    peak = {"mb": 0.0, "series": []}
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            m = rss_mb(pid)
            peak["mb"] = max(peak["mb"], m)
            peak["series"].append((round(time.time() - t_start, 2), round(m)))
            time.sleep(0.1)
    threading.Thread(target=sample, daemon=True).start()
    try:
        r.rpc("step", {"ticks": 1})
        cam = {"Transform": {"position": spec["cam_pos"], "look_at": spec["cam_at"]},
               "Camera": {"priority": 100, "fov_degrees": spec.get("fov", 45), "near": spec.get("near", 0.05), "far": spec.get("far", 200)}}
        r.rpc("world.spawn", {"name": "StressCam", "components": cam})
        r.rpc("step", {"ticks": 2, "render": "each"})
        res["baseline_rss_mb"] = round(rss_mb(pid))
        tf = {"position": {"x": 0, "y": 0, "z": 0}}
        tf.update(spec.get("transform", {}))
        t0 = time.time()
        r.rpc("world.spawn", {"name": "Model", "components": {"Transform": tf, "MeshRenderer": dict({"mesh": spec["mesh"]}, **spec.get("mr_extra", {}))}})
        t1 = time.time()
        r.rpc("step", {"ticks": 1, "render": "each"})
        t2 = time.time()
        res.update(spawn_s=round(t1 - t0, 3), first_frame_s=round(t2 - t1, 3), load_s=round(t2 - t0, 3))
        r.rpc("step", {"ticks": spec.get("extra_steps", 30), "render": "each"})
        res["stats"] = r.rpc("render.stats")
        res["capture"] = os.path.join(CAPTURES, name + ".png")
        r.rpc("capture", {"path": res["capture"]})
        res["describe"] = r.rpc("assets.describe", {"path": spec["mesh"]})
        base_rot = spec.get("transform", {}).get("rotation", {"yaw": 0})
        r.rpc("perf", {"reset": True})
        for k in range(10):
            rot = dict(base_rot)
            rot["yaw"] = rot.get("yaw", 0) + (k + 1) * 0.5
            r.rpc("world.set", {"entity": "Model", "components": {"Transform": {"rotation": rot}}})
            r.rpc("step", {"ticks": 1, "render": "each"})
        res["moving_stats"] = r.rpc("render.stats")
        res["moving_perf"] = r.rpc("perf")
        for variant in spec.get("variants", []):
            for call in variant["set"]:
                r.rpc("world.set", call)
            r.rpc("step", {"ticks": 5, "render": "each"})
            path = os.path.join(CAPTURES, f"{name}_{variant['name']}.png")
            r.rpc("capture", {"path": path})
            res.setdefault("variant_captures", []).append(path)
        res["end_rss_mb"] = round(rss_mb(pid))
    finally:
        stop.set()
        r.close()
    res["peak_rss_mb"] = round(peak["mb"])
    res["rss_series"] = peak["series"][::5]
    try:
        text = open(time_out, encoding="utf-8").read()
        res["time"] = text
        for line in text.splitlines():
            if "peak memory footprint" in line:
                res["peak_footprint_mb"] = round(int(line.split()[0]) / 1e6)
    except OSError:
        pass
    json.dump(res, open(os.path.join(LOGS, name + ".json"), "w", encoding="utf-8", newline="\n"), indent=1)
    s, m = res["stats"], res["moving_stats"]
    print(json.dumps({"case": name, "file_mb": res["file_mb"], "load_s": res["load_s"], "peak_rss_mb": res["peak_rss_mb"],
                      "triangles": s.get("triangles"), "draws": s.get("draw_calls"), "gpu_ms": (s.get("gpu") or {}).get("ms"),
                      "gpu_ms_moving": (m.get("gpu") or {}).get("ms"), "capture": res["capture"]}))


def freeze(mesh):
    """Whether the runtime answers a `state` call sent half a second into a load."""
    r = start(4794)
    try:
        r.rpc("step", {"ticks": 1})
        r.rpc("world.spawn", {"name": "StressCam", "components": CAMERA})
        r.rpc("step", {"ticks": 1, "render": "each"})
        t0 = time.time()

        def load():
            r.rpc("world.spawn", {"name": "Model", "components": {"Transform": {}, "MeshRenderer": {"mesh": mesh}}})
            r.rpc("step", {"ticks": 1, "render": "each"})
            print("load done at", round(time.time() - t0, 2), "s", flush=True)
        th = threading.Thread(target=load)
        th.start()
        time.sleep(0.5)
        t1 = time.time()
        r.rpc("state")
        print("a state call sent at", round(t1 - t0, 2), "s was answered after", round(time.time() - t1, 2), "s", flush=True)
        th.join()
    finally:
        r.close()


def split(meshes):
    """assets.import alone (reading, parsing, tangents), then the spawn, upload and first frame."""
    for mesh in meshes:
        r = start(4792)
        try:
            r.rpc("step", {"ticks": 1})
            r.rpc("world.spawn", {"name": "StressCam", "components": CAMERA})
            r.rpc("step", {"ticks": 1, "render": "each"})
            t0 = time.time()
            r.rpc("assets.import", {"path": mesh})
            t1 = time.time()
            r.rpc("world.spawn", {"name": "Model", "components": {"Transform": {}, "MeshRenderer": {"mesh": mesh}}})
            r.rpc("step", {"ticks": 1, "render": "each"})
            print(mesh, {"import_s": round(t1 - t0, 3), "spawn_upload_first_frame_s": round(time.time() - t1, 3)}, flush=True)
        finally:
            r.close()


def profile(mesh, seconds):
    """macOS `sample` of the runtime while it loads; the report names the hot lines (import.cpp)."""
    r = start(4793)
    try:
        r.rpc("step", {"ticks": 1})
        r.rpc("world.spawn", {"name": "StressCam", "components": CAMERA})
        r.rpc("step", {"ticks": 1, "render": "each"})
        out = os.path.join(LOGS, "sample_" + os.path.basename(mesh) + ".txt")
        sp = subprocess.Popen(["/usr/bin/sample", str(r.proc.pid), str(seconds), "1", "-mayDie", "-file", out],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.5)
        t0 = time.time()
        r.rpc("world.spawn", {"name": "Model", "components": {"Transform": {}, "MeshRenderer": {"mesh": mesh}}})
        r.rpc("step", {"ticks": 1, "render": "each"})
        print("load_s", round(time.time() - t0, 2))
        sp.wait()
        print(out)
    finally:
        r.close()


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    os.makedirs(LOGS, exist_ok=True)
    os.makedirs(CAPTURES, exist_ok=True)
    what = sys.argv[1]
    if what == "freeze":
        freeze(sys.argv[2])
    elif what == "split":
        split(sys.argv[2:])
    elif what == "profile":
        profile(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "8")
    elif what == "all":
        for name, spec in CASES.items():
            if not name.startswith("_") and os.path.exists(os.path.join(PROJECT, spec["mesh"])):
                measure(name, spec)
    else:
        measure(what, json.loads(sys.argv[2]) if len(sys.argv) > 2 else CASES[what])


if __name__ == "__main__":
    main()
