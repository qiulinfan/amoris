#!/usr/bin/env python3
"""Send Python to a Blender GUI that runs tools/blender/gui_bridge.py and print its reply.

    python tools/blender/send.py FILE.py [--port 7811]
    python tools/blender/send.py -c "print(len(bpy.data.objects))"
"""
import argparse
import json
import os
import socket
import sys


def send(code: str, port: int, timeout: float) -> dict:
    with socket.create_connection(('127.0.0.1', port), timeout=timeout) as s:
        s.sendall(json.dumps({'code': code, 'token': os.environ.get('AMORIS_BLENDER_TOKEN', '')}).encode('utf-8'))
        s.shutdown(socket.SHUT_WR)
        data = b''
        while chunk := s.recv(1 << 16):
            data += chunk
    return json.loads(data.decode('utf-8'))


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('file', nargs='?')
    p.add_argument('-c', dest='code')
    p.add_argument('--port', type=int, default=int(os.environ.get('AMORIS_BLENDER_PORT', '7811')))
    p.add_argument('--timeout', type=float, default=1800)
    a = p.parse_args()
    code = a.code if a.code is not None else open(a.file, encoding='utf-8').read()
    r = send(code, a.port, a.timeout)
    sys.stdout.write(r['stdout'])
    if r['error']:
        sys.stderr.write(r['error'])
    return 0 if r['ok'] else 1


if __name__ == '__main__':
    sys.exit(main())
