"""A minimal MCP stdio client for a seat-bound player session (docs/spec/player.md 7): starts
`pocket mcp <project> --seat <seat>` (which attaches to a running `pocket serve` of the project, or
runs the game in process), lists its tools, and plays a few calls: session, observe, act, wait, an
unavailable affordance, a developer tool and the omniscient view (both refused), describe.
docs/evidence/player/mcp-session.txt is its output against samples/sailing-course.

Usage: python tools/player_mcp_probe.py <path to pocket(.exe)> <project> <seat>"""
import json
import subprocess
import sys

pocket, project, seat = sys.argv[1:4]
p = subprocess.Popen([pocket, "mcp", project, "--seat", seat], stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
n = 0


def send(method, params=None, notify=False):
    global n
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not notify:
        n += 1
        msg["id"] = n
    p.stdin.write(json.dumps(msg) + "\n")
    p.stdin.flush()
    if notify:
        return None
    while True:
        line = p.stdout.readline()
        if not line:
            raise SystemExit("no answer: " + p.stderr.read())
        r = json.loads(line)
        if r.get("id") == n:
            return r


init = send("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "probe", "version": "0"}})
print("instructions:", init["result"].get("instructions", "")[:120], "...")
send("notifications/initialized", notify=True)
tools = send("tools/list", {})
print("tools:", [t["name"] for t in tools["result"]["tools"]])
for name, args in [("player", {"action": "session"}),
                   ("player", {"action": "observe"}),
                   ("player", {"action": "act", "actions": [{"do": "start", "intent": "sail_to",
                                                             "target": "Mark1"}]}),
                   ("player", {"action": "wait"}),
                   ("player", {"action": "wait"}),
                   ("player", {"action": "observe"}),
                   ("player", {"action": "act", "actions": [{"do": "use", "entity": "Crate1",
                                                             "verb": "take_aboard"}]}),
                   ("world", {"action": "tree"}),
                   ("player", {"action": "observe", "omniscient": True}),
                   ("player", {"action": "describe", "part": "intents", "name": "sail_to"})]:
    r = send("tools/call", {"name": name, "arguments": args})
    res = r["result"]
    text = res["content"][0]["text"]
    print(f"--- {name} {args} error={res.get('isError', False)} ({len(text)} bytes)")
    print(text[:700])
p.stdin.close()
p.wait(timeout=20)
