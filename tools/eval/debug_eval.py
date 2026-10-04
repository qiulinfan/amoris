#!/usr/bin/env python3
"""Agent-native debugging evaluation (charter 6, "an agent locates a planted defect").

For each planted bug: copy samples/sailing with the evaluation's helm layer
(tools/eval/debug_eval/scripts: a Helm component the player steers with, and the `helm` system that
turns it into the Boat's controls), plant one small TypeScript mistake, start `pocket serve`, hand
an LLM agent (opencode, by default GLM 5.3 Flash) a player's description of the SYMPTOM only, and
let it work through the host's CLI alone: its shell runs `pocket ...` and a few text filters, its
file tools reach only an empty scratch directory, so the game's scripts are reachable only through
`pocket scripts read/write/apply`. Then stop the host and check, objectively, on a fresh host built
from the scripts the agent left on disk: every feature's behavioural check (the bug's own and the
others', so a fix that breaks something else fails), and whether a scenario's hash chain is bit
identical to the clean game's.

Each run is recorded under docs/evidence/debug-eval/<bug>/<run>/: the prompt, the agent's raw event
stream (transcript.jsonl) and a readable transcript (transcript.md), the planted and the agent's
diffs, the host's stderr, and result.json (success, wall time, tool and CLI calls, tokens).

    python3 tools/eval/debug_eval.py --selftest                 # clean passes, every planted bug fails
    python3 tools/eval/debug_eval.py --bugs steer-sign,reach-squared --trials 2
    python3 tools/eval/debug_eval.py --bugs goto-radians --variant debugger   # must use the debugger
    python3 tools/eval/debug_eval.py --bugs goto-radians --via mcp           # MCP tools as well
    python3 tools/eval/debug_eval.py --report     # the results table and tallies (docs/bench/debug-eval.md)
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
DEFAULT_MODEL = "zai-coding-plan/glm-5.3-flash"


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
    out = {}
    for steer in (0, -1, 1):
        fresh(h)
        edit(h, setc("Sloop", "Helm", {"steer": steer}))
        h.call("time.step", {"ticks": 90})
        out[steer] = boat(h)["Boat"]["heading_deg"]
    left, right = heading_change(out[0], out[-1]), heading_change(out[0], out[1])
    ok = left < -10 and right > 10
    return ok, f"heading after 1.5 s from 90: wheel 0 {out[0]:.1f}, left {out[-1]:.1f} ({left:+.1f}), right {out[1]:.1f} ({right:+.1f})"


def crate_pos(h: Host, name):
    return h.call("world.get", {"entity": name, "components": ["Transform"]})["components"]["Transform"]["position"]


def check_goto(h: Host):
    fresh(h)
    edit(h, setc("Sloop", "Helm", {"goto": "Crate4"}))
    best = 1e9
    for _ in range(30):
        h.call("time.step", {"ticks": 30})
        b = boat(h)["Transform"]["position"]
        c = crate_pos(h, "Crate4")
        best = min(best, math.hypot(c[0] - b[0], c[2] - b[2]))
        if best < 3:
            break
    return best < 3, f"closest approach to Crate4 within 15 s: {best:.1f} m (pass < 3 m)"


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

# A scenario exercising every feature, for the hash chain against the clean game.
SCENARIO = [
    (30, [setc("Sloop", "Helm", {"steer": -1})]),
    (90, [setc("Sloop", "Helm", {"steer": 1})]),
    (150, [setc("Sloop", "Helm", {"steer": 0, "goto": "Crate4"})]),
    (400, [setc("Crate2", "Transform", {"position": [-10, 0, 10]}), setc("Sloop", "Helm", {"goto": "Crate2"})]),
    (520, [setc("Sloop", "Helm", {"goto": None, "anchor": True})]),
    (600, [setc("Crate1", "Transform", {"position": [0, 0, 0]}), setc("Sloop", "Crew", {"take": "Crate1"})]),
]


def scenario_hashes(pocket: str, proj: Path, inputs: Path, ticks=720):
    with open(inputs, "w") as f:
        for tick, edits in SCENARIO:
            ops = [{"op": "set", **e["set"]} for e in edits]
            f.write(json.dumps({"tick": tick, "source": {"player": 0}, "name": "world_edit",
                                "params": {"edits": ops}}) + "\n")
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


# --- The agent -----------------------------------------------------------------------------------

PROMPT = """You are helping the player of a small sailing game made with the Pocket3D engine.

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


