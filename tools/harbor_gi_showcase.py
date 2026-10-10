#!/usr/bin/env python3
"""Freeze Harbor geometry and capture honest, same-tick native probe-GI comparisons.

Run `pocket serve out/harbor-gi/project --port 8794` on a copy of site/demos/harbor.
`freeze` exports only render meshes/lights for baking; `capture` leaves GI enabled.
Neither phase modifies the original Harbor scene or its sailing rules.
"""
import argparse
import copy
import json
from pathlib import Path
import shutil
import struct
import urllib.parse

from showcase_record import http_call

ROOT = Path(__file__).resolve().parents[1]


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def query(host, components):
    return http_call(host, "world.query", {"with": components, "fields": components, "limit": 1000})


def validate_asset(asset):
    if not asset.is_file():
        raise ValueError("Restore the audited DutchShip.glb first, or pass --asset PATH")
    with asset.open("rb") as stream:
        header = stream.read(12)
    if len(header) != 12 or struct.unpack("<III", header) != (0x46546C67, 2, asset.stat().st_size):
        raise ValueError("DutchShip must be a complete GLB 2.0 file")


def prepare(output, asset):
    validate_asset(asset)
    project = output / "project"
    if not project.exists():
        shutil.copytree(ROOT / "site/demos/harbor", project)
    target = project / "models/third-party/DutchShip.glb"
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.resolve() != asset.resolve():
        shutil.copy2(asset, target)
    write(output / "asset-receipt.json", {"asset": "DutchShip.glb",
        "bytes": asset.stat().st_size, "license": "CC0-1.0",
        "source": "https://polyhaven.com/a/dutch_ship_medium", "provenance": "site/assets/provenance.json"})
    print(project, flush=True)


def verify_host(host, output):
    url = urllib.parse.urlparse(host)
    if url.scheme != "http" or url.hostname not in {"127.0.0.1", "localhost"}:
        raise ValueError("The showcase controls only a local preview host")
    info = http_call(host, "project.info")
    if Path(info["root"]).resolve() != (output / "project").resolve():
        raise ValueError("Host must run the dedicated out/harbor-gi/project preview copy")
    status = http_call(host, "status")
    if not status["paused"] or status["mode"] != "edit":
        raise ValueError("Pause the preview's edit world before making a fixed-pose comparison")
    return status


def freeze(host, output):
    status = verify_host(host, output)
    if status["tick"] == 0:
        http_call(host, "time.step", {"ticks": 60})
    status = verify_host(host, output)
    if status["tick"] != 60:
        raise ValueError("Use a fresh preview at tick 0 or the prepared tick-60 snapshot")
    asset = output / "project/models/third-party/DutchShip.glb"
    validate_asset(asset)
    rows = query(host, ["Model", "Transform"]) + query(host, ["Light", "Transform"]) + query(host, ["Environment"])
    rows.sort(key=lambda row: row["id"])
    entities = []
    for row in rows:
        components = {key: value for key, value in row.items() if key not in {"id", "name"}}
        entities.append({"name": row.get("name", str(row["id"])), "components": components})
    sky_scene = {"format": "pocket-scene", "version": 1, "entities": entities}
    bake_scene = copy.deepcopy(sky_scene)
    for entity in bake_scene["entities"]:
        if "Environment" in entity["components"]:
            env = entity["components"]["Environment"]
            # The strict baker imports this placeholder, then overrides it with the exact sky cube.
            env.update(sky="color", sky_color=[0, 0, 0], baked_gi="", neural_gi="")
    bake_root = output / "bake"
    write(bake_root / "sky-source.json", sky_scene)
    write(bake_root / "scene.json", bake_scene)
    target = bake_root / "models/third-party/DutchShip.glb"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(asset, target)
    boat = http_call(host, "world.get", {"entity": "Sloop", "components": ["Transform"]})
    environment = query(host, ["Environment"])[0]
    receipt = {"tick": status["tick"], "world_hash": status["world_hash"], "boat": boat,
               "environment": environment,
               "excluded": ["procedural ocean", "physics", "wind", "game rules"],
               "scope": "Fixed-pose diffuse mesh lighting; live Harbor retains ocean and sailing systems."}
    write(output / "snapshot.json", receipt)
    print(json.dumps(receipt, ensure_ascii=False), flush=True)


