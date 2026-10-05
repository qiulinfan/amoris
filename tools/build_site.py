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

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "out/site")
    args = parser.parse_args()
    output = args.output.resolve()
    source = ROOT / "site"
    if output == ROOT or output == source or output in source.parents:
        raise SystemExit("Output must be a separate build directory.")
    if output == (ROOT / "out/site").resolve() and output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)
    for name in ["index.html", "site.css", "site.js"]:
        shutil.copy2(source / name, output / name)
    for name in ["assets", "media", "licenses"]:
        if (source / name).is_dir():
            shutil.copytree(source / name, output / name, dirs_exist_ok=True)
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
            if target.suffix not in {".css", ".js", ".png", ".jpg", ".svg", ".mp4", ".vtt"}:
                return match.group(0)
            if target not in versions:
                versions[target] = hashlib.sha256(target.read_bytes()).hexdigest()[:12]
            return f'{key}="{url.path}?v={versions[target]}"'
        page.write_text(re.sub(r'(href|src|poster)="([^"]+)"', version, page.read_text()))
    (output / ".nojekyll").write_text("")
    print(json.dumps({"site": str(output), "entry": str(output / "index.html")}))


if __name__ == "__main__":
    main()
