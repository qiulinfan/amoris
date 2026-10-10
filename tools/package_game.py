#!/usr/bin/env python3
"""Package an already-built native Amoris game as a self-contained macOS app."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def package(project, binary, output, name):
    if sys.platform != "darwin":
        raise ValueError("This packager currently builds macOS game applications")
    project, binary, output = project.resolve(), binary.resolve(), output.resolve()
    if not (project / "project.toml").is_file() or not (project / "scene.json").is_file():
        raise ValueError("The game project needs project.toml and scene.json")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("Build the native pocket executable before packaging")
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9 -]{0,63}", name):
        raise ValueError("Use a short application name containing letters, digits, spaces or hyphens")
    if output == project or output.is_relative_to(project):
        raise ValueError("Package output must be outside the game project")
    output.mkdir(parents=True, exist_ok=True)
    mac = sys.platform == "darwin"
    destination = output / (name + ".app" if mac else name)
    # Do not replace an application that may be running. A fresh output is explicit and reviewable.
    if destination.exists():
        raise ValueError(f"Output already exists: {destination}; choose a new --output directory")
    if mac:
        with binary.open("rb") as f:
            magic = f.read(4)
        if magic not in {b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"}:
            raise ValueError("A macOS .app requires a real Mach-O executable")
        resources = destination / "Contents/Resources"
        executable = destination / "Contents/MacOS/AmorisGame"
    else:
        resources = destination
        executable = destination / ("AmorisGame.exe" if sys.platform == "win32" else "AmorisGame")
    executable.parent.mkdir(parents=True, exist_ok=True)
    resources.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, executable)
    executable.chmod(0o755)
    bundled_project = resources / "game"
    shutil.copytree(project, bundled_project, ignore=shutil.ignore_patterns(
        ".pocket", ".git", ".vscode", "node_modules", "target", "__pycache__"))
    shutil.copy2(ROOT / "LICENSE", resources / "ENGINE-LICENSE")
    if mac:
        iconset = output / (name + ".iconset")
        iconset.mkdir()
        icon = ROOT / "assets/branding/amoris-icon-macos.png"
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                filename = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
                subprocess.run(["/usr/bin/sips", "-z", str(size * scale), str(size * scale),
                                str(icon), "--out", str(iconset / filename)],
                               check=True, stdout=subprocess.DEVNULL)
        subprocess.run(["/usr/bin/iconutil", "-c", "icns", str(iconset),
                        "-o", str(resources / "AmorisGame.icns")], check=True)
        identifier = "dev.amoris.game." + re.sub(r"[^a-z0-9]", "", name.lower())
        info = dict(CFBundleName=name, CFBundleDisplayName=name,
                    CFBundleIdentifier=identifier, CFBundleExecutable="AmorisGame",
                    CFBundlePackageType="APPL", CFBundleVersion="1",
                    CFBundleShortVersionString="0.1.0", CFBundleIconFile="AmorisGame.icns",
                    NSHighResolutionCapable=True, NSPrincipalClass="NSApplication",
                    LSApplicationCategoryType="public.app-category.games")
        with (destination / "Contents/Info.plist").open("wb") as f:
            plistlib.dump(info, f)
    files = []
    for path in sorted(bundled_project.rglob("*")):
        if path.is_file():
            files.append(dict(file=str(path.relative_to(resources)), bytes=path.stat().st_size))
    receipt = dict(application=str(destination), platform=sys.platform,
                   project_files=files,
                   launch="Double-click; bundled native runtime starts the game in walk mode.")
    (output / "package-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/pocket")
    parser.add_argument("--output", type=Path, default=ROOT / "out/game")
    parser.add_argument("--name", default="Amoris Town")
    args = parser.parse_args()
    package(args.project, args.binary, args.output, args.name)
