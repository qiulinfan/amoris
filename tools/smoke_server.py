#!/usr/bin/env python3
"""Smoke test of the host (docs/spec/server.md): starts `pocket serve` on a copy of
samples/sailing and drives it the three ways clients do.

1. The CLI (`pocket status`, `pocket world ...`, `pocket step`, `pocket undo`, `pocket play`, a
   refused typo with its suggestion).
2. `POST /api/call` and `GET /api/catalog`, and the editor's `/ws` (requests, the history, events,
   world.changed and status pushes, Play and Stop).
3. MCP over Streamable HTTP (`initialize`, `tools/list` with its size, a `world` call, a refused
   call).

Standard library only. Usage: python3 tools/smoke_server.py [--pocket target/debug/pocket]
"""

import argparse
import base64
import json
import os
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FAILURES = []


def check(cond, what):
    print(("ok   " if cond else "FAIL ") + what)
    if not cond:
        FAILURES.append(what)


# --- HTTP -----------------------------------------------------------------------------------------

def post(url, body, headers=None):
    req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST")
    req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.status, dict(r.headers), r.read().decode()


def call(base, method, params=None):
    _, _, text = post(base + "/api/call", {"id": 1, "method": method, "params": params or {}})
    return json.loads(text)


# --- A minimal WebSocket client (RFC 6455, text frames) ------------------------------------------

class WS:
    def __init__(self, port, path="/ws"):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=10)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n"
                        f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
                        "Sec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.s.recv(1)
        assert b" 101 " in head.split(b"\r\n")[0], head
        self.buf = b""

    def send(self, obj):
        data = json.dumps(obj).encode()
        mask = os.urandom(4)
        n = len(data)
        hdr = bytes([0x81])
        if n < 126:
            hdr += bytes([0x80 | n])
        elif n < 65536:
            hdr += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            hdr += bytes([0x80 | 127]) + struct.pack(">Q", n)
        self.s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _read(self, n):
        while len(self.buf) < n:
            chunk = self.s.recv(65536)
            if not chunk:
                raise EOFError
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv(self, timeout=10):
        self.s.settimeout(timeout)
        b0, b1 = self._read(2)
        n = b1 & 0x7F
        if n == 126:
            n = struct.unpack(">H", self._read(2))[0]
        elif n == 127:
            n = struct.unpack(">Q", self._read(8))[0]
        payload = self._read(n)
        if b0 & 0x0F == 0x8:
            raise EOFError
        return json.loads(payload)

    def until(self, pred, timeout=10):
        """Frames until one matches; returns (match, every frame seen)."""
        seen, end = [], time.time() + timeout
        while time.time() < end:
            f = self.recv(max(0.1, end - time.time()))
            seen.append(f)
            if pred(f):
                return f, seen
        return None, seen

    def request(self, rid, method, params=None, timeout=10):
        self.send({"id": rid, "method": method, "params": params or {}})
        f, seen = self.until(lambda f: f.get("id") == rid, timeout)
        return f, seen


# --- MCP over Streamable HTTP --------------------------------------------------------------------

def sse_json(text):
    for line in text.splitlines():
        if line.startswith("data:") and line[5:].strip():
            return json.loads(line[5:])
    return json.loads(text)


class MCP:
    def __init__(self, base):
        self.url = base + "/mcp"
        self.session = None
        self.n = 0

    def rpc(self, method, params=None, notify=False):
        body = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            body["params"] = params
        if not notify:
            self.n += 1
            body["id"] = self.n
        headers = {"accept": "application/json, text/event-stream",
                   "mcp-protocol-version": "2025-06-18"}
        if self.session:
            headers["mcp-session-id"] = self.session
        status, hdrs, text = post(self.url, body, headers)
        sid = {k.lower(): v for k, v in hdrs.items()}.get("mcp-session-id")
        if sid:
            self.session = sid
        return None if notify else sse_json(text)


