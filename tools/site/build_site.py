#!/usr/bin/env python3
"""Build the hand-authored Amoris showcase and MkDocs user documentation."""
from pathlib import Path
import argparse
import hashlib
import json
import re
import shutil
import subprocess
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "out/site")
    parser.add_argument("--game-dir", type=Path, default=ROOT / "out/windward-web",
                        help="Windward folder made by cargo xtask export-web")
    parser.add_argument("--emberlight-dir", type=Path, default=ROOT / "out/emberlight-web",
                        help="Emberlight folder made by cargo xtask export-web")
    args = parser.parse_args()
    output = args.output.resolve()
    source = ROOT / "site"
    games = {"windward": args.game_dir.resolve(), "emberlight": args.emberlight_dir.resolve()}
    for name, game in games.items():
        if not (game / "index.html").is_file() or not (game / "game/package.json").is_file():
            raise SystemExit(f"Export the playable game first: cargo xtask export-web samples/{name} "
                             f"--out out/{name}-web --zip out/{name}-web.zip")
    if output == ROOT or output == source or output in source.parents:
        raise SystemExit("Output must be a separate build directory.")
    for game in games.values():
        if output.is_relative_to(game) or game.is_relative_to(output):
            raise SystemExit("The site output and prepared games must be separate directories.")
    if output == (ROOT / "out/site").resolve() and output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)
    for name in ["index.html", "site.css", "site.js"]:
        shutil.copy2(source / name, output / name)
    for name in ["assets", "media", "licenses"]:
        if (source / name).is_dir():
            shutil.copytree(source / name, output / name, dirs_exist_ok=True)
    for name, game in games.items():
        shutil.copytree(game, output / "play" / name, dirs_exist_ok=True)
    subprocess.run([
        "mkdocs", "build", "--strict", "--config-file", str(source / "mkdocs.yml"),
        "--site-dir", str(output / "documentation"),
    ], check=True, cwd=ROOT)
    # Stable content versions prevent updated videos/scripts from reusing cached drafts.
    versions = {}
    for page in output.rglob("*.html"):
        def version(match):
            key, value = match.groups()
            url = urlsplit(value)
            target = (page.parent / url.path).resolve()
            if url.scheme or url.netloc or not target.is_relative_to(output) or not target.is_file():
                return match.group(0)
            if target.suffix not in {".css", ".js", ".png", ".jpg", ".webp", ".svg", ".mp4", ".vtt"}:
                return match.group(0)
            if target not in versions:
                versions[target] = hashlib.sha256(target.read_bytes()).hexdigest()[:12]
            return f'{key}="{url.path}?v={versions[target]}"'
        page.write_text(re.sub(r'(href|src|poster)="([^"]+)"', version, page.read_text()))
    (output / ".nojekyll").write_text("")
    print(json.dumps({"site": str(output), "entry": str(output / "index.html")}))


if __name__ == "__main__":
    main()
