#!/usr/bin/env python3
"""Agent-native debugging evaluation (charter 6, "an agent locates a planted defect").

For each planted bug: copy samples/sailing with the evaluation's helm layer
(tools/eval/debug_eval/scripts: a Helm component the player steers with, and the `helm` system that
turns it into the Boat's controls), plant one small TypeScript mistake, start `pocket serve`, hand
an LLM agent (opencode, by default GLM 5.3 Flash) a player's description of the SYMPTOM only, and
let it work through the host's CLI alone. opencode's permissions let its shell run `pocket ...` and
a few text filters and its file tools reach only an empty scratch directory; the shell itself runs
under sandbox-exec, which lets it read and write only that scratch directory (no home directory, no
repository, no other temporary directory, no network but the local host). The game's scripts are
therefore reachable only through `pocket scripts read/write/apply`. opencode runs with its own
configuration only (XDG_CONFIG_HOME in the run directory, Claude Code compatibility off): the
user's global rules, skills and MCP servers are not loaded; only their settings for the model's
provider (the API key file) are copied. The run directory's name says nothing about the bug.

Then the host is stopped and the scripts the agent left on disk are checked, objectively, on a
fresh host: every feature's behavioural check (the bug's own and the others', so a fix that breaks
something else fails) and that the rules' constants are unchanged (together: fixed), and whether a
scenario's world hash chain is identical to the clean game's.

Each run is recorded under docs/evidence/debug-eval/<bug>/<run>/: the prompt, the agent's raw event
stream (transcript.jsonl) and a readable transcript (transcript.md), the planted and the agent's
diffs, the host's stderr, and result.json (success, wall time, tool and CLI calls, tokens). A run
never replaces a recorded one: it takes the next free trial number.

    python3 tools/eval/debug_eval.py --selftest      # checks, scenario and sandbox are sound (macOS)
    python3 tools/eval/debug_eval.py --bugs steer-sign,reach-squared --trials 2
    python3 tools/eval/debug_eval.py --bugs goto-radians --variant debugger   # must use the debugger
    python3 tools/eval/debug_eval.py --bugs goto-radians --via mcp           # MCP tools as well
    python3 tools/eval/debug_eval.py --bugs anchor-stale --evidence /tmp/x   # a run kept elsewhere
    python3 tools/eval/debug_eval.py --recheck    # re-checks every recorded fix with this file's checks
    python3 tools/eval/debug_eval.py --report     # re-reads the transcripts, writes the results table
    python3 tools/eval/debug_eval.py --list

The pocket binary defaults to $CARGO_TARGET_DIR/release/pocket (--pocket to change). Standard
library only. macOS has no `timeout`; the time and call budgets are enforced here, and every host a
run starts is stopped.
"""

import argparse
import difflib
import json
import math
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
OVERLAY = Path(__file__).resolve().parent / "debug_eval" / "scripts"
EVIDENCE = REPO / "docs" / "evidence" / "debug-eval"
DOC = REPO / "docs" / "bench" / "debug-eval.md"
DEFAULT_MODEL = "zai-coding-plan/glm-5.3-flash"
TMP_PREFIX = "pocket-eval-"          # says nothing about the bug: the agent sees its working directory


# --- The planted bugs ----------------------------------------------------------------------------

@dataclass
class Bug:
    name: str
    file: str               # under the project
    old: str                # exact text in the clean game
    new: str                # the planted text
    symptom: str            # what the player reports, nothing about the code
    kind: str               # the mistake, for the report
    check: str              # the behavioural check that must fail with the bug and pass without
    defect: tuple           # the planted file's lines (stripped) that are the defect: "located" names one


BUGS = [
    Bug(
        name="steer-sign",
        file="scripts/helm.ts",
        old="            let rudder = clamp(h.steer[r], -1, 1) * RUDDER_GAIN;\n",
        new="            let rudder = -clamp(h.steer[r], -1, 1) * RUDDER_GAIN;\n",
        symptom=("When I turn the wheel to the left (the Sloop's Helm steer at -1) the sloop turns to the "
                 "right, and when I turn it to the right (steer 1) it goes left."),
        kind="sign error: the wheel's rudder is negated",
        check="steer",
        defect=("let rudder = -clamp(h.steer[r], -1, 1) * RUDDER_GAIN;",),
    ),
    Bug(
        name="goto-radians",
        file="scripts/helm.ts",
        old="                    const bearing = Math.atan2(dx, -dz) * 180 / Math.PI;\n",
        new="                    const bearing = Math.atan2(dx, -dz);\n",
        symptom=("When I ask the helmsman to steer for a crate (the Sloop's Helm goto set to Crate4), the "
                 "sloop never heads for it: it swings away and sails off somewhere else, and it does the "
                 "same whichever crate I pick."),
        kind="unit mix-up: the mark's bearing in radians compared with a heading in degrees",
        check="goto",
        # The bearing in radians, or the line that compares it with the heading in degrees.
        defect=("const bearing = Math.atan2(dx, -dz);",
                "rudder = clamp(angleDiff(bearing, b.heading_deg[r]) * PILOT_GAIN, -1, 1);"),
    ),
    Bug(
        name="reach-squared",
        file="scripts/rules.ts",
        old=("            const across = Math.hypot(p.x - at.x[r], p.z - at.z[r]);\n"
             "            if (across > REACH || Math.abs(p.y - at.y[r]) > REACH_UP) {\n"
             "                ctx.emit(\"interact.ignored\", { code: \"sail.out_of_reach\", crate: target, range_m: Math.round(across * 10) / 10 }, { subject: boat });\n"),
        new=("            const dx = p.x - at.x[r];\n"
             "            const dz = p.z - at.z[r];\n"
             "            const across = dx * dx + dz * dz; // squared: no square root on every try\n"
             "            if (across > REACH || Math.abs(p.y - at.y[r]) > REACH_UP) {\n"
             "                ctx.emit(\"interact.ignored\", { code: \"sail.out_of_reach\", crate: target, range_m: Math.round(Math.sqrt(across) * 10) / 10 }, { subject: boat });\n"),
        symptom=("My crew won't take crates aboard even when one floats right alongside the boat, two or three "
                 "metres away (I tell them with the Sloop's Crew take set to the crate). They say it is out "
                 "of reach."),
        kind="squared distance compared with an unsquared reach (reach 1.7 m instead of 3 m)",
        check="reach",
        # The squared distance, or the comparison with the unsquared reach.
        defect=("const across = dx * dx + dz * dz; // squared: no square root on every try",
                "if (across > REACH || Math.abs(p.y - at.y[r]) > REACH_UP) {"),
    ),
    Bug(
        name="anchor-stale",
        file="scripts/helm.ts",
        old=("            let sail = clamp(h.sail[r], 0, 1);\n"
             "            if (h.anchor[r] === 1) {\n"
             "                sail = 0;\n"
             "                rudder = 0;\n"
             "            }\n"
             "            b.rudder[r] = rudder;\n"
             "            b.hoist[r] = sail;\n"),
        new=("            const wanted = clamp(h.sail[r], 0, 1);\n"
             "            let sail = wanted;\n"
             "            if (h.anchor[r] === 1) {\n"
             "                sail = 0;\n"
             "                rudder = 0;\n"
             "            }\n"
             "            b.rudder[r] = rudder;\n"
             "            b.hoist[r] = wanted;\n"),
        symptom=("When I drop the anchor (the Sloop's Helm anchor set to true) the sloop never slows down: it "
                 "keeps sailing as fast as before."),
        kind="stale variable: the hoist is written from the value read before the anchor rule",
        check="anchor",
        defect=("b.hoist[r] = wanted;",),
    ),
    Bug(
        name="tally-off-by-one",
        file="scripts/rules.ts",
        old=("            ctx.world.despawn(crate);\n"
             "            tally.taken[r] = tally.taken[r] + 1;\n"
             "            tally.worth[r] = tally.worth[r] + value;\n"
             "            const left = tally.total[r] - tally.taken[r];\n"),
        new=("            const left = tally.total[r] - tally.taken[r];\n"
             "            ctx.world.despawn(crate);\n"
             "            tally.taken[r] = tally.taken[r] + 1;\n"
             "            tally.worth[r] = tally.worth[r] + value;\n"),
        symptom=("I took all four crates aboard, but the game never says that all the crates are aboard, and "
                 "after the last one it still says one crate is left."),
        kind="off by one: the crates left are counted before the crate just taken",
        check="tally",
        # The two lines in the wrong order: `left`, and the increment it must follow.
        defect=("const left = tally.total[r] - tally.taken[r];", "tally.taken[r] = tally.taken[r] + 1;"),
    ),
    Bug(
        name="goto-wrap",
        file="scripts/helm.ts",
        old=("    let d = (a - b) % 360;\n"
             "    if (d > 180) d -= 360;\n"
             "    if (d <= -180) d += 360;\n"
             "    return d;\n"),
        new=("    let d = (a - b) % 360;\n"
             "    if (d > 180) d -= 360;\n"
             "    return d;\n"),
        symptom=("When I ask the helmsman to steer for a crate that is behind us on the right (the Sloop's Helm "
                 "goto), he turns the long way round, to the left, nearly a full circle, before heading for it."),
        kind="a missing wrap: an angle difference below -180 degrees is not brought back into range",
        check="goto_behind",
        # The missing statement belongs between these two.
        defect=("if (d > 180) d -= 360;", "return d;"),
    ),
]
BUG = {b.name: b for b in BUGS}


