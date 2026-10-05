#!/usr/bin/env python3
"""Encode actual browser screencast frames saved by the CUA recording session."""
from pathlib import Path
import json
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "out/showcase"
MEDIA = ROOT / "site/media"


def segment(name, end=None):
    folder = OUT / name
    data = json.loads((folder / "frames.json").read_text())
    frames = data["frames"]
    origin = frames[0]["timestamp"]
    if end is not None:
        frames = [frame for frame in frames if frame["timestamp"] - origin <= end]
    assert len(frames) > 10
    manifest = folder / "timing.ffconcat"
    lines = ["ffconcat version 1.0"]
    for index, frame in enumerate(frames):
        path = folder / f"{frame['index']:05d}.jpg"
        assert path.is_file()
        duration = max(1/120, min(0.15, frames[index+1]["timestamp"]-frame["timestamp"])) if index+1 < len(frames) else 1/30
        lines.extend([f"file '{path}'", f"duration {duration:.8f}"])
    lines.append(f"file '{folder / (str(frames[-1]['index']).zfill(5)+'.jpg')}'")
    manifest.write_text("\n".join(lines)+"\n")
    video = folder / "segment.mp4"
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0",
                    "-i", str(manifest), "-vf", "fps=30,format=yuv420p", "-c:v", "libx264", "-preset", "medium",
                    "-crf", "23", "-movflags", "+faststart", str(video)], check=True)
    return video


def main():
    MEDIA.mkdir(parents=True, exist_ok=True)
    encoded = segment("editor-high-detail")
    video = MEDIA / "editor.mp4"
    shutil.copy2(encoded, video)
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i", str(video),
                    "-frames:v", "1", "-q:v", "3", "-update", "1", str(MEDIA / "editor-poster.jpg")], check=True)
    (MEDIA / "editor.vtt").write_text("""WEBVTT

00:00.000 --> 00:02.000
Select a real scene entity and inspect its component fields.

00:02.000 --> 00:05.500
Edit the imported ship's tint, then undo through the shared history.

00:05.500 --> 00:10.500
Play runs an explicit fork of the edit world on the real Rust host.

00:10.500 --> 00:14.000
Pause and step a single simulation tick. The WebGPU viewport follows the same world.

00:14.000 --> 00:18.000
Stop discards the Play fork and returns to the original edit world.
""")
    print(json.dumps({"editor_video": str(video), "source": "actual CUA browser screencast", "host": "real Rust host", "mock": False}))


if __name__ == "__main__":
    main()
