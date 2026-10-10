#!/usr/bin/env python3
"""Records the machine's conditions before or between measurements (docs/bench/quiet-2026-10-09.md).

Windows only (the R1 laptop): power source and plan, the NVIDIA GPU's temperature, clocks, power
and throttle reasons (nvidia-smi), the CPU load over a few seconds (typeperf), and GPU-engine use
(both adapters). The JSON is safe to publish: numeric conditions and generic process basenames,
without PIDs, engine identifiers, process command lines or complete process inventories.

    python tools/machine_conditions.py out/bench-runs/quiet/conditions-start.json [--label before-dx12]
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path, PureWindowsPath


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
        return None


def nvidia():
    fields = ("name,driver_version,temperature.gpu,clocks.sm,clocks.max.sm,clocks.mem,power.draw,"
              "pstate,utilization.gpu,memory.used,clocks_event_reasons.active")
    p = subprocess.run(["nvidia-smi", f"--query-gpu={fields}", "--format=csv,noheader,nounits"],
                       capture_output=True, text=True, timeout=60)
    keys = fields.split(",")
    values = [v.strip() for v in p.stdout.strip().split(",")]
    return {"query": dict(zip(keys, values))}


def gpu_users(samples):
    """Normalize PowerShell's singleton/list JSON to the public process-name/percentage schema."""
    samples = samples if isinstance(samples, list) else [samples] if samples else []
    return [{"process": PureWindowsPath(str(s.get("process") or "unknown")).name,
             "pct": s.get("pct")} for s in samples if isinstance(s, dict)]


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
    battery = ps_json("Get-CimInstance Win32_Battery | Select-Object BatteryStatus")
    # BatteryStatus 2: on AC power (not discharging); 1: discharging (on battery).
    status = battery.get("BatteryStatus") if isinstance(battery, dict) else None
    result["on_ac_power"] = status == 2 if status is not None else None
    result["power_plan"] = ps("powercfg /getactivescheme")
    result["adapters"] = ps_json("Get-CimInstance Win32_VideoController | Select-Object Name,"
                                 " DriverVersion")
    result["nvidia"] = nvidia()
    result["cpu_load"] = cpu_load(a.cpu_seconds)
    result["gpu_engine_users"] = gpu_users(ps_json(
        "(Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction SilentlyContinue)."
        "CounterSamples | Where-Object { $_.CookedValue -gt 0.5 } | ForEach-Object { $p = "
        "[int]($_.InstanceName -replace '^pid_(\\d+)_.*','$1'); [pscustomobject]@{ "
        "process = (Get-Process -Id $p -ErrorAction SilentlyContinue).ProcessName; "
        "pct = [math]::Round($_.CookedValue, 1) } }"))
    out = Path(a.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8", newline="\n")
    q = result["nvidia"]["query"]
    names = sorted({p["process"] for p in result["gpu_engine_users"]})
    print(f"{result['time']} {a.label}: AC {result['on_ac_power']}, CPU "
          f"{result['cpu_load']['mean_pct']}%, RTX {q.get('temperature.gpu')} C "
          f"{q.get('clocks.sm')}/{q.get('clocks.max.sm')} MHz {q.get('power.draw')} W "
          f"{q.get('pstate')} util {q.get('utilization.gpu')}% reasons "
          f"{q.get('clocks_event_reasons.active')}; GPU users: {', '.join(names)}", file=sys.stderr)


if __name__ == "__main__":
    main()