# --- Building a project --------------------------------------------------------------------------

def build_project(dest: Path, bug: Bug | None) -> Path:
    """samples/sailing plus the helm layer at dest, with `bug` planted."""
    proj = dest / "sailing"
    shutil.copytree(REPO / "samples" / "sailing", proj)
    for f in OVERLAY.glob("*.ts"):
        shutil.copy(f, proj / "scripts" / f.name)
    scene = json.loads((proj / "scene.json").read_text())
    for e in scene["entities"]:
        if e["name"] == "Sloop":
            e["components"]["Helm"] = {}
    (proj / "scene.json").write_text(json.dumps(scene, indent=2) + "\n")
    if bug:
        path = proj / bug.file
        text = path.read_text()
        if text.count(bug.old) != 1:
            raise SystemExit(f"{bug.name}: the clean text is not in {bug.file} exactly once")
        path.write_text(text.replace(bug.old, bug.new))
    return proj


def scripts_of(proj: Path) -> dict:
    return {p.name: p.read_text() for p in sorted((proj / "scripts").glob("*.ts"))}


def scripts_with(bug: Bug | None) -> dict:
    """The game's scripts (with `bug` planted), built in a temporary directory deleted at once: a run
    never leaves a clean copy of the game where its agent could find it."""
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        return scripts_of(build_project(tmp, bug))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def project_with(dest: Path, scripts: dict) -> Path:
    """The game (scene and project files) at dest with exactly these scripts."""
    proj = build_project(dest, None)
    shutil.rmtree(proj / "scripts")
    (proj / "scripts").mkdir()
    for name, text in scripts.items():
        (proj / "scripts" / name).write_text(text)
    return proj


# The rules' constants (`const REACH = 3;`): the prompt forbids changing them to hide a symptom.
RULE_CONST = re.compile(r"(?m)^const ([A-Z][A-Z0-9_]*)\s*=\s*([^;\n]+);")


def rules_changed(clean: dict, final: dict) -> list:
    """The rules' constants of the clean game that the final scripts no longer declare with the same
    value (whitespace ignored)."""
    out = []
    for name, text in clean.items():
        have = {k: re.sub(r"\s+", "", v) for k, v in RULE_CONST.findall(final.get(name, ""))}
        for k, v in RULE_CONST.findall(text):
            if have.get(k) != re.sub(r"\s+", "", v):
                out.append(f"{name}: {k} = {v.strip()} -> {have.get(k, 'gone')}")
    return out


def defect_lines(bug: Bug, planted_text: str) -> list:
    """The line numbers of the defect in the planted file."""
    return [i for i, line in enumerate(planted_text.splitlines(), 1) if line.strip() in bug.defect]


def is_located(bug: Bug, answer, lines) -> bool:
    """The agent's final JSON names the planted file and one of the defect's lines exactly."""
    return bool(answer) and isinstance(answer.get("file"), str) \
        and answer["file"].lstrip("./") in (bug.file, bug.file.split("/")[-1]) \
        and isinstance(answer.get("line"), int) and answer["line"] in lines


def diff(a: dict, b: dict, la: str, lb: str) -> str:
    out = []
    for name in sorted(set(a) | set(b)):
        out += difflib.unified_diff(a.get(name, "").splitlines(True), b.get(name, "").splitlines(True),
                                    f"{la}/scripts/{name}", f"{lb}/scripts/{name}")
    return "".join(out)


# --- The host ------------------------------------------------------------------------------------

class Host:
    """`pocket serve <proj> --port 0`, stopped on exit."""

    def __init__(self, pocket: str, proj: Path, errlog: Path):
        self.pocket, self.proj = pocket, proj
        self.err = open(errlog, "w")
        self.proc = subprocess.Popen([pocket, "serve", str(proj), "--port", "0"], stdout=subprocess.PIPE,
                                     stderr=self.err, text=True, start_new_session=True)
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError(f"pocket serve did not start; see {errlog}")
        self.info = json.loads(line)
        self.url = self.info["url"]

    def env(self):
        return {**os.environ, "POCKET_HOST": self.url}

    def call(self, method, params=None):
        """A method through the CLI (`pocket call <method> '<json>' --json`)."""
        r = subprocess.run([self.pocket, "call", method, json.dumps(params or {}), "--json"],
                           capture_output=True, text=True, env=self.env(), timeout=300)
        v = json.loads(r.stdout) if r.stdout.strip() else {"error": {"code": "cli", "message": r.stderr}}
        if isinstance(v, dict) and "error" in v and len(v) == 1:
            raise RuntimeError(f"{method}: {v['error']}")
        return v

    def stop(self):
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait()
        self.err.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.stop()


# --- Behavioural checks, through the CLI ---------------------------------------------------------

def fresh(h: Host):
    h.call("snapshots.restore", {"tick": 0})


def edit(h: Host, *ops):
    h.call("world.edit", {"ops": list(ops)})


def setc(entity, component, value):
    return {"set": {"entity": entity, "component": component, "value": value}}


def boat(h: Host):
    g = h.call("world.get", {"entity": "Sloop", "components": ["Boat", "Transform", "Tally"]})["components"]
    return g


def heading_change(a, b):
    d = (b - a) % 360
    return d - 360 if d > 180 else d


def events_after(h: Host, seq):
    return h.call("events.since", {"seq": seq, "limit": 500})


def last_seq(h: Host):
    return h.call("events.since", {"limit": 1})["last"] or 0


def check_steer(h: Host):
    # The clean game turns 23 degrees left and 14 right of the wheel-0 course (weather helm pulls
    # it to port); the sign error turns 14 right and 23 left. 5 degrees leaves room for a fix
    # written differently.
    out = {}
    for steer in (0, -1, 1):
        fresh(h)
        edit(h, setc("Sloop", "Helm", {"steer": steer}))
        h.call("time.step", {"ticks": 90})
        out[steer] = boat(h)["Boat"]["heading_deg"]
    left, right = heading_change(out[0], out[-1]), heading_change(out[0], out[1])
    ok = left < -5 and right > 5
    return ok, (f"heading after 1.5 s from 90: wheel 0 {out[0]:.1f}, left {out[-1]:.1f} ({left:+.1f}), "
                f"right {out[1]:.1f} ({right:+.1f}) (pass: left < -5, right > +5)")


