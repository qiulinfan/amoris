"""Runs inside RenderDoc's UI: `qrenderdoc --python tools/renderdoc_analyze.py`, as
`python tools/renderdoc_frame.py analyze CAPTURE` launches it (RenderDoc's embedded Python 3.8 and
its `renderdoc` module; not runnable with the system Python).

Inputs from the environment: POCKET_RDC (the capture), POCKET_RDC_OUT (the JSON to write),
POCKET_RDC_TOP (how many actions to list) and POCKET_RDC_D3D12_COUNTERS=1 (Windows' Developer Mode
is on, so RenderDoc's D3D12 counters work). Replays the capture; for the draw with the most
vertices (instances times indices) writes its vertex and fragment shaders in every disassembly
target the replay offers, their raw bytes (DXIL or SPIR-V) and their reflection; then measures
every action's GPU duration (GPUCounter.EventGPUDuration) where the replay allows it. Exits the
UI when done.
"""
import json
import os
import traceback

import renderdoc as rd

RDC = os.environ.get("POCKET_RDC", "")
OUT = os.environ.get("POCKET_RDC_OUT", "")
TOP = int(os.environ.get("POCKET_RDC_TOP", "15"))


def ok(result):
    code = getattr(result, "code", result)
    succeeded = getattr(rd, "ResultCode", getattr(rd, "ReplayStatus", None)).Succeeded
    return code == succeeded


def flatten(actions, sfile, path, out, parent=None):
    for a in actions:
        name = a.GetName(sfile)
        if a.children:
            flatten(a.children, sfile, path + [name], out, a)
        else:
            out.append((a, name, " / ".join(path), parent))


def shader_info(controller, state, stage, stem, label):
    refl = state.GetShaderReflection(stage)
    if refl is None:
        return None
    pipe = state.GetGraphicsPipelineObject()
    info = {
        "entry": refl.entryPoint,
        "encoding": str(refl.encoding),
        "inputs": [{"name": s.varName, "semantic": "%s%d" % (s.semanticName, s.semanticIndex),
                    "components": s.compCount, "type": str(s.varType),
                    "system": str(s.systemValue)} for s in refl.inputSignature],
        "outputs": len(refl.outputSignature),
        "constant_blocks": [c.name for c in refl.constantBlocks],
        "read_only": [r.name for r in refl.readOnlyResources],
        "read_write": [r.name for r in refl.readWriteResources],
        "samplers": [s.name for s in refl.samplers],
        "disassembly": {},
    }
    raw = bytes(refl.rawBytes)
    ext = ".spv" if "SPIRV" in info["encoding"].upper() else ".dxil"
    with open(stem + "-" + label + ext, "wb") as f:
        f.write(raw)
    info["raw"] = stem + "-" + label + ext
    info["raw_bytes"] = len(raw)
    for target in controller.GetDisassemblyTargets(True):
        # AMD's ISA targets (RenderDoc's AMD plugin) take no DXIL and are not this GPU's.
        if target.startswith(("GCN", "RDNA", "AMDIL")):
            continue
        text = controller.DisassembleShader(pipe, refl, target)
        slug = "".join(c if c.isalnum() else "-" for c in target).strip("-").lower()
        path = "%s-%s-%s.txt" % (stem, label, slug)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        info["disassembly"][target] = path
    return info


