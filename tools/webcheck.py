#!/usr/bin/env python3
"""Load a page in a headless Chrome (WebGPU on) and report what it says, with no window or prompt
(AGENTS.md, Working on this machine). The page reports by setting `document.title` to
`DONE <json>` or `FAIL <json>`; this prints that JSON, the console and any exceptions, and exits 0
for DONE, 1 for FAIL or a timeout.

    python3 tools/webcheck.py http://127.0.0.1:8700/index.html [--timeout 60] [--serve DIR --port N]

`--serve DIR` serves DIR on 127.0.0.1 for the duration (with the wasm MIME type and the headers
that cross-origin isolation needs). Chrome runs with a throwaway profile, so the owner's profile is
never touched. Needs websocket-client (pip install websocket-client).
"""
import argparse
import functools
import http.server
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

import websocket

CHROMES = [
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "google-chrome",
    "chromium",
]


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {**http.server.SimpleHTTPRequestHandler.extensions_map, ".wasm": "application/wasm", ".js": "text/javascript", ".mjs": "text/javascript"}

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *args):
        pass


def serve(directory, port):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), functools.partial(Handler, directory=directory))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def find_chrome():
    for c in CHROMES:
        if os.path.isfile(c) or shutil.which(c):
            return c if os.path.isfile(c) else shutil.which(c)
    sys.exit("no Chrome or Edge found")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("--timeout", type=float, default=60)
    ap.add_argument("--serve")
    ap.add_argument("--port", type=int, default=0)
    a = ap.parse_args()
    server = serve(os.path.abspath(a.serve), a.port) if a.serve else None
    profile = tempfile.mkdtemp(prefix="webcheck-")
    debug_port = free_port()
    chrome = subprocess.Popen([find_chrome(), "--headless=new", f"--remote-debugging-port={debug_port}", "--remote-debugging-address=127.0.0.1",
                               f"--user-data-dir={profile}", "--no-first-run", "--no-default-browser-check", "--enable-unsafe-webgpu",
                               "--enable-features=Vulkan,WebGPU", "--ignore-gpu-blocklist", "about:blank"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    logs, exceptions, verdict = [], [], None
    try:
        deadline = time.time() + 20
        while True:
            try:
                targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{debug_port}/json", timeout=2).read())
                page = next(t for t in targets if t["type"] == "page")
                break
            except Exception:
                if time.time() > deadline:
                    sys.exit("Chrome did not start its debugging endpoint")
                time.sleep(0.2)
        ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=5, suppress_origin=True)
        seq = [0]

        def send(method, params=None):
            seq[0] += 1
            ws.send(json.dumps({"id": seq[0], "method": method, "params": params or {}}))
            return seq[0]

        for m in ("Runtime.enable", "Log.enable", "Page.enable"):
            send(m)
        send("Page.navigate", {"url": a.url})
        end = time.time() + a.timeout
        poll = 0.0
        while time.time() < end and verdict is None:
            try:
                msg = json.loads(ws.recv())
            except websocket.WebSocketTimeoutException:
                msg = {}
            method = msg.get("method")
            if method == "Runtime.consoleAPICalled":
                p = msg["params"]
                logs.append(f"[{p['type']}] " + " ".join(str(x.get("value", x.get("description", ""))) for x in p["args"]))
            elif method == "Runtime.exceptionThrown":
                d = msg["params"]["exceptionDetails"]
                exceptions.append((d.get("exception") or {}).get("description") or d.get("text"))
            elif method == "Log.entryAdded":
                e = msg["params"]["entry"]
                logs.append(f"[{e['level']}] {e.get('text')}")
            elif msg.get("result", {}).get("result", {}).get("type") == "string":
                title = msg["result"]["result"]["value"]
                if title.startswith("DONE ") or title.startswith("FAIL "):
                    verdict = title
            if time.time() > poll:
                poll = time.time() + 0.5
                send("Runtime.evaluate", {"expression": "document.title", "returnByValue": True})
        ws.close()
    finally:
        chrome.terminate()
        try:
            chrome.wait(10)
        except subprocess.TimeoutExpired:
            chrome.kill()
        shutil.rmtree(profile, ignore_errors=True)
        if server:
            server.shutdown()
    for line in logs:
        print(line)
    for e in exceptions:
        print("[exception]", e)
    if verdict is None:
        print("TIMEOUT: the page did not set its title to DONE or FAIL")
        sys.exit(1)
    print(verdict)
    sys.exit(0 if verdict.startswith("DONE ") else 1)


if __name__ == "__main__":
    main()
