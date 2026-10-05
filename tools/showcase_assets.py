#!/usr/bin/env python3
"""Fetch the pinned CC0 showcase resources, verify SHA-256, and pack portable GLBs."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path
import urllib.request

from showcase_pack import pack

ROOT = Path(__file__).resolve().parents[1]


def fetch(output):
    lock = json.loads((ROOT / "site/assets/models.lock.json").read_text())
    models = []
    for asset in lock["assets"]:
        directory = output / asset["asset"]
        for file in asset["files"]:
            target = (directory / file["file"]).resolve()
            if not target.is_relative_to(directory.resolve()):
                raise ValueError("Asset path escapes its directory")
            if target.is_file() and hashlib.sha256(target.read_bytes()).hexdigest() == file["sha256"]:
                continue
            request = urllib.request.Request(file["url"], headers={"User-Agent":"Amoris-showcase/1.0"})
            with urllib.request.urlopen(request, timeout=90) as response:
                data = response.read(file["size"] + 1)
            if len(data) != file["size"] or hashlib.sha256(data).hexdigest() != file["sha256"]:
                raise ValueError(f"Source no longer matches the reviewed asset: {file['url']}")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        model = output / asset["packed"]
        print(json.dumps(pack(directory/asset["entry"], model)), flush=True)
        models.append(model)
    return models


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output",type=Path,default=ROOT/"out/showcase/models")
    parser.add_argument("--projects",action="store_true",help="Create detail/stress projects and dress the sailing sample")
    args = parser.parse_args()
    models = fetch(args.output.resolve())
    if args.projects:
        from showcase_projects import static_project, ship_project
        helmet,ship = models
        static_project("detail",helmet)
        for count in [64,256,1024]:static_project(f"stress-{count}",helmet,count)
        ship_project(ship)
        player = ROOT / "site/demos/agent-sailing"
        if player.is_dir():
            target = player / "models/third-party" / ship.name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ship, target)
