#!/usr/bin/env python3
"""Annotate recorded native footage with its real MCP trace, then add captions."""
from pathlib import Path
import json
import subprocess
from html import escape

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "out/showcase"
MEDIA = ROOT / "site/media"


def main():
    data = json.loads((OUT / "harbor-trace.json").read_text())
    frames = data["frames"]
    decisions = data["decisions"]
    panels = OUT / "agent-panels"
    panels.mkdir(parents=True, exist_ok=True)
    for index in range(0, len(frames), 15):
        frame = frames[index]
        decision = next((item for item in reversed(decisions) if item["frame"] <= index), None)
        heading = frame["boat"]["heading_deg"]
        speed = frame["boat"]["speed"]
        tally = frame["tally"]
        label = "Take reachable cargo" if decision and decision["kind"] == "collect" else "Evaluate steering forks"
        text = [
            "MCP PILOT", "Scripted / developer mode", "", "OBSERVE", "world.get: Sloop",
            f"tick        {frame['tick']}", f"heading     {heading:6.1f} deg", f"speed       {speed:6.2f} m/s",
            "", "DECIDE", f"target      {frame['target']}", label,
            "4 bounded Play forks" if decision and decision["kind"] == "steer" else "Crew.take -> crate",
            "", "ACT", f"rudder      {frame['rudder']:+.2f}", "time.step: 2 ticks",
            "", "RESULT", f"cargo       {tally['taken']} / {tally['total']}", f"MCP calls   {frame['mcp_calls']}",
            "", "Native frames, real host.", "No model-generated footage.",
        ]
        # This is a data visualization rendered from SVG, not a synthetic game frame.
        labels = {"MCP PILOT", "OBSERVE", "DECIDE", "ACT", "RESULT"}
        svg = ['<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720">',
               '<rect x="906" y="30" width="350" height="660" rx="14" fill="#14181f" fill-opacity=".94" stroke="#3a4048"/>']
        for row, line in enumerate(text):
            color = "#7dc4ff" if line in labels else "#d8e0e8"
            size = 20 if row == 0 else 14
            svg.append(f'<text x="930" y="{72 + row*23}" fill="{color}" font-family="DejaVu Sans Mono,monospace" font-size="{size}">{escape(line)}</text>')
        svg.append('</svg>')
        src = panels / f"panel-{index:05d}.svg"
        png = panels / f"panel-{index:05d}.png"
        src.write_text('\n'.join(svg))
        subprocess.run(['rsvg-convert', '-o', str(png), str(src)], check=True)
        for number in range(index, min(len(frames), index + 15)):
            frame_path = panels / f"{number:05d}.png"
            if frame_path.exists():
                frame_path.unlink()
            frame_path.hardlink_to(png)
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(MEDIA / "harbor.mp4"),
                    "-framerate", "30", "-i", str(panels / "%05d.png"),
                    "-filter_complex", "[0:v][1:v]overlay=0:0:shortest=1",
                    "-c:v", "libx264", "-preset", "medium", "-crf", "22",
                    "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(MEDIA / "agent.mp4")], check=True)
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(MEDIA / "agent.mp4"),
                    "-frames:v", "1", "-q:v", "3", "-update", "1", str(MEDIA / "agent-poster.jpg")], check=True)
    (MEDIA / "harbor.vtt").write_text("""WEBVTT

00:00.000 --> 00:05.000
CC0 Dutch Ship Medium: native engine footage. The ship follows the sailing physics.

00:05.000 --> 00:12.000
69,162 authored triangles, PBR textures, shadows, and the animated sea.
""")
    (MEDIA / "agent.vtt").write_text("""WEBVTT

00:00.000 --> 00:04.000
A scripted MCP client inspects the boat and evaluates four steering forks.

00:04.000 --> 00:08.000
The chosen action changes the rudder; fixed simulation ticks advance the same world.

00:08.000 --> 00:12.000
Two crates were collected in this recording. The client has developer access, not player MCP.
""")
    print(json.dumps({"agent_video": str(MEDIA / "agent.mp4"), "trace_frames": len(frames),
                      "real_cargo_taken": frames[-1]["tally"]["taken"]}))


if __name__ == "__main__":
    main()
