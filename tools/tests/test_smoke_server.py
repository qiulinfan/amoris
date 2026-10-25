"""The smoke test's WebSocket client against a scripted local server: no host is started.

    python -m unittest tools/tests/test_smoke_server.py
"""
import importlib.util
import json
import socket
import struct
import threading
import time
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("smoke_server", TOOLS / "host/smoke_server.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


def frame(obj):
    """A server's text frame (unmasked)."""
    data = json.dumps(obj).encode()
    n = len(data)
    if n < 126:
        return bytes([0x81, n]) + data
    return bytes([0x81, 126]) + struct.pack(">H", n) + data


def read_request(conn):
    """One masked client text frame, parsed."""
    def exact(n):
        out = b""
        while len(out) < n:
            out += conn.recv(n - len(out))
        return out
    _, b1 = exact(2)
    n = b1 & 0x7F
    if n == 126:
        n = struct.unpack(">H", exact(2))[0]
    elif n == 127:
        n = struct.unpack(">Q", exact(8))[0]
    mask = exact(4)
    return json.loads(bytes(b ^ mask[i % 4] for i, b in enumerate(exact(n))))


class Server:
    """Accepts one WebSocket and runs `script(conn)` on it."""

    def __init__(self, script):
        self.sock = socket.create_server(("127.0.0.1", 0))
        self.port = self.sock.getsockname()[1]
        self.thread = threading.Thread(target=self.serve, args=(script,), daemon=True)
        self.thread.start()

    def serve(self, script):
        conn, _ = self.sock.accept()
        with conn:
            head = b""
            while b"\r\n\r\n" not in head:
                head += conn.recv(1)
            conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                         b"Connection: Upgrade\r\n\r\n")
            script(conn)
            time.sleep(0.5)

    def close(self):
        self.thread.join(5)
        self.sock.close()


class WsTest(unittest.TestCase):
    def test_pushes_before_an_answer_wait_for_until(self):
        # The host pushes Play's reset (and a profile) before it answers play.start.
        def script(conn):
            req = read_request(conn)
            conn.sendall(frame({"event": "profile", "data": {"ticks": 0}})
                         + frame({"event": "world.changed", "data": {"reset": True}})
                         + frame({"id": req["id"], "result": {"mode": "play"}}))
        server = Server(script)
        ws = smoke.WS(server.port)
        self.addCleanup(ws.s.close)
        answer = ws.request(5, "play.start", {"speed": 2})
        self.assertEqual(answer, {"id": 5, "result": {"mode": "play"}})
        reset, seen = ws.until(lambda f: f.get("event") == "world.changed" and f["data"].get("reset"), 2)
        self.assertIsNotNone(reset)
        self.assertEqual([f.get("event") for f in seen], ["profile", "world.changed"])
        self.assertEqual(ws.until(lambda f: True, 0.2), (None, []))
        server.close()

    def test_a_frame_split_across_a_timeout_is_kept_whole(self):
        data = frame({"event": "history", "data": {"undo": ["edit"]}})
        def script(conn):
            conn.sendall(data[:3])
            time.sleep(0.4)
            conn.sendall(data[3:])
        server = Server(script)
        ws = smoke.WS(server.port)
        self.addCleanup(ws.s.close)
        self.assertEqual(ws.until(lambda f: True, 0.1), (None, []))
        h, _ = ws.until(lambda f: f.get("event") == "history", 3)
        self.assertEqual(h, {"event": "history", "data": {"undo": ["edit"]}})
        server.close()

    def test_an_unanswered_request_keeps_its_pushes(self):
        def script(conn):
            read_request(conn)
            conn.sendall(frame({"event": "events", "data": []}))
        server = Server(script)
        ws = smoke.WS(server.port)
        self.addCleanup(ws.s.close)
        self.assertIsNone(ws.request(7, "time.step", timeout=0.3))
        ev, _ = ws.until(lambda f: f.get("event") == "events", 0.1)
        self.assertIsNotNone(ev)
        server.close()


if __name__ == "__main__":
    unittest.main()
