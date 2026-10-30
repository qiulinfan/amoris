#!/usr/bin/env python3
"""A test pilot for Thornkeep: plays the whole game through the player seat's own controls.

    python samples/thornkeep/checks/pilot.py host [--host http://127.0.0.1:7878] [--fresh]
    python samples/thornkeep/checks/pilot.py page --cdp 9333 --page <target id prefix>

It reads the sellsword and the foes from the world (developer reads: `world.get`, `world.query`)
and acts only as the seat `player` does: it sets forward, strafe, run, yaw, the held buttons and the
pressed ones, and pulses primary. It walks the road, the bailey, the stair, the shrine and the ring
by waypoints; locks on to the nearest foe; rolls from blows it sees begin; swings while the foe is
open, a heavy blow at a shield, bolts at archers; guards and backs off after a combo; drinks when
hurt and clear. Against the Warden it stays out of reach while he swings and goes in for three blows
while he recovers. After a fall it rises at the shrine and goes on from the nearest waypoint; when
stuck against something it steps aside, as a player would. It ends when the keep is freed or time
runs out. `host` drives a running `amoris serve` in Play (`--fresh` restarts Play first); `page`
drives an exported game in a browser over CDP (the page's `window.amorisPlayer`) without bringing
its window forward. `--smooth` turns the look at most 3 radians a second: for a run that is filmed.
"""
import argparse
import itertools
import json
import math
import sys
import time
import urllib.request

ROUTE = [(0, 30), (0, 10), (-7, 7.5), (0, -17), (0, -24), (0, -31.5), (-12, -28), (0, -36), (0, -48), (0, -56),
         (0, -66), (0, -78)]
BITS = {'dodge': 1, 'block': 2, 'lock': 4, 'heavy': 8, 'bolt': 16, 'knives': 32, 'flask': 64}
FREE, ATTACK, ROLL, HURT, STAGGER, CAST, DEAD, DRINK, CHARGE = range(9)
BRIGAND, BRUTE, ARCHER, WARDEN = 1, 2, 3, 4
FOE_FIELDS = ['Fighter.kind', 'Fighter.hp', 'Fighter.action', 'Fighter.action_s', 'Transform.position']


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

    def query(self, with_, fields):
        return self.call('world.query', {'with': with_, 'fields': fields})

    def read(self):
        """What one step reads: the keep, the sellsword and the foes."""
        return (self.query(['Keep'], ['Keep.freed']), self.get('Sellsword', ['Transform', 'Fighter', 'Player']),
                self.query(['Brain', 'Fighter', 'Transform'], FOE_FIELDS))


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

    def command(self, method, params, source=''):
        opts = ', {source: "player"}' if source else ''
        return json.loads(self.eval(f'window.amorisPlayer.amoris.command({json.dumps(method)}, {json.dumps(params)}{opts})'
                                    '.then(r => JSON.stringify(r ?? null))'))

    def act(self, actions):
        self.command('player.act', {'actions': actions}, source='player')

    def get(self, entity, components):
        return self.command('world.get', {'entity': entity, 'components': components})['components']

    def query(self, with_, fields):
        return self.command('world.query', {'with': with_, 'fields': fields})

    def read(self):
        """What one step reads, in one round trip: the keep, the sellsword and the foes."""
        c = 'window.amorisPlayer.amoris.command'
        r = self.eval(f'Promise.all([{c}("world.query", {{with: ["Keep"], fields: ["Keep.freed"]}}), '
                      f'{c}("world.get", {{entity: "Sellsword", components: ["Transform", "Fighter", "Player"]}}), '
                      f'{c}("world.query", {{with: ["Brain", "Fighter", "Transform"], fields: {json.dumps(FOE_FIELDS)}}})])'
                      '.then(r => JSON.stringify(r))')
        keep, me, foes = json.loads(r)
        return keep, me['components'], foes


def yaw_to(dx, dz):
    return math.atan2(-dx, -dz)


RING = (0.0, -71.0)  # the ring of pillars before the throne


def away(p, q, yaw):
    """Forward and strafe (for a walker looking along `yaw`) that move from `q` and back toward the
    ring's centre, so a retreat does not end in a corner."""
    ax, az = p[0] - q[0], p[2] - q[2]
    n = math.hypot(ax, az) or 1.0
    cx, cz = RING[0] - p[0], RING[1] - p[2]
    m = math.hypot(cx, cz)
    pull = 0.0 if m < 6 else min(1.5, (m - 6) / 4)
    dx, dz = ax / n + pull * cx / (m or 1.0), az / n + pull * cz / (m or 1.0)
    k = math.hypot(dx, dz) or 1.0
    dx, dz = dx / k, dz / k
    s, c = math.sin(yaw), math.cos(yaw)
    return dx * -s + dz * -c, dx * c + dz * -s


