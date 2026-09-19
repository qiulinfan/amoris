#!/usr/bin/env python3
"""Serve the packed web builds and collect evidence from the page.

    python3 tools/scripts/web_evidence.py [--port 4718] [--dist dist/web] [--out tests/evidence/web]

GET  /<project>/            a build made by `pocket pack <project> --web`, with caching disabled
                            so a rebuild is what the browser loads next.
POST /evidence/<file>       writes the request body to <out>/<file>; the page sends what the
                            runtime captured (`pocket.command("capture", {path})` then
                            `Module.FS.readFile(path)`) or a JSON report.
"""
import argparse
import http.server
import os
import re
import sys

NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=4718)
    ap.add_argument("--dist", default="dist/web")
    ap.add_argument("--out", default="tests/evidence/web")
    a = ap.parse_args()
    dist = os.path.abspath(a.dist)
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kw):
            super().__init__(*args, directory=dist, **kw)

        def end_headers(self):
            self.send_header("Cache-Control", "no-store")
            super().end_headers()

        def do_POST(self):
            m = re.match(r"^/evidence/([^/?]+)$", self.path)
            if not m or not NAME.match(m.group(1)):
                self.send_error(404, "unknown path")
                return
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > 64 * 1024 * 1024:
                self.send_error(400, "bad length")
                return
            body = self.rfile.read(length)
            path = os.path.join(out, m.group(1))
            with open(path, "wb") as f:
                f.write(body)
            reply = ('{"ok":true,"path":%s,"bytes":%d}' % (repr(path).replace("'", '"'), len(body))).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(reply)))
            self.end_headers()
            self.wfile.write(reply)

        def log_message(self, fmt, *args):
            sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))

    with http.server.ThreadingHTTPServer(("127.0.0.1", a.port), Handler) as srv:
        print("serving %s on http://127.0.0.1:%d/ (evidence -> %s)" % (dist, a.port, out), flush=True)
        srv.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
