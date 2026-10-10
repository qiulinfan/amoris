#!/usr/bin/env python3
"""Record genuine native frames and a scripted MCP sailing session.

Run a real host for site/demos/harbor before this script. Control calls reuse the
repository's tested MCP client. Capture uses the host's native offscreen renderer.
The pilot runs outside the simulation and has developer privileges, not player MCP.
"""
from pathlib import Path
import argparse
import json
import math
import shutil
import subprocess
import urllib.request

from smoke_server import MCP, call

ROOT = Path(__file__).resolve().parents[1]


def http_call(base, method, params=None):
    response = call(base, method, params)
    if "error" in response:
        raise RuntimeError(json.dumps(response["error"]))
    return response["result"]


class Pilot:
    def __init__(self, base):
        self.base = base
        self.client = MCP(base)
        response = self.client.rpc("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "Amoris scripted showcase pilot", "version": "1"},
        })
        assert response["result"]["serverInfo"]["name"] == "pocket"
        self.client.rpc("notifications/initialized", notify=True)
        self.calls = 0
        self.remaining = ["Crate1", "Crate2", "Crate3", "Crate4"]
        self.target = None
        self.rudder = 0.0
        self.decisions = []

    def tool(self, name, arguments):
        response = self.client.rpc("tools/call", {"name": name, "arguments": arguments})
        self.calls += 1
        result = response["result"]
        if result.get("isError"):
            raise RuntimeError(result["content"][0]["text"])
        return json.loads(result["content"][0]["text"])

    def boat(self):
        return self.tool("world", {"action": "get", "entity": "Sloop", "components": ["Transform", "Boat", "Tally"]})

    def edit(self, component, value, label):
        return self.tool("world", {"action": "edit", "ops": [
            {"set": {"entity": "Sloop", "component": component, "value": value}},
        ], "label": label})

    def decide(self, frame):
        boat = self.boat()
        p = boat["components"]["Transform"]["position"]
        if not self.remaining:
            self.edit("Boat", {"rudder": -0.15, "sheet": 0.8}, "Sail beyond the completed course")
            return
        positions = []
        for name in self.remaining:
            crate = self.tool("world", {"action": "get", "entity": name, "components": ["Transform"]})
            target = crate["components"]["Transform"]["position"]
            positions.append((math.hypot(p[0] - target[0], p[2] - target[2]), name, target))
        distance, name, target = min(positions)
        self.target = name
        if distance < 2.85:
            self.edit("Crew", {"take": name}, "Take the reachable crate aboard")
            self.tool("time", {"action": "step", "ticks": 1})
            self.remaining.remove(name)
            self.decisions.append({"frame": frame, "kind": "collect", "target": name, "distance_m": distance})
            return

        base_hash = http_call(self.base, "status")["world_hash"]
        trials = []
        # Explicit, bounded look-ahead forks. They are not the implementation of a tick or edit.
        for rudder in [-0.9, -0.3, 0.3, 0.9]:
            self.tool("play", {"action": "start", "paused": True})
            self.edit("Boat", {"rudder": rudder, "sheet": 1, "hoist": 1}, "Evaluate a steering fork")
            self.tool("time", {"action": "step", "ticks": 45})
            future = self.boat()
            q = future["components"]["Transform"]["position"]
            gap = math.hypot(q[0] - target[0], q[2] - target[2])
            desired = math.degrees(math.atan2(target[0] - q[0], -(target[2] - q[2]))) % 360
            heading = future["components"]["Boat"]["heading_deg"]
            error = abs((desired - heading + 180) % 360 - 180)
            # Distance dominates; orientation discourages approaching while facing away.
            trials.append({"rudder": rudder, "distance_m": gap, "heading_error_deg": error, "score": gap + 0.014 * error})
            self.tool("play", {"action": "stop"})
            assert http_call(self.base, "status")["world_hash"] == base_hash
        chosen = min(trials, key=lambda trial: trial["score"])
        self.rudder = chosen["rudder"]
        self.edit("Boat", {"rudder": self.rudder, "sheet": 1, "hoist": 1}, "Apply the chosen steering action")
        self.decisions.append({"frame": frame, "kind": "steer", "target": name, "distance_m": distance,
                               "rudder": self.rudder, "trials": trials, "edit_hash_restored": True})


def encode(frames, output, fps):
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-framerate", str(fps),
        "-i", str(frames / "%05d.png"), "-c:v", "libx264", "-preset", "medium",
        "-crf", "22", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output),
    ], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="http://127.0.0.1:8768")
    parser.add_argument("--frames", type=int, default=360)
    parser.add_argument("--fps", type=int, default=30)
    parser.add_argument("--name", default="harbor")
    args = parser.parse_args()
    output = ROOT / "site/media"
    output.mkdir(parents=True, exist_ok=True)
    frames = ROOT / "out/showcase" / (args.name + "-frames")
    frames.mkdir(parents=True, exist_ok=True)
    pilot = Pilot(args.host)
    http_call(args.host, "snapshots.restore", {"tick": 0})
    http_call(args.host, "time.step", {"ticks": 60})
    trace = []
    for index in range(args.frames):
        if index % 15 == 0:
            pilot.decide(index)
        pilot.tool("time", {"action": "step", "ticks": 2})
        boat = pilot.boat()
        p = boat["components"]["Transform"]["position"]
        u = index / max(1, args.frames - 1)
        angle = 0.18 + 0.7 * u
        eye = [p[0] - 10 * math.cos(angle), 5.8 - 0.3 * u, p[2] + 10 * math.sin(angle) + 6]
        target = [p[0] + 1, 2.8, p[2] - 1]
        capture = http_call(args.host, "capture", {"position": eye, "look_at": target, "fov_deg": 44,
                                                   "width": 1280, "height": 720})
        assert capture["pending_assets"] == 0
        shutil.copy2(capture["path"], frames / f"{index:05d}.png")
        status = http_call(args.host, "status")
        trace.append({"frame": index, "video_s": index / args.fps, "tick": status["tick"],
                      "world_hash": status["world_hash"], "target": pilot.target, "rudder": pilot.rudder,
                      "boat": boat["components"]["Boat"], "position": p,
                      "tally": boat["components"]["Tally"], "mcp_calls": pilot.calls,
                      "capture_tick": capture["tick"], "instances": capture["instances"]})
        if index % 30 == 0:
            print(f"frame {index}/{args.frames}: tick {status['tick']}, cargo {trace[-1]['tally']['taken']}/4, target {pilot.target}", flush=True)
    video = output / (args.name + ".mp4")
    encode(frames, video, args.fps)
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(video),
                    "-frames:v", "1", "-q:v", "3", "-update", "1", str(output / (args.name + "-poster.jpg"))], check=True)
    (ROOT / "out/showcase" / (args.name + "-trace.json")).write_text(json.dumps({"frames": trace, "decisions": pilot.decisions}, indent=2) + "\n")
    receipt = {"file": args.name + ".mp4", "kind": "actual_native_capture", "transport": "scripted MCP developer client",
               "frames": args.frames, "fps": args.fps, "size": [1280, 720], "seed": 1,
               "engine_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
               "cargo_taken": trace[-1]["tally"]["taken"], "mcp_calls": pilot.calls}
    (ROOT / "out/showcase" / (args.name + "-receipt.json")).write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    main()
