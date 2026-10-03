#!/usr/bin/env python3
"""Drive a headless runtime over its control server from Python (AGENTS.md, Development helpers).

    from runtime import Runtime
    with Runtime("playground", port=4770, size="960x540") as r:
        r.rpc("step", {"ticks": 30})
        r.rpc("capture", {"path": "/tmp/shot.png"})

The runtime is the debug build's (`--release` for build/release), serving on 127.0.0.1:<port>,
paused, with the sample's bundle (bundle it first with `./.pocket/pocket ts samples/<name>`).
As a script: `python3 tools/scripts/dev/runtime.py <sample> <method> ['<json params>']...` runs
each call in order on one runtime and prints the answers.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))
ENV = dict(os.environ, DEVELOPER_DIR=os.environ.get("DEVELOPER_DIR", "/Library/Developer/CommandLineTools"), POCKET_ROOT=ROOT)


class Runtime:
    def __init__(self, sample, port=4770, size="960x540", release=False, editor=False, extra=(), exe=None):
        config = "release" if release else "debug"
        project = sample if os.path.isabs(sample) else os.path.join(ROOT, "samples", sample)
        name = os.path.basename(project.rstrip("/"))
        args = [exe or os.path.join(ROOT, "build", config, "bin", "pocket_runtime" + (".exe" if os.name == "nt" else "")), "--project", project,
                "--bundle", os.path.join(ROOT, "build", "ts", name + ".js"), "--headless", "--serve", str(port),
                "--paused", "--json", "--size", size, "--frames", "1000000", "--log-level", "warn", *extra]
        if editor:
            args += ["--editor", os.path.join(ROOT, "build", "ts", "editor.js")]
        self.port = port
        self.proc = subprocess.Popen(args, env=ENV, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(150):
            try:
                self.rpc("state")
                return
            except Exception:
                if self.proc.poll() is not None:
                    raise RuntimeError(f"the runtime exited ({self.proc.returncode}) before it served")
                time.sleep(0.2)
        raise RuntimeError(f"no answer on port {port}")

    def rpc(self, method, params=None):
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}/rpc", data=body, headers={"Content-Type": "application/json"})
        answer = json.loads(urllib.request.urlopen(req, timeout=300).read())
        if "error" in answer:
            raise RuntimeError(f"{method}: {answer['error']}")
        return answer["result"]

    def close(self):
        try:
            self.rpc("quit")
        except Exception:
            pass
        try:
            self.proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    calls = sys.argv[2:]
    with Runtime(sys.argv[1]) as r:
        i = 0
        while i < len(calls):
            method = calls[i]
            params = {}
            if i + 1 < len(calls) and calls[i + 1].startswith("{"):
                params = json.loads(calls[i + 1])
                i += 1
            print(json.dumps(r.rpc(method, params), ensure_ascii=False))
            i += 1