# --- The run -------------------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pocket", default=str(REPO / "target/debug/pocket"))
    ap.add_argument("--keep", action="store_true", help="keep the project copy")
    a = ap.parse_args()
    pocket = a.pocket
    if not Path(pocket).exists():
        sys.exit(f"{pocket} is not built (cargo build -p pocket-app, or --pocket PATH)")
    tmp = Path(tempfile.mkdtemp(prefix="pocket-smoke-"))
    proj = tmp / "sailing"
    shutil.copytree(REPO / "samples/sailing", proj)
    errlog = open(tmp / "serve.err", "w")
    server = subprocess.Popen([pocket, "serve", str(proj), "--port", "0"],
                              stdout=subprocess.PIPE, stderr=errlog, text=True)
    try:
        line = server.stdout.readline()
        info = json.loads(line)
        base, port = info["url"], info["port"]
        print(f"host at {base}")

        def cli(*args, ok=True):
            r = subprocess.run([pocket, *args], cwd=proj, capture_output=True, text=True,
                               timeout=60)
            if ok and r.returncode != 0:
                print(r.stdout, r.stderr)
            return r

        # 1. The CLI first.
        r = cli("status")
        check(r.returncode == 0 and "edit paused" in r.stdout, f"cli status: {r.stdout.strip()}")
        n = len(json.loads((proj / "scene.json").read_text())["entities"])
        r = cli("world", "tree")
        check(r.stdout.count("\n") == n and "#3 Sloop" in r.stdout, f"cli world tree: {n} entities")
        r = cli("world", "set", "Crate1", "Cargo", "value=5")
        check(r.returncode == 0 and "set Cargo on Crate1" in r.stdout, f"cli world set: {r.stdout.strip()}")
        r = cli("world", "get", "Crate1", "Cargo", "--json")
        check(json.loads(r.stdout)["components"]["Cargo"]["value"] == 5, "cli edit applied")
        r = cli("undo")
        check(r.returncode == 0 and r.stdout.startswith("undone"), f"cli undo: {r.stdout.strip()}")
        r = cli("world", "get", "Crate1", "Cargo", "--json")
        check(json.loads(r.stdout)["components"]["Cargo"]["value"] == 1, "cli undo restored the value")
        r = cli("step", "120", "--until", "event:sail.set")
        check("stopped by" in r.stdout and "sail.set" in r.stdout, f"cli step --until: {r.stdout.strip()}")
        r = cli("step", "600", "--until", "Sloop.Boat.speed>1")
        check("stopped: Boat.speed" in r.stdout, f"cli step until a field: {r.stdout.strip()}")
        r = cli("world", "set", "Sloop", "Boat", "ruder=1", ok=False)
        check(r.returncode == 1 and "request.unknown_field" in r.stderr and "rudder" in r.stderr,
              f"cli refusal: {r.stderr.strip()}")
        r = cli("play", "start", "--speed", "4")
        check("play running" in r.stdout, f"cli play start: {r.stdout.strip()}")
        time.sleep(0.5)
        r = cli("play", "stop")
        check("edit paused" in r.stdout, f"cli play stop: {r.stdout.strip()}")
        r = cli("help", "step")
        check("until" in r.stdout and "watch" in r.stdout, "cli help from the catalog")

        # 2. /api/call and /api/catalog.
        cat = json.loads(urllib.request.urlopen(base + "/api/catalog", timeout=10).read())
        names = {c["name"] for c in cat}
        check({"world.edit", "time.step", "play.start", "events.since"} <= names,
              f"catalog: {len(cat)} commands with schemas")
        s = call(base, "status")["result"]
        check(s["mode"] == "edit" and s["paused"] is True, "api status")
        e = call(base, "world.edit", {"ops": [{"destroy": {"entity": "Crate2"}}]})
        check("result" in e, "api world.edit destroy")
        u = call(base, "history.undo")
        got = call(base, "world.get", {"entity": "Crate2"})
        check("result" in u and got["result"]["id"] == 5, "api undo revives Crate2 under its id")
        bad = call(base, "wrld.tree")
        check(bad["error"]["code"] == "request.unknown_method", "api unknown method refused")

        # 3. The editor's WebSocket.
        ws = WS(port)
        first = ws.recv()
        check(first.get("event") == "status", "ws: status pushed on connect")
        f, _ = ws.request(1, "subscribe", {"topics": ["world.changed", "events", "history", "log"]})
        check(f and "result" in f, "ws subscribe")
        f, seen = ws.request(2, "world.edit", {"ops": [{"set": {"entity": "Crate3", "component": "Cargo", "value": {"value": 9}}}]})
        check(f and "result" in f, "ws world.edit")
        h, seen2 = ws.until(lambda f: f.get("event") == "history", 5)
        check(h is not None and h["data"]["undo"], "ws: history pushed after the edit")
        # Pushes coalesce over 100 ms: a reset (the CLI's Play and Stop just before) covers the
        # edit too.
        c, _ = ws.until(lambda f: f.get("event") == "world.changed", 5)
        check(c is not None and (c["data"].get("reset") or [6, "Cargo"] in c["data"]["changed"]),
              f"ws: world.changed pushed {c and c['data']}")
        f, _ = ws.request(3, "history.undo")
        check(f and "result" in f, "ws history.undo")
        f, seen = ws.request(4, "time.step", {"ticks": 30}, timeout=20)
        check(f and f["result"]["tick"] > 0, "ws time.step")
        f, _ = ws.request(5, "play.start", {"speed": 2})
        check(f and f["result"]["mode"] == "play", "ws play.start")
        r, _ = ws.until(lambda f: f.get("event") == "world.changed" and f["data"].get("reset"), 5)
        check(r is not None, "ws: world.changed reset on Play")
        ev, _ = ws.until(lambda f: f.get("event") == "events", 2)
        print(f"info ws: events pushed during Play: {'yes' if ev else 'none in 2 s'}")
        f, _ = ws.request(6, "play.stop")
        check(f and f["result"]["mode"] == "edit", "ws play.stop")
        since = call(base, "events.since", {"limit": 3})["result"]
        check(len(since["events"]) >= 1, f"api events.since: last {since['last']}")
        if since["events"]:
            why = call(base, "events.why", {"seq": since["events"][-1]["seq"]})
            check("result" in why, "api events.why")

        # 4. MCP over HTTP.
        m = MCP(base)
        init = m.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "smoke", "version": "1"}})
        check(init["result"]["serverInfo"]["name"] == "pocket", "mcp initialize")
        m.rpc("notifications/initialized", notify=True)
        tl = m.rpc("tools/list", {})
        tools = tl["result"]["tools"]
        size = len(json.dumps(tools))
        check(len(tools) == 10, f"mcp tools/list: {len(tools)} tools, {size} bytes "
                                 f"(~{size // 4} tokens)")
        r = m.rpc("tools/call", {"name": "world", "arguments": {"action": "tree", "filter": "Sloop"}})
        text = r["result"]["content"][0]["text"]
        check(not r["result"].get("isError") and "Sloop" in text, f"mcp world tree: {text[:80]}")
        r = m.rpc("tools/call", {"name": "world", "arguments": {"action": "get", "entity": "Slop"}})
        check(r["result"].get("isError") and "sim.entity_not_found" in r["result"]["content"][0]["text"],
              "mcp refusal carries the problem")
        r = m.rpc("tools/call", {"name": "time", "arguments": {"action": "step", "ticks": 10}})
        check(not r["result"].get("isError"), "mcp time step")
        r = m.rpc("tools/call", {"name": "debug", "arguments": {"action": "state"}})
        text = r["result"]["content"][0]["text"]
        check(not r["result"].get("isError") and "breakpoints" in text, f"mcp debug state: {text[:80]}")
    finally:
        server.send_signal(signal.SIGTERM)
        try:
            server.wait(10)
        except subprocess.TimeoutExpired:
            server.kill()
        check(not (proj / ".pocket/host.json").exists(), "host.json removed on shutdown")
        errlog.close()
        if a.keep:
            print(f"kept {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)
    print(f"\n{len(FAILURES)} failures")
    sys.exit(1 if FAILURES else 0)


if __name__ == "__main__":
    main()
