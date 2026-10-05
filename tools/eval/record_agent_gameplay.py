#!/usr/bin/env python3
"""Film a hash-verified replay of real DeepSeek-selected player intentions."""
from pathlib import Path
import argparse
import hashlib
from html import escape
import json
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from showcase_record import encode
from agent_gameplay import replay_actions


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("episode", type=Path)
    p.add_argument("--pocket", type=Path, default=ROOT / "target/release/pocket")
    p.add_argument("--name", default="deepseek-gameplay")
    args = p.parse_args()
    episode = args.episode.resolve()
    actions = json.loads((episode / "actions.json").read_text())
    summary = json.loads((episode / "agent/summary.json").read_text())
    out = ROOT / "out/agent-showcase" / (args.name + "-capture")
    frames, panels = out / "frames", out / "panels"
    frames.mkdir(parents=True, exist_ok=True)
    panels.mkdir(exist_ok=True)
    media = ROOT / "site/media"
    trace = []

    def capture(game, state):
        boat = game.host.get("Sloop", "Boat", "Transform", "Tally")
        at = boat["Transform"]["position"]
        tick = state["tick"]
        decision = next((a for a in reversed(actions) if a["before"]["tick"] < tick
                         and a["tool"] not in {"observe", "wait"}), None)
        name = decision["tool"] if decision else "observe"
        target = (decision or {}).get("arguments", {}).get("target", "")
        image = game.host.call("capture", {"position": [at[0]-10, 5.8, at[2]+10],
                              "look_at": [at[0]+1, 2.8, at[2]-1], "fov_deg": 44,
                              "width": 1280, "height": 720})
        if image["pending_assets"]:
            # The first large asset can need more than one capture timeout to load.
            image = game.host.call("capture", {"position": [at[0]-10, 5.8, at[2]+10],
                                  "look_at": [at[0]+1, 2.8, at[2]-1], "fov_deg": 44,
                                  "width": 1280, "height": 720})
        if image["pending_assets"]:
            raise RuntimeError("Assets still pending in recorded native frame")
        assert game.host.state()["world_hash"] == state["world_hash"]
        index = len(trace)
        shutil.copy2(image["path"], frames / f"{index:05d}.png")
        tally, telemetry = boat["Tally"], boat["Boat"]
        lines = ["DEEPSEEK FLASH", "Real player tool calls", "", "PLAYER OBSERVATION", "30 m range-limited radar",
                 f"tick        {tick}", f"heading     {telemetry['heading_deg']:6.1f} deg",
                 f"speed       {telemetry['speed']:6.2f} m/s", "", "MODEL INTENTION",
                 f"{name} {target}", "Game helmsman executes it", "Native physics unchanged", "",
                 "RESULT", f"cargo       {tally['taken']} / {tally['total']}", f"worth       {tally['worth']}", "",
                 "Hash-verified native replay", "Model requests and timing", "are retained in the report."]
        svg = ['<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720">',
               '<rect x="906" y="34" width="350" height="652" rx="14" fill="#14181f" fill-opacity=".94" stroke="#3a4048"/>']
        for row, text in enumerate(lines):
            color = "#c2b8c8" if row in {0, 3, 9, 14} else "#d8e0e8"
            svg.append(f'<text x="930" y="{77+row*25}" fill="{color}" font-family="DejaVu Sans Mono,monospace" font-size="{20 if row==0 else 14}">{escape(text)}</text>')
        svg.append('</svg>')
        panel = panels / f"{index:05d}.svg"
        panel.write_text('\n'.join(svg))
        subprocess.run(["rsvg-convert", "-o", str(panels/f"{index:05d}.png"), str(panel)], check=True)
        trace.append({"frame": index, "tick": tick, "world_hash": state["world_hash"],
                      "capture_kind": state.get("capture_kind", "two_tick_sample"), "tally": tally,
                      "decision": decision, "native_capture": image})
        if index % 120 == 0:
            print(f"native agent replay frame {index}, tick {tick}, cargo {tally['taken']}/{tally['total']}", flush=True)

    parity = replay_actions(actions, pocket=args.pocket.resolve(), run_dir=out/"native",
                            on_step=capture, capture_stride=2)
    if not parity["passed"]:
        raise RuntimeError("Replay diverged")
    native = out / "native.mp4"
    encode(frames, native, 30)
    video = media / f"{args.name}.mp4"
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(native),
                    "-framerate", "30", "-i", str(panels/"%05d.png"),
                    "-filter_complex", "[0:v][1:v]overlay=0:0:shortest=1,tpad=stop_mode=clone:stop_duration=2", "-c:v", "libx264",
                    "-crf", "22", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(video)], check=True)
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(video),
                    "-frames:v", "1", "-q:v", "3", "-update", "1", str(media/f"{args.name}-poster.jpg")], check=True)
    (media/f"{args.name}.vtt").write_text("WEBVTT\n\n00:00.000 --> 00:12.000\nReal DeepSeek Flash decisions through a range-limited player gateway, reproduced in the native renderer.\n\n00:12.000 --> 01:00.000\nEvery action and final world hash match the recorded model run. Native sailing physics and cargo rules remain unchanged.\n")
    receipt = {"name": args.name, "frames": len(trace), "fps": 30, "dimensions": [1280,720],
               "sha256": hashlib.sha256(video.read_bytes()).hexdigest(), "model": summary["response_models"],
               "kind": "hash_verified_native_replay_of_actual_model_calls", "judge": parity["judge"],
               "actions_sha256": hashlib.sha256((episode/"actions.json").read_bytes()).hexdigest(),
               "summary_sha256": hashlib.sha256((episode/"agent/summary.json").read_bytes()).hexdigest(),
               "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               "replay_passed": True, "note": "Fixed 2-tick sampling at 60Hz, encoded at 30fps; not measured game FPS."}
    (out/"receipt.json").write_text(json.dumps(receipt, indent=2)+"\n")
    (out/"frames.json").write_text(json.dumps(trace, indent=2)+"\n")
    (out/"replay.json").write_text(json.dumps(parity, indent=2)+"\n")
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__": main()