def opencode_config(workdir: Path, via: str, host_url: str):
    allow = ["pocket", "pocket *", "grep *", "head *", "tail *", "wc *", "sort *", "jq *", "echo *",
             "cat *", "sleep *"]
    bash = {"*": "deny", **{p: "allow" for p in allow}}
    cfg = {"$schema": "https://opencode.ai/config.json",
           "permission": {"*": "deny", "bash": bash, "read": {"*": "allow"}, "edit": {"*": "allow"},
                          "external_directory": "deny"}}
    if via == "mcp":
        cfg["mcp"] = {"pocket": {"type": "remote", "url": host_url + "/mcp", "enabled": True}}
        cfg["permission"]["pocket_*"] = "allow"
    (workdir / "opencode.json").write_text(json.dumps(cfg, indent=2) + "\n")


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


def one_run(a, bug: Bug, trial: int, reference_hashes):
    label = a.via + ("" if a.variant == "free" else f"-{a.variant}")
    out = EVIDENCE / bug.name / f"{label}-{trial}"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    tmp = Path(tempfile.mkdtemp(prefix=f"pocket-debug-eval-{bug.name}-"))
    started = time.time()
    try:
        proj = build_project(tmp, bug)
        planted = scripts_of(proj)
        clean = scripts_of(build_project(tmp / "clean", None))
        (out / "planted.diff").write_text(diff(clean, planted, "clean", "planted"))
        lines = planted_lines(bug, (proj / bug.file).read_text())
        workdir = tmp / "agent"
        workdir.mkdir()
        bindir = tmp / "bin"
        bindir.mkdir()
        os.symlink(os.path.abspath(a.pocket), bindir / "pocket")
        prompt = PROMPT.format(symptom=bug.symptom, max_calls=a.max_calls, max_minutes=a.max_time // 60,
                               debugger=DEBUGGER_RULE if a.variant == "debugger" else "",
                               mcp=MCP_RULE if a.via == "mcp" else "")
        (out / "prompt.txt").write_text(prompt)
        with Host(a.pocket, proj, out / "serve.err") as host:
            opencode_config(workdir, a.via, host.url)
            env = {**os.environ, "POCKET_HOST": host.url, "PATH": f"{bindir}:{os.environ['PATH']}"}
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
        check_proj = build_project(tmp / "check", None)
        shutil.rmtree(check_proj / "scripts")
        shutil.copytree(proj / "scripts", check_proj / "scripts")
        checks = run_checks(a.pocket, check_proj, out / "check-serve.err")
        hashes, herr = scenario_hashes(a.pocket, check_proj, tmp / "scenario.jsonl")
        identical = hashes is not None and hashes == reference_hashes
        diverged = None
        if hashes and not identical:
            diverged = next((i for i, (x, y) in enumerate(zip(hashes, reference_hashes)) if x != y), None)
        own = checks[bug.check][0]
        success = all(ok for ok, _ in checks.values())
        ans = stats.pop("answer")
        located = bool(ans) and ans.get("file", "").lstrip("./") in (bug.file, bug.file.split("/")[-1]) \
            and isinstance(ans.get("line"), int) and lines[0] - 2 <= ans["line"] <= lines[1] + 2
        result = {
            "bug": bug.name, "kind": bug.kind, "trial": trial, "model": a.model, "via": a.via,
            "variant": a.variant,
            "success": success, "bug_check": own, "checks": checks,
            "hash_chain_identical_to_clean": identical, "hash_first_divergence_tick": diverged,
            "hash_error": herr or None,
            "answer": ans, "located": located, "planted_at": {"file": bug.file, "lines": list(lines)},
            "stopped_by": stopped, "agent_exit": code,
            "agent_s": agent_s, "wall_s": round(time.time() - started, 1),
            "budget": {"max_calls": a.max_calls, "max_time_s": a.max_time}, **stats,
            "debug_used": uses_debugger(stats["cli"]),
        }
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"  {bug.name} #{trial}: success={success} (own check {own}), located={located}, "
              f"{stats['tool_calls']} tool calls ({stats['pocket_calls']} pocket), {agent_s} s, "
              f"{stats['tokens']['total']} tokens, stopped_by={stopped}", flush=True)
        return result
    finally:
        kill_strays(tmp)
        if not a.keep:
            shutil.rmtree(tmp, ignore_errors=True)


