#!/usr/bin/env python3
"""Frozen, provider-independent game-development tasks graded by the native Amoris host.

The candidate receives only prepare()'s project and prompt. Grade code, golden source and task
mutations remain outside that directory. Models/tools/budgets are owned by the calling runner.
No API keys, provider requests or source-string success tests are used here.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import shutil
import signal
import subprocess
import threading
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

REPO = Path(__file__).resolve().parents[2]
BENCH = REPO / "bench" / "agent-dev"
TEMPLATE = BENCH / "template"
VERSION = "courier-v1"


@dataclass(frozen=True)
class Task:
    id: str
    title: str
    kind: str
    file: str
    requirement: str
    old: str
    new: str

    def metadata(self) -> dict:
        return {"id": self.id, "title": self.title, "kind": self.kind,
                "allowed_files": [self.file], "suite_version": VERSION,
                "prompt": prompt(self)}


TASKS = (
    Task("collection-range", "Repair collection radius", "bugfix", "scripts/collect.ts",
         "Parcels two or three metres away are incorrectly rejected. Collection must accept horizontal "
         "distance at most 3 units, and vertical distance at most 2 units, inclusive. Reject either "
         "distance outside these bounds. Keep missing-parcel handling, consumed intents and score intact.",
         "if (distanceSquared > 9 ||", "if (distanceSquared > 3 ||"),
    Task("score-values", "Repair parcel value scoring", "bugfix", "scripts/collect.ts",
         "Collecting valuable parcels increases score by just one. Score must add each Cargo.value "
         "exactly once (including value zero), while collected counts parcels. Rejected or repeated "
         "attempts must not score. Preserve collection range and objective behavior.",
         "wallet.points[r] = wallet.points[r] + cargo.value;", "wallet.points[r] = wallet.points[r] + 1;"),
    Task("objective-victory", "Implement collection victory", "feature", "scripts/progress.ts",
         "Implement victory in updateProgress. When Wallet.total is positive and collected reaches "
         "or exceeds total, set that courier's Progress.completed and emit game.won once, with "
         "{collected, points} and that courier as subject. No victory for an empty objective. Persist "
         "the flag in the component so later ticks and forced script reloads do not repeat the event. "
         "Keep the existing score milestone feature.",
         '''            if (wallet.total[r] > 0 && wallet.collected[r] >= wallet.total[r] && progress.completed[r] === 0) {
                progress.completed[r] = 1;
                ctx.emit("game.won", { collected: wallet.collected[r], points: wallet.points[r] }, { subject: e });
            }''',
         "            // TODO: implement the collection objective's victory transition."),
    Task("dash-cooldown", "Implement persistent dash cooldown", "feature", "scripts/dash.ts",
         "Add dash behavior to moveCourier. A request on a tick at or after DashState.ready_at "
         "multiplies that tick's clamped movement by 4, sets ready_at to ctx.tick + 5, and emits "
         "courier.dashed with {ready_at} and the courier as subject. Earlier requests keep normal "
         "movement. Consume every request, whether accepted or rejected. Normal movement remains "
         "one unit per axis per tick, with each axis clamped to [-1,1]. Cooldown must live in "
         "DashState and survive forced reload and Play's explicit fork without changing the edit world.",
         '''            if (input.request[r] === 1 && ctx.tick >= state.ready_at[r]) {
                multiplier = 4;
                state.ready_at[r] = ctx.tick + 5;
                ctx.emit("courier.dashed", { ready_at: state.ready_at[r] }, { subject: e });
            }''',
         "            // TODO: implement dash acceptance and its persisted deadline."),
    Task("milestone-event", "Implement once-only score milestone", "feature", "scripts/progress.ts",
         "Implement the score milestone in updateProgress. Each courier reaching at least 10 "
         "Wallet.points for the first time sets its Progress.milestone and emits score.milestone "
         "with {points} and that courier as subject. It must not emit below 10, after falling below "
         "and crossing again, on later ticks, or after forced reload. Independent couriers each have "
         "their own flag. Keep collection victory working.",
         '''            if (wallet.points[r] >= 10 && progress.milestone[r] === 0) {
                progress.milestone[r] = 1;
                ctx.emit("score.milestone", { points: wallet.points[r] }, { subject: e });
            }''',
         "            // TODO: implement each courier's persisted score milestone."),
    Task("damage-and-defeat", "Implement armored damage and defeat", "feature", "scripts/damage.ts",
         "Implement applyDamage. Consume DamageInput.amount on the next tick. Positive hits reduce "
         "health by max(0, amount - Vitals.armor), clamped at zero; armor never heals. A hit leaving "
         "health at zero sets Vitals.defeated and emits game.lost with {health:0} and that courier "
         "as subject exactly once. Zero input does nothing. The defeated flag must survive reload. "
         "Preserve all unrelated collection, score, movement and progress behavior.",
         '''            const damage = Math.max(0, amount - vitals.armor[r]);
            vitals.health[r] = Math.max(0, vitals.health[r] - damage);
            if (vitals.health[r] === 0 && vitals.defeated[r] === 0) {
                vitals.defeated[r] = 1;
                ctx.emit("game.lost", { health: 0 }, { subject: e });
            }''',
         "            // TODO: implement armored damage and the persisted defeat transition."),
)
TASK_BY_ID = {t.id: t for t in TASKS}


def task_by_id(task: str | Task) -> Task:
    return TASK_BY_ID[task] if isinstance(task, str) else task


def prompt(task: str | Task) -> str:
    t = task_by_id(task)
    return ("You are developing Courier Arena in the running Amoris engine.\n\n" + t.requirement +
            "\n\nUse the native host's scripts/world/time/play APIs to inspect, change, apply and verify "
            "the game. scripts.guide explains the SDK. State belongs in ECS components; systems are "
            "stateless. Only edit " + t.file + ". Preserve existing component schemas, other modules, "
            "scene.json, project.toml and check.toml. Do not change the engine or grader. Apply your "
            "scripts and check their types. Finish with a concise description and your runtime checks.\n")


def files_of(project: Path) -> dict[str, str]:
    return {p.relative_to(project).as_posix(): p.read_text() for p in sorted(project.rglob("*"))
            if p.is_file() and ".pocket" not in p.parts}


def prepare(task: str | Task, dest: str | Path) -> dict:
    """Create a fresh candidate project exactly at dest; refuse an existing destination."""
    t, dest = task_by_id(task), Path(dest).resolve()
    shutil.copytree(TEMPLATE, dest, ignore=shutil.ignore_patterns(".pocket", "__pycache__"))
    f = dest / t.file
    source = f.read_text()
    if source.count(t.old) != 1:
        raise RuntimeError(f"frozen task {t.id}: mutation anchor is not unique")
    f.write_text(source.replace(t.old, t.new))
    return {**t.metadata(), "project_path": str(dest), "fixture_sha256": suite_fingerprint()}


def suite_fingerprint() -> str:
    h = hashlib.sha256()
    for p in sorted(TEMPLATE.rglob("*")):
        if p.is_file() and ".pocket" not in p.parts:
            h.update(p.relative_to(TEMPLATE).as_posix().encode() + b"\0" + p.read_bytes() + b"\0")
    h.update(json.dumps([t.metadata() for t in TASKS], sort_keys=True).encode())
    return h.hexdigest()


class Host:
    """Own one native host, with bounded startup/calls and unconditional cleanup."""
    def __init__(self, pocket: str | Path, project: Path, stderr: Path):
        self.log = stderr.open("w")
        env = {**os.environ, "CARGO_TARGET_DIR": str(REPO / "target")}
        self.proc = subprocess.Popen([str(pocket), "serve", str(project), "--port", "0"],
                                     stdout=subprocess.PIPE, stderr=self.log, text=True,
                                     start_new_session=True, env=env)
        q = queue.Queue()
        threading.Thread(target=lambda: q.put(self.proc.stdout.readline()), daemon=True).start()
        try:
            line = q.get(timeout=30)
            if not line:
                raise RuntimeError(f"native host failed to start; see {stderr}")
            self.info = json.loads(line)
            self.url = self.info["url"]
        except BaseException:
            self.stop()
            raise

    def call(self, method: str, params: dict | None = None) -> dict:
        req = urllib.request.Request(self.url + "/api/call",
              data=json.dumps({"id": 1, "method": method, "params": params or {}}).encode(),
              headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r:
            out = json.load(r)
        if "error" in out:
            raise RuntimeError(f"{method}: {out['error']}")
        return out["result"]

    def get(self, *components: str, entity: str = "Courier") -> dict:
        return self.call("world.get", {"entity": entity, "components": list(components)})["components"]

    def set(self, component: str, value: dict, entity: str = "Courier"):
        return self.call("world.edit", {"ops": [{"set": {"entity": entity,
                         "component": component, "value": value}}]})

    def fresh(self):
        self.call("snapshots.restore", {"tick": 0})
        self.step()

    def step(self, ticks: int = 1):
        result = self.call("time.step", {"ticks": ticks})
        if result.get("errors"):
            raise RuntimeError(f"script tick failed: {result['errors']}")
        return result

    def sequence(self) -> int:
        return self.call("events.since", {"limit": 1}).get("last") or 0

    def events(self, seq: int, name: str) -> list:
        return [e for e in self.call("events.since", {"seq": seq, "limit": 500})["events"]
                if e["name"] == name]

    def reload(self):
        return self.call("scripts.apply", {"force": True})

    def stop(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait()
        self.log.close()
        if self.proc.stdout:
            self.proc.stdout.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.stop()


class Assertions:
    def __init__(self):
        self.cases = []

    def check(self, name: str, actual, expected):
        self.cases.append({"case": name, "passed": actual == expected,
                           "actual": actual, "expected": expected})

    def receipt(self) -> dict:
        return {"passed": all(c["passed"] for c in self.cases), "cases": self.cases}


def collect_at(h: Host, parcel: str, xyz: list[float]):
    h.set("Transform", {"position": xyz}, entity=parcel)
    h.set("Command", {"take": parcel})
    h.step()


def check_range(h: Host) -> dict:
    a = Assertions()
    for xyz, accept in [([2.5, 0, 0], True), ([3, 0, 0], True), ([3.001, 0, 0], False),
                        ([2.5, 0, 2], False), ([0, 2, 0], True), ([0, 2.001, 0], False)]:
        h.fresh()
        seq = h.sequence()
        collect_at(h, "ParcelA", xyz)
        g = h.get("Wallet", "Command")
        a.check(f"range {xyz}", g["Wallet"]["collected"], int(accept))
        a.check(f"consume range attempt {xyz}", g["Command"]["take"], None)
        a.check(f"range event {xyz}", len(h.events(seq, "cargo.taken")), int(accept))
        parcels = h.call("world.query", {"with": ["Cargo"], "name": "ParcelA"})
        a.check(f"parcel survives iff rejected {xyz}", len(parcels), int(not accept))
    h.fresh()
    h.call("world.edit", {"ops": [{"spawn": {"name": "EmptyMark", "components": {"Transform": {}}}}]})
    h.set("Command", {"take": "EmptyMark"})
    h.step()
    a.check("non-parcel target does not collect", h.get("Wallet")["Wallet"]["collected"], 0)
    a.check("non-parcel intent consumed", h.get("Command")["Command"]["take"], None)
    return a.receipt()


def check_score(h: Host) -> dict:
    h.fresh()
    a = Assertions()
    for parcel, value in [("ParcelA", 0), ("ParcelB", 7), ("ParcelC", 11)]:
        h.set("Cargo", {"value": value}, entity=parcel)
        collect_at(h, parcel, [1, 0, 0])
    w = h.get("Wallet")["Wallet"]
    a.check("sum actual values including zero", w["points"], 18)
    a.check("count parcels independently of value", w["collected"], 3)
    h.step(4)
    a.check("idle ticks cannot score again", h.get("Wallet")["Wallet"]["points"], 18)
    h.fresh()
    collect_at(h, "ParcelB", [4, 0, 0])
    a.check("rejected cargo cannot score", h.get("Wallet")["Wallet"]["points"], 0)
    return a.receipt()


def check_victory(h: Host) -> dict:
    h.fresh()
    a, seq = Assertions(), h.sequence()
    h.set("Wallet", {"collected": 2, "total": 3, "points": 12})
    h.step()
    a.check("before objective completion", len(h.events(seq, "game.won")), 0)
    h.set("Wallet", {"collected": 3})
    h.step()
    h.reload()
    h.step(5)
    evs = h.events(seq, "game.won")
    a.check("victory exactly once across forced reload", len(evs), 1)
    if evs:
        a.check("victory data", evs[0]["data"], {"collected": 3, "points": 12})
        a.check("victory subject", evs[0].get("subject"), h.call("world.get", {"entity": "Courier"})["id"])
    a.check("completion flag persists", h.get("Progress")["Progress"]["completed"], True)
    h.fresh()
    seq = h.sequence()
    h.set("Wallet", {"collected": 0, "total": 0})
    h.step()
    a.check("empty objective is not won", len(h.events(seq, "game.won")), 0)
    a.check("empty objective remains incomplete", h.get("Progress")["Progress"]["completed"], False)
    return a.receipt()


def check_dash(h: Host) -> dict:
    h.fresh()
    a, seq = Assertions(), h.sequence()
    h.set("DashInput", {"dx": 2, "dz": -2, "request": True})
    stepped = h.step()
    g = h.get("DashState", "DashInput", "Transform")
    a.check("deadline is five ticks after accepted dash", g["DashState"]["ready_at"], stepped["tick"] + 5)
    a.check("dash clamps both axes and multiplies by four", g["Transform"]["position"], [4.0, 0.0, -4.0])
    a.check("accepted request consumed", g["DashInput"]["request"], False)
    deadline = g["DashState"]["ready_at"]
    a.check("first dash event", len(h.events(seq, "courier.dashed")), 1)
    h.reload()
    h.set("DashInput", {"request": True})
    h.step()
    g = h.get("DashState", "DashInput", "Transform")
    a.check("cooldown survives forced reload", g["Transform"]["position"], [5.0, 0.0, -5.0])
    a.check("rejected request consumed", g["DashInput"]["request"], False)
    a.check("blocked request keeps deadline", g["DashState"]["ready_at"], deadline)
    # Play is the engine's explicit fork: work on its branch, stop and compare edit state.
    before = h.get("DashState", "DashInput", "Transform")
    h.call("play.start", {"paused": True})
    h.set("DashInput", {"request": True})
    h.step()
    a.check("fork carries cooldown", h.get("Transform")["Transform"]["position"], [6.0, 0.0, -6.0])
    h.call("play.stop")
    a.check("fork leaves edit state intact", h.get("DashState", "DashInput", "Transform"), before)
    h.set("DashInput", {"dx": 0, "dz": 0})
    h.step(3)
    h.set("DashInput", {"dx": 1, "dz": 0, "request": True})
    h.step()
    a.check("dash available at deadline", h.get("Transform")["Transform"]["position"], [9.0, 0.0, -5.0])
    return a.receipt()


def check_milestone(h: Host) -> dict:
    h.fresh()
    a, seq = Assertions(), h.sequence()
    h.set("Wallet", {"points": 9})
    h.step()
    a.check("no milestone below threshold", len(h.events(seq, "score.milestone")), 0)
    h.set("Wallet", {"points": 10})
    h.step()
    h.reload()
    h.step(3)
    h.set("Wallet", {"points": 4})
    h.step()
    h.set("Wallet", {"points": 17})
    h.step()
    evs = h.events(seq, "score.milestone")
    a.check("once only across reload and recrossing", len(evs), 1)
    if evs:
        a.check("milestone records crossing score", evs[0]["data"], {"points": 10})
    a.check("milestone flag persists", h.get("Progress")["Progress"]["milestone"], True)
    h.call("world.edit", {"ops": [{"spawn": {"name": "SecondCourier", "components":
                        {"Wallet": {"points": 14}, "Progress": {}}}}]})
    h.step()
    a.check("each courier has own milestone", len(h.events(seq, "score.milestone")), 2)
    a.check("second courier persists its own flag", h.get("Progress", entity="SecondCourier")["Progress"]["milestone"], True)
    return a.receipt()


def check_damage(h: Host) -> dict:
    h.fresh()
    a, seq = Assertions(), h.sequence()
    h.set("Vitals", {"armor": 5})
    h.set("DamageInput", {"amount": 12})
    h.step()
    a.check("armor absorbs five damage", h.get("Vitals")["Vitals"]["health"], 93)
    a.check("damage consumed", h.get("DamageInput")["DamageInput"]["amount"], 0)
    h.step(2)
    a.check("consumed hit not repeated", h.get("Vitals")["Vitals"]["health"], 93)
    h.set("DamageInput", {"amount": 3})
    h.step()
    a.check("armor does not heal", h.get("Vitals")["Vitals"]["health"], 93)
    a.check("nonlethal hits do not lose game", len(h.events(seq, "game.lost")), 0)
    h.set("DamageInput", {"amount": 200})
    h.step()
    a.check("lethal damage clamps health", h.get("Vitals")["Vitals"]["health"], 0)
    a.check("defeat flag persists", h.get("Vitals")["Vitals"]["defeated"], True)
    h.reload()
    h.set("DamageInput", {"amount": 20})
    h.step()
    evs = h.events(seq, "game.lost")
    a.check("defeat event once across reload and later hits", len(evs), 1)
    if evs:
        a.check("defeat data", evs[0]["data"], {"health": 0})
    h.fresh()
    seq = h.sequence()
    h.set("Vitals", {"health": 0})
    h.step()
    a.check("zero damage input does nothing", len(h.events(seq, "game.lost")), 0)
    return a.receipt()


CHECKS: dict[str, Callable[[Host], dict]] = {
    "collection-range": check_range, "score-values": check_score, "objective-victory": check_victory,
    "dash-cooldown": check_dash, "milestone-event": check_milestone, "damage-and-defeat": check_damage,
}


def grade(task: str | Task, project: str | Path, pocketbin: str | Path,
          output: str | Path | None = None) -> dict:
    """Grade a frozen candidate through actual native engine execution, never source matching."""
    t, project, pocketbin = task_by_id(task), Path(project).resolve(), Path(pocketbin).resolve()
    started = time.monotonic()
    output = Path(output).resolve() if output else project.parent / (project.name + "-grade")
    output.mkdir(parents=True, exist_ok=True)
    baseline = files_of(TEMPLATE)
    baseline[t.file] = baseline[t.file].replace(t.old, t.new)
    final = files_of(project)
    changed = sorted(k for k in set(baseline) | set(final) if baseline.get(k) != final.get(k))
    violations = [k for k in changed if k != t.file]
    types = subprocess.run([str(pocketbin), "check", str(project), "--json"],
                           capture_output=True, text=True, timeout=90, cwd=REPO)
    (output / "native-check.json").write_text(types.stdout or "{}")
    try:
        type_result = json.loads(types.stdout)
        type_ok = any(step["name"] == "types" and step["verdict"] == "Pass" for step in type_result.get("steps", []))
        integrity_ok = types.returncode == 0 and type_result.get("verdict") == "Pass"
    except ValueError:
        type_result, type_ok, integrity_ok = {"stderr": types.stderr}, False, False
    checks = {}
    if type_ok:
        try:
            with Host(pocketbin, project, output / "host.stderr") as h:
                for name, fn in CHECKS.items():
                    try:
                        checks[name] = fn(h)
                    except Exception as e:
                        checks[name] = {"passed": False, "error": str(e), "cases": []}
        except Exception as e:
            checks["host"] = {"passed": False, "error": str(e), "cases": []}
    focus = checks.get(t.id, {}).get("passed", False)
    regression = len(checks) == len(CHECKS) and all(v["passed"] for n, v in checks.items() if n != t.id)
    receipt = {"format": 1, "suite_version": VERSION, "fixture_sha256": suite_fingerprint(),
               "grader_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
               "task": t.id, "kind": t.kind, "passed": bool(type_ok and integrity_ok and focus and regression and not violations),
               "feature_passed": focus, "regressions_passed": regression, "types_passed": type_ok, "integrity_passed": integrity_ok,
               "scope_passed": not violations, "out_of_scope_files": violations,
               "changed_files": changed, "checks": checks,
               "native_checks": type_result.get("steps", []),
               "type_summary": [step for step in type_result.get("steps", []) if step["name"] == "types"],
               "duration_s": round(time.monotonic() - started, 3)}
    (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


def selftest(pocketbin: str | Path, output: str | Path) -> dict:
    """Prove each planted fixture fails its focus and restored golden source passes the entire suite."""
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    results = []
    for task in TASKS:
        candidate = output / task.id / "candidate"
        prepare(task, candidate)
        bad = grade(task, candidate, pocketbin, output / task.id / "before")
        shutil.copyfile(TEMPLATE / task.file, candidate / task.file)
        good = grade(task, candidate, pocketbin, output / task.id / "after")
        results.append({"task": task.id, "initial_focus_fails": not bad["feature_passed"],
                        "initial_regressions_pass": bad["regressions_passed"],
                        "initial_types_pass": bad["types_passed"], "initial_integrity_pass": bad["integrity_passed"], "golden_passes": good["passed"]})
        print(json.dumps(results[-1]), flush=True)
    receipt = {"format": 1, "suite_version": VERSION, "fixture_sha256": suite_fingerprint(),
               "passed": all(r["initial_focus_fails"] and r["initial_regressions_pass"]
                             and r["initial_types_pass"] and r["initial_integrity_pass"] and r["golden_passes"] for r in results),
               "tasks": results}
    (output / "selftest.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("command", choices=["list", "prepare", "grade", "selftest"])
    p.add_argument("--task", choices=TASK_BY_ID)
    p.add_argument("--project", type=Path)
    p.add_argument("--output", type=Path)
    p.add_argument("--pocket", type=Path,
                   default=Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target")) / "release" / "pocket")
    a = p.parse_args()
    if a.command == "list":
        print(json.dumps({"suite_version": VERSION, "fixture_sha256": suite_fingerprint(),
                          "tasks": [t.metadata() for t in TASKS]}, indent=2))
    elif a.command == "prepare":
        if not a.task or not a.project:
            p.error("prepare requires --task and --project (a fresh path)")
        print(json.dumps(prepare(a.task, a.project), indent=2))
    elif a.command == "grade":
        if not a.task or not a.project:
            p.error("grade requires --task and --project")
        r = grade(a.task, a.project, a.pocket, a.output)
        print(json.dumps(r, indent=2))
        raise SystemExit(0 if r["passed"] else 1)
    else:
        if not a.output:
            p.error("selftest requires --output (fresh case paths)")
        r = selftest(a.pocket, a.output)
        print(json.dumps(r, indent=2))
        raise SystemExit(0 if r["passed"] else 1)


if __name__ == "__main__":
    main()
