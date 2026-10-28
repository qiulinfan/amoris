#!/usr/bin/env python3
"""Make Wind Road showcase media from actual native frames and model tool records.

Raw frames, action records and replay verification stay in ignored out/. The public output is
video, captions, a poster and concise media metadata. Tool overlays contain no private reasoning.
Requires ffmpeg, ffprobe and rsvg-convert, as the existing showcase recording tools do.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
from fractions import Fraction
from html import escape
import json
from pathlib import Path
import shutil
import subprocess
import sys
import textwrap

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "tools/eval")]
from agent_dev_bench import Host
from windward_showcase import Session


def run(*args):
    subprocess.run(list(map(str, args)), check=True)


def overlay(path, title, subtitle, lines, footer):
    svg = ['<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720">',
           '<rect x="0" y="0" width="1280" height="90" fill="#1d1c1f"/>',
           '<rect x="960" y="90" width="320" height="540" fill="#262428"/>',
           '<rect x="0" y="630" width="1280" height="90" fill="#1d1c1f"/>',
           f'<text x="24" y="36" fill="#ece7e2" font-family="sans-serif" font-size="24">{escape(title)}</text>',
           f'<text x="24" y="65" fill="#bbc5b9" font-family="sans-serif" font-size="15">{escape(subtitle)}</text>']
    rows = []
    for line in lines:
        rows.extend(textwrap.wrap(line, 32, break_long_words=True) or [""])
    for i, line in enumerate(rows[:24]):
        svg.append(f'<text x="978" y="{120+i*21}" fill="#ece7e2" font-family="monospace" font-size="13">{escape(line)}</text>')
    for i, line in enumerate(textwrap.wrap(footer, 145)):
        svg.append(f'<text x="24" y="{661+i*21}" fill="#b5aca6" font-family="sans-serif" font-size="14">{escape(line)}</text>')
    svg.append('</svg>')
    src = path.with_suffix(".svg")
    src.write_text("\n".join(svg))
    run("rsvg-convert", "-o", path, src)


def finish(name, work, caption, note):
    media = ROOT / "site/media"
    target = media / f"{name}.mp4"
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-framerate", "30", "-i",
        work / "frames/%05d.png", "-framerate", "30", "-i", work / "panels/%05d.png",
        "-filter_complex", "[0:v]scale=960:540,pad=1280:720:0:90:color=0x1d1c1f[v];[v][1:v]overlay=0:0:shortest=1",
        "-c:v", "libx264", "-preset", "medium", "-crf", "22", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart", target)
    publish(name, caption, note)


def publish(name, caption, note):
    media = ROOT / "site/media"
    video = media / f"{name}.mp4"
    run("ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i", video,
        "-frames:v", "1", "-q:v", "3", "-update", "1", media / f"{name}-poster.jpg")
    probe = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=width,height,avg_frame_rate,nb_frames:format=duration", "-of", "json", str(video)]))
    stream = probe["streams"][0]
    duration = float(probe["format"]["duration"])
    end = int(duration * 1000)
    stamp = lambda ms: f"{ms//3600000:02d}:{ms//60000%60:02d}:{ms//1000%60:02d}.{ms%1000:03d}"
    (media / f"{name}.vtt").write_text(f"WEBVTT\n\n00:00.000 --> {stamp(end)}\n{caption}\n")
    manifest = media / "manifest.json"
    data = json.loads(manifest.read_text())
    data["clips"] = [c for c in data["clips"] if c["name"] != name]
    data["clips"].append({"name": name, "dimensions": [stream["width"], stream["height"]],
                          "frames": int(stream["nb_frames"]), "fps": float(Fraction(stream["avg_frame_rate"])),
                          "duration_seconds": duration,
                          "recorded_on": datetime.now(timezone.utc).date().isoformat(), "capture": note})
    manifest.write_text(json.dumps(data, indent=2) + "\n")


def gameplay(args):
    source = args.episode
    result = json.loads((source / "result.json").read_text())
    calls = json.loads((source / "native-calls.json").read_text())
    trace = [json.loads(l) for l in (source / "agent/trace.jsonl").read_text().splitlines()]
    decisions = [e for e in trace if e["type"] == "tool" and e["name"] == "fly"]
    with Host(args.amoris, source / "candidate", args.work / "host.log") as host:
        session = Session(host, args.work)
        host.call("play.start", {"paused": True})
        host.step(1)
        frame = 0
        decision_index = -1
        panel = None
        last_observation = {}
        for call in calls:
            if call["method"] == "player.observe":
                last_observation = call["reply"].get("result", {})
            elif call["method"] == "player.act":
                decision_index += 1
                session.call("player.act", call["params"], "player")
                choice = decisions[decision_index]["arguments"]
                ins = {x["name"]: x.get("value") for x in last_observation.get("instruments", [])}
                panel = args.work / f"choice-{decision_index:04d}.png"
                overlay(panel, "The model takes the controls", "DeepSeek Flash · native player seat · recorded action replay",
                        ["PLAYER OBSERVATION", f"Gates {ins.get('gates', 0)} / 10",
                         f"Speed {ins.get('speed_mps', '?')} m/s", f"Height {ins.get('altitude_m', '?')} m",
                         f"Heading {ins.get('heading_rad', '?')} rad", "", "MODEL ACTION",
                         choice.get("note", "fly"), "", "player.act", f"forward {choice['forward']:+.2f}",
                         f"strafe  {choice['strafe']:+.2f}", f"gust    {choice.get('gust', False)}", "",
                         "player.wait", f"up to {choice['ticks']} ticks", "", "No steering executor.", "Player perception only."],
                        "Actual model-selected controls. The game pauses during model calls; replay omits that latency. Native game footage, no audio.")
            elif call["method"] == "player.wait":
                target = call["reply"]["result"]["tick"]
                tick = host.call("status")["tick"]
                while tick < target:
                    tick = host.step(min(2, target - tick))["tick"]
                    capture = host.call("capture", {"frames": 1, "width": 1280, "height": 720})
                    if capture.get("pending_assets"):
                        capture = host.call("capture", {"frames": 1, "width": 1280, "height": 720})
                    if capture.get("pending_assets"):
                        raise RuntimeError("Pending assets in recorded replay")
                    shutil.copy2(capture["path"], args.work / "frames" / f"{frame:05d}.png")
                    (args.work / "panels" / f"{frame:05d}.png").hardlink_to(panel)
                    frame += 1
                    if frame % 150 == 0:
                        print(f"replay frame {frame}, tick {tick}", flush=True)
        final = host.call("status")
        verified = final["world_hash"] == result["state"]["world_hash"] and final["tick"] == result["state"]["tick"]
        (args.work / "replay.json").write_text(json.dumps({"passed": verified, "recorded": result["state"], "replayed": final}, indent=2) + "\n")
        if not verified:
            raise RuntimeError("The replay differs from the model run")
        # The HUD refreshes every five ticks. Verify the actual model boundary first, then let
        # its already-completed result reach the HUD for the final presentation hold.
        host.step(6)
        capture = host.call("capture", {"frames": 1, "width": 1280, "height": 720})
        finish_shot = args.work / "completed.png"
        shutil.copy2(capture["path"], finish_shot)
        panel = args.work / "completed-panel.png"
        course = result["course"][0]
        finish_data = next((c["reply"].get("result", {}).get("until", {}).get("event", {}).get("data", {})
                            for c in reversed(calls) if c["method"] == "player.wait"), {})
        medal = {0: "none", 1: "bronze", 2: "silver", 3: "gold"}[course["Course.medal"]]
        rival = {0: "No rival result", 1: "Model beat the rival", 2: "Rival finished first"}.get(finish_data.get("rival", 0))
        overlay(panel, "The recorded flight", "DeepSeek Flash · verified native action replay",
                ["RECORDED RESULT", f"Gates {course['Course.gates']} / 10",
                 f"Time {course['Course.elapsed_s']:.1f} s", f"Medal {medal.upper()}", rival, "",
                 "MODEL SESSION", f"{result['agent']['provider_calls']} API requests",
                 f"{result['agent']['tool_calls']} tool calls", "", "REPLAY", "Final world hash matched."],
                "The replay matched the model run. The completed result is held for readability.")
        for i in range(60):
            (args.work / "frames" / f"{frame+i:05d}.png").hardlink_to(finish_shot)
            (args.work / "panels" / f"{frame+i:05d}.png").hardlink_to(panel)
    finish("windroad-player", args.work,
           "DeepSeek Flash chooses joystick inputs from native player perception. Hash-verified native replay; model latency omitted.",
           "Hash-verified replay of actual DeepSeek Flash calls through the native HTTP player projection; simulation sampled every two ticks.")


def debugging(args):
    trace = [json.loads(l) for l in (args.episode / "agent/trace.jsonl").read_text().splitlines()]
    tools = [e for e in trace if e["type"] == "tool"]
    shots = sorted(args.episode.glob("tool-*.png"))
    if len(shots) != len(tools):
        raise RuntimeError("A tool capture is missing; don't misalign the recorded results")
    selected = []
    for shot, event in zip(shots, tools):
        name = event["name"]
        if name in {"debug_breakpoints_set", "debug_eval", "scripts_patch", "scripts_apply", "world_edit", "history_undo"}:
            if "error" not in event["result"]:
                selected.append((shot, event))
        elif name == "world_get" and any("Course.gates" in k for k in event["result"].get("fields", {})):
            selected.append((shot, event))
    n = 0
    for index, (shot, event) in enumerate(selected):
        panel = args.work / f"tool-{index:03d}.png"
        params = json.dumps(event["arguments"], ensure_ascii=False)
        result = event["result"]
        if event["name"] == "scripts_apply":
            result = {k: result[k] for k in ("outcome", "typecheck", "applied_at", "diagnostics", "systems") if k in result}
        returned = json.dumps(result.get("value", result.get("fields", result)), ensure_ascii=False)
        name = "scripts.read / scripts.write" if event["name"] == "scripts_patch" else event["name"].replace("_", ".")
        overlay(panel, "Inspect a frame. Fix the gate.", "DeepSeek Flash · actual debugger and scene-editing API calls · edited session",
                ["NATIVE CALL", name, "", "PARAMETERS", params[:350], "", "RETURNED", returned[:580]],
                "Prepared reproduction with a planted defect. The model inspects paused values before fixing and hot-applying the predicate. Cuts omit API latency.")
        for _ in range(120):
            (args.work / "frames" / f"{n:05d}.png").hardlink_to(shot)
            (args.work / "panels" / f"{n:05d}.png").hardlink_to(panel)
            n += 1
    # Replay the exact recorded scene edit with a fixed detail camera, making the movement visible.
    edit = next(e for e in tools if e["name"] == "world_edit")
    with Host(args.amoris, args.episode / "candidate", args.work / "edit-host.log") as host:
        original = host.call("world.get", {"entity": "Arch4"})["components"]["Transform"]["position"]
        for label, method, params in [("Original position", None, None),
                                      ("Recorded world.edit: +2 metres", "world.edit", edit["arguments"]),
                                      ("Recorded history.undo: restored", "history.undo", {})]:
            if method:
                host.call(method, params)
            at = host.call("world.get", {"entity": "Arch4"})["components"]["Transform"]["position"]
            c = host.call("capture", {"position": [original[0]+20, original[1]+9, original[2]+24],
                "look_at": original, "width": 960, "height": 540, "frames": 2})
            shot = args.work / f"edit-{n}.png"
            shutil.copy2(c["path"], shot)
            panel = args.work / f"edit-panel-{n}.png"
            overlay(panel, "Edit the scene through the same API", "Replay of the model's recorded scene edit · fixed camera",
                    ["SCENE EDIT", "Arch4", "", label, "", "Transform.position", *[f"{x:.6f}" for x in at],
                     "", "Original y", f"{original[1]:.6f}", "", "Native world.edit", "Native history.undo"],
                    "Actual engine captures of the recorded edit and undo. The camera is fixed so the two-metre movement is visible.")
            for _ in range(90):
                (args.work / "frames" / f"{n:05d}.png").hardlink_to(shot)
                (args.work / "panels" / f"{n:05d}.png").hardlink_to(panel)
                n += 1
        if at != original:
            raise RuntimeError("The recorded undo did not restore the scene")
    finish("windroad-debug", args.work,
           "Real DeepSeek Flash debugger calls: inspect paused locals, repair a planted gate predicate and hot-apply. Recorded scene edit and undo shown afterward.",
           "Actual native captures paired with actual model tool results; edited for length. The scene-edit close-up replays the same edit and undo with a fixed camera.")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("mode", choices=["gameplay", "debug", "intro"])
    p.add_argument("episode", type=Path)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--amoris", type=Path, default=ROOT / "target/release/amoris")
    args = p.parse_args()
    args.episode, args.work, args.amoris = args.episode.resolve(), args.work.resolve(), args.amoris.resolve()
    if not args.work.is_relative_to(ROOT / "out") or args.work.exists():
        raise SystemExit("Use a fresh work directory under ignored out/")
    (args.work / "frames").mkdir(parents=True)
    (args.work / "panels").mkdir()
    if args.mode == "intro":
        shutil.copy2(args.episode, ROOT / "site/media/windroad.mp4")
        publish("windroad", "Windward: a scripted API speedrun in the native Amoris renderer. "
                "The controller reads developer world state; this is separate from the model-selected flight.",
                "Existing scripted developer-state speedrun; native frames every two simulation ticks, no audio.")
    else:
        (gameplay if args.mode == "gameplay" else debugging)(args)


if __name__ == "__main__":
    main()
