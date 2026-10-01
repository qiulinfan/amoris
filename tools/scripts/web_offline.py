#!/usr/bin/env python3
"""Check that a packed web game installs and starts without a network (docs/web.md, Installed and offline).

    python3 tools/scripts/web_evidence.py &                       # serves dist/web on :4718
    python3 tools/scripts/web_offline.py walker [--out tests/evidence/web]

Runs a headless Google Chrome with a profile of its own (nothing of the user's is read) over the
DevTools protocol: opens http://localhost:4718/<project>/, waits for the service worker to keep the
pack, reads the manifest, then takes the network away, reloads, and asks the game for its state and
a capture. Writes offline-<project>.json and offline-<project>.png. Needs `pip install websocket-client`.
"""
import argparse
import base64
import json
import os
import shutil
import subprocess
import tempfile
import time
import urllib.request

import websocket

CHROME = os.environ.get("POCKET_CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("project")
    ap.add_argument("--port", type=int, default=4718)
    ap.add_argument("--out", default="tests/evidence/web")
    a = ap.parse_args()
    url = f"http://localhost:{a.port}/{a.project}/"
    profile = tempfile.mkdtemp(prefix="pocket-chrome-")
    debug = 9333
    proc = subprocess.Popen([CHROME, "--headless=new", f"--user-data-dir={profile}", f"--remote-debugging-port={debug}",
                             f"--remote-allow-origins=http://127.0.0.1:{debug}", "--no-first-run", "--enable-unsafe-webgpu",
                             "--window-size=960,540", "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        tabs = []
        for _ in range(100):
            try:
                tabs = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{debug}/json").read())
                break
            except Exception:
                time.sleep(0.1)
        page = next(t for t in tabs if t["type"] == "page")
        ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=120, suppress_origin=True)
        counter = [0]

        def call(method, params=None):
            counter[0] += 1
            ws.send(json.dumps({"id": counter[0], "method": method, "params": params or {}}))
            while True:
                m = json.loads(ws.recv())
                if m.get("id") == counter[0]:
                    return m

        def evaluate(expr):
            r = call("Runtime.evaluate", {"expression": expr, "awaitPromise": True, "returnByValue": True})
            result = r.get("result", {})
            if "exceptionDetails" in result:
                raise RuntimeError(json.dumps(result["exceptionDetails"])[:400])
            return result.get("result", {}).get("value")

        call("Network.enable")
        call("Page.enable")
        call("Page.navigate", {"url": url})
        time.sleep(2)
        online = evaluate("""(async () => {
            const reg = await navigator.serviceWorker.ready;
            for (let i = 0; i < 100 && !(await caches.keys()).length; i++) await new Promise(r => setTimeout(r, 100));
            await new Promise(r => setTimeout(r, 1000));
            const keys = await caches.keys();
            const kept = keys.length ? (await (await caches.open(keys[0])).keys()).map(r => r.url.slice(location.href.length) || "./") : [];
            const manifest = await (await fetch(document.querySelector('link[rel=manifest]').href)).json();
            return { scope: reg.scope, caches: keys, kept, manifest };
        })()""")
        call("Network.emulateNetworkConditions", {"offline": True, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1})
        call("Page.reload", {"ignoreCache": False})
        time.sleep(2)
        offline = evaluate("""(async () => {
            await Promise.race([window.pocket.ready, new Promise((_, no) => setTimeout(() => no(new Error("the game did not start in 20 s")), 20000))]);
            window.pocket.command("step", { ticks: 30 });
            await window.pocket.commandAsync("capture", { path: "/offline.png" });
            const png = window.pocket.read("/offline.png");
            let bin = ""; for (let i = 0; i < png.length; i += 0x8000) bin += String.fromCharCode.apply(null, png.subarray(i, i + 0x8000));
            return {
                controlled: !!navigator.serviceWorker.controller,
                online: navigator.onLine,
                state: window.pocket.command("state", {}).state,
                from_cache: performance.getEntriesByType("resource").filter(e => e.transferSize === 0).map(e => e.name.slice(location.href.length)),
                png: btoa(bin),
            };
        })()""")
        png = base64.b64decode(offline.pop("png"))
        os.makedirs(a.out, exist_ok=True)
        with open(os.path.join(a.out, f"offline-{a.project}.png"), "wb") as f:
            f.write(png)
        report = {"url": url, "chrome": json.loads(urllib.request.urlopen(f"http://127.0.0.1:{debug}/json/version").read())["Browser"], "online": online, "offline": offline}
        with open(os.path.join(a.out, f"offline-{a.project}.json"), "w") as f:
            json.dump(report, f, indent=2)
        print(json.dumps({"kept": len(online["kept"]), "controlled": offline["controlled"], "online": offline["online"], "state_keys": len(offline["state"]), "png_bytes": len(png)}))
        return 0 if offline["controlled"] and offline["state"] else 1
    finally:
        proc.terminate()
        proc.wait()
        shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
