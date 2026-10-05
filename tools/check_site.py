#!/usr/bin/env python3
"""Verify the actual built showcase links, anchors, captions, and video files."""
from pathlib import Path
from html.parser import HTMLParser
from urllib.parse import urlsplit, unquote
import argparse
import hashlib
import json
import subprocess

ROOT = Path(__file__).resolve().parents[1]


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.links = []
        self.ids = set()

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if values.get("id"):
            self.ids.add(values["id"])
        for key in ["href", "src", "poster"]:
            if values.get(key):
                self.links.append(values[key])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", type=Path, default=ROOT / "out/site")
    parser.add_argument("--links-only", action="store_true")
    parser.add_argument("--base-path", default="/amoris", help="GitHub Pages project mount path")
    args = parser.parse_args()
    site = args.site.resolve()
    pages = {}
    for path in site.rglob("*.html"):
        page = Page()
        page.feed(path.read_text())
        pages[path.resolve()] = page
    assert site / "index.html" in pages
    checked = 0
    problems = []
    for source, page in pages.items():
        for link in page.links:
            url = urlsplit(link)
            if url.scheme or url.netloc or link.startswith("//"):
                continue
            if url.path.startswith("/"):
                path = unquote(url.path)
                if args.base_path and path.startswith(args.base_path.rstrip("/") + "/"):
                    path = path[len(args.base_path.rstrip("/")):]
                target = site / path.lstrip("/")
            elif url.path:
                target = source.parent / unquote(url.path)
            else:
                target = source
            target = target.resolve()
            if target.is_dir():
                target /= "index.html"
            checked += 1
            if not target.is_file():
                problems.append(f"{source.relative_to(site)} -> {link}: missing target")
            elif url.fragment and target in pages and unquote(url.fragment) not in pages[target].ids:
                problems.append(f"{source.relative_to(site)} -> {link}: missing anchor")
    # The generated search index is part of the documentation's actual runtime.
    search = json.loads((site / "documentation/search/search_index.json").read_text())
    assert len(search["docs"]) >= 5
    manifest = json.loads((site / "media/manifest.json").read_text())
    assert len(manifest["clips"]) >= 3
    assert len({clip["name"] for clip in manifest["clips"]}) == len(manifest["clips"])
    for clip in manifest["clips"]:
        video = site / "media" / (clip["name"] + ".mp4")
        if hashlib.sha256(video.read_bytes()).hexdigest() != clip["sha256"]:
            problems.append(f"media/{video.name}: checksum does not match the capture manifest")
        if not args.links_only:
            probe = json.loads(subprocess.check_output([
                "ffprobe", "-v", "error", "-show_entries", "stream=codec_name,width,height,avg_frame_rate,nb_frames",
                "-show_entries", "format=duration", "-of", "json", str(video),
            ], text=True))
            stream = probe["streams"][0]
            assert stream["codec_name"] == "h264"
            assert [stream["width"], stream["height"]] == clip["dimensions"]
            assert stream["avg_frame_rate"] == "30/1"
            assert int(stream["nb_frames"]) == clip["frames"]
        caption = site / "media" / (clip["name"] + ".vtt")
        assert caption.read_text().startswith("WEBVTT")
    if problems:
        raise SystemExit("\n".join(problems))
    print(json.dumps({"html_pages": len(pages), "local_references": checked, "video_files": len(manifest["clips"]),
                      "search_entries": len(search["docs"]), "problems": 0}))


if __name__ == "__main__":
    main()
