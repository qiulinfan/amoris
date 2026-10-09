"""Replays the player layer's review findings against `pocket serve` over HTTP (docs/spec/player.md
7 and 10): a seat's developer calls (refused with permission.denied, the world unchanged), a
restore under the players' session (the next wait stops at the restored timeline's first decision
and carries its events), and a rule that throws at tick 3 (the seat's wait halts; a developer's
resume, a restore or a hot update ends the halt). Copies samples/sailing-course into <scratch>
twice (the second with the planted throw), serves each, and prints one line per call.
docs/evidence/player/seat-http.txt is its output.

Usage: python tools/player_http_probe.py <path to pocket(.exe)> <scratch dir> [port]"""
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

WINDOWS = os.name == "nt"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PLANTED = 'if (ctx.tick === 3) throw new Error("planted at tick 3");'
AT = "    run(ctx, { boats, courses, marks }) {\n"


def copy(scratch, name, planted):
    to = os.path.join(scratch, name)
    shutil.rmtree(to, ignore_errors=True)
    shutil.copytree(os.path.join(ROOT, "samples", "sailing-course"), to,
                    ignore=shutil.ignore_patterns(".pocket"))
    if planted:
        rules = os.path.join(to, "scripts", "rules.ts")
        text = open(rules, encoding="utf-8").read()
        open(rules, "w", encoding="utf-8", newline="").write(
            text.replace(AT, AT + "        " + PLANTED + "\n", 1))
    return to


def call(url, method, params, seat=None):
    body = {"id": 1, "method": method, "params": params}
    if seat is not None:
        body["seat"] = seat
    req = urllib.request.Request(url + "/api/call", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=120).read())


def serve(pocket, project, port):
    flags = subprocess.CREATE_NEW_PROCESS_GROUP if WINDOWS else 0
    p = subprocess.Popen([pocket, "serve", project, "--port", str(port)], stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, creationflags=flags)
    url = f"http://127.0.0.1:{port}"
    for _ in range(600):
        try:
            call(url, "status", {})
            return p, url
        except OSError:
            time.sleep(0.1)
    p.kill()
    raise SystemExit("pocket serve did not answer")


def stop(p):
    p.send_signal(signal.CTRL_BREAK_EVENT if WINDOWS else signal.SIGTERM)
    try:
        p.wait(timeout=20)
    except subprocess.TimeoutExpired:
        p.kill()


def show(who, method, params, answer, keys=()):
    if "error" in answer:
        e = answer["error"]
        out = f"error {e['code']}"
        if e["code"] == "permission.denied":
            out += f" (role {e['detail'].get('role')}, needs {e['detail'].get('needs')})"
    else:
        r = answer["result"]
        picked = {k: r.get(k) for k in keys} if isinstance(r, dict) else {"text": str(r)[:60]}
        out = "ok " + json.dumps(picked, separators=(",", ":"))
    print(f"{who:9} {method} {json.dumps(params, separators=(',', ':'))} -> {out}")


def kinds(answer):
    return [f"{e.get('kind')}@{e.get('tick')}" for e in answer.get("result", {}).get("events", [])]


def main():
    pocket, scratch = sys.argv[1], sys.argv[2]
    port = int(sys.argv[3]) if len(sys.argv) > 3 else 8741
    os.makedirs(scratch, exist_ok=True)

    print("== 1. A seat's developer calls (finding 1)")
    p, url = serve(pocket, copy(scratch, "course", False), port)
    try:
        seat = "skipper"
        crate = {"entity": "Crate4", "components": ["Cargo"]}
        show("developer", "world.get", crate, call(url, "world.get", crate), ["components"])
        for m, params in [
            ("world.query", {"with": ["Perceivable"]}),
            ("world.get", {"entity": "Crate4"}),
            ("world.edit", {"ops": [{"set": {"entity": "Crate4", "component": "Cargo",
                                             "value": {"value": 99}}}]}),
            ("time.step", {"ticks": 5}),
            ("time.control", {"pause": False}),
            ("events.since", {}),
            ("debug.rewind", {"tick": 0}),
        ]:
            show("seat", m, params, call(url, m, params, seat))
        show("developer", "world.get", crate, call(url, "world.get", crate), ["components"])
        show("developer", "status", {}, call(url, "status", {}), ["tick"])
        obs = {"projection": "json"}
        show("seat", "player.observe", obs, call(url, "player.observe", obs, seat),
             ["tick", "omniscient"])
        show("seat", "player.observe", {"omniscient": True},
             call(url, "player.observe", {"omniscient": True}, seat))
        show("seat", "player.describe", {"entity": "Crate4"},
             call(url, "player.describe", {"entity": "Crate4"}, seat))

        print("== 2. A restore under the players' session (finding 3)")
        act = {"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                            "params": {"arrive_m": 8}}]}
        show("seat", "player.act", act, call(url, "player.act", act, seat), ["outcomes"])
        far = {"until": {"event": "mark.rounded"}}
        show("seat", "player.wait", far, call(url, "player.wait", far, seat),
             ["tick", "stopped", "cursor"])
        show("seat", "player.session", {}, call(url, "player.session", {}, seat),
             ["tick", "cursor"])
        show("developer", "snapshots.restore", {"tick": 0},
             call(url, "snapshots.restore", {"tick": 0}), ["restored"])
        w = call(url, "player.wait", {"ticks": 120}, seat)
        show("seat", "player.wait", {"ticks": 120}, w, ["from_tick", "tick", "stopped", "cursor"])
        print(f"          events: {kinds(w)}")
    finally:
        stop(p)

    print("== 3. A rule that throws at tick 3 (finding 2)")
    project = copy(scratch, "course-throws", True)
    p, url = serve(pocket, project, port + 1)
    try:
        wait = {"ticks": 10, "until": {"event": "crate.taken"}}
        keys = ["from_tick", "tick", "stopped", "halted"]
        show("seat", "player.wait", wait, call(url, "player.wait", wait, seat), keys)
        s = call(url, "player.session", {}, seat)
        print(f"seat      player.session -> halted {s['result']['status']['halted']}")
        show("seat", "player.wait", wait, call(url, "player.wait", wait, seat), keys)
        dev_wait = {"seat": seat, "ticks": 1}
        d = call(url, "player.wait", dev_wait)
        print(f"developer player.wait {json.dumps(dev_wait)} -> halted "
              f"{d['result']['halted']['code']} cause {d['result']['halted']['detail']['cause']['code']}")
        for pause in (False, True):
            show("developer", "time.control", {"pause": pause},
                 call(url, "time.control", {"pause": pause}), ["paused"])
        show("seat", "player.wait", wait, call(url, "player.wait", wait, seat), keys)
        show("developer", "snapshots.restore", {"tick": 0},
             call(url, "snapshots.restore", {"tick": 0}), ["restored"])
        show("seat", "player.wait", wait, call(url, "player.wait", wait, seat), keys)
        rules = os.path.join(project, "scripts", "rules.ts")
        text = open(rules, encoding="utf-8").read()
        open(rules, "w", encoding="utf-8", newline="").write(text.replace(PLANTED, "", 1))
        show("developer", "scripts.apply", {}, call(url, "scripts.apply", {}), ["outcome"])
        show("seat", "player.wait", wait, call(url, "player.wait", wait, seat), keys)
    finally:
        stop(p)


if __name__ == "__main__":
    main()
