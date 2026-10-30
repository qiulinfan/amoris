#!/usr/bin/env python3
"""A test pilot for Wellspring: plays the whole quest through the player seat's own controls.

    python samples/wellspring/checks/pilot.py host [--host http://127.0.0.1:7878]
    python samples/wellspring/checks/pilot.py page --cdp 9333 --page <target id prefix>

It reads the keeper's position and the quest from the world (a developer read), and acts only as
the seat `player` does: it sets forward, strafe, run, rise, yaw and pitch and pulses primary. It
walks a route of waypoints between the quest's places, faces each handle, wheel and the bell and
clicks, sidesteps when it stops making progress, and swims where the way crosses water. `host`
drives a running `amoris serve` in Play; `page` drives an exported game in a browser over CDP
(the page's `window.amorisPlayer`), without bringing its window forward. It prints each step and
ends when the bell is rung or a step fails. `--smooth` turns the look at most 2.5 radians a second,
in steps 30 times a second, instead of at once: for a run that is filmed.
"""
import argparse
import itertools
import json
import math
import sys
import time
import urllib.request

# The route: (label, waypoints, then what to use at the end: (kind, entity) or None).
ROUTE = [
    ('the village well', [(203.0, 534.0)], ('use', 'Handle0')),
    ('the keepers\' stone', [(200.0, 522.0)], ('use', 'Tablet0')),
    ('the west hill cairn', [(196.0, 470.0), (178.0, 420.0), (165.0, 375.0), (156.0, 342.0)], ('use', 'Handle1')),
    ('the west stream sluice', [(170.0, 300.0), (200.0, 250.0), (225.0, 205.0), (236.0, 186.0)], ('use', 'SluiceWheel1')),
    ('turn it open', [], ('turn', 'SluiceWheel1')),
    ('the cave spring sluice', [(262.0, 160.0), (300.0, 125.0), (312.0, 116.0)], ('use', 'SluiceWheel0')),
    ('turn it open', [], ('turn', 'SluiceWheel0')),
    ('across the river to the east', [(330.0, 150.0), (348.0, 175.0), (356.0, 215.0), (365.0, 245.0),
                                      (395.0, 290.0), (420.0, 300.0), (450.0, 380.0), (466.0, 414.0)],
     ('use', 'Handle2')),
    ('the aqueduct intake', [(440.0, 360.0), (418.0, 300.0), (404.0, 268.0)], ('use', 'SluiceWheel2')),
    ('turn it open', [], ('turn', 'SluiceWheel2')),
    ('down the river and across its mouth', [(390.0, 300.0), (370.0, 380.0), (352.0, 440.0), (325.0, 472.0),
                                             (300.0, 505.0), (284.0, 538.0)], None),
    ('out along the jetty', [(281.5, 546.2), (286.5, 547.3), (290.5, 548.2)], ('board', 'Raft')),
    ('across the lake to the island', [(305.0, 553.0), (318.0, 560.0), (328.0, 565.0)], ('paddle', None)),
    ('the shrine bell', [(336.0, 570.0), (339.0, 572.0)], ('ring', 'Bell')),
]


class Host:
    """A running `amoris serve`: developer reads and the seat's actions over HTTP."""

    def __init__(self, url):
        self.url = url

    def call(self, method, params):
        req = urllib.request.Request(f'{self.url}/api/call', data=json.dumps({'method': method, 'params': params}).encode(),
                                     headers={'Content-Type': 'application/json'})
        r = json.load(urllib.request.urlopen(req, timeout=60))
        if r.get('error'):
            raise RuntimeError(json.dumps(r['error'])[:600])
        return r['result']

    def act(self, actions):
        self.call('player.act', {'seat': 'player', 'actions': actions})

    def get(self, entity, components):
        return self.call('world.get', {'entity': entity, 'components': components})['components']