def crate_pos(h: Host, name):
    return h.call("world.get", {"entity": name, "components": ["Transform"]})["components"]["Transform"]["position"]


def check_goto(h: Host):
    # The clean helmsman passes Crate4 at 2.7 m, the radians bug at 19 m at best; 6 m leaves room
    # for a pursuit written differently.
    fresh(h)
    edit(h, setc("Sloop", "Helm", {"goto": "Crate4"}))
    best = 1e9
    for _ in range(30):
        h.call("time.step", {"ticks": 30})
        b = boat(h)["Transform"]["position"]
        c = crate_pos(h, "Crate4")
        best = min(best, math.hypot(c[0] - b[0], c[2] - b[2]))
        if best < 6:
            break
    return best < 6, f"nearest to Crate4 within 15 s (sampled every 0.5 s): {best:.1f} m (pass < 6 m)"


def check_goto_behind(h: Host):
    fresh(h)
    edit(h, setc("Crate1", "Transform", {"position": [-10, 0, 10]}), setc("Sloop", "Helm", {"goto": "Crate1"}))
    h.call("time.step", {"ticks": 120})
    hd = boat(h)["Boat"]["heading_deg"]
    turn = heading_change(90, hd)
    return turn > 15, f"mark at bearing 225 (behind, to starboard), heading 90: after 2 s heading {hd:.1f} ({turn:+.1f}; pass > +15, the short way)"


def check_reach(h: Host):
    fresh(h)
    edit(h, setc("Sloop", "Helm", {"sail": 0}))
    results = []
    for crate, pos, want in (("Crate1", [0, 0, 2.5], True), ("Crate2", [1.0, 0, -1.2], True),
                             ("Crate3", [0.5, 0, -3.6], False)):
        seq = last_seq(h)
        edit(h, setc(crate, "Transform", {"position": pos}), setc("Sloop", "Crew", {"take": crate}))
        h.call("time.step", {"ticks": 2})
        evs = events_after(h, seq)["events"]
        taken = any(e["name"] == "crate.taken" for e in evs)
        results.append((crate, round(math.hypot(pos[0], pos[2]), 2), want, taken))
    ok = all(w == t for *_, w, t in results)
    return ok, "; ".join(f"{c} at {d} m: {'taken' if t else 'refused'} (want {'taken' if w else 'refused'})"
                         for c, d, w, t in results)


def check_anchor(h: Host):
    fresh(h)
    h.call("time.step", {"ticks": 240})
    v0 = boat(h)["Boat"]["speed"]
    edit(h, setc("Sloop", "Helm", {"anchor": True}))
    h.call("time.step", {"ticks": 480})
    speeds, hoist = [], None
    for _ in range(6):
        h.call("time.step", {"ticks": 20})
        b = boat(h)["Boat"]
        speeds.append(abs(b["speed"]))
        hoist = b["hoist_now"]
    mean = sum(speeds) / len(speeds)
    ok = hoist < 0.05 and mean < 1.5
    return ok, f"speed {v0:.2f} m/s at anchoring; 8-10 s later mean {mean:.2f} m/s, sail {hoist:.2f} (pass: sail 0 and < 1.5 m/s)"


def check_tally(h: Host):
    fresh(h)
    edit(h, setc("Sloop", "Helm", {"sail": 0}))
    h.call("time.step", {"ticks": 1})          # muster counts the crates on the first tick
    seq = last_seq(h)
    for crate in ("Crate1", "Crate2", "Crate3", "Crate4"):
        b = boat(h)["Transform"]["position"]
        edit(h, setc(crate, "Transform", {"position": [b[0], 0, b[2] + 1.0]}), setc("Sloop", "Crew", {"take": crate}))
        h.call("time.step", {"ticks": 2})
    evs = events_after(h, seq)["events"]
    lefts = [e["data"]["left"] for e in evs if e["name"] == "crate.taken"]
    alls = [e for e in evs if e["name"] == "crates.all"]
    ok = lefts == [3, 2, 1, 0] and len(alls) == 1
    return ok, f"left after each crate: {lefts} (want [3, 2, 1, 0]); crates.all events: {len(alls)}"


CHECKS = {"steer": check_steer, "goto": check_goto, "goto_behind": check_goto_behind, "reach": check_reach,
          "anchor": check_anchor, "tally": check_tally}

def op_set(entity, component, value):
    return {"op": "set", "entity": entity, "component": component, "value": value}


# A scenario exercising every feature, compared with the clean game by its world hash chain (every
# tick, `pocket hashes --inputs`). The world hash covers the components and the persisted resources,
# among them the event inbox (each tick's events), so an event's field (`left`) counts too. The
# Sloop starts at the origin, heading 90: the four crates are brought alongside and taken first,
# Crate1 at 2.5 m (inside REACH, outside the squared-distance bug's 1.7 m), so every `left` and
# `crates.all` happens; the helmsman then steers for two spawned marks, since the crates are gone.
# --selftest asserts that every planted bug changes the chain.
SCENARIO = [
    (2, [op_set("Crate1", "Transform", {"position": [0, 0, 2.5]}), op_set("Sloop", "Crew", {"take": "Crate1"})]),
    (4, [op_set("Crate2", "Transform", {"position": [1.0, 0, -1.2]}), op_set("Sloop", "Crew", {"take": "Crate2"})]),
    (6, [op_set("Crate3", "Transform", {"position": [-1.0, 0, 1.0]}), op_set("Sloop", "Crew", {"take": "Crate3"})]),
    (8, [op_set("Crate4", "Transform", {"position": [0.6, 0, 0.8]}), op_set("Sloop", "Crew", {"take": "Crate4"})]),
    (10, [{"op": "spawn", "name": "Mark1", "components": {"Transform": {"position": [26, 0, -6]}}},
          {"op": "spawn", "name": "Mark2", "components": {"Transform": {"position": [-10, 0, 10]}}}]),
    (30, [op_set("Sloop", "Helm", {"steer": -1})]),
    (90, [op_set("Sloop", "Helm", {"steer": 1})]),
    (150, [op_set("Sloop", "Helm", {"steer": 0, "goto": "Mark1"})]),
    (400, [op_set("Sloop", "Helm", {"goto": "Mark2"})]),
    (520, [op_set("Sloop", "Helm", {"goto": None, "anchor": True})]),
]
SCENARIO_TICKS = 720


def scenario_hashes(pocket: str, proj: Path, inputs: Path, ticks=SCENARIO_TICKS):
    with open(inputs, "w") as f:
        for tick, edits in SCENARIO:
            f.write(json.dumps({"tick": tick, "source": {"player": 0}, "name": "world_edit",
                                "params": {"edits": edits}}) + "\n")
    r = subprocess.run([pocket, "hashes", str(proj), "--ticks", str(ticks), "--inputs", str(inputs)],
                       capture_output=True, text=True, timeout=600)
    if r.returncode != 0:
        return None, r.stdout + r.stderr
    return json.loads(r.stdout)["hashes"], ""


def run_checks(pocket: str, proj: Path, errlog: Path):
    """Every behavioural check on a fresh host of `proj`: {name: [ok, detail]}."""
    out = {}
    with Host(pocket, proj, errlog) as h:
        for name, fn in CHECKS.items():
            try:
                out[name] = list(fn(h))
            except Exception as e:  # a refusal (the scripts do not load, an entity is gone) fails it
                out[name] = [False, f"check failed to run: {e}"]
    return out


