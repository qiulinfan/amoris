#!/usr/bin/env python3
"""Run actual model-selected Windward player or debugger calls; keep raw runs in out/.

Gameplay uses the native HTTP seat projection. The fly convenience tool only sets the exact
controls the model chose, then waits the requested ticks; it contains no steering policy.
Debugging uses catalog schemas and an exact-text edit lowered to scripts.read/scripts.write.
No requests occur on import. Credentials are read from an explicit external file.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "tools/eval")]
from agent_dev_bench import Host
from deepseek_agent import Ledger, Limits, run_agent


def tool(name, description, properties, required=()):
    return {"type": "function", "function": {"name": name, "description": description,
        "parameters": {"type": "object", "properties": properties,
                       "required": list(required), "additionalProperties": False}}}


class Session:
    def __init__(self, host, output):
        self.host, self.output = host, output
        self.calls = []

    def call(self, method, params=None, seat=None):
        body = {"id": 1, "method": method, "params": params or {}}
        if seat:
            body["seat"] = seat
        req = urllib.request.Request(self.host.url + "/api/call", data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as r:
            result = json.load(r)
        self.calls.append({"method": method, "params": params or {}, "seat": seat, "reply": result})
        (self.output / "native-calls.json").write_text(json.dumps(self.calls, indent=2) + "\n")
        return result.get("result", result)

    def observe(self):
        return self.call("player.observe", {"budget_tokens": 1600}, "player")

    def fly(self, args):
        ticks = args["ticks"]
        if type(ticks) is not int or not 1 <= ticks <= 120:
            return {"error": {"message": "ticks must be 1..120"}}
        controls = {"forward": args["forward"], "strafe": args["strafe"], "run": args.get("gust", False)}
        if any(type(v) not in (float, int) or not -1 <= v <= 1 for v in (controls["forward"], controls["strafe"])):
            return {"error": {"message": "controls must be within -1..1"}}
        actions = [{"do": "set", "controls": controls}]
        if args.get("burst"):
            actions.append({"do": "pulse", "control": "primary"})
        act = self.call("player.act", {"actions": actions, "resume": True}, "player")
        if "error" in act:
            return act
        wait = self.call("player.wait", {"ticks": ticks, "until": {"event": "wind.finish"}}, "player")
        result = {"observation": self.observe(), "time": wait}
        print(json.dumps({"tool": "fly", "controls": controls, "ticks": ticks,
                          "instruments": result["observation"].get("instruments", [])}), flush=True)
        return result


def gameplay(session, args, ledger):
    s = session
    # The renderer and recorder remain developer clients; the model's calls all name its seat.
    refusal = s.call("world.query", {"with": ["Flight"]}, "player")
    if refusal.get("error", {}).get("code") != "permission.denied":
        raise RuntimeError("The native player projection did not refuse developer access")
    number = {"type": "number", "minimum": -1, "maximum": 1}
    tools = [tool("observe", "Your native player observation: visible gates, own flight instruments, events.", {}),
             tool("fly", "Set exactly these joystick inputs, hold for ticks (60 Hz), then observe. "
                  "There is no autopilot. strafe +1 turns RIGHT (heading decreases); forward +1 dives. "
                  "The wing takes time to bank and unbank. Zero strafe levels the wing; zero forward levels the nose.",
                  {"forward": number, "strafe": number, "gust": {"type": "boolean"},
                   "burst": {"type": "boolean"}, "ticks": {"type": "integer", "minimum": 1, "maximum": 120},
                   "note": {"type": "string", "description": "Short public action label; no private reasoning."}},
                  ("forward", "strafe", "ticks"))]
    prompt = ("Fly Windward through all ten gates in order. You have only your player's native perception "
              "and controls, no world query, edits, script access, or steering executor. Gate order is zero-based; "
              "gates instrument is how many you passed. Gate centres are visible pos_m. Your position is "
              "(position_x_m, altitude_m, position_z_m), heading and pitch are radians. Heading h means "
              "forward (-sin(h),0,-cos(h)); strafe +1 decreases heading. Positive forward dives; negative climbs. "
              "You must account for slow pitch/bank response and release controls before overshooting. "
              "Use short holds near a gate and longer holds when lined up far away. Gate radius is 8.6 metres. "
              "The game is paused during model calls, 60 ticks are one second. "
              "Make every joystick and wait choice yourself. Prioritize completing the course over beating the rival. "
              "Continue until ten gates or the light runs out. Report actual progress honestly.")
    return run_agent("You are a game player using the native Amoris player API. Use tools to play; "
                     "keep public action labels short. Do not ask for privileged access.", prompt,
                     tools, lambda name, a: s.observe() if name == "observe" else s.fly(a),
                     args.output / "agent", Limits(budget_usd="5", max_requests=240,
                     max_tool_calls=700, max_seconds=2400, max_output_tokens=2048,
                     reasoning_effort="low"), args.credential_file, ledger)


def debugging(session, args, ledger):
    s = session
    allowed = {"world.tree", "world.get", "world.query", "world.edit", "history.undo", "history.list",
               "scripts.list", "scripts.read", "scripts.apply", "time.step", "debug.state", "debug.eval",
               "debug.breakpoints.set", "debug.breakpoints.clear", "debug.continue", "debug.step"}
    catalog = json.load(urllib.request.urlopen(s.host.url + "/api/catalog"))
    names = {x["name"].replace(".", "_"): x["name"] for x in catalog if x["name"] in allowed}
    tools = [{"type": "function", "function": {"name": x["name"].replace(".", "_"),
             "description": x["doc"], "parameters": x["params"]}} for x in catalog if x["name"] in allowed]
    tools.append(tool("scripts_patch", "Replace one exact occurrence in scripts/main.ts. Lowered to native "
                      "scripts.read then scripts.write; compiles but does not apply. Read the actual text first.",
                      {"old": {"type": "string"}, "new": {"type": "string"}}, ("old", "new")))

    def dispatch(name, a):
        if name == "scripts_patch":
            old = s.call("scripts.read", {"path": "main.ts"})["text"]
            if not a["old"] or old.count(a["old"]) != 1:
                return {"error": {"message": "The old text must match exactly once"}}
            result = s.call("scripts.write", {"path": "main.ts", "text": old.replace(a["old"], a["new"], 1)})
        else:
            result = s.call(names[name], a)
        # Native completed-boundary captures alongside real tools, including held debugger states.
        try:
            c = s.host.call("capture", {"width": 960, "height": 540, "frames": 1})
            shutil.copy2(c["path"], args.output / f"tool-{len(s.calls):04d}.png")
        except Exception:
            pass
        return result

    prompt = ("The first wind gate fails to count when the glider passes through its opening. "
              "I have paused a reproducible attempt near Gate0; the glider is inside the visible ring. "
              "Find the cause using the native script debugger: set a breakpoint in the gate test, "
              "step into it and evaluate the relevant local values before editing. Fix only the defective "
              "predicate in main.ts, preserve the gate radius and game rules, clear breakpoints, continue, "
              "apply the script, then step and verify Course.gates increases. "
              "After the fix, demonstrate reversible scene editing: find the decorative Arch4, "
              "move it upward 2 metres with world.edit, read its Transform to verify, undo once, "
              "and verify the original position is back. Do not save scene.json or edit other scripts. "
              "Finish with a short report of the values you actually observed. "
              "You may read main.ts in line ranges; scripts_patch saves repeating the whole file.")
    return run_agent("You are an Amoris game developer. All supplied tools call the real engine. "
                     "Use its paused-frame debugger to establish the cause, then hot-apply the minimal fix.",
                     prompt, tools, dispatch, args.output / "agent", Limits(budget_usd="5", max_requests=55,
                     max_tool_calls=120, max_seconds=1200, max_output_tokens=4096,
                     reasoning_effort="high"), args.credential_file, ledger)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("mode", choices=["gameplay", "debug"])
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--credential-file", type=Path, required=True)
    p.add_argument("--ledger", type=Path, default=ROOT / "out/windward-showcase/ledger.json")
    p.add_argument("--amoris", type=Path, default=ROOT / "target/release/amoris")
    args = p.parse_args()
    args.output = args.output.resolve()
    if args.output.exists():
        raise SystemExit("Use a fresh output directory")
    if not args.output.is_relative_to(ROOT / "out"):
        raise SystemExit("Run output belongs under ignored out/")
    if args.credential_file.resolve().is_relative_to(ROOT):
        raise SystemExit("Credentials must be outside the repository")
    args.output.mkdir(parents=True)
    candidate = args.output / "candidate"
    shutil.copytree(ROOT / "samples/windward", candidate, ignore=shutil.ignore_patterns(".amoris", "__pycache__"))
    if args.mode == "debug":
        script = candidate / "scripts/main.ts"
        clean = script.read_text()
        correct = "gates.cols.Gate.radius[r] * gates.cols.Gate.radius[r]"
        assert clean.count(correct) == 1
        script.write_text(clean.replace(correct, "gates.cols.Gate.radius[r]", 1))
    ledger = Ledger(args.ledger, "5")  # The shared evaluator's preflight ceiling.
    with Host(args.amoris.resolve(), candidate, args.output / "host.log") as host:
        s = Session(host, args.output)
        host.call("play.start", {"paused": True})
        if args.mode == "debug":
            host.set("Transform", {"position": [170, 160, 55]}, entity="Glider")
            host.set("Flight", {"speed": 0, "invuln_s": 100}, entity="Glider")
        host.step(1)
        initial = host.call("status")
        (args.output / "initial.json").write_text(json.dumps(initial, indent=2) + "\n")
        summary = (gameplay if args.mode == "gameplay" else debugging)(s, args, ledger)
        state = host.call("status")
        judge = host.call("world.query", {
            "with": ["Course"], "fields": ["Course.gates", "Course.total", "Course.medal", "Course.elapsed_s"]})
        verdict = {"state": state, "course": judge, "agent": summary}
        (args.output / "result.json").write_text(json.dumps(verdict, indent=2) + "\n")
        print(json.dumps(verdict), flush=True)


if __name__ == "__main__":
    main()
