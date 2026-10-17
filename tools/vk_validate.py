#!/usr/bin/env python3
"""Runs one command with the Khronos Vulkan validation layer and synchronization validation enabled
for that process only (docs/bench/gpu-matrix.md 8).

    python tools/vk_validate.py --log out/vkvalid/x.vvl.txt [--sdk DIR] -- CMD ARGS...

The layer comes from the Vulkan SDK's `Bin` directory (`--sdk`, else `$VULKAN_SDK/Bin`, else
scoop's `~/scoop/apps/vulkan/current/Bin`). It is not registered with the system, so no other
process loads it: this one gets `VK_LAYER_PATH`, `VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation`
and `VK_LAYER_SETTINGS_PATH`, a settings file written beside the log that enables the core checks
with spirv-val, synchronization validation (`validate_sync`, `syncval_full_validation`) and every
error, warning, performance and information message into the log without a message limit, and
turns off the shader validation cache (it would be written under the user's profile). Release
builds have no wgpu debug messenger, so the log receives everything. Prints the log's messages
counted by severity and id, and exits with the command's status.
"""
import argparse
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

SETTINGS = """\
khronos_validation.validate_core = true
khronos_validation.check_shaders = true
khronos_validation.check_shaders_caching = false
khronos_validation.validate_sync = true
khronos_validation.syncval_full_validation = true
khronos_validation.syncval_message_extra_properties = true
khronos_validation.stateless_param = true
khronos_validation.object_lifetime = true
khronos_validation.thread_safety = true
khronos_validation.unique_handles = true
khronos_validation.validate_best_practices = false
khronos_validation.debug_action = VK_DBG_LAYER_ACTION_LOG_MSG
khronos_validation.log_filename = {log}
khronos_validation.report_flags = error,warn,perf,info
khronos_validation.enable_message_limit = false
"""
MESSAGE = re.compile(r"^Validation (Error|Warning|Performance Warning|Information): \[ (\S+) \]")


def sdk_bin(given):
    for d in (given, os.environ.get("VULKAN_SDK") and Path(os.environ["VULKAN_SDK"]) / "Bin",
              Path.home() / "scoop" / "apps" / "vulkan" / "current" / "Bin"):
        if d and (Path(d) / "VkLayer_khronos_validation.json").exists():
            return Path(d)
    raise SystemExit("no VkLayer_khronos_validation.json: pass --sdk <Vulkan SDK>/Bin")


def counts(text):
    """The layer log -> Counter of (severity, message id)."""
    return Counter(m.groups() for m in map(MESSAGE.match, text.splitlines()) if m)


def main():
    split = sys.argv.index("--") if "--" in sys.argv else len(sys.argv)
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--log", required=True, help="the layer's log file (replaced)")
    ap.add_argument("--sdk", help="the Vulkan SDK's Bin directory")
    args = ap.parse_args(sys.argv[1:split])
    cmd = sys.argv[split + 1:]
    if not cmd:
        ap.error("no command after --")
    log = Path(args.log).resolve()
    log.parent.mkdir(parents=True, exist_ok=True)
    log.unlink(missing_ok=True)
    settings = log.with_name(log.name + ".settings")
    settings.mkdir(exist_ok=True)
    (settings / "vk_layer_settings.txt").write_text(
        SETTINGS.format(log=log.as_posix()), encoding="utf-8")
    env = {k: v for k, v in os.environ.items()
           if k not in ("VK_LAYER_PATH", "VK_ADD_LAYER_PATH", "VK_INSTANCE_LAYERS",
                        "VK_LOADER_LAYERS_ENABLE", "VK_LAYER_SETTINGS_PATH")}
    env.update(VK_LAYER_PATH=str(sdk_bin(args.sdk)),
               VK_INSTANCE_LAYERS="VK_LAYER_KHRONOS_validation",
               VK_LAYER_SETTINGS_PATH=str(settings / "vk_layer_settings.txt"))
    code = subprocess.run(cmd, env=env).returncode
    text = log.read_text(encoding="utf-8", errors="replace") if log.exists() else ""
    found = counts(text)
    print(f"validation log {log}: " + (", ".join(
        f"{n} {severity.lower()} {name}" for (severity, name), n in sorted(found.items()))
        or "no messages"))
    return code


if __name__ == "__main__":
    sys.exit(main())