def clean_reference(pocket: str) -> dict:
    """The clean game's scripts and scenario hash chain."""
    clean = scripts_with(None)
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        hashes, herr = scenario_hashes(pocket, project_with(tmp, clean), tmp / "scenario.jsonl")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if hashes is None:
        raise SystemExit(f"the clean game's hash chain failed: {herr}")
    return {"scripts": clean, "hashes": hashes}


def evaluate(pocket: str, scripts: dict, reference: dict, errlog: Path) -> dict:
    """Checks a game's final scripts on a fresh host: the behavioural checks, the rules' constants,
    and the scenario's hash chain against the clean game's. `success` is the first two."""
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        proj = project_with(tmp, scripts)
        checks = run_checks(pocket, proj, errlog)
        hashes, herr = scenario_hashes(pocket, proj, tmp / "scenario.jsonl")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    changed = rules_changed(reference["scripts"], scripts)
    diverged = None
    if hashes is not None and hashes != reference["hashes"]:
        diverged = next((i for i, (x, y) in enumerate(zip(hashes, reference["hashes"])) if x != y),
                        min(len(hashes), len(reference["hashes"])))
    return {
        "success": all(ok for ok, _ in checks.values()) and not changed, "checks": checks,
        "rules_kept": not changed, "rules_changed": changed,
        "hash_chain_identical_to_clean": hashes is not None and diverged is None,
        "hash_first_divergence_tick": diverged, "hash_error": herr or None,
    }


# --- The agent -----------------------------------------------------------------------------------

PROMPT = """You are helping the player of a small sailing game made with the Amoris engine.

The game is running in a host that you reach only through the `pocket` command-line client, which
is on your PATH and already connected to it (POCKET_HOST is set). The game is paused at tick 0 in
its edit world; `pocket step <ticks>` advances it.

The game: a sloop (the entity `Sloop`) sails on a sea in a steady breeze, with crates (`Crate1` to
`Crate4`) adrift. The player plays by setting the Sloop's controls, for example
`pocket world set Sloop Helm steer=-1` (the wheel), `pocket world set Sloop Helm goto=Crate2` (the
helmsman steers for a crate), `pocket world set Sloop Helm anchor=true`, or
`pocket world set Sloop Crew take=Crate1` (the crew takes a crate aboard if it is within reach).

The player reports:
  "{symptom}"

Your job: find the cause of this in the game's scripts, fix it, and confirm the fix in the running
game.

Rules:
- Use only `pocket` commands (plus grep/head/tail/wc/sort/jq on their output). Start with
  `pocket help`; `pocket help <command>` explains one. The host has commands to inspect the world,
  step time with stop conditions, read events and logs, debug the scripts (breakpoints, watches,
  evaluation) and read, write and apply scripts.
{mcp}- Read the scripts with `pocket scripts read <path>`. Change one by writing its whole new text with
  `pocket scripts write <path> - <<'EOF' ... EOF` (or from a file you create in your working
  directory), then load it into the game with `pocket scripts apply`.
{debugger}- Fix the code; do not hide the symptom by changing the scene, the player's controls or constants
  that set the game's rules.
- Budget: at most {max_calls} tool calls and {max_minutes} minutes.

When you are done, end your final message with one line of JSON:
{{"cause": "<one sentence>", "file": "scripts/<name>.ts", "line": <the line of the defect>, "fixed": true}}
"""


MCP_RULE = """- The same host is also attached to you as MCP tools (`pocket_world`, `pocket_scripts`,
  `pocket_time`, `pocket_events`, `pocket_debug`, ...): use them or the CLI, as you prefer.
"""
DEBUGGER_RULE = """- Before you change any code, confirm the cause in the running game with the script debugger
  (`pocket debug <action> '<json>'`): stop the game inside the system you suspect with a breakpoint
  or a data watch, and look at the values there (`pocket debug state`, `pocket debug eval`). Say in
  your final message what the debugger showed.
"""


def user_provider(model: str) -> dict:
    """The user's own opencode settings for the model's provider (e.g. its API key file): the one
    thing an evaluation run takes from their global configuration."""
    home = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "opencode" / "opencode.json"
    try:
        cfg = json.loads(home.read_text())
    except (OSError, json.JSONDecodeError):
        return {}
    name = model.split("/", 1)[0]
    p = (cfg.get("provider") or {}).get(name)
    return {name: p} if p else {}


def opencode_config(path: Path, via: str, host_url: str, model: str, shell: Path):
    allow = ["pocket", "pocket *", "grep *", "head *", "tail *", "wc *", "sort *", "jq *", "echo *",
             "cat *", "sleep *"]
    bash = {"*": "deny", **{p: "allow" for p in allow}}
    cfg = {"$schema": "https://opencode.ai/config.json", "shell": str(shell),
           "permission": {"*": "deny", "bash": bash, "read": {"*": "allow"}, "edit": {"*": "allow"},
                          "external_directory": "deny"}}
    provider = user_provider(model)
    if provider:
        cfg["provider"] = provider
    if via == "mcp":
        cfg["mcp"] = {"pocket": {"type": "remote", "url": host_url + "/mcp", "enabled": True}}
        cfg["permission"]["pocket_*"] = "allow"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(cfg, indent=2) + "\n")


# The agent's shell runs under sandbox-exec. opencode's bash permissions match command words, not
# paths: `grep x ../game/scripts/helm.ts` or `pocket step 1 > /tmp/x` pass them. The sandbox reads
# and writes only the scratch directory (and the shell's own temporary directory, for heredocs),
# reads the directory holding `pocket`, and talks only to the local host. Under SBPL a later rule
# overrides an earlier one only for the operation it names: the allows name file-read-data, as the
# deny does (an allow of file-read* does not lift it).
SANDBOX = """(version 1)
(allow default)
(deny file-read-data file-write* {denied})
(deny file-write* (subpath "/"))
(allow file-write* (subpath "/dev"))
(allow file-read-data file-write* (subpath "{work}") (subpath "{shtmp}"))
(allow file-read-data (subpath "{bin}"))
(deny network*)
(allow network* (remote ip "localhost:*"))
"""


def denied_roots():
    """Everything the agent must not read: home directories, other volumes, the temporary
    directories (other runs, other agents' work), and this repository wherever it is."""
    roots = {"/Users", "/Volumes", "/private/tmp", "/private/var/folders", str(Path.home().resolve()),
             str(REPO.resolve()), str(Path(tempfile.gettempdir()).resolve())}
    return " ".join(f'(subpath "{r}")' for r in sorted(roots))


def agent_layout(tmp: Path, pocket: str, via: str, host_url: str, model: str):
    """The agent's scratch directory and environment under `tmp` (which also holds the game): the
    `pocket` on its PATH, the sandboxed shell, and opencode's own configuration directory."""
    tmp = tmp.resolve()
    workdir, bindir, shtmp, cfgdir = tmp / "agent", tmp / "bin", tmp / "shtmp", tmp / "config"
    for d in (workdir, bindir, shtmp, cfgdir):
        d.mkdir()
    os.symlink(os.path.abspath(pocket), bindir / "pocket")
    profile = tmp / "shell.sb"
    profile.write_text(SANDBOX.format(denied=denied_roots(), work=workdir, shtmp=shtmp, bin=bindir))
    shell = tmp / "shell" / "sh"     # opencode runs a shell named `sh` as `sh -c <command>`
    shell.parent.mkdir()
    shell.write_text(f"#!/bin/sh\nTMPDIR={shtmp}/ exec /usr/bin/sandbox-exec -f {profile} /bin/bash \"$@\"\n")
    shell.chmod(0o755)
    opencode_config(cfgdir / "opencode" / "opencode.json", via, host_url, model, shell)
    env = {**os.environ, "POCKET_HOST": host_url, "PATH": f"{bindir}:{os.environ['PATH']}",
           "XDG_CONFIG_HOME": str(cfgdir), "OPENCODE_DISABLE_CLAUDE_CODE": "1",
           "OPENCODE_DISABLE_EXTERNAL_SKILLS": "1"}
    return workdir, shell, env


