#!/usr/bin/env python3
"""Captures one frame of an example with RenderDoc and analyzes the capture (docs/bench/dx12.md,
"Per-frame profiling"; AGENTS.md's building notes).

    python tools/renderdoc_frame.py capture --backend dx12 --adapter nvidia --frame 70 \\
        --name dense-dx12 -- many_cubes --dense --bench 120
    python tools/renderdoc_frame.py analyze out/profiler/rdc/dense-dx12.rdc

`capture` runs the release example (`target/release/examples/<name>`, build it first) under
`renderdoccmd capture` with `POCKET_RENDERDOC_FRAME=N`: the renderer brackets its N-th frame with
RenderDoc's in-application API (crates/pocket-render/src/renderdoc.rs), so headless runs are
captured as well as windowed ones. `WGPU_DISCARD_HAL_LABELS=0` keeps wgpu's pass labels as
markers. On Vulkan, RenderDoc's layer is loaded for this process only (`VK_ADD_LAYER_PATH` and
`VK_INSTANCE_LAYERS`), so it need not be registered system-wide. The capture is moved to
`out/profiler/rdc/<name>.rdc` (never committed).

`analyze` replays the capture in `qrenderdoc --python tools/renderdoc_analyze.py` (RenderDoc's
embedded Python): every action with its GPU duration (`GPUCounter.EventGPUDuration`, measured on
replay; RenderDoc's D3D12 counters need Windows' Developer Mode, so on D3D12 they are skipped
unless `--d3d12-counters`), and for the draw with the most vertices its pipeline's vertex and
fragment shaders as each disassembly target RenderDoc offers (DXIL on Direct3D 12, SPIR-V on
Vulkan) plus their reflection (inputs, resources).
This script then counts the disassembly's instructions, branches, loops, samples, derivatives,
loads, stores and discards (DXIL operations where they are called, not where they are declared).
Results: `out/profiler/rdc/<name>.json` and `<name>-<target>.txt` next to the capture.

RenderDoc is found as `RENDERDOC_DIR`, then `~/scoop/apps/renderdoc/current`, then on `PATH`.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXE = ".exe" if os.name == "nt" else ""
OUT = ROOT / "out" / "profiler" / "rdc"


def renderdoc_dir():
    for d in [os.environ.get("RENDERDOC_DIR"), Path.home() / "scoop" / "apps" / "renderdoc" / "current"]:
        if d and (Path(d) / f"renderdoccmd{EXE}").exists():
            return Path(d)
    found = shutil.which("renderdoccmd")
    if found:
        return Path(found).resolve().parent
    sys.exit("RenderDoc not found: set RENDERDOC_DIR")


def capture(args):
    rd = renderdoc_dir()
    exe = ROOT / "target" / "release" / "examples" / f"{args.example[0]}{EXE}"
    if not exe.exists():
        sys.exit(f"{exe} is missing: cargo build --release -p pocket-render --examples")
    OUT.mkdir(parents=True, exist_ok=True)
    stem = OUT / args.name
    env = dict(os.environ, POCKET_BACKEND=args.backend, POCKET_RENDERDOC_FRAME=str(args.frame),
               WGPU_DISCARD_HAL_LABELS="0")
    if args.adapter:
        env["POCKET_ADAPTER"] = args.adapter
    if args.backend == "vulkan":
        # The layer manifest beside renderdoc.dll, enabled for this process only.
        env["VK_ADD_LAYER_PATH"] = str(rd)
        env["VK_INSTANCE_LAYERS"] = "VK_LAYER_RENDERDOC_Capture"
        env["ENABLE_VULKAN_RENDERDOC_CAPTURE"] = "1"
    before = set(OUT.glob(f"{args.name}*.rdc"))
    cmd = [str(rd / f"renderdoccmd{EXE}"), "capture", "-w", "-c", str(stem), str(exe), *args.example[1:]]
    t = time.time()
    p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=args.timeout)
    log = (p.stdout + p.stderr).strip().splitlines()
    new = sorted(set(OUT.glob(f"{args.name}*.rdc")) - before, key=lambda f: f.stat().st_mtime)
    if not new:
        print("\n".join(log[-20:]), file=sys.stderr)
        sys.exit(f"no capture was written (exit {p.returncode})")
    final = OUT / f"{args.name}.rdc"
    final.unlink(missing_ok=True)
    new[-1].replace(final)
    info = {"capture": str(final.relative_to(ROOT)), "backend": args.backend, "adapter": args.adapter,
            "frame": args.frame, "command": [args.example[0], *args.example[1:]],
            "seconds": round(time.time() - t, 1), "bytes": final.stat().st_size,
            "log": [line for line in log if "renderdoc" in line.lower() or "trace" in line.lower()][-10:]}
    (OUT / f"{args.name}-capture.json").write_text(json.dumps(info, indent=1) + "\n", newline="\n")
    print(json.dumps(info, indent=1))


# Disassembly statistics: what each instruction set calls an instruction, a branch, a loop, a
# texture sample, a derivative, a load, a store and a discard.
SPIRV = {
    "instructions": r"^\s*(?:%\w+\s*=\s*)?Op(?!Name|MemberName|Decorate|MemberDecorate|Source|Line|"
                    r"NoLine|String|ModuleProcessed|Capability|Extension|ExtInstImport|MemoryModel|"
                    r"EntryPoint|ExecutionMode|Type|Constant|Variable|Function\b|FunctionEnd|Label|"
                    r"FunctionParameter|Undef|Spec)\w+",
    "conditional_branches": r"\bOpBranchConditional\b|\bOpSwitch\b",
    "loops": r"\bOpLoopMerge\b",
    "selections": r"\bOpSelectionMerge\b",
    "phis": r"\bOpPhi\b",
    "samples": r"\bOpImage\w*Sample\w*\b|\bOpImageFetch\b|\bOpImageGather\b",
    "derivatives": r"\bOpD[PF]\w*\b|\bOpFwidth\w*\b",
    "loads": r"\bOpLoad\b",
    "stores": r"\bOpStore\b",
    "kills": r"\bOpKill\b|\bOpTerminateInvocation\b|\bOpDemoteToHelperInvocation\b",
}
DXIL = {
    "instructions": r"^\s+(?:%[\w.]+\s*=\s*)?(?:call|br|switch|ret|load|store|getelementptr|phi|select|"
                    r"fadd|fsub|fmul|fdiv|frem|add|sub|mul|udiv|sdiv|urem|srem|shl|lshr|ashr|and|or|"
                    r"xor|fcmp|icmp|extractvalue|insertvalue|bitcast|zext|sext|trunc|fptoui|fptosi|"
                    r"uitofp|sitofp|fptrunc|fpext|alloca|atomicrmw|cmpxchg)\b",
    "conditional_branches": r"^\s+br i1\b|^\s+switch\b",
    "phis": r"=\s*phi\b",
    "samples": r"@dx\.op\.sample\w*|@dx\.op\.textureLoad|@dx\.op\.textureGather\w*",
    # DerivCoarseX/Y and DerivFineX/Y are unary operations 83 to 86.
    "derivatives": r"@dx\.op\.unary\.f\d+\(i32 8[3-6],|@dx\.op\.calculateLOD",
    "loads": r"@dx\.op\.(?:rawBufferLoad|bufferLoad|cbufferLoadLegacy)",
    "stores": r"@dx\.op\.(?:rawBufferStore|bufferStore|storeOutput)",
    "kills": r"@dx\.op\.discard",
    "dx_ops": r"@dx\.op\.\w+",
}


def dxil_loops(text):
    """Loops in LLVM IR: the blocks a later block of the same function branches back to, each
    counted once (a loop's header; a `continue` is a second back edge to the same header)."""
    seen, headers, function = set(), set(), 0
    for line in text.splitlines():
        label = re.match(r"^; <label>:([\w.]+)|^([\w.]+):", line)
        if label:
            seen.add(label.group(1) or label.group(2))
        elif re.match(r"^\s+br\b", line):
            headers.update((function, t) for t in re.findall(r"label %([\w.]+)", line) if t in seen)
        elif line.startswith("define "):
            seen, function = set(), function + 1
    return len(headers)


def stats(text, target):
    table = DXIL if "dxil" in target.lower() or "dxbc" in target.lower() else SPIRV
    out = {"lines": text.count("\n") + 1}
    if table is DXIL:
        # The module also declares every operation it calls, once (`declare ... @dx.op.sampleGrad.f32
        # (...)`): count what the function bodies do.
        text = "\n".join(line for line in text.splitlines() if not line.startswith("declare "))
    for key, pattern in table.items():
        out[key] = len(re.findall(pattern, text, re.M))
    # Function-local arrays (dynamically indexed, so not in registers): count and bytes.
    if table is DXIL:
        out["loops"] = dxil_loops(text)
        ops = re.findall(r"@dx\.op\.(\w+)", text)
        out["dx_op_kinds"] = {k: ops.count(k) for k in sorted(set(ops), key=ops.count, reverse=True)[:12]}
        arrays = [int(n) * (2 if t in ("half", "i16") else 4)
                  for n, t in re.findall(r"alloca \[(\d+) x (float|i32|half|i16)\]", text)]
    else:
        arrays = [int(n) * 4 * int(c or 1) for _, c, n in re.findall(
            r"OpVariable %_ptr_Function__arr_(v(\d)float|float|int|uint)_uint_(\d+)", text)]
    out["local_arrays"] = len(arrays)
    out["local_array_bytes"] = sum(arrays)
    return out


def shader_stats(rd, result):
    """Per stage: the instruction set's statistics (DXC's DXIL disassembly; SPIR-V disassembled
    by RenderDoc's spirv-dis) and the driver's own statistics where it reports them (NVIDIA's
    Vulkan driver through VK_KHR_pipeline_executable_properties: registers, binary size)."""
    out = {}
    for stage, info in (result.get("shaders") or {}).items():
        if not info:
            continue
        s = {}
        raw = Path(info["raw"])
        if raw.suffix == ".spv":
            dis = rd / "plugins" / "spirv" / f"spirv-dis{EXE}"
            p = subprocess.run([str(dis), str(raw)], capture_output=True, text=True)
            if p.returncode == 0:
                raw.with_suffix(".spvasm").write_text(p.stdout, newline="\n")
                s.update(stats(p.stdout, "spir-v"))
        for target, path in info.get("disassembly", {}).items():
            text = Path(path).read_text(errors="replace")
            if target == "DXC DXIL":
                s.update(stats(text, "dxil"))
            if "pipeline_executable" in target.lower():
                for key, value in re.findall(r"^([A-Z][\w ]+): (\d+)", text, re.M):
                    # NVIDIA 617's "Local Memory Size" reads 2^36 plus the bytes: keep the bytes.
                    s["driver " + key.lower()] = int(value) & 0xFFFFFFFF
        out[stage] = s
    return out


def relative(value):
    """The result with this checkout's absolute paths made relative to it (records get published)."""
    if isinstance(value, dict):
        return {k: relative(v) for k, v in value.items()}
    if isinstance(value, list):
        return [relative(v) for v in value]
    if isinstance(value, str) and Path(value).is_absolute():
        try:
            return Path(value).resolve().relative_to(ROOT).as_posix()
        except ValueError:
            return value
    return value


def analyze(args):
    rd = renderdoc_dir()
    rdc = Path(args.capture).resolve()
    out = rdc.with_suffix(".json")
    env = dict(os.environ, POCKET_RDC=str(rdc), POCKET_RDC_OUT=str(out), POCKET_RDC_TOP=str(args.top),
               POCKET_RDC_D3D12_COUNTERS="1" if args.d3d12_counters else "0")
    out.unlink(missing_ok=True)
    script = ROOT / "tools" / "renderdoc_analyze.py"
    log = rdc.with_name(rdc.stem + "-qrenderdoc.log")
    # The UI's output goes to a file: a pipe would keep this script waiting on anything the UI
    # left running. A first start shows RenderDoc's analytics prompt before running the script;
    # answer it once (or set "Analytics_TotalOptOut" in %APPDATA%/qrenderdoc/UI.config).
    with open(log, "w") as f:
        p = subprocess.Popen([str(rd / f"qrenderdoc{EXE}"), "--python", str(script)], env=env,
                             stdout=f, stderr=subprocess.STDOUT)
        try:
            p.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            p.kill()
            sys.exit(f"qrenderdoc did not finish in {args.timeout} s (a dialog waiting?); see {log}")
    if not out.exists():
        sys.exit(f"the analysis wrote nothing (exit {p.returncode}); see {log}")
    result = json.loads(out.read_text())
    if "error" in result:
        sys.exit(f"analysis failed: {result['error']}")
    result["shader_stats"] = shader_stats(rd, result)
    out.write_text(json.dumps(relative(result), indent=1) + "\n", newline="\n")
    print(f"{rdc.name}: {result['api']}, {result['actions']} actions, {result['draws']} draws; "
          f"GPU durations: {result['gpu_counter']}")
    for a in result["top"]:
        ms = f"{a['gpu_ms']:9.3f} ms" if a["gpu_ms"] is not None else "        - ms"
        print(f"  {ms}  eid {a['eid']:5}  {a['marker'][:40]:40}  {a['name'][:60]}")
    if "largest_draw" in result:
        d = result["largest_draw"]
        print(f"  largest draw: eid {d['eid']} {d['name'][:60]} ({d['instances']} instances)")
    for stage, s in result["shader_stats"].items():
        print(f"  {stage}: " + ", ".join(f"{k} {v}" for k, v in s.items() if not isinstance(v, dict)))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("capture", help="capture one frame of an example")
    c.add_argument("--backend", choices=["dx12", "vulkan"], required=True)
    c.add_argument("--adapter", default="", help="POCKET_ADAPTER")
    c.add_argument("--frame", type=int, default=70, help="the renderer's frame to capture, from 1")
    c.add_argument("--name", required=True, help="the capture's name under out/profiler/rdc/")
    c.add_argument("--timeout", type=float, default=600)
    c.add_argument("example", nargs="+", help="-- the example's name and its arguments")
    c.set_defaults(fn=capture)
    a = sub.add_parser("analyze", help="per-event GPU durations and the longest draw's shaders")
    a.add_argument("capture")
    a.add_argument("--top", type=int, default=15, help="actions to list, longest first")
    a.add_argument("--d3d12-counters", action="store_true",
                   help="fetch D3D12 counters too (needs Windows' Developer Mode)")
    a.add_argument("--timeout", type=float, default=900)
    a.set_defaults(fn=analyze)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