def report():
    """Re-reads every run's transcript (so the counts follow this file's summarize), updates its
    result.json, writes summary.json and prints the results table and the feature tallies."""
    runs = []
    for res in sorted(EVIDENCE.glob("*/*/result.json")):
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
        r["dir"] = str(res.parent.relative_to(EVIDENCE))
        runs.append(r)
    order = {b.name: i for i, b in enumerate(BUGS)}
    runs.sort(key=lambda r: (r.get("variant", "free"), r["via"], order.get(r["bug"], 99), r["trial"]))
    (EVIDENCE / "summary.json").write_text(json.dumps(
        [{k: r.get(k) for k in ("dir", "bug", "variant", "via", "trial", "success", "bug_check", "located",
                                "hash_chain_identical_to_clean", "tool_calls", "pocket_calls", "tool_errors",
                                "agent_s", "wall_s", "tokens", "debug_used", "stopped_by", "cli", "refusals",
                                "output_chars", "largest_output_chars", "first_s", "bash_timeouts", "blocked_s",
                                "eval_unseen_locals", "stale_bundle_after_restore")} for r in runs], indent=2) + "\n")
    print("| Run | Bug | Fixed | Located | Hash = clean | Tool calls | `pocket` calls | Agent s | Tokens (k, cached) | Debugger | Stopped by |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for r in runs:
        t = r["tokens"]
        print(f"| [{r['dir']}]({r['dir']}/transcript.md) | {r['bug']} | {'yes' if r['success'] else 'NO'} | "
              f"{'yes' if r['located'] else 'no'} | {'yes' if r['hash_chain_identical_to_clean'] else 'no'} | "
              f"{r['tool_calls']} | {r['pocket_calls']} | {r['agent_s']:.0f} | {t['total'] / 1000:.0f} ({t['cache_read'] / 1000:.0f}) | "
              f"{'yes' if r['debug_used'] else '-'} | {r['stopped_by'] or '-'} |")
    print()
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
    return 0


def selftest(a):
    """The clean game passes every check; each planted bug fails its own check."""
    ok = True
    tmp = Path(tempfile.mkdtemp(prefix="pocket-debug-eval-selftest-"))
    try:
        for bug in [None, *[BUG[n] for n in a.bugs]]:
            name = bug.name if bug else "clean"
            proj = build_project(tmp / name, bug)
            checks = run_checks(a.pocket, proj, tmp / f"{name}.err")
            hashes, herr = scenario_hashes(a.pocket, proj, tmp / f"{name}.jsonl")
            for c, (passed, detail) in checks.items():
                print(f"  {name:17} {c:12} {'pass' if passed else 'FAIL'}  {detail}")
            if bug is None:
                good = all(p for p, _ in checks.values())
            else:
                good = not checks[bug.check][0]
            others = [c for c, (p, _) in checks.items() if not p and bug and c != bug.check]
            print(f"{'ok  ' if good else 'BAD '} {name}: " + ("every check passes" if bug is None else
                  f"its own check ({bug.check}) fails" + (f"; also fails {others}" if others else "")) +
                  (f"; hash chain {len(hashes)} ticks" if hashes else f"; {herr}"))
            ok &= good
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return ok


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    target = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target")
    ap.add_argument("--pocket", default=str(target / "release" / "pocket"),
                    help="the pocket binary (default $CARGO_TARGET_DIR/release/pocket)")
    ap.add_argument("--bugs", default=",".join(b.name for b in BUGS))
    ap.add_argument("--trials", type=int, default=1)
    ap.add_argument("--first-trial", type=int, default=1)
    ap.add_argument("--model", default=DEFAULT_MODEL)
    ap.add_argument("--via", default="cli", choices=["cli", "mcp"],
                    help="cli: the shell's `pocket` only; mcp: also the host's MCP tools")
    ap.add_argument("--variant", default="free", choices=["free", "debugger"],
                    help="free: the agent picks its tools; debugger: it must confirm the cause with the "
                         "script debugger before changing code")
    ap.add_argument("--max-time", type=int, default=900, help="seconds the agent may take")
    ap.add_argument("--max-calls", type=int, default=60, help="tool calls the agent may make")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--report", action="store_true",
                    help="re-read every recorded run, update its result.json, print the tables")
    ap.add_argument("--keep", action="store_true", help="keep the temporary projects")
    a = ap.parse_args()
    a.bugs = [b.strip() for b in a.bugs.split(",") if b.strip()]
    for n in a.bugs:
        if n not in BUG:
            raise SystemExit(f"no bug '{n}'; bugs: {', '.join(BUG)}")
    if a.report:
        return report()
    if a.list:
        for b in BUGS:
            print(f"{b.name:17} {b.file:17} {b.kind}\n{'':17} symptom: {b.symptom}")
        return 0
    if not Path(a.pocket).exists():
        raise SystemExit(f"{a.pocket} is not built (cargo build --release -p pocket-app, or --pocket PATH)")
    if a.selftest:
        return 0 if selftest(a) else 1
    tmp = Path(tempfile.mkdtemp(prefix="pocket-debug-eval-ref-"))
    try:
        reference, herr = scenario_hashes(a.pocket, build_project(tmp, None), tmp / "scenario.jsonl")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if reference is None:
        raise SystemExit(f"the clean game's hash chain failed: {herr}")
    results = []
    for trial in range(a.first_trial, a.first_trial + a.trials):
        for n in a.bugs:
            results.append(one_run(a, BUG[n], trial, reference))
    print(json.dumps([{k: r[k] for k in ("bug", "trial", "success", "located", "tool_calls", "agent_s")}
                      for r in results]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