# opencode's refusal of a denied call lists every permission rule, including the allowances it adds
# for the user's own skill directories: kept out of the evidence.
RULES = re.compile(r"(relevant rules )\[.*?\](?=\\n|\"|$)")


def redact(line: str) -> str:
    return RULES.sub(r"\1[...]", line)


def run_opencode(prompt, workdir: Path, env, model, max_time, max_calls, raw_path: Path):
    cmd = [shutil.which("opencode") or "opencode", "run", "--format", "json", "--auto", "--model", model,
           "--dir", str(workdir), prompt]
    started = time.time()
    proc = subprocess.Popen(cmd, cwd=workdir, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, start_new_session=True)
    events, stopped = [], {"by": None}
    calls = [0]
    raw = open(raw_path, "w")

    def kill(why):
        if stopped["by"] is None:
            stopped["by"] = why
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    def reader():
        for line in proc.stdout:
            line = redact(line)
            raw.write(line)
            raw.flush()
            try:
                e = json.loads(line)
            except json.JSONDecodeError:
                continue
            e["_t"] = round(time.time() - started, 2)
            events.append(e)
            if e.get("type") == "tool_use":
                calls[0] += 1
                if calls[0] >= max_calls:
                    kill("call_budget")

    err_chunks = []
    t_out = threading.Thread(target=reader, daemon=True)
    t_err = threading.Thread(target=lambda: err_chunks.append(proc.stderr.read()), daemon=True)
    t_out.start()
    t_err.start()
    while proc.poll() is None:
        if time.time() - started > max_time:
            kill("time_budget")
        time.sleep(0.5)
    t_out.join(10)
    t_err.join(10)
    kill(stopped["by"])   # whatever it left running in its group
    raw.close()
    return events, stopped["by"], round(time.time() - started, 1), "".join(err_chunks)[-20000:], proc.returncode


# A refusal as the CLI prints it: `code: message` at the start of a line.
REFUSAL = re.compile(r"(?m)^((?:request|debug|script|scripts|sim|host|history|play|snapshots|events|command|project)"
                     r"\.[a-z_.]+): ")
APPLY = re.compile(r"pocket\s+scripts\s+apply|scripts\.apply")
RESTORE = re.compile(r"pocket\s+snapshots\s+restore|snapshots\.restore|debug\.rewind|\"action\": \"rewind\"")
# The first time the agent did each of these (seconds from its first event).
FIRSTS = {"scripts_read": r"pocket\s+scripts\s+read", "debug": r"pocket\s+debug\s+(?!state\b)",
          "scripts_write": r"pocket\s+scripts\s+(write|apply)"}
POCKET_CMD = re.compile(r"(?:^|[;&|(]\s*|\n)\s*pocket\s+([a-z.]+)(?:\s+([a-z][a-z._]*))?")


def summarize(events):
    """Counts and the readable transcript from opencode's event stream."""
    tools, cli, errors, usage, cost, turns = {}, {}, 0, {"input": 0, "output": 0, "reasoning": 0,
                                                          "cache_read": 0, "total": 0}, 0.0, 0
    md, texts, step_text = [], [], []
    refusals, out_bytes, biggest, timeouts, firsts = {}, 0, 0, 0, {}
    blocked_s, eval_unseen, applies, restores = 0.0, 0, [], []
    t0 = next((e["timestamp"] for e in events if "timestamp" in e), 0)
    for e in events:
        if "_t" not in e:   # a transcript read back: seconds from its first event
            e["_t"] = round((e.get("timestamp", t0) - t0) / 1000, 2)
        kind, part = e.get("type"), e.get("part") or {}
        if kind == "tool_use":
            name = part.get("tool", "?")
            tools[name] = tools.get(name, 0) + 1
            st = part.get("state") or {}
            if st.get("status") == "error":
                errors += 1
            inp = st.get("input") or {}
            if name == "bash":
                command = inp.get("command", "")
                for m in POCKET_CMD.finditer(command):
                    sub, act = m.group(1), m.group(2)
                    key = f"{sub} {act}" if act and sub in ("debug", "world", "scripts", "time", "play",
                                                           "snapshots", "call", "help") else sub
                    cli[key] = cli.get(key, 0) + 1
                shown = command
            elif name.startswith("pocket_"):
                key = f"mcp {name[7:]} {inp.get('action', '')}".strip()
                cli[key] = cli.get(key, 0) + 1
                shown = json.dumps(inp)
            else:
                shown = json.dumps(inp)[:400]
            output = st.get("output") if st.get("status") != "error" else st.get("error")
            output = str(output or "")
            out_bytes += len(output)
            biggest = max(biggest, len(output))
            for code in REFUSAL.findall(output):
                refusals[code] = refusals.get(code, 0) + 1
            if re.search(r"timed out|timeout", output, re.I) and name == "bash":
                timeouts += 1
            if re.search(r"exceeding timeout \d+ ms|-32001: Request timed out", output):
                took = (st.get("time") or {})
                blocked_s += ((took.get("end") or 0) - (took.get("start") or 0)) / 1000
                if name != "bash":
                    timeouts += 1
            eval_unseen += len(re.findall(r"debug\.eval_failed: The expression threw: ReferenceError: \w+ is not defined",
                                          output))
            # Applies and restores in call order (a shell line may hold several).
            command = inp.get("command", "") if name == "bash" else json.dumps(inp)
            ok = st.get("status") == "completed" and "scripts.refused" not in output
            for m in APPLY.finditer(command):
                if ok:
                    applies.append(e["_t"] + m.start() * 1e-6)
            if name == "pocket_scripts" and inp.get("action") == "apply" and ok:
                applies.append(e["_t"])
            restored = "restored tick" in output or '"restored"' in output
            for m in RESTORE.finditer(command):
                if restored:
                    restores.append(e["_t"] + m.start() * 1e-6)
            if name == "bash":
                for k, pat in FIRSTS.items():
                    if k not in firsts and re.search(pat, inp.get("command", "")):
                        firsts[k] = e["_t"]
            if len(output) > 2500:
                output = output[:2500] + f"\n... ({len(output)} chars)"
            md.append(f"**[{e['_t']} s] {name}** ({st.get('status')})\n\n```\n{shown}\n```\n\n```\n{output}\n```\n")
        elif kind == "text":
            t = part.get("text", "")
            step_text.append(t)
            md.append(f"**[{e['_t']} s] agent:**\n\n{t}\n")
        elif kind == "step_finish":
            turns += 1
            tk = part.get("tokens") or {}
            for k in ("input", "output", "reasoning", "total"):
                usage[k] += tk.get(k, 0) or 0
            usage["cache_read"] += (tk.get("cache") or {}).get("read", 0) or 0
            cost += part.get("cost") or 0.0
            if any(s.strip() for s in step_text):
                texts.append("\n".join(step_text))
            step_text = []
        elif kind == "error":
            md.append(f"**[{e['_t']} s] error:** `{json.dumps(e.get('error') or e)[:800]}`\n")
    if any(s.strip() for s in step_text):
        texts.append("\n".join(step_text))
    # A restore after a fix was applied puts the old scripts back (snapshots.restore switches to the
    # bundle the snapshot was kept with): the time until the agent applied again.
    stale = None
    if applies:
        after = [t for t in restores if t > applies[0]]
        if after:
            again = [t for t in applies if t > after[0]]
            stale = {"restore_at_s": after[0], "reapplied_at_s": again[0] if again else None,
                     "lost_s": round((again[0] if again else events[-1]["_t"]) - after[0], 1),
                     "ended_with_old_scripts": max(restores) > max(applies)}
    final = texts[-1] if texts else ""
    answer = None
    for line in reversed(final.strip().splitlines()):
        line = line.strip().strip("`").strip()
        if line.startswith("{") and "cause" in line:
            try:
                answer = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    return {"tools": tools, "tool_calls": sum(tools.values()), "tool_errors": errors, "cli": cli,
            "pocket_calls": sum(cli.values()), "turns": turns, "tokens": usage, "cost_usd": round(cost, 6),
            "refusals": refusals, "output_chars": out_bytes, "largest_output_chars": biggest,
            "bash_timeouts": timeouts, "blocked_s": round(blocked_s, 1), "eval_unseen_locals": eval_unseen,
            "stale_bundle_after_restore": stale, "first_s": firsts, "answer": answer}, "\n".join(md)