def analyze():
    rd.InitialiseReplay(rd.GlobalEnvironment(), [])
    cap = rd.OpenCaptureFile()
    if not ok(cap.OpenFile(RDC, "", None)):
        raise RuntimeError("cannot open " + RDC)
    if cap.LocalReplaySupport() != rd.ReplaySupport.Supported:
        raise RuntimeError("the capture cannot be replayed here")
    result, controller = cap.OpenCapture(rd.ReplayOptions(), None)
    if not ok(result):
        raise RuntimeError("cannot replay: %s" % result)
    sfile = controller.GetStructuredFile()
    props = controller.GetAPIProperties()
    d3d12 = props.pipelineType == rd.GraphicsAPI.D3D12
    leaves = []
    flatten(controller.GetRootActions(), sfile, [], leaves)
    parents = {}
    rows = []
    for a, name, marker, parent in leaves:
        if parent is not None:
            parents[a.eventId] = parent.eventId
        rows.append({
            "eid": a.eventId, "name": name, "marker": marker, "gpu_ms": None,
            "indices": a.numIndices, "instances": a.numInstances,
            "draw": bool(a.flags & rd.ActionFlags.Drawcall),
            "dispatch": bool(a.flags & rd.ActionFlags.Dispatch),
        })
    out = {
        "capture": RDC,
        "api": str(props.pipelineType),
        "vendor": str(props.vendor),
        "actions": len(rows),
        "draws": sum(r["draw"] for r in rows),
    }
    # The shaders first: a failed counter fetch can lose the replay's device.
    draws = sorted((r for r in rows if r["draw"]), key=lambda r: r["instances"] * r["indices"],
                   reverse=True)
    if draws:
        target = draws[0]
        controller.SetFrameEvent(target["eid"], True)
        state = controller.GetPipelineState()
        if state.GetShaderReflection(rd.ShaderStage.Pixel) is None and target["eid"] in parents:
            # An indirect sub-draw: the state is the indirect call's.
            controller.SetFrameEvent(parents[target["eid"]], True)
            state = controller.GetPipelineState()
        stem = os.path.splitext(OUT)[0]
        out["largest_draw"] = target
        out["shaders"] = {
            "vertex": shader_info(controller, state, rd.ShaderStage.Vertex, stem, "vs"),
            "fragment": shader_info(controller, state, rd.ShaderStage.Pixel, stem, "ps"),
        }
        out["disassembly"] = {}
        for label, info in out["shaders"].items():
            for target_name, path in ((info or {}).get("disassembly") or {}).items():
                out["disassembly"]["%s: %s" % (label, target_name)] = path
    # Per-event GPU durations, measured on replay. RenderDoc's D3D12 counters need Windows'
    # Developer Mode (they set a stable power state); without it the fetch loses the device.
    counters = controller.EnumerateCounters()
    if d3d12 and os.environ.get("POCKET_RDC_D3D12_COUNTERS") != "1":
        out["gpu_counter"] = ("skipped: D3D12 counters need Windows Developer Mode "
                              "(--d3d12-counters once it is on)")
    elif rd.GPUCounter.EventGPUDuration not in counters:
        out["gpu_counter"] = "not offered by this replay"
    else:
        desc = controller.DescribeCounter(rd.GPUCounter.EventGPUDuration)
        results = controller.FetchCounters([rd.GPUCounter.EventGPUDuration])
        durations = {}
        for r in results:
            v = r.value.d if desc.resultByteWidth == 8 else r.value.f
            durations[r.eventId] = v * 1000.0
        out["gpu_counter"] = "EventGPUDuration: %d events" % len(results)
        out["gpu_total_ms"] = round(sum(durations.values()), 4)
        for r in rows:
            if r["eid"] in durations:
                r["gpu_ms"] = round(durations[r["eid"]], 4)
        by_marker = {}
        for r in rows:
            by_marker[r["marker"]] = round(by_marker.get(r["marker"], 0.0) + (r["gpu_ms"] or 0.0), 4)
        out["by_marker"] = by_marker
        if "largest_draw" in out:
            out["largest_draw"]["gpu_ms"] = round(durations.get(out["largest_draw"]["eid"], 0.0), 4)
    rows.sort(key=lambda r: (r["gpu_ms"] or 0.0, r["instances"] * r["indices"]), reverse=True)
    out["top"] = rows[:TOP]
    controller.Shutdown()
    cap.Shutdown()
    return out


def main():
    try:
        result = analyze()
    except Exception as e:  # reported in the output, which the caller reads
        result = {"error": "%s\n%s" % (e, traceback.format_exc())}
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(result, f, indent=1)
    # Leave before the UI opens.
    os._exit(0)


main()
