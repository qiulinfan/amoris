#!/usr/bin/env python3
"""Build reproducible projects from acquired high-detail GLBs; never modify their geometry."""
import argparse
import json
import math
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]


def rotation(position, target):
    x, y, z = [b-a for a,b in zip(position,target)]
    yaw = math.atan2(-x, -z)
    pitch = math.atan2(y, math.hypot(x,z))
    sy, cy = math.sin(yaw/2), math.cos(yaw/2)
    sx, cx = math.sin(pitch/2), math.cos(pitch/2)
    return [cy*sx,sy*cx,-sy*sx,cy*cx]


def entity(name, components):
    return {"name":name,"components":components}


def static_project(name, model, count=1):
    dest = ROOT / "site/demos" / name
    assets = dest / "models/third-party"
    assets.mkdir(parents=True,exist_ok=True)
    shutil.copy2(model,assets/model.name)
    (dest/"scripts").mkdir(exist_ok=True)
    (dest/"scripts/main.ts").write_text('import { game } from "pocket";\nexport default game({ components: [], systems: [] });\n')
    (dest/"project.toml").write_text(f'name = "{name}"\nrate = 60\nseed = 7\n')
    shutil.copy2(ROOT/"samples/anim/tsconfig.json",dest/"tsconfig.json")
    side = math.ceil(math.sqrt(count))
    if count==1:
        position,target=[.85,.52,1.22],[0,.35,0]
    else:
        position,target=[side*.2,side*.32,side*.68],[0,.28,0]
    entities=[
        entity("Camera",{"Transform":{"position":position,"rotation":rotation(position,target)},"Camera":{"fov_deg":44}}),
        entity("Sun",{"Transform":{"rotation":rotation([4,6,5],[0,0,0])},"Light":{"kind":"directional","intensity":5,"shadows":True}}),
        entity("Environment",{"Environment":{"sky":"color","sky_color":[.018,.024,.032],"ambient":.7,"bloom":.04}}),
        entity("Ground",{"Transform":{"position":[0,-.004,0]},"Model":{"mesh":"plane","scale":[side*1.2,1,side*1.2],"color":[.034,.043,.053,1],"roughness":.5}}),
    ]
    for i in range(count):
        entities.append(entity(f"Helmet{i+1:04d}",{"Transform":{"position":[(i%side-(side-1)/2)*.7,0,(i//side-(side-1)/2)*.72]},"Model":{"mesh":f"models/third-party/{model.name}"}}))
    (dest/"scene.json").write_text(json.dumps({"format":"pocket-scene","version":1,"entities":entities},indent=2)+"\n")
    return dest


def ship_project(model):
    dest=ROOT/"site/demos/harbor"
    assets=dest/"models/third-party";assets.mkdir(parents=True,exist_ok=True)
    shutil.copy2(model,assets/model.name)
    scene=json.loads((dest/"scene.json").read_text())
    scene["entities"]=[e for e in scene["entities"] if "ShipPart" not in e["components"] and not e["name"].startswith(("Island ","Tree ","Rock ","Lighthouse ","Pier "))]
    for e in scene["entities"]:
        if e["name"]=="Sloop":e["components"]["Model"]["visible"]=False
    # The model's authored bow is +X; the simulated boat's forward is -Z.
    q=[0,math.sqrt(.5),0,math.sqrt(.5)]
    scene["entities"].append(entity("Dutch Ship",{
        "Transform":{"position":[0,.38,0],"rotation":q},
        "Model":{"mesh":f"models/third-party/{model.name}","scale":[.28,.28,.28]},
        "ShipPart":{"boat":3,"ox":0,"oy":.38,"oz":0,"qx":q[0],"qy":q[1],"qz":q[2],"qw":q[3]}
    }))
    (dest/"scene.json").write_text(json.dumps(scene,indent=2)+"\n")
    (dest/"project.toml").write_text((dest/"project.toml").read_text().replace("Golden Harbor","Dutch Ship — sailing"))
    return dest


def bistro_project(model):
    dest=ROOT/"site/demos/bistro"
    assets=dest/"models/third-party";assets.mkdir(parents=True,exist_ok=True)
    shutil.copy2(model,assets/model.name)
    (dest/"scripts").mkdir(exist_ok=True)
    (dest/"scripts/main.ts").write_text('import { game } from "pocket";\nexport default game({ components: [], systems: [] });\n')
    (dest/"project.toml").write_text('name = "Bistro Exterior"\nrate = 60\nseed = 7\n')
    shutil.copy2(ROOT/"site/demos/detail/tsconfig.json",dest/"tsconfig.json")
    position,target=[12,5,18],[0,5,0]
    scene={"format":"pocket-scene","version":1,"entities":[
        entity("Camera",{"Transform":{"position":position,"rotation":rotation(position,target)},"Camera":{"fov_deg":55}}),
        entity("Sun",{"Transform":{"rotation":rotation([5,9,4],[0,0,0])},"Light":{"kind":"directional","intensity":5,"shadows":True}}),
        entity("Environment",{"Environment":{"ambient":1.0,"exposure_ev":.55,"bloom":.05}}),
        entity("Bistro",{"Model":{"mesh":f"models/third-party/{model.name}"},"Transform":{}})
    ]}
    (dest/"scene.json").write_text(json.dumps(scene,indent=2)+"\n")
    return dest


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helmet",type=Path)
    parser.add_argument("--ship",type=Path)
    parser.add_argument("--bistro",type=Path)
    parser.add_argument("--stress",type=int,nargs="*",default=[64,256,1024])
    args=parser.parse_args()
    projects=[]
    if args.helmet:
        projects.append(static_project("detail",args.helmet))
        for count in args.stress:
            if not 1<=count<=4096:raise ValueError("Keep the locally measured stress sweep bounded")
            projects.append(static_project(f"stress-{count}",args.helmet,count))
    if args.ship:projects.append(ship_project(args.ship))
    if args.bistro:projects.append(bistro_project(args.bistro))
    print(json.dumps({"projects":[str(p) for p in projects]}))