def uses_debugger(cli: dict) -> bool:
    """Whether the agent set a breakpoint or a watch, or evaluated or stepped in a pause."""
    return any(re.search(r"debug\.?\s?(breakpoints\.set|watch|eval|step|pause|wait)", k) for k in cli)


def kill_strays(tmp: Path):
    """Hosts an agent started in the run's directory (it should not, but a run must leave none)."""
    r = subprocess.run(["pgrep", "-f", str(tmp)], capture_output=True, text=True)
    for pid in r.stdout.split():
        try:
            os.kill(int(pid), signal.SIGTERM)
        except (ProcessLookupError, ValueError):
            pass


def planted_lines(bug: Bug, proj_text: str):
    start = proj_text.index(bug.new)
    first = proj_text[:start].count("\n") + 1
    return first, first + bug.new.count("\n") - 1


def free_run_dir(root: Path, bug: Bug, label: str, trial: int):
    """The run's evidence directory, `<label>-<trial>` or the next free trial number: a recorded run
    is never replaced."""
    while (root / bug.name / f"{label}-{trial}").exists():
        trial += 1
    return root / bug.name / f"{label}-{trial}", trial


def one_run(a, bug: Bug, trial: int, reference):
    label = a.via + ("" if a.variant == "free" else f"-{a.variant}")
    out, got = free_run_dir(a.evidence, bug, label, trial)
    if got != trial:
        print(f"  {bug.name}: {label}-{trial} is recorded; this run is {label}-{got}", flush=True)
    trial = got
    out.mkdir(parents=True)
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    started = time.time()
    try:
        proj = build_project(tmp, bug)
        planted = scripts_of(proj)
        (out / "planted.diff").write_text(diff(reference["scripts"], planted, "clean", "planted"))
        planted_text = (proj / bug.file).read_text()
        lines, defect = planted_lines(bug, planted_text), defect_lines(bug, planted_text)
        prompt = PROMPT.format(symptom=bug.symptom, max_calls=a.max_calls, max_minutes=a.max_time // 60,
                               debugger=DEBUGGER_RULE if a.variant == "debugger" else "",
                               mcp=MCP_RULE if a.via == "mcp" else "")
        (out / "prompt.txt").write_text(prompt)
        with Host(a.pocket, proj, out / "serve.err") as host:
            workdir, _, env = agent_layout(tmp, a.pocket, a.via, host.url, a.model)
            print(f"  {bug.name} #{trial}: host {host.url}, agent {a.model} via {a.via} ...", flush=True)
            events, stopped, agent_s, stderr, code = run_opencode(
                prompt, workdir, env, a.model, a.max_time, a.max_calls, out / "transcript.jsonl")
        kill_strays(tmp)
        stats, md = summarize(events)
        (out / "transcript.md").write_text(
            f"# {bug.name}, run {label}-{trial}\n\nModel {a.model}, via {a.via}, prompt variant {a.variant}. "
            f"Prompt: [prompt.txt](prompt.txt).\n\n" + md)
        if stderr.strip():
            (out / "opencode.stderr").write_text(stderr)
        final = scripts_of(proj)
        (out / "fix.diff").write_text(diff(planted, final, "planted", "agent"))
        # The checks run on a fresh copy of the game whose scripts are the agent's.
        verdict = evaluate(a.pocket, final, reference, out / "check-serve.err")
        ans = stats.pop("answer")
        result = {
            "bug": bug.name, "kind": bug.kind, "trial": trial, "model": a.model, "via": a.via,
            "variant": a.variant, "isolation": "sandbox",
            **verdict, "bug_check": verdict["checks"][bug.check][0],
            "answer": ans, "located": is_located(bug, ans, defect),
            "planted_at": {"file": bug.file, "lines": list(lines), "defect_lines": defect},
            "stopped_by": stopped, "agent_exit": code,
            "agent_s": agent_s, "wall_s": round(time.time() - started, 1),
            "budget": {"max_calls": a.max_calls, "max_time_s": a.max_time}, **stats,
            "debug_used": uses_debugger(stats["cli"]),
        }
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"  {bug.name} #{trial}: success={result['success']} (own check {result['bug_check']}), "
              f"located={result['located']}, {stats['tool_calls']} tool calls ({stats['pocket_calls']} pocket), "
              f"{agent_s} s, {stats['tokens']['total']} tokens, stopped_by={stopped}", flush=True)
        return result
    finally:
        kill_strays(tmp)
        if not a.keep:
            shutil.rmtree(tmp, ignore_errors=True)