class Pilot:
    def __init__(self, game, smooth):
        self.game = game
        self.smooth = smooth
        self.yaw = None
        self.at = time.time()

    def set(self, **c):
        for k in ('forward', 'strafe'):
            if k in c:
                c[k] = max(-1.0, min(1.0, c[k]))
        if self.smooth and 'yaw' in c:
            # The look turns toward the asked yaw at a bounded rate.
            now = time.time()
            if self.yaw is None:
                self.yaw = self.game.get('Sellsword', ['Walker'])['Walker']['yaw']
            dy = (c['yaw'] - self.yaw + math.pi) % math.tau - math.pi
            step = 3.0 * max(0.02, min(0.2, now - self.at))
            self.yaw = (self.yaw + max(-step, min(step, dy)) + math.pi) % math.tau - math.pi
            c['yaw'] = self.yaw
        self.at = time.time()
        self.game.act([{'do': 'set', 'controls': c}])

    def swing(self):
        self.game.act([{'do': 'pulse', 'control': 'primary'}])

    def run(self, seconds):
        t0 = time.time()
        wp = 0
        last_log = -9.0
        rolled = -9.0
        swung = []
        was_dead = False
        stuck_at, stuck_t, wiggle = None, 0.0, 0.0
        shunned = {}  # foes it could not reach, until when
        while time.time() - t0 < seconds:
            now = time.time() - t0
            keep, me, foes = self.game.read()
            if keep and keep[0].get('Keep.freed'):
                print(f'{now:6.1f} the keep is freed', flush=True)
                self.set(forward=0.0, strafe=0.0, held=0)
                return True
            p = me['Transform']['position']
            f = me['Fighter']
            pl = me['Player']
            near = []
            for row in foes:
                q = row['Transform.position']
                if row['Fighter.action'] == DEAD or abs(q[1] - p[1]) > 2.5:
                    continue
                near.append((math.hypot(q[0] - p[0], q[2] - p[2]), row['name'], q,
                             {'kind': row['Fighter.kind'], 'hp': row['Fighter.hp'], 'action': row['Fighter.action'],
                              'action_s': row['Fighter.action_s']}))
            near = [x for x in near if shunned.get(x[1], 0.0) < now]
            # The nearest first, but a wounded foe within reach before a fresh one.
            near.sort(key=lambda x: x[0] - (2.0 if x[3]['hp'] < 30 and x[0] < 4.5 else 0.0))
            if now - last_log > 3:
                print(f"{now:6.1f} at ({p[0]:.1f}, {p[1]:.1f}, {p[2]:.1f}) hp {f['hp']:.0f} stamina {f['stamina']:.0f} "
                      f"flasks {pl.get('flasks')} near {[(n, round(d, 1), round(c['hp'])) for d, n, q, c in near[:3]]}",
                      flush=True)
                last_log = now
            if f['action'] == DEAD:
                self.set(forward=0.0, strafe=0.0, held=0)
                was_dead = True
                time.sleep(0.3)
                continue
            if was_dead:
                # Risen: go on from the waypoint nearest to where it stands.
                wp = min(range(len(ROUTE)), key=lambda i: math.hypot(ROUTE[i][0] - p[0], ROUTE[i][1] - p[2]))
                was_dead = False
            busy = f['action'] != FREE
            if stuck_at is None or math.hypot(p[0] - stuck_at[0], p[2] - stuck_at[2]) > 0.4:
                stuck_at, stuck_t = p, now
            elif now - stuck_t > 3 and not busy and near and 2.3 < near[0][0] < 10 and near[0][3]['kind'] != WARDEN:
                # Not getting closer to a foe out of reach (an archer behind a house): leave it a while.
                shunned[near[0][1]] = now + 20.0
                wiggle = now + 1.0
                stuck_at, stuck_t = p, now
            elif now - stuck_t > 4 and not busy and not (near and near[0][0] < 6):
                wiggle = now + 1.0
                stuck_at, stuck_t = p, now
            if now < wiggle:
                self.set(forward=0.3, strafe=1.0 if int(now) % 2 else -1.0, held=0)
                time.sleep(0.06)
                continue
            fight = [x for x in near if x[0] < 10]
            if fight:
                d, n, q, c = fight[0]
                kind = c['kind']
                if not pl.get('lock'):
                    self.set(pressed=BITS['lock'])
                face = yaw_to(q[0] - p[0], q[2] - p[2])
                if kind == WARDEN:
                    # Out of his reach while he swings, in for three blows while he recovers.
                    swinging = c['action'] in (ATTACK, CHARGE, CAST)
                    swung = [t for t in swung if now - t < 2.0]
                    back, aside = away(p, q, face)
                    if swinging and d < 4.6 and c['action_s'] < 0.5 and f['stamina'] > 22 and now - rolled > 0.6 and not busy:
                        self.set(forward=back, strafe=aside, yaw=face, pressed=BITS['dodge'], held=0)
                        rolled = now
                    elif swinging:
                        # Just outside his reach, ready to step back in.
                        self.set(forward=back if d < 3.6 else 0.0, strafe=aside if d < 3.6 else 0.0, yaw=face, held=0)
                    elif f['hp'] < 75 and pl.get('flasks', 0) > 0 and not busy:
                        # Hurt: out of his reach first, then drink.
                        if d < 5.0:
                            self.set(forward=back, strafe=aside, yaw=face, held=0, run=True)
                        else:
                            self.set(forward=0.0, strafe=0.0, pressed=BITS['flask'], held=0, run=False)
                    elif d > 5 and f['focus'] >= 34 and not busy:
                        self.set(forward=0.0, strafe=0.0, yaw=face, pressed=BITS['bolt'], held=0)
                    elif len(swung) >= 3 or f['stamina'] < 35:
                        self.set(forward=back if d < 3.6 else 0.0, strafe=aside if d < 3.6 else 0.4, yaw=face, held=0)
                    elif d > 2.3:
                        self.set(forward=1.0, strafe=0.0, yaw=face, held=0, run=True)
                    elif not busy:
                        self.set(forward=0.0, strafe=0.0, yaw=face, held=0, run=False)
                        self.swing()
                        swung.append(now)
                else:
                    threat = any(dd < 3.4 and cc['action'] in (ATTACK, CHARGE) and cc['action_s'] < 0.45
                                 for dd, _, _, cc in near)
                    if threat and f['stamina'] > 22 and now - rolled > 0.7 and not busy:
                        side = 1.0 if int(now * 2) % 2 else -1.0
                        self.set(forward=-0.3, strafe=side, yaw=face, pressed=BITS['dodge'], held=0)
                        rolled = now
                    elif (f['hp'] < 70 and pl.get('flasks', 0) > 0 and not busy
                          and not any(dd < 3.0 and cc['action'] in (ATTACK, CHARGE) for dd, _, _, cc in near)):
                        self.set(forward=0.0, strafe=0.0, pressed=BITS['flask'], held=0)
                    elif kind == ARCHER and d > 4 and f['focus'] >= 34 and not busy:
                        self.set(forward=0.0, strafe=0.0, yaw=face, pressed=BITS['bolt'], held=0)
                    elif d > 2.3:
                        self.set(forward=1.0, strafe=0.0, yaw=face, held=0)
                    elif f['stamina'] > 28 and c['action'] not in (ATTACK, CHARGE) and not busy:
                        swung = [t for t in swung if now - t < 2.5]
                        if len(swung) >= 3:
                            # A breath after a combo: guard and back off a step.
                            self.set(forward=-0.6, strafe=0.0, yaw=face, held=BITS['block'])
                        elif kind == BRUTE and f['stamina'] > 45:
                            self.set(forward=0.0, strafe=0.0, yaw=face, pressed=BITS['heavy'], held=0)
                            swung.append(now)
                        else:
                            self.set(forward=0.0, strafe=0.0, yaw=face, held=0)
                            self.swing()
                            swung.append(now)
                    else:
                        self.set(forward=-0.5, strafe=0.0, yaw=face, held=BITS['block'])
            elif f['hp'] < 100 and pl.get('flasks', 0) > 0 and not busy:
                self.set(forward=0.0, pressed=BITS['flask'], held=0)
            elif wp < len(ROUTE):
                tx, tz = ROUTE[wp]
                if math.hypot(tx - p[0], tz - p[2]) < 1.5:
                    wp += 1
                else:
                    self.set(forward=1.0, strafe=0.0, yaw=yaw_to(tx - p[0], tz - p[2]), held=0)
            else:
                self.set(forward=0.0, held=0)
            time.sleep(0.02)
        return False


def main():
    a = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    a.add_argument('mode', choices=['host', 'page'])
    a.add_argument('--host', default='http://127.0.0.1:7878')
    a.add_argument('--cdp', type=int, default=9333)
    a.add_argument('--page', default='')
    a.add_argument('--fresh', action='store_true')
    a.add_argument('--smooth', action='store_true')
    a.add_argument('--seconds', type=float, default=1500)
    args = a.parse_args()
    if args.mode == 'host':
        game = Host(args.host)
        if args.fresh:
            game.call('play.stop', {})
            game.call('play.start', {})
            time.sleep(1)
    else:
        game = Page(args.cdp, args.page)
    try:
        freed = Pilot(game, args.smooth).run(args.seconds)
    except RuntimeError as e:
        # The keep freed ends the episode (project.toml's done_event): the seat's next act is refused.
        if 'time.episode_over' not in str(e):
            raise
        print('the episode is over: the keep is freed', flush=True)
        freed = True
    sys.exit(0 if freed else 1)


if __name__ == '__main__':
    main()