def environment(host, entity, path):
    http_call(host, "world.edit", {"ops": [{"set": {"entity": entity, "component": "Environment",
        "value": {"baked_gi": path, "neural_gi": "", "gi_intensity": 1.0}}}],
        "label": "Harbor GI showcase lighting"})


def capture(host, output, width, height):
    status = verify_host(host, output)
    snapshot = json.loads((output / "snapshot.json").read_text())
    if status["tick"] != snapshot["tick"]:
        raise ValueError("World tick changed since the static bake")
    probes = output / "project/lighting/harbor-probes.json"
    if not probes.is_file():
        raise ValueError("Bake project/lighting/harbor-probes.json first")
    gi = json.loads(probes.read_text())
    p = snapshot["boat"]["components"]["Transform"]["position"]
    views = [
        {"id": "hero", "label": "港湾全景", "position": [p[0]-9.838,5.8,p[2]+7.790],
         "look_at": [p[0]+1,2.8,p[2]-1], "fov_deg": 44,
         "description": "帆船、天空和海面保持原貌，对比船身上的遮挡与间接照明。"},
        {"id": "deck", "label": "船身与甲板", "position": [p[0]-3.3,2.35,p[2]+4.25],
         "look_at": [p[0]+0.5,1.5,p[2]-0.1], "fov_deg": 46,
         "description": "近距离观察木质船身与甲板：探针同时表达天空遮挡和表面反弹。"},
        {"id": "sails", "label": "帆与索具", "position": [p[0]-5,4.8,p[2]+6],
         "look_at": [p[0],3.8,p[2]], "fov_deg": 40,
         "description": "查看白帆、索具和船身附近的间接光照层次。"},
    ]
    directory = output / "view"
    directory.mkdir(parents=True, exist_ok=True)
    traces = []
    env_id = snapshot["environment"]["id"]
    for view in views:
        for mode, path in [("off", ""), ("on", "lighting/harbor-probes.json")]:
            environment(host, env_id, path)
            params = {key: view[key] for key in ["position", "look_at", "fov_deg"]}
            params.update(width=width, height=height)
            frame = None
            for _ in range(12):
                frame = http_call(host, "capture", params)
                if not frame["pending_assets"]:
                    break
            if frame["pending_assets"]:
                raise RuntimeError("Capture still has pending assets")
            if frame["tick"] != snapshot["tick"]:
                raise RuntimeError("Comparison must preserve the baked geometry's tick")
            target = directory / "images" / f'{view["id"]}-{mode}.png'
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(frame["path"], target)
            traces.append({"view": view["id"], "mode": mode, "camera": params, "capture": frame})
            print(f'{view["id"]}/{mode}: {target}', flush=True)
        view["before"] = f'images/{view["id"]}-off.png'
        view["after"] = f'images/{view["id"]}-on.png'
    environment(host, env_id, "lighting/harbor-probes.json")
    data = {"title": "Harbor", "host_url": host, "resolution": [width,height],
            "probe_count": len(gi["probes"]), "views": views,
            "source_note": "真实 Amoris Metal 捕获",
            "limitations": "停船快照烘焙，仅帆船和箱子的漫反射；海面保留原有天空、太阳与反射模型。"}
    (directory / "data.js").write_text("window.HARBOR_GI = " + json.dumps(data, ensure_ascii=False, allow_nan=False) + ";\n")
    write(output / "capture-trace.json", traces)
    template = ROOT / "tools/harbor_gi/index.html"
    if template.exists():
        shutil.copy2(template, directory / "index.html")
    shutil.copy2(ROOT / "site/assets/icon.svg", directory / "logo.svg")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=["prepare", "freeze", "capture"])
    parser.add_argument("--host", default="http://127.0.0.1:8794")
    parser.add_argument("--output", type=Path, default=ROOT/"out/harbor-gi")
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--asset", type=Path, default=ROOT/"site/demos/harbor/models/third-party/DutchShip.glb")
    args = parser.parse_args()
    if args.phase == "prepare":
        prepare(args.output.resolve(), args.asset.resolve())
    elif args.phase == "freeze":
        freeze(args.host, args.output.resolve())
    else:
        capture(args.host, args.output.resolve(), args.width, args.height)