class Page:
    """An exported game in a browser tab, over CDP: the page's own command channel."""

    def __init__(self, port, prefix):
        import websocket
        pages = json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list'))
        page = next(p for p in pages if p['id'].startswith(prefix))
        self.ws = websocket.create_connection(page['webSocketDebuggerUrl'], timeout=60, suppress_origin=True)
        self.ids = itertools.count(1)

    def eval(self, expr):
        i = next(self.ids)
        self.ws.send(json.dumps({'id': i, 'method': 'Runtime.evaluate',
                                 'params': {'expression': expr, 'awaitPromise': True, 'returnByValue': True}}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get('id') == i:
                r = m.get('result', {})
                if 'exceptionDetails' in r:
                    raise RuntimeError(json.dumps(r['exceptionDetails'])[:600])
                return r.get('result', {}).get('value')

    def act(self, actions):
        self.eval(f'window.amorisPlayer.amoris.command("player.act", {{actions: {json.dumps(actions)}}}, {{source: "player"}})'
                  '.then(() => 1)')

    def get(self, entity, components):
        r = self.eval(f'window.amorisPlayer.amoris.command("world.get", {{entity: {json.dumps(entity)}, '
                      f'components: {json.dumps(components)}}}).then(r => JSON.stringify(r))')
        return json.loads(r)['components']


class Smooth:
    """Turns the look toward each yaw and pitch it is set to at a bounded rate, in steps."""

    RATE = 2.5  # radians a second
    STEP = 1 / 30

    def __init__(self, game):
        self.game = game
        self.yaw = None
        self.pitch = 0.0

    def get(self, entity, components):
        return self.game.get(entity, components)

    def act(self, actions):
        if self.yaw is None:
            w = self.game.get('Keeper', ['Walker'])['Walker']
            self.yaw, self.pitch = w['yaw'], w['pitch']
        for a in actions:
            c = a.get('controls', {})
            if a.get('do') != 'set' or 'yaw' not in c:
                continue
            dy = (c['yaw'] - self.yaw + math.pi) % math.tau - math.pi
            dp = c.get('pitch', self.pitch) - self.pitch
            n = math.ceil(max(abs(dy), abs(dp)) / (self.RATE * self.STEP))
            for k in range(1, n):
                yaw = (self.yaw + dy * k / n + math.pi) % math.tau - math.pi
                self.game.act([{'do': 'set', 'controls': {'yaw': yaw,
                                                          'pitch': self.pitch + dp * k / n}}])
                time.sleep(self.STEP)
        for a in actions:
            c = a.get('controls', {})
            if a.get('do') == 'set' and 'yaw' in c:
                self.yaw = c['yaw']
                self.pitch = c.get('pitch', self.pitch)
        self.game.act(actions)


def pose(game):
    c = game.get('Keeper', ['Transform', 'Walker', 'Quest'])
    return c['Transform']['position'], c['Walker'], c['Quest']


def heading(fx, fz, tx, tz):
    """The yaw that looks from (fx, fz) toward (tx, tz): 0 looks along -z, positive turns left."""
    return math.atan2(-(tx - fx), -(tz - fz))


def walk(game, x, z, log, arrive=1.6, limit_s=150, detours=2):
    start = time.time()
    best, stuck_since, side, tries = 1e9, time.time(), 1, 0
    while time.time() - start < limit_s:
        p, w, _ = pose(game)
        d = math.hypot(x - p[0], z - p[2])
        if d < arrive:
            game.act([{'do': 'set', 'controls': {'forward': 0.0, 'strafe': 0.0, 'run': False, 'rise': False}}])
            return True
        if d < best - 0.5:
            best, stuck_since = d, time.time()
        strafe = 0.0
        if time.time() - stuck_since > 2.5:
            tries += 1
            if tries > 3 and detours > 0:
                # Something in the way: go round it, six metres to one side, then on.
                h = heading(p[0], p[2], x, z) + side * math.pi / 2
                walk(game, p[0] - math.sin(h) * 6, p[2] - math.cos(h) * 6, log, 1.6, 20, 0)
                tries, best, stuck_since = 0, 1e9, time.time()
                detours -= 1
                continue
            # No progress: step aside for a moment, then try again.
            strafe, side = side, -side
            game.act([{'do': 'set', 'controls': {'strafe': strafe, 'forward': 0.3}}, {'do': 'pulse', 'control': 'jump'}])
            time.sleep(0.7)
            stuck_since = time.time()
            continue
        swimming = bool(w.get('in_liquid'))
        game.act([{'do': 'set', 'controls': {'yaw': heading(p[0], p[2], x, z), 'pitch': -0.1, 'forward': 1.0,
                                             'strafe': 0.0, 'run': d > 6 and not swimming, 'rise': swimming}}])
        time.sleep(0.15)
    log(f'  could not reach ({x}, {z})')
    return False


def face_and_click(game, entity, clicks, log, pause=1.6, turn=0.0):
    tf = game.get(entity, ['Transform'])['Transform']
    t = tf['position']
    p, _, _ = pose(game)
    d = math.hypot(t[0] - p[0], t[2] - p[2])
    if entity.startswith('SluiceWheel'):
        # A wheel is turned from in front of it: its axle is its own +z, out of the sluice house.
        q = tf['rotation']
        fx, fz = 2 * (q[0] * q[2] + q[3] * q[1]), 1 - 2 * (q[0] * q[0] + q[1] * q[1])
        a = math.atan2(fz, fx) + turn
        walk(game, t[0] + math.cos(a) * 1.9, t[2] + math.sin(a) * 1.9, log, arrive=0.5, limit_s=40, detours=1)
        p, _, _ = pose(game)
    elif d > 2.0 or turn:
        # Up to the thing, stopping short of it (from another side when `turn` is set).
        a = math.atan2(p[2] - t[2], p[0] - t[0]) + turn
        walk(game, t[0] + math.cos(a) * 1.8, t[2] + math.sin(a) * 1.8, log, arrive=0.6, limit_s=30, detours=0)
        p, _, _ = pose(game)
    eye_dy = t[1] - p[1]
    flat = math.hypot(t[0] - p[0], t[2] - p[2])
    game.act([{'do': 'set', 'controls': {'forward': 0.0, 'strafe': 0.0, 'run': False,
                                         'yaw': heading(p[0], p[2], t[0], t[2]),
                                         'pitch': math.atan2(eye_dy, max(flat, 0.1))}}])
    time.sleep(0.4)
    for _ in range(clicks):
        game.act([{'do': 'pulse', 'control': 'primary'}])
        time.sleep(pause)


def paddle(game, points, log, limit_s=180):
    """On the raft: steer by the look's yaw toward each point and paddle; step off at the last."""
    start = time.time()
    for x, z in points:
        while time.time() - start < limit_s:
            r = game.get('Raft', ['Transform'])['Transform']['position']
            if math.hypot(x - r[0], z - r[2]) < 3.5:
                break
            # A board paddles along its own heading (its -z) and A / D (strafe) turn it: steer by
            # the angle between its bow and the point, and look where it goes.
            q = game.get('Raft', ['Transform'])['Transform']['rotation']
            bx = -(2 * (q[0] * q[2] + q[3] * q[1]))
            bz = -(1 - 2 * (q[0] * q[0] + q[1] * q[1]))
            # Headings as atan2(x, z); strafe +1 turns the bow toward a smaller one.
            err = math.atan2(x - r[0], z - r[2]) - math.atan2(bx, bz)
            err = (err + math.pi) % math.tau - math.pi
            game.act([{'do': 'set', 'controls': {'yaw': heading(r[0], r[2], x, z), 'forward': 1.0 if abs(err) < 1.2 else 0.3,
                                                 'strafe': max(-1.0, min(1.0, -1.5 * err))}}])
            time.sleep(0.3)
    game.act([{'do': 'set', 'controls': {'forward': 0.0}}, {'do': 'pulse', 'control': 'secondary'}])
    time.sleep(1.0)
    log(f'   stepped off the raft {time.time() - start:.0f} s after casting off')


def run(game, log):
    for label, points, use in ROUTE:
        log(f'-> {label}')
        if use is not None and use[0] == 'paddle':
            paddle(game, points, log)
            continue
        for x, z in points:
            if not walk(game, x, z, log):
                return False
        if use is None:
            continue
        kind, entity = use
        if kind == 'ring':
            # The bell answers once the river has reached the lake.
            for _ in range(120):
                if pose(game)[2]['reached']:
                    break
                time.sleep(2)
        before = pose(game)[2]
        if kind == 'board':
            for turn in (0.0, 0.6, -0.6):
                face_and_click(game, entity, 1, log, turn=turn)
                if pose(game)[1].get('mount'):
                    log('   on the raft')
                    break
            continue
        for attempt, turn in enumerate((0.0, 0.9, -0.9, 1.8)):
            face_and_click(game, entity, 4 if kind == 'turn' else 1, log, turn=turn)
            q = pose(game)[2]
            if [q[k] for k in ('found', 'handles', 'opened', 'rung')] !=                     [before[k] for k in ('found', 'handles', 'opened', 'rung')] or kind == 'use' and entity.startswith('Tablet'):
                break
            log(f'   nothing happened at {entity}; trying again from another side')
        log(f'   handles {q["found"]} carried {q["handles"]} open {q["opened"]} reached {q["reached"]} rung {q["rung"]}')
        if kind == 'ring' and q['rung']:
            return True
    return False


def main():
    a = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    a.add_argument('target', choices=['host', 'page'])
    a.add_argument('--host', default='http://127.0.0.1:7878')
    a.add_argument('--cdp', type=int, default=9333)
    a.add_argument('--page', default='')
    a.add_argument('--smooth', action='store_true', help='turn the look at a bounded rate')
    args = a.parse_args()
    game = Host(args.host) if args.target == 'host' else Page(args.cdp, args.page)
    if args.smooth:
        game = Smooth(game)
    t0 = time.time()

    def log(msg):
        print(f'[{time.time() - t0:7.1f}s] {msg}', flush=True)
    ok = run(game, log)
    log('the bell rang: done' if ok else 'stopped before the end')
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
