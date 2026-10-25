#!/usr/bin/env python3
"""Generate bounded moving-floater cases and alternate native resident_water builds.

All generated projects, raw measurements and the summary stay under --out (default out/).
Build the resident_water example first. This measures complete native ticks and publication,
not isolated GPU kernels, browser workers or snapshot serialization.
"""
import argparse
import copy
import datetime
import json
import pathlib
import re
import shutil
import subprocess

REPO = pathlib.Path(__file__).resolve().parents[2]


def project(destination, regions, bodies):
    source = REPO / "tests/fixtures/scenes/water-feedback"
    destination.mkdir(parents=True)
    shutil.copyfile(source / "project.toml", destination / "project.toml")
    shutil.copytree(source / "scripts", destination / "scripts")
    scene = json.loads((source / "scene.json").read_text())
    template = next(e for e in scene["entities"] if "Floater" in e["components"])
    lake = next(e for e in scene["entities"] if "WaterRegion" in e["components"])
    assert len(template["components"]["Floater"]["points"]) == 27
    assert template["components"]["Velocity"]["linear"] == [3.0, 0.0, 0.0]
    assert lake["components"]["WaterRegion"]["size"] == [128, 128]
    assert lake["components"]["WaterRegion"]["cell_m"] == 0.25
    scene["entities"] = [
        e for e in scene["entities"] if "Floater" not in e["components"]
        and "WaterRegion" not in e["components"]
    ]
    # Bed stays first (id 1), as required by the authored numeric voxel-volume reference.
    assert scene["entities"][0]["name"] == "Bed"
    floor = next(e for e in scene["entities"] if e["name"] == "Floor")["components"]
    half_width = 16 + 20 * (regions - 1)
    floor["Transform"]["position"][0] = half_width
    floor["Collider"]["shape"]["Cuboid"]["half_extents"][0] = half_width
    floor["Model"]["scale"][0] = half_width * 2
    for region in range(regions):
        offset = region * 40
        water = copy.deepcopy(lake)
        water["name"] = f"Lake{region}"
        water["components"]["Transform"] = {"position": [offset, 0, 0]}
        scene["entities"].append(water)
        for n in range(bodies):
            body = copy.deepcopy(template)
            body["name"] = f"Float{region}_{n}"
            body["components"]["Transform"]["position"] = [
                offset + 3.5 + (n % 16) * 1.5, 2.09, 3.5 + (n // 16) * 2.5
            ]
            scene["entities"].append(body)
    (destination / "scene.json").write_text(json.dumps(scene, separators=(",", ":")) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", action="append", required=True, metavar="NAME=EXE")
    parser.add_argument("--case", action="append", metavar="REGIONSxBODIES")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--ticks", type=int, default=180)
    parser.add_argument("--different-solvers", action="store_true",
                        help="compare numerical algorithms; require exact repeats within each build")
    parser.add_argument("--out", type=pathlib.Path)
    args = parser.parse_args()
    if args.repeats < 1 or not 64 <= args.ticks <= 180:
        parser.error("repeats must be positive; ticks must be 64..180 for the authored body layout")
    builds = {}
    for item in args.build:
        name, separator, executable = item.partition("=")
        if not separator or not re.fullmatch(r"[\w-]+", name) or name in builds:
            parser.error("each build needs a unique NAME=EXE")
        path = pathlib.Path(executable).resolve()
        if not path.is_file():
            parser.error(f"build does not exist: {path}")
        builds[name] = path
    cases = []
    for item in args.case or ["1x4", "1x16", "1x64", "1x128", "4x16"]:
        match = re.fullmatch(r"([1-9][0-9]*)x([1-9][0-9]*)", item)
        if not match:
            parser.error("case must be REGIONSxBODIES")
        regions, bodies = map(int, match.groups())
        if regions > 4 or bodies > 128 or item in cases:
            parser.error("cases must be unique, with at most four regions and 128 bodies per region")
        cases.append(item)
    root = args.out or REPO / "out/water-scaling" / datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=False)
    results = {}
    for case in cases:
        regions, bodies = map(int, case.split("x"))
        destination = root / case / "project"
        project(destination, regions, bodies)
        references = {}
        physical = {}
        hashes = {}
        for repeat in range(1, args.repeats + 1):
            order = list(builds.items())
            if repeat % 2 == 0:
                order.reverse()
            for name, executable in order:
                key = f"{case}-{name}-{repeat}"
                print(key, flush=True)
                log = root / f"{key}.log"
                with log.open("w") as output:
                    subprocess.run([str(executable), "--bench-project", str(destination),
                                    "--ticks", str(args.ticks)], cwd=REPO, stdout=output,
                                   stderr=subprocess.STDOUT, check=True)
                result = json.loads(next(line for line in log.read_text().splitlines()
                                         if line.startswith("{")))
                ledger = result["reaction"]
                if len(ledger) != regions or any(row["WaterFeedback.applied_total"][0] <= 0 for row in ledger):
                    raise RuntimeError(f"{key}: a region has no actual horizontal feedback")
                reference_key = name if args.different_solvers else "all builds"
                reference = references.get(reference_key)
                if reference is not None and ledger != reference:
                    raise RuntimeError(f"{key}: reaction ledgers differ across builds/repeats")
                references[reference_key] = ledger
                state = [result["resident_state"], result["water_state"]]
                if reference_key in physical and physical[reference_key] != state:
                    raise RuntimeError(f"{key}: physical state/digest differs")
                physical[reference_key] = state
                if name in hashes and hashes[name] != result["world_hash"]:
                    raise RuntimeError(f"{key}: repeated world hash differs")
                hashes[name] = result["world_hash"]
                results[key] = result
                (root / "summary.json").write_text(json.dumps(results, indent=2) + "\n")
    print(root / "summary.json")


if __name__ == "__main__":
    main()
