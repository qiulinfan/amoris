"""Run Python inside a running Blender GUI from another process, without touching its window.

Start Blender with this file and it serves 127.0.0.1:PORT (AMORIS_BLENDER_PORT, default 7811); each
request is Python source run on Blender's main thread through a timer, so the GUI redraws every
change while it happens. `tools/blender/send.py` is the client:

    blender --python tools/blender/gui_bridge.py [file.blend]
    python tools/blender/send.py script.py        # or: -c "print(bpy.data.objects.keys())"

The reply is JSON: {"ok": bool, "stdout": str, "error": traceback or null}. Code may call
`redraw()` between steps to show intermediate states. Requests run one at a time; the server binds
loopback only and refuses a request without the token in AMORIS_BLENDER_TOKEN when that is set.
"""
import contextlib
import io
import json
import os
import queue
import socket
import threading
import traceback

import bpy

PORT = int(os.environ.get('AMORIS_BLENDER_PORT', '7811'))
TOKEN = os.environ.get('AMORIS_BLENDER_TOKEN', '')
jobs: queue.Queue = queue.Queue()


def redraw():
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            area.tag_redraw()
    bpy.ops.wm.redraw_timer(type='DRAW_WIN_SWAP', iterations=1)


def run(source: str) -> dict:
    out = io.StringIO()
    scope = {'bpy': bpy, 'redraw': redraw, '__name__': '__bridge__'}
    try:
        with contextlib.redirect_stdout(out):
            exec(compile(source, '<bridge>', 'exec'), scope)
        return {'ok': True, 'stdout': out.getvalue(), 'error': None}
    except Exception:
        return {'ok': False, 'stdout': out.getvalue(), 'error': traceback.format_exc()}


def pump():
    try:
        source, reply = jobs.get_nowait()
    except queue.Empty:
        return 0.05
    reply.put(run(source))
    return 0.0


def read_all(conn: socket.socket) -> bytes:
    chunks = []
    while True:
        chunk = conn.recv(1 << 16)
        if not chunk:
            return b''.join(chunks)
        chunks.append(chunk)


def serve():
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(('127.0.0.1', PORT))
    server.listen(4)
    while True:
        conn, _ = server.accept()
        with conn:
            try:
                request = json.loads(read_all(conn).decode('utf-8'))
                if TOKEN and request.get('token') != TOKEN:
                    result = {'ok': False, 'stdout': '', 'error': 'bad token'}
                else:
                    reply: queue.Queue = queue.Queue()
                    jobs.put((request['code'], reply))
                    result = reply.get()
            except Exception:
                result = {'ok': False, 'stdout': '', 'error': traceback.format_exc()}
            conn.sendall(json.dumps(result).encode('utf-8'))


def heartbeat():
    # Windows Graphics Capture delivers a frame only when the window presents one; an idle Blender
    # presents nothing, and a window recorder would see a stalled track. Redraw every two seconds.
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            area.tag_redraw()
    return 2.0


threading.Thread(target=serve, daemon=True).start()
bpy.app.timers.register(pump, persistent=True)
bpy.app.timers.register(heartbeat, persistent=True)
print(f'gui_bridge: serving 127.0.0.1:{PORT}')