def apply_diff(scripts: dict, patch_text: str) -> dict:
    """`scripts` with a run's fix.diff applied (`patch`)."""
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        (tmp / "scripts").mkdir()
        for name, text in scripts.items():
            (tmp / "scripts" / name).write_text(text)
        if patch_text.strip():
            r = subprocess.run(["patch", "-p1", "-E", "--batch", "--forward", "-d", str(tmp)],
                               input=patch_text, capture_output=True, text=True)
            if r.returncode != 0:
                raise RuntimeError(f"fix.diff does not apply: {r.stdout}{r.stderr}")
        return scripts_of(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def recheck(a):
    """Re-checks every recorded run's final scripts (its planted game plus its fix.diff) with this
    file's checks, rules, scenario and `located`, and updates result.json. The verdicts it was
    recorded with stay under `as_run`."""
    reference = clean_reference(a.pocket)
    scratch = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        for res in sorted(a.evidence.glob("*/*/result.json")):
            r = json.loads(res.read_text())
            bug = BUG[r["bug"]]
            planted = scripts_with(bug)
            final = apply_diff(planted, (res.parent / "fix.diff").read_text())
            defect = defect_lines(bug, planted[bug.file.split("/")[-1]])
            r.setdefault("as_run", {k: r.get(k) for k in ("success", "located", "checks",
                                                          "hash_chain_identical_to_clean",
                                                          "hash_first_divergence_tick")})
            r.setdefault("isolation", "permissions")
            verdict = evaluate(a.pocket, final, reference, scratch / "serve.err")
            r.update(verdict)
            r["bug_check"] = verdict["checks"][bug.check][0]
            r["located"] = is_located(bug, r.get("answer"), defect)
            r["planted_at"]["defect_lines"] = defect
            res.write_text(json.dumps(r, indent=2) + "\n")
            print(f"  {res.parent.relative_to(a.evidence)}: fixed {r['success']} (as run {r['as_run']['success']}), "
                  f"rules kept {r['rules_kept']}, hash = clean {r['hash_chain_identical_to_clean']} "
                  f"(as run {r['as_run']['hash_chain_identical_to_clean']}), located {r['located']} "
                  f"(as run {r['as_run']['located']})", flush=True)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return 0


TABLE_BEGIN = "<!-- results table: written by `python3 tools/eval/debug_eval.py --report` -->"
TABLE_END = "<!-- end of results table -->"


def report(a):
    """Re-reads every run's transcript (so the counts follow this file's summarize), rewrites its
    result.json, transcript.md and redacted transcript.jsonl, writes summary.json and the results
    table in docs/bench/debug-eval.md (for the default evidence directory; printed otherwise), and
    prints the tallies."""
    runs = []
    for res in sorted(a.evidence.glob("*/*/result.json")):
        r = json.loads(res.read_text())
        raw = res.parent / "transcript.jsonl"
        lines = [redact(l) for l in raw.read_text().splitlines()]
        raw.write_text("\n".join(lines) + "\n")
        events = [json.loads(l) for l in lines if l.strip().startswith("{")]
        stats, md = summarize(events)
        label = res.parent.name.rsplit("-", 1)[0]
        (res.parent / "transcript.md").write_text(
            f"# {r['bug']}, run {label}-{r['trial']}\n\nModel {r['model']}, via {r['via']}, prompt variant "
            f"{r.get('variant', 'free')}. Prompt: [prompt.txt](prompt.txt).\n\n" + md)
        stats.pop("answer")
        r.update(stats)
        r["debug_used"] = uses_debugger(stats["cli"])
        res.write_text(json.dumps(r, indent=2) + "\n")
        r["dir"] = str(res.parent.relative_to(a.evidence))
        runs.append(r)
    order = {b.name: i for i, b in enumerate(BUGS)}
    runs.sort(key=lambda r: (r.get("variant", "free"), r["via"], order.get(r["bug"], 99), r["trial"]))
    (a.evidence / "summary.json").write_text(json.dumps(
        [{k: r.get(k) for k in ("dir", "bug", "variant", "via", "trial", "isolation", "success", "bug_check",
                                "rules_kept", "located", "hash_chain_identical_to_clean", "as_run",
                                "tool_calls", "pocket_calls",
                                "tool_errors", "agent_s", "wall_s", "tokens", "debug_used", "stopped_by", "cli",
                                "refusals", "output_chars", "largest_output_chars", "first_s", "bash_timeouts",
                                "blocked_s", "eval_unseen_locals", "stale_bundle_after_restore")} for r in runs],
        indent=2) + "\n")

    def yes(v):
        return "yes" if v else "no"
    base = os.path.relpath(a.evidence, DOC.parent)
    table = ["| Run | Bug | Fixed | Located | Hash = clean | Tool calls | `pocket` calls | "
             "Agent s | Tokens k (cached) | Debugger | Stopped by |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in runs:
        t = r["tokens"]
        table.append(
            f"| [{r['dir']}]({base}/{r['dir']}/transcript.md) | {r['bug']} | {'yes' if r['success'] else 'NO'} | "
            f"{yes(r['located'])} | {yes(r.get('hash_chain_identical_to_clean'))} | "
            f"{r['tool_calls']} | {r['pocket_calls']} | "
            f"{r['agent_s']:.0f} | {t['total'] / 1000:.0f} ({t['cache_read'] / 1000:.0f}) | "
            f"{'yes' if r['debug_used'] else '-'} | {(r['stopped_by'] or '-').replace('_', ' ')} |")
    if a.evidence.resolve() == EVIDENCE.resolve() and TABLE_BEGIN in DOC.read_text():
        doc = DOC.read_text()
        head, rest = doc.split(TABLE_BEGIN, 1)
        tail = rest.split(TABLE_END, 1)[1]
        DOC.write_text(head + TABLE_BEGIN + "\n" + "\n".join(table) + "\n" + TABLE_END + tail)
        print(f"results table written to {DOC.relative_to(REPO)}\n")
    else:
        print("\n".join(table) + "\n")
    for variant in sorted({r.get("variant", "free") + "/" + r["via"] for r in runs}):
        rs = [r for r in runs if r.get("variant", "free") + "/" + r["via"] == variant]
        uses, calls, refs = {}, {}, {}
        for r in rs:
            for k, n in r["cli"].items():
                uses[k] = uses.get(k, 0) + 1
                calls[k] = calls.get(k, 0) + n
            for k, n in r["refusals"].items():
                refs[k] = refs.get(k, 0) + n
        ok = sum(r["success"] for r in rs)
        print(f"{variant}: {ok}/{len(rs)} fixed, {sum(r['located'] for r in rs)}/{len(rs)} located; "
              f"median agent time {sorted(r['agent_s'] for r in rs)[len(rs) // 2]} s; "
              f"median tool calls {sorted(r['tool_calls'] for r in rs)[len(rs) // 2]}")
        print("  commands (runs using it / calls): " + ", ".join(
            f"{k} {uses[k]}/{calls[k]}" for k in sorted(uses, key=lambda k: (-uses[k], -calls[k]))))
        print("  refusals: " + (", ".join(f"{k} {n}" for k, n in sorted(refs.items(), key=lambda x: -x[1])) or "none"))
        stale = [r["stale_bundle_after_restore"] for r in rs if r.get("stale_bundle_after_restore")]
        print(f"  calls that blocked until the agent's shell gave up: {sum(r['bash_timeouts'] for r in rs)} in "
              f"{sum(1 for r in rs if r['bash_timeouts'])} runs, {sum(r['blocked_s'] for r in rs):.0f} s; "
              f"debug.eval 'is not defined' on a listed local: {sum(r['eval_unseen_locals'] for r in rs)} in "
              f"{sum(1 for r in rs if r['eval_unseen_locals'])} runs")
        again = [x for x in stale if x["reapplied_at_s"] is not None]
        print(f"  restored a snapshot after applying the fix (the old scripts came back): {len(stale)} runs; "
              f"{len(again)} applied again, {sum(x['lost_s'] for x in again):.0f} s after the restore in all; "
              f"{sum(1 for x in stale if x['ended_with_old_scripts'])} left the host running the old scripts")
        print(f"  tool output per run (chars): median "
              f"{sorted(r['output_chars'] for r in rs)[len(rs) // 2]:,}, largest single "
              f"{max(r['largest_output_chars'] for r in rs):,}")
    near = [f"{r['dir']} (line {r['answer']['line']}, defect {r['planted_at'].get('defect_lines')})"
            for r in runs if not r["located"] and r.get("answer") and isinstance(r["answer"].get("line"), int)
            and any(abs(r["answer"]["line"] - n) <= 2 for n in r["planted_at"].get("defect_lines") or [])]
    print(f"named the defect's file with a line one or two off: {len(near)} runs" + (": " + "; ".join(near) if near else ""))
    return 0


# Commands run through the agent's sandboxed shell by --selftest, as opencode runs them (`sh -c`):
# (command, whether it must work, text its output must contain).
PROBES = [
    ("pocket status", True, "tick 0"),
    ("pocket scripts read scripts/helm.ts | grep -c atan2", True, "1"),
    ("echo note > notes.txt && cat notes.txt", True, "note"),
    ("cat <<'EOF' | wc -l\na\nb\nEOF", True, "2"),
    ("head -3 ../sailing/scripts/helm.ts", False, "Operation not permitted"),
    ("grep -rn REACH {repo}/samples/sailing/scripts", False, "Operation not permitted"),
    ("grep -c . {home}/.config/opencode/opencode.json", False, "Operation not permitted"),
    ("pocket step 1 > {tmpdir}/pocket-eval-probe.out", False, "Operation not permitted"),
    ("pocket hashes ../sailing --ticks 1", False, "Operation not permitted"),
]


def sandbox_probe(pocket: str, model: str) -> bool:
    """The agent's layout around a running host: what its shell can and cannot reach, and what
    opencode resolves with the run's configuration (when opencode is installed)."""
    ok = True
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        proj = build_project(tmp, None)
        with Host(pocket, proj, tmp / "serve.err") as host:
            workdir, shell, env = agent_layout(tmp, pocket, "cli", host.url, model)
            # Printed with the machine's paths as placeholders.
            places = {"{repo}": str(REPO), "{home}": str(Path.home()), "{tmpdir}": tempfile.gettempdir().rstrip("/")}
            for template, must_work, want in PROBES:
                cmd = template.format(**{k[1:-1]: v for k, v in places.items()})
                r = subprocess.run([str(shell), "-c", cmd], cwd=workdir, env=env, capture_output=True,
                                   text=True, timeout=60)
                got = (r.stdout + r.stderr).strip()
                good = want in got and (r.returncode == 0 or not must_work)
                ok &= good
                shown = got.splitlines()[0] if got else "(no output)"
                for k, v in places.items():
                    shown = shown.replace(v, k)
                print(f"{'ok  ' if good else 'BAD '} sandbox {'allows' if must_work else 'refuses'}: "
                      f"{template.splitlines()[0]}  ->  {shown}")
            if shutil.which("opencode"):
                def debug(*what):
                    r = subprocess.run(["opencode", "debug", *what], cwd=workdir, env=env, capture_output=True,
                                       text=True, timeout=120, stdin=subprocess.DEVNULL)
                    return r.stdout
                cfg = json.loads(debug("config"))
                skills = json.loads(debug("skill"))
                paths = dict(line.split(None, 1) for line in debug("paths").splitlines() if line.strip())
                # opencode 1.18 reads <config>/AGENTS.md, ~/.claude/CLAUDE.md unless Claude Code
                # compatibility is off, the `instructions` setting, and AGENTS.md, CLAUDE.md or
                # CONTEXT.md from the working directory upwards.
                found = [str(d / n) for d in [Path(paths.get("config", "/")), workdir, *workdir.parents]
                         for n in ("AGENTS.md", "CLAUDE.md", "CONTEXT.md") if (d / n).exists()]
                checks = [("opencode's configuration directory is the run's", paths.get("config", "").startswith(str(tmp.resolve()))),
                          ("its shell is the sandboxed one", cfg.get("shell") == str(shell)),
                          (f"MCP servers: {sorted(cfg.get('mcp') or {})}", not cfg.get("mcp")),
                          (f"skills: {[s['name'] for s in skills]}", all(s.get("location") == "<built-in>" for s in skills)),
                          (f"instruction files: {found + (cfg.get('instructions') or [])}, ~/.claude "
                           f"{'off' if env.get('OPENCODE_DISABLE_CLAUDE_CODE') else 'ON'}",
                           not found and not cfg.get("instructions") and bool(env.get("OPENCODE_DISABLE_CLAUDE_CODE")))]
                for what, good in checks:
                    ok &= good
                    print(f"{'ok  ' if good else 'BAD '} opencode: {what}")
    finally:
        kill_strays(tmp)
        shutil.rmtree(tmp, ignore_errors=True)
    return ok


def selftest(a):
    """The clean game passes every check and its scenario replays identically; each planted bug fails
    its own check and changes the scenario's hash chain; hiding a symptom by changing a rule's
    constant passes the behavioural checks but is not a fix; the agent's sandbox holds."""
    ok = True
    reference = clean_reference(a.pocket)
    print(f"clean reference: hash chain of {len(reference['hashes'])} ticks")
    cases = [("clean", None, reference["scripts"])] + [(n, BUG[n], scripts_with(BUG[n])) for n in a.bugs]
    # Forbidden by the prompt, and invisible to the behavioural checks and the scenario.
    for name, const_old, const_new in (("steer-sign", "const RUDDER_GAIN = 0.6;", "const RUDDER_GAIN = -0.6;"),
                                       ("reach-squared", "const REACH = 3;", "const REACH = 9;")):
        if name in a.bugs:
            s = scripts_with(BUG[name])
            f = BUG[name].file.split("/")[-1]
            s[f] = s[f].replace(const_old, const_new)
            cases.append((f"{name}+{const_new.split()[1]}", "tweak", s))
    tmp = Path(tempfile.mkdtemp(prefix=TMP_PREFIX))
    try:
        for name, bug, scripts in cases:
            v = evaluate(a.pocket, scripts, reference, tmp / "serve.err")
            for c, (passed, detail) in v["checks"].items():
                print(f"  {name:22} {c:12} {'pass' if passed else 'FAIL'}  {detail}")
            same = ("hash chain identical" if v["hash_chain_identical_to_clean"] else
                    f"hash chain diverges at tick {v['hash_first_divergence_tick']}")
            if bug is None:
                good = v["success"] and v["hash_chain_identical_to_clean"]
                what = f"every check passes, rules kept, {same} in a second run"
            elif bug == "tweak":
                good = not v["success"] and not v["rules_kept"]
                what = (f"behavioural checks {'all pass' if all(p for p, _ in v['checks'].values()) else 'fail'}, "
                        f"{same}; not a fix: {v['rules_changed']}")
            else:
                others = [c for c, (p, _) in v["checks"].items() if not p and c != bug.check]
                good = not v["checks"][bug.check][0] and not v["hash_chain_identical_to_clean"]
                what = f"its own check ({bug.check}) fails" + (f"; also fails {others}" if others else "") + f"; {same}"
            print(f"{'ok  ' if good else 'BAD '} {name}: {what}")
            ok &= good
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    ok &= sandbox_probe(a.pocket, a.model)
    return ok


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    target = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
    ap.add_argument("--pocket", default=str(target / "release" / "pocket"),
                    help="the pocket binary (default $CARGO_TARGET_DIR/release/pocket)")
    ap.add_argument("--bugs", default=",".join(b.name for b in BUGS))
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--first-trial", type=int, default=1,
                    help="the first trial number (a recorded one is skipped to the next free number)")
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--via", default="cli", choices=["cli", "mcp"],
                    help="cli: the shell's `pocket` only; mcp: also the host's MCP tools")
    ap.add_argument("--variant", default="free", choices=["free", "debugger"],
                    help="free: the agent picks its tools; debugger: it must confirm the cause with the "
                         "script debugger before changing code")
    ap.add_argument("--max-time", type=int, default=900, help="seconds the agent may take")
    ap.add_argument("--max-calls", type=int, default=60, help="tool calls the agent may make")
    ap.add_argument("--evidence", default=str(EVIDENCE),
                    help="where runs are recorded (default docs/evidence/debug-eval)")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--recheck", action="store_true",
                    help="re-check every recorded run's fix with this file's checks and update result.json")
    ap.add_argument("--report", action="store_true",
                    help="re-read every recorded run, update its result.json and the results table, print tallies")
    ap.add_argument("--keep", action="store_true", help="keep the temporary projects")
    a = ap.parse_args()
    a.evidence = Path(a.evidence).resolve()
    a.bugs = [b.strip() for b in a.bugs.split(",") if b.strip()]
    for n in a.bugs:
        if n not in BUG:
            raise SystemExit(f"no bug '{n}'; bugs: {', '.join(BUG)}")
    if a.report:
        return report(a)
    if a.list:
        for b in BUGS:
            print(f"{b.name:17} {b.file:17} {b.kind}\n{'':17} symptom: {b.symptom}")
        return 0
    if not Path(a.pocket).exists():
        raise SystemExit(f"{a.pocket} is not built (cargo build --release -p pocket-app, or --pocket PATH)")
    if not Path("/usr/bin/sandbox-exec").exists():
        raise SystemExit("the agent's shell runs under /usr/bin/sandbox-exec (macOS)")
    if a.selftest:
        return 0 if selftest(a) else 1
    if a.recheck:
        return recheck(a)
    reference = clean_reference(a.pocket)
    results = []
    for trial in range(a.first_trial, a.first_trial + a.trials):
        for n in a.bugs:
            results.append(one_run(a, BUG[n], trial, reference))
    print(json.dumps([{k: r[k] for k in ("bug", "trial", "success", "located", "tool_calls", "agent_s")}
                      for r in results]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
