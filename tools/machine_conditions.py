#!/usr/bin/env python3
"""Records the machine's conditions before or between measurements (docs/bench/quiet-2026-10-09.md).

Windows only (the R1 laptop): power source and plan, the NVIDIA GPU's temperature, clocks, power
and throttle reasons (nvidia-smi), the CPU load over a few seconds (typeperf), which processes use
a GPU engine (the GPU Engine performance counters, both adapters), the busiest processes, and the
processes that would disturb a benchmark (compilers, browsers, other agents, the engine itself)
with their command lines. Prints a one-line summary and writes everything as JSON.

    python tools/machine_conditions.py docs/evidence/quiet/conditions-start.json [--label before-dx12]
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

WATCH = ("cargo", "rustc", "chrome", "node", "python", "pocket", "bun", "codex", "claude",
         "CodeSetup", "many_cubes", "splats", "lod_field", "aa_eval", "neural", "rt_shadows",
         "path_trace", "gi_trace", "link", "cl", "clang", "msedgewebview2", "Code")


def ps(script):
    p = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                       capture_output=True, text=True, timeout=120)
    return p.stdout.strip()


def ps_json(script):
    out = ps(script + " | ConvertTo-Json -Depth 4 -Compress")
    if not out:
        return None
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        return {"unparsed": out[:2000]}


def nvidia():
    fields = ("name,driver_version,temperature.gpu,clocks.sm,clocks.max.sm,clocks.mem,power.draw,"
              "pstate,utilization.gpu,memory.used,clocks_event_reasons.active")
    p = subprocess.run(["nvidia-smi", f"--query-gpu={fields}", "--format=csv,noheader,nounits"],
                       capture_output=True, text=True, timeout=60)
    keys = fields.split(",")
    values = [v.strip() for v in p.stdout.strip().split(",")]
    full = subprocess.run(["nvidia-smi"], capture_output=True, text=True, timeout=60).stdout
    procs = full[full.find("Processes:"):] if "Processes:" in full else ""
    return {"query": dict(zip(keys, values)), "processes_table": procs}


def cpu_load(samples):
    p = subprocess.run(["typeperf", r"\Processor(_Total)\% Processor Time", "-si", "1", "-sc",
                        str(samples)], capture_output=True, text=True, timeout=60 + samples)
    vals = []
    for line in p.stdout.splitlines():
        parts = line.strip().split('","')
        if len(parts) == 2:
            try:
                vals.append(float(parts[1].strip('"')))
            except ValueError:
                pass
    return {"samples_pct": [round(v, 1) for v in vals],
            "mean_pct": round(sum(vals) / len(vals), 1) if vals else None}


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("--label", default="")
    ap.add_argument("--cpu-seconds", type=int, default=5)
    a = ap.parse_args()
    result = {"label": a.label, "time": time.strftime("%Y-%m-%d %H:%M:%S")}
    result["battery"] = ps_json("Get-CimInstance Win32_Battery | Select-Object Name, BatteryStatus,"
                                " EstimatedChargeRemaining")
    # BatteryStatus 2: on AC power (not discharging); 1: discharging (on battery).
    status = (result["battery"] or {}).get("BatteryStatus") if isinstance(result["battery"], dict) else None
    result["on_ac_power"] = status == 2 if status is not None else None
    result["power_plan"] = ps("powercfg /getactivescheme")
    result["adapters"] = ps_json("Get-CimInstance Win32_VideoController | Select-Object Name,"
                                 " DriverVersion")
    result["nvidia"] = nvidia()
    result["cpu_load"] = cpu_load(a.cpu_seconds)
    result["gpu_engine_users"] = ps_json(
        "(Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction SilentlyContinue)."
        "CounterSamples | Where-Object { $_.CookedValue -gt 0.5 } | ForEach-Object { $p = "
        "[int]($_.InstanceName -replace '^pid_(\\d+)_.*','$1'); [pscustomobject]@{ pid = $p; "
        "process = (Get-Process -Id $p -ErrorAction SilentlyContinue).ProcessName; engine = "
        "$_.InstanceName; pct = [math]::Round($_.CookedValue, 1) } }")
    result["busiest_processes"] = ps_json(
        "(Get-Counter '\\Process(*)\\% Processor Time' -ErrorAction SilentlyContinue)."
        "CounterSamples | Where-Object { $_.InstanceName -notin @('_total','idle') } | "
        "Sort-Object CookedValue -Descending | Select-Object -First 10 | ForEach-Object { "
        "[pscustomobject]@{ process = $_.InstanceName; pct_of_one_core = "
        "[math]::Round($_.CookedValue, 1) } }")
    pattern = "^(" + "|".join(WATCH) + ")"
    result["watched_processes"] = ps_json(
        "Get-CimInstance Win32_Process | Where-Object { $_.Name -match '" + pattern + "' } | "
        "ForEach-Object { [pscustomobject]@{ pid = $_.ProcessId; name = $_.Name; cmd = if "
        "($_.CommandLine) { $_.CommandLine.Substring(0, [Math]::Min(200, $_.CommandLine.Length)) }"
        " } }")
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8", newline="\n")
    q = result["nvidia"]["query"]
    names = sorted({p["name"] for p in (result["watched_processes"] or []) if isinstance(p, dict)})
    print(f"{result['time']} {a.label}: AC {result['on_ac_power']}, CPU "
          f"{result['cpu_load']['mean_pct']}%, RTX {q.get('temperature.gpu')} C "
          f"{q.get('clocks.sm')}/{q.get('clocks.max.sm')} MHz {q.get('power.draw')} W "
          f"{q.get('pstate')} util {q.get('utilization.gpu')}% reasons "
          f"{q.get('clocks_event_reasons.active')}; watched: {', '.join(names)}", file=sys.stderr)


if __name__ == "__main__":
    main()
