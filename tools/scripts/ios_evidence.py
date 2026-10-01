#!/usr/bin/env python3
"""The iOS Simulator evidence (tests/evidence/ios/): samples packed as apps (`pocket run <name>
--ios`), started paused with their control server open, stepped and driven from the Mac through it
(a simulator shares the Mac's network), and photographed with `simctl io screenshot`.

Needs Xcode with an iOS simulator runtime (`xcodebuild -downloadPlatform iOS`) and the iOS
dependencies (`pocket setup --target ios-sim`):
    python3 tools/scripts/ios_evidence.py [--device "iPhone 18 Pro"]
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "dev"))
from runtime import ENV, ROOT  # noqa: E402

OUT = os.path.join(ROOT, "tests", "evidence", "ios")
XCODE = os.environ.get("POCKET_XCODE", "/Applications/Xcode.app/Contents/Developer")
PORT = 4731   # each sample's app on its own port, one after another


def rpc(method, params=None, timeout=60):
    body = json.dumps({"id": 1, "method": method, "params": params or {}}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}/rpc", data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        answer = json.loads(r.read())
    if "error" in answer:
        raise RuntimeError(f"{method}: {answer['error']}")
    return answer["result"]


def wait_for_server(seconds=60):
    end = time.time() + seconds
    while time.time() < end:
        try:
            return rpc("window.info", timeout=2)
        except Exception:  # noqa: BLE001 - not up yet
            time.sleep(0.5)
    raise RuntimeError("the app's control server did not come up")


def simctl(*args):
    subprocess.run(["xcrun", "simctl", *args], env={**os.environ, "DEVELOPER_DIR": XCODE}, check=True, capture_output=True)


def screenshot(udid, path):
    simctl("io", udid, "screenshot", path)


def run(sample, device):
    """The sample's app started paused with its control server on a port of its own; answers the
    device, the app and what the window is."""
    global PORT
    PORT += 1
    started = subprocess.run([os.path.join(ROOT, ".pocket", "pocket"), "run", sample, "--ios", "--device", device, "--json", "--", "--serve", str(PORT), "--paused"],
                             env=ENV, cwd=ROOT, capture_output=True, text=True)
    report = json.loads(started.stdout)
    if not report.get("ok"):
        raise RuntimeError(f"{sample}: {report.get('summary')}")
    window = wait_for_server()
    if window.get("title", "").lower().find(sample) < 0:
        raise RuntimeError(f"{sample}: another app answered on port {PORT} ({window.get('title')})")
    return report["data"]["udid"], report["data"]["bundle_id"], window


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--device", default="iPhone 18 Pro")
    a = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    record = {"device": a.device, "samples": {}}

    # hello: two seconds stepped from the Mac, the ball's bounces read back.
    udid, app, window = run("hello", a.device)
    rpc("step", {"ticks": 120, "render": True})
    record["samples"]["hello"] = {"window": window, "state": rpc("state")["state"], "gpu": rpc("report")["gpu"]}
    screenshot(udid, os.path.join(OUT, "hello.png"))
    simctl("terminate", udid, app)

    # crates: the car driven right through its action for a second and a half, the ball let go.
    udid, app, window = run("crates", a.device)
    rpc("step", {"ticks": 30, "render": True})
    before = rpc("state")["state"]
    rpc("input.hold", {"action": "move_x", "ticks": 90})
    rpc("input.press", {"action": "fire"})
    rpc("step", {"ticks": 150, "render": True})
    after = rpc("state")["state"]
    record["samples"]["crates"] = {"window": window, "before": before, "after": after}
    screenshot(udid, os.path.join(OUT, "crates.png"))
    simctl("terminate", udid, app)

    # walker: the hero walked forward (-z, W) for a second.
    udid, app, window = run("walker", a.device)
    rpc("step", {"ticks": 30, "render": True})
    start = rpc("state")["state"]
    rpc("input.hold", {"action": "move_z", "sign": -1, "ticks": 60})
    rpc("step", {"ticks": 90, "render": True})
    walked = rpc("state")["state"]
    record["samples"]["walker"] = {"window": window, "before": {k: start[k] for k in ("player.x", "player.z")}, "after": {k: walked[k] for k in ("player.x", "player.z", "hero.clip")}}
    screenshot(udid, os.path.join(OUT, "walker.png"))
    simctl("terminate", udid, app)

    with open(os.path.join(OUT, "ios.json"), "w") as f:
        json.dump(record, f, indent=2)
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
