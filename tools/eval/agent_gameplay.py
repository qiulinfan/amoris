#!/usr/bin/env python3
"""Restricted project-level sailing gateway; not global engine authorization.

The provider receives only tools()/dispatch() results. Native host URLs, entity IDs,
world hashes, fixture files and judge results remain private to the evaluator.
Navigation is an explicitly disclosed in-game Helm executor, not an LLM policy.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import urllib.request

REPO = Path(__file__).resolve().parents[2]
FIXTURE = REPO / "site/demos/agent-sailing"
OBSERVE_RADIUS = 30.0
TOOL_NAMES = {"observe", "helm", "navigate", "take", "wait"}


def tools() -> list[dict]:
    def tool(name, description, properties, required=()):
        return {"type": "function", "function": {"name": name, "description": description,
                "parameters": {"type": "object", "properties": properties,
                               "required": list(required), "additionalProperties": False}}}
    number = lambda low, high, description: {"type": "number", "minimum": low, "maximum": high,
                                           "description": description}
    target = {"type": "string", "pattern": r"^cargo-[1-9][0-9]*$",
              "description": "Alias from the current observe result, never an engine entity name."}
    return [
        tool("observe", "Observe your boat, wind, progress and cargo within 30 metres. No time passes.", {}),
        tool("helm", "Set the wheel and sail. Cancels cargo navigation. The game crew auto-trims the sheet; "
             "optional heading_deg engages the disclosed native game Helm heading executor. Advances one tick.",
             {"steer": number(-1, 1, "-1 left/port, +1 right/starboard."),
              "sail": number(0, 1, "Sail hoist fraction; zero furls the sail."),
              "heading_deg": number(0, 360, "Optional compass heading: 0 toward north (-z), 90 east (+x).")},
             ("steer", "sail")),
        tool("navigate", "Ask the disclosed game Helm executor to steer directly toward currently observed cargo. "
             "It uses native sailing physics and auto-trim, not a route or tacking planner. It does not collect cargo. "
             "Advance with wait and issue take when can_take is true. Advances one tick.",
             {"target": target, "sail": number(0, 1, "Sail hoist fraction; default 0.7.")}, ("target",)),
        tool("take", "Tell your native Crew to collect currently observed cargo when within 3 metres horizontally "
             "and vertically. The original rules decide success and consume the intent. Advances one tick.",
             {"target": target}, ("target",)),
        tool("wait", "Advance the unchanged native simulation by 1..120 ticks; 60 ticks are one second. "
             "Your current helm/navigation intent persists. Returns a new observation.",
             {"ticks": {"type": "integer", "minimum": 1, "maximum": 120}}, ("ticks",)),
    ]


def fixture_fingerprint(project: Path = FIXTURE) -> str:
    h = hashlib.sha256()
    for p in sorted(project.rglob("*")):
        if p.is_file() and ".pocket" not in p.parts and "__pycache__" not in p.parts:
            h.update(p.relative_to(project).as_posix().encode() + b"\0")
            with p.open("rb") as f:
                for chunk in iter(lambda: f.read(1024 * 1024), b""): h.update(chunk)
            h.update(b"\0")
    return h.hexdigest()


class NativeHost:
    """Private local host; its unrestricted call method is never a provider tool."""
    def __init__(self, pocket: Path, project: Path, log_path: Path):
        self.log = log_path.open("w")
        self.proc = subprocess.Popen([str(pocket), "serve", str(project), "--port", "0"],
                stdout=subprocess.PIPE, stderr=self.log, text=True, start_new_session=True,
                env={**os.environ, "CARGO_TARGET_DIR": str(REPO / "target")})
        q = queue.Queue()
        threading.Thread(target=lambda: q.put(self.proc.stdout.readline()), daemon=True).start()
        try:
            line = q.get(timeout=30)
            if not line: raise RuntimeError("Native host did not start; inspect its private log.")
            self.info = json.loads(line)
            self.url = self.info["url"]
            self.call("time.control", {"pacing": "stepped", "pause": True})
        except BaseException:
            self.stop()
            raise

    def call(self, method: str, params: dict | None = None):
        req = urllib.request.Request(self.url + "/api/call", data=json.dumps(
            {"id": 1, "method": method, "params": params or {}}).encode(),
            headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r: result = json.load(r)
        if "error" in result: raise RuntimeError(f"Native call {method} failed: {result['error']}")
        return result["result"]

    def get(self, entity: str | int, *components: str):
        return self.call("world.get", {"entity": entity, "components": list(components)})["components"]

    def set(self, component: str, value: dict):
        return self.call("world.edit", {"ops": [{"set": {
            "entity": "Sloop", "component": component, "value": value}}]})

    def state(self):
        s = self.call("status")
        return {"tick": s["tick"], "world_hash": s["world_hash"]}

    def step(self, ticks):
        r = self.call("time.step", {"ticks": ticks})
        if r.get("errors"): raise RuntimeError(f"Native script errors: {r['errors']}")
        return r

    def stop(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try: self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait()
        self.log.close()
        if self.proc.stdout: self.proc.stdout.close()


class Rejected(ValueError):
    def __init__(self, code, message):
        self.code, self.message = code, message
        super().__init__(message)


class Gameplay:
    def __init__(self, pocket: str | Path, project: str | Path | None = None,
                 run_dir: str | Path | None = None, *, on_step=None, step_stride: int = 2):
        self.source_project = Path(project or FIXTURE).resolve()
        self.fixture_sha256 = fixture_fingerprint(self.source_project)
        self.run_dir = Path(run_dir or tempfile.mkdtemp(prefix="amoris-player-")).resolve()
        self.run_dir.mkdir(parents=True, exist_ok=True)
        self.project = self.run_dir / "project"
        shutil.copytree(self.source_project, self.project,
                        ignore=shutil.ignore_patterns(".pocket", "__pycache__"))
        self.host = NativeHost(Path(pocket).resolve(), self.project, self.run_dir / "host.log")
        self.actions: list[dict] = []
        self.aliases: dict[int, str] = {}
        self._observed: set[str] = set()
        self.on_step = on_step
        self.step_stride = max(1, int(step_stride))
        try:
            self.host.step(1)  # native start systems initialize Tally and boat readings
            self.initial_state = self.host.state()
        except BaseException:
            self.close()
            raise

    @staticmethod
    def tools(): return tools()

    def close(self): self.host.stop()
    def __enter__(self): return self
    def __exit__(self, *_): self.close()

    def _advance(self, ticks):
        if self.on_step is None:
            self.host.step(ticks)
            return
        remaining = ticks
        while remaining:
            current_tick = self.host.state()["tick"]
            until_sample = self.step_stride - current_tick % self.step_stride
            n = min(remaining, until_sample)
            self.host.step(n)
            remaining -= n
            state = self.host.state()
            if state["tick"] % self.step_stride == 0:
                self.on_step(self, state)  # private evaluator callback; never model-visible

    def _boat(self): return self.host.get("Sloop", "Boat", "Transform", "Tally", "Helm")

    def _cargo(self, boat):
        p = boat["Transform"]["position"]
        rows = self.host.call("world.query", {"with": ["Cargo", "Transform"],
                  "fields": ["Transform.position"]})
        visible = []
        for row in rows:
            at = row["Transform.position"]
            dx, dz = at[0] - p[0], at[2] - p[2]
            distance = math.hypot(dx, dz)
            if distance > OBSERVE_RADIUS: continue
            alias = self.aliases.get(row["id"])
            if alias is None:
                alias = f"cargo-{len(self.aliases) + 1}"
                self.aliases[row["id"]] = alias
            visible.append({"target": alias, "bearing_deg": round(math.degrees(math.atan2(dx, -dz)) % 360, 2),
                "distance_m": round(distance, 3), "can_take": distance <= 3 and abs(at[1] - p[1]) <= 3,
                "_entity": row["id"]})
        return visible

    def _observation(self):
        b = self._boat()
        cargo = self._cargo(b)
        self._observed = {r["target"] for r in cargo}
        wind = self.host.get("Breeze", "Wind")["Wind"]
        boat = b["Boat"]
        h = b["Helm"]
        return {"tick": self.host.state()["tick"], "rate_hz": 60, "sight_range_m": OBSERVE_RADIUS,
            "boat": {k: boat[k] for k in ["heading_deg", "speed", "awa_deg", "aws", "heel_deg", "afloat", "aground"]},
            "wind": {"from_deg": wind["from_deg"], "speed_m_s": wind["speed"]},
            "helm": {"steer": h["steer"], "sail": h["sail"],
                     "heading_deg": h["heading_deg"] if h["heading_enabled"] else None,
                     "navigation_target": self.aliases.get(h["goto"]) if h["goto"] else None},
            "progress": {"collected": b["Tally"]["taken"], "total": b["Tally"]["total"], "worth": b["Tally"]["worth"]},
            "cargo": [{k: v for k, v in r.items() if not k.startswith("_")} for r in cargo]}

    @staticmethod
    def _number(v, lo, hi, name):
        if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or not lo <= v <= hi:
            raise Rejected("invalid_argument", f"{name} must be a finite number in [{lo}, {hi}].")
        return v

    def _target(self, alias):
        if not isinstance(alias, str) or not re.fullmatch(r"cargo-[1-9][0-9]*", alias) or alias not in self._observed:
            raise Rejected("target_not_observed", "Use a cargo alias from the current observation.")
        visible = self._cargo(self._boat())
        match = next((r for r in visible if r["target"] == alias), None)
        if match is None: raise Rejected("target_not_observed", "That cargo is no longer visible.")
        return match

    def dispatch(self, name: str, arguments: dict | None = None):
        arguments = {} if arguments is None else arguments
        before = self.host.state()
        native_intents = []
        try:
            if name not in TOOL_NAMES: raise Rejected("tool_not_allowed", "Only the five player tools are available.")
            if not isinstance(arguments, dict): raise Rejected("invalid_argument", "Tool arguments must be an object.")
            allowed = {"observe": set(), "helm": {"steer", "sail", "heading_deg"},
                       "navigate": {"target", "sail"}, "take": {"target"}, "wait": {"ticks"}}[name]
            if set(arguments) - allowed: raise Rejected("invalid_argument", "Unexpected argument fields.")
            if name == "helm":
                steer = self._number(arguments.get("steer"), -1, 1, "steer")
                sail = self._number(arguments.get("sail"), 0, 1, "sail")
                heading = arguments.get("heading_deg")
                if "heading_deg" in arguments: self._number(heading, 0, 360, "heading_deg")
                value = {"steer": steer, "sail": sail, "goto": None, "anchor": False,
                         "heading_enabled": heading is not None}
                if heading is not None: value["heading_deg"] = heading % 360
                self.host.set("Helm", value)
                native_intents.append({"component": "Helm", "value": value})
                self._advance(1)
            elif name == "navigate":
                target = self._target(arguments.get("target"))
                sail = self._number(arguments.get("sail", 0.7), 0, 1, "sail")
                value = {"goto": target["_entity"], "sail": sail, "anchor": False, "heading_enabled": False}
                self.host.set("Helm", value)
                native_intents.append({"component": "Helm", "value": value})
                self._advance(1)
            elif name == "take":
                target = self._target(arguments.get("target"))
                self.host.set("Crew", {"take": target["_entity"]})
                native_intents.append({"component": "Crew", "value": {"take": target["_entity"]}})
                self._advance(1)
            elif name == "wait":
                ticks = arguments.get("ticks")
                if isinstance(ticks, bool) or not isinstance(ticks, int) or not 1 <= ticks <= 120:
                    raise Rejected("invalid_argument", "ticks must be an integer in [1, 120].")
                self._advance(ticks)
            result = {"ok": True, "observation": self._observation()}
        except Rejected as e:
            result = {"ok": False, "error": {"code": e.code, "message": e.message},
                      "observation": self._observation()}
        after = self.host.state()
        self.actions.append({"sequence": len(self.actions), "tool": name,
            "arguments": copy.deepcopy(arguments), "before": before, "after": after,
            "native_intents": native_intents, "result": copy.deepcopy(result)})
        return result

    def judge(self):
        """Private grading result: never send to the provider."""
        state = self.host.state()
        tally = self._boat()["Tally"]
        return {"collected": tally["taken"], "total": tally["total"], "worth": tally["worth"],
                "score": tally["taken"] / max(1, tally["total"]), "success": tally["taken"] == tally["total"] > 0,
                "tick": state["tick"], "world_hash": state["world_hash"], "fixture_sha256": self.fixture_sha256}


def replay_actions(actions: list[dict], *, pocket: str | Path, project=None, run_dir=None,
                   on_step=None, capture_stride: int = 2):
    """Fresh native replay, optionally capture after chunks of two ticks; verify every action hash.

    Callback signature on_step(gameplay, private_state). It may read/capture, not edit.
    Original wait inputs remain unchanged even when native stepping is split for capture.
    Samples align to global tick multiples; an odd final boundary gets one explicitly
    labeled final callback so the terminal collection state can also be pictured.
    """
    with Gameplay(pocket, project, run_dir, on_step=on_step, step_stride=capture_stride) as g:
        parity = []
        for expected in actions:
            result = g.dispatch(expected["tool"], expected["arguments"])
            actual = g.actions[-1]
            same = all(actual[k] == expected[k] for k in ["before", "after", "result", "native_intents"])
            parity.append({"sequence": actual["sequence"], "passed": same,
                           "expected_hash": expected["after"]["world_hash"],
                           "actual_hash": actual["after"]["world_hash"]})
            if not same: raise RuntimeError(f"Fresh replay diverged at action {actual['sequence']}.")
        if on_step is not None and g.host.state()["tick"] % capture_stride:
            on_step(g, {**g.host.state(), "capture_kind": "final_boundary"})
        return {"passed": all(x["passed"] for x in parity), "actions": len(actions),
                "parity": parity, "judge": g.judge(), "replayed_actions": g.actions}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pocket", required=True, type=Path)
    parser.add_argument("--out", default=REPO / "out/agent-gameplay-selftest", type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    with Gameplay(args.pocket, run_dir=args.out / "probe") as g:
        print(json.dumps(g.dispatch("observe", {}), indent=2))
        print(json.dumps(g.judge()))


if __name__ == "__main__": main()
