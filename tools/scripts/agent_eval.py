#!/usr/bin/env python3
"""The agent benchmark: can a program change a running Pocket project through the engine's own
interface, given the task in words and the documentation? (docs/agent-eval.md)

Every task starts a sample paused in a headless runtime with the JSON-RPC server, hands the task
to a runner, then checks the world through the same commands. Runners:

    reference   the solutions, in this file, through the RPC: the harness checking itself
    null        does nothing: the floor
    <command>   any program: it gets the task as JSON on stdin ({"task", "project", "rpc_url",
                "docs"}) and may print a JSON object with an "answer" as its last line

    python3 tools/scripts/agent_eval.py --runner reference --json > tests/evidence/agent-eval/reference.json
    python3 tools/scripts/agent_eval.py --runner "claude -p" --tasks spawn_named,recolor
"""
import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
from pocket_env import IosEnv, PocketEnv  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
POCKET = os.path.join(ROOT, ".pocket", "pocket")
DOCS = ["docs/mcp.md", "docs/sdk.md", "docs/generated/sdk.md", "docs/design/world-model.md", "docs/generated/components.md", "docs/design/input.md", "docs/design/scenarios.md", "docs/design/assets.md", "docs/design/pocket-ui.md", "docs/design/rendering.md", "docs/design/cameras.md", "docs/design/animation.md", "docs/design/physics.md", "docs/design/physics2d.md", "docs/design/paths.md", "docs/design/combat.md", "docs/design/sprites.md", "docs/design/tilemaps.md", "docs/design/navigation.md", "docs/design/behavior.md", "docs/design/audio.md", "docs/design/particles.md", "docs/design/terrain.md", "docs/design/water.md", "docs/design/wind.md", "docs/design/timelines.md", "docs/design/dialogue.md", "docs/design/localization.md", "docs/design/networking.md"]
# Where a run's copies live: outside the repository, so an agent finds the game and the docs there
# and nothing of the harness (whose checks are the answers) beside them. Made by run().
EVAL_DIR = None


def near(a, b, tol=0.01):
    return abs(a - b) <= tol


def color_is(c, r, g, b):
    return abs(c["r"] - r) < 0.2 and abs(c["g"] - g) < 0.2 and abs(c["b"] - b) < 0.2


# ---- tasks: text, project, ticks to run before the task, a precondition the setup must meet,
# the reference solution, the check.
def spawn_named_solve(env):
    env.command("world.spawn", {"name": "Beacon", "components": {"Transform": {"position": {"x": 2, "y": 1, "z": -3}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 1, "g": 0, "b": 0, "a": 1}}}})


def spawn_named_check(env, answer):
    ident = env.command("world.find", {"path": "Beacon"})
    if not ident:
        return False, "no entity named Beacon"
    t = env.command("world.get", {"entity": ident, "component": "Transform"})["position"]
    m = env.command("world.get", {"entity": ident, "component": "MeshRenderer"})
    if not (near(t["x"], 2) and near(t["y"], 1) and near(t["z"], -3)):
        return False, f"Beacon is at {t}"
    if m.get("mesh") != "cube":
        return False, f"Beacon draws {m.get('mesh')!r}"
    if not color_is(m["color"], 1, 0, 0):
        return False, f"Beacon's color is {m['color']}"
    return True, "a red cube at (2, 1, -3)"


def recolor_solve(env):
    env.command("world.set", {"entity": "Ball", "component": "MeshRenderer", "value": {"color": {"r": 0, "g": 1, "b": 0, "a": 1}}})


def recolor_check(env, answer):
    m = env.command("world.get", {"entity": "Ball", "component": "MeshRenderer"})
    return (color_is(m["color"], 0, 1, 0), f"Ball's color is {m['color']}")


def tile_edit_solve(env):
    env.command("tilemap.set", {"entity": "Level", "layer": "ground", "tile_x": 3, "tile_y": 2, "id": 0})


def tile_edit_check(env, answer):
    t = env.command("tilemap.tile", {"entity": "Level", "tile_x": 3, "tile_y": 2, "layer": "ground"})
    layers = [l for l in t["layers"] if l["layer"] == "ground"]
    if not layers:
        return False, "no ground layer"
    ok = layers[0]["gid"] == 1 and layers[0].get("id") == 0
    return ok, f"cell (3, 2) on ground holds gid {layers[0]['gid']}"


def clear_enemies_before(env):
    # Enemies come every three quarters of a second and go when they reach the player, so a
    # fixed tick can land in a gap: step on, a little at a time, until some are about.
    n = 0
    for _ in range(60):
        n = len(env.command("world.query", {"name": "Enemy"})["entities"])
        if n >= 1:
            break
        env.command("step", {"ticks": 10})
    return n >= 1, f"{n} enemies about"


def clear_enemies_solve(env):
    for row in env.command("world.query", {"name": "Enemy"})["entities"]:
        env.command("world.destroy", {"entity": row["id"]})


def clear_enemies_check(env, answer):
    rows = env.command("world.query", {"name": "Enemy"})["entities"]
    total = env.command("world.summary", {})["entities"]
    if total < 5:
        return False, f"only {total} entities are left: more than the enemies went"
    return len(rows) == 0, f"{len(rows)} enemies remain"


def play_clip_solve(env):
    env.command("animation.play", {"entity": "Arm", "clip": "turn", "loop": True})


def play_clip_check(env, answer):
    a = env.command("world.get", {"entity": "Arm", "component": "Animator"})
    ok = a.get("clip") == "turn" and bool(a.get("loop", True)) and bool(a.get("playing", True))
    return ok, f"Arm plays {a.get('clip')!r} loop {a.get('loop')} playing {a.get('playing')}"


def path_length_solve(env):
    return env.command("nav.path", {"from": {"x": -8, "y": 0, "z": -8}, "to": {"x": 8, "y": 0, "z": 8}})["length"]


def path_length_check(env, answer):
    truth = env.command("nav.path", {"from": {"x": -8, "y": 0, "z": -8}, "to": {"x": 8, "y": 0, "z": 8}})["length"]
    try:
        given = float(answer)
    except (TypeError, ValueError):
        return False, f"no number answered (truth {truth:.3f})"
    return abs(given - truth) <= 0.03 * truth, f"answered {given:.3f}, the path is {truth:.3f}"


def count_overlap_solve(env):
    return len(env.command("physics.overlap", {"center": {"x": 0, "y": 0, "z": 0}, "radius": 4}))


def count_overlap_check(env, answer):
    truth = len(env.command("physics.overlap", {"center": {"x": 0, "y": 0, "z": 0}, "radius": 4}))
    try:
        given = int(answer)
    except (TypeError, ValueError):
        return False, f"no number answered (truth {truth})"
    return given == truth, f"answered {given}, there are {truth}"


def last_hit_root(env):
    hits = env.command("events.since", {"seq": 0, "type": "player.hit", "limit": 4000})["events"]
    if not hits:
        return None
    why = env.command("events.why", {"seq": hits[-1]["seq"], "limit": 32})
    root = why["root"]
    for e in why["chain"]:
        if e["seq"] == root:
            return e["type"]
    return why["chain"][0]["type"] if why["chain"] else None


def event_cause_before(env):
    hits = env.command("events.since", {"seq": 0, "type": "player.hit", "limit": 4000})["events"]
    return len(hits) > 0, f"{len(hits)} hits so far"


def event_cause_solve(env):
    return last_hit_root(env)


def event_cause_check(env, answer):
    truth = last_hit_root(env)
    if truth is None:
        return False, "no player.hit happened in the setup"
    return answer == truth, f"answered {answer!r}, the root cause is {truth!r}"


# ---- script tasks: the runner edits scripts/main.ts of a copy of the sample; the harness bundles
# the copy again, reloads the script context and checks the exposed state.
def entry_script(project_dir):
    """The project's entry script from its project.toml (scripts/main.ts unless it says otherwise)."""
    with open(os.path.join(project_dir, "project.toml"), encoding="utf-8") as f:
        for line in f:
            m = re.match(r'\s*entry\s*=\s*"([^"]+)"', line)
            if m:
                return m.group(1)
    return "scripts/main.ts"


def edit_main(project_dir, transform):
    path = os.path.join(project_dir, entry_script(project_dir))
    with open(path, encoding="utf-8") as f:
        text = f.read()
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(transform(text))


def state_key(st, name):
    """A state key written by expose, whether the state is flat ("player.y") or nested."""
    if name in st:
        return st[name]
    cur = st
    for part in name.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return None
        cur = cur[part]
    return cur


def expose_count_solve(env, project_dir):
    edit_main(project_dir, lambda t: t + '\nexpose("answer", () => world.summary().entities);\n')


def expose_count_check(env, answer):
    st = env.command("state", {})["state"]
    truth = env.command("world.summary", {})["entities"]
    if "answer" not in st:
        return False, "no state key 'answer' after the reload"
    return st["answer"] == truth, f"answer {st['answer']}, entities {truth}"


def ball_lower_solve(env, project_dir):
    edit_main(project_dir, lambda t: t.replace("new Ball(3.0)", "new Ball(1.0)"))


def ball_lower_check(env, answer):
    st = env.command("state", {})["state"]
    y = st.get("ball.y")
    if y is None:
        return False, "no ball.y in the state"
    return 0.0 <= y <= 1.05, f"the ball starts its fall at y {y}"


def spawn_stars_solve(env, project_dir):
    edit_main(project_dir, lambda t: t + '\nonStart(() => { for (let i = 0; i < 5; i++) world.spawn(`Star${i}`, { components: { Transform: { position: { x: i, y: 2, z: 0 }, scale: { x: 0.2, y: 0.2, z: 0.2 } }, MeshRenderer: { mesh: "sphere" } } }); });\n')


def spawn_stars_check(env, answer):
    for i in range(5):
        ident = env.command("world.find", {"path": f"Star{i}"})
        if not ident:
            return False, f"no entity named Star{i}"
        t = env.command("world.get", {"entity": ident, "component": "Transform"})["position"]
        if not (near(t["x"], i) and near(t["y"], 2) and near(t["z"], 0)):
            return False, f"Star{i} is at {t}"
        if env.command("world.get", {"entity": ident, "component": "MeshRenderer"}).get("mesh") != "sphere":
            return False, f"Star{i} is not a sphere"
    return True, "Star0 to Star4 along x at y 2, spheres"


def double_jump_solve(env, project_dir):
    # A second jump in the air, once per flight: a counter cleared on the ground.
    def transform(t):
        a = "const coins = new Set<number>();"
        b = '    if (input.pressed("jump") && body.grounded) {\n        vy = 10.5;'
        if a not in t or b not in t:
            raise RuntimeError("the sprites script changed shape")
        t = t.replace(a, a + "\nlet airJumps = 0;   // jumps taken since the player last stood", 1)
        return t.replace(b, '    if (body.grounded) airJumps = 0;\n    if (input.pressed("jump") && (body.grounded || airJumps < 1)) {\n        if (!body.grounded) airJumps++;\n        vy = 10.5;', 1)
    edit_main(project_dir, transform)


def double_jump_check(env, answer):
    # Jump, press jump again in the air, and see how high the player got: a single jump rises 2.3.
    st = lambda: env.command("state", {})["state"]  # noqa: E731
    for _ in range(240):
        if state_key(st(), "player.grounded"):
            break
        env.command("step", {"ticks": 1})
    s = st()
    if not state_key(s, "player.grounded"):
        return False, "the player never landed"
    start = state_key(s, "player.y")
    env.command("input.press", {"action": "jump"})
    env.command("step", {"ticks": 12})
    if state_key(st(), "player.grounded"):
        return False, "the first jump did not lift the player"
    env.command("input.press", {"action": "jump"})
    apex = start
    for _ in range(80):
        env.command("step", {"ticks": 1})
        apex = max(apex, state_key(st(), "player.y"))
    rise = apex - start
    return rise > 3.0, f"rose {rise:.2f} with jump pressed again in the air (a single jump rises 2.3)"


def coin_respawn_solve(env, project_dir):
    # A collected coin comes back where it was after three seconds of simulation time.
    def transform(t):
        imp = "tilemap, tween, world } from \"pocket\";"
        a = "const coins = new Set<number>();"
        col = "            coins.delete(id);\n            tween.cancelAll(id);\n            world.destroy(id);"
        if imp not in t or a not in t or col not in t:
            raise RuntimeError("the sprites script changed shape")
        t = t.replace(imp, "tilemap, timer, tween, world } from \"pocket\";", 1)
        t = t.replace(a, a + "\nlet respawned = 0;", 1)
        back = (
            "\n            const at = { x: c.position.x, y: c.position.y };"
            "\n            timer.after(3, () => {"
            "\n                const again = world.spawn(`CoinBack${respawned++}`, { components: { Transform: { position: { x: at.x, y: at.y, z: 0 } }, Sprite: { texture: \"assets/coin.png\", size: { x: 0.5, y: 0.5 }, layer: 1, filter: \"nearest\" } } });"
            "\n                sprites.play(again, \"coin\", { speed: 1 });"
            "\n                coins.add(again);"
            "\n            });"
        )
        return t.replace(col, col + back, 1)
    edit_main(project_dir, transform)


def coin_respawn_check(env, answer):
    # Stand the player on a coin: the count drops at once and is back three seconds later, not before.
    st = lambda: env.command("state", {})["state"]  # noqa: E731
    before = state_key(st(), "coins")
    home = env.command("world.get", {"entity": "Player", "component": "Transform"})["position"]
    coin = env.command("world.get", {"entity": "Coin0", "component": "Transform"})["position"]
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": coin["x"], "y": coin["y"], "z": 0}}})
    env.command("step", {"ticks": 2})
    taken = state_key(st(), "coins")
    if taken != before - 1:
        return False, f"{before} coins, {taken} after standing on one"
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": home}})   # away again, or it would take the coin back at once
    env.command("step", {"ticks": 100})
    early = state_key(st(), "coins")
    env.command("step", {"ticks": 90})
    after = state_key(st(), "coins")
    return early == before - 1 and after == before, f"{before} coins, {taken} once taken, {early} after 1.7 s, {after} after 3.2 s"


def jump_sound_solve(env, project_dir):
    # A tone written as a WAV beside the art, played by the jump.
    import math
    import wave
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with wave.open(os.path.join(project_dir, "assets", "jump.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(22050)
        frames = bytearray()
        for i in range(int(22050 * 0.15)):
            t = i / 22050
            v = int(12000 * math.sin(2 * math.pi * (440 + 600 * t) * t) * (1 - t / 0.15))
            frames += v.to_bytes(2, "little", signed=True)
        w.writeframes(bytes(frames))

    def transform(t):
        head = "import { Label, events,"
        jump = "        events.emit(\"player.jumped\", { x: Number(p.x.toFixed(2)) }, { subject: player });"
        if head not in t or jump not in t:
            raise RuntimeError("the sprites script changed shape")
        t = t.replace(head, "import { Label, audio, events,", 1)
        return t.replace(jump, jump + "\n        audio.play(\"assets/jump.wav\");", 1)
    edit_main(project_dir, transform)


def coin_chime_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with open(os.path.join(project_dir, "assets", "chime.sfx"), "w", encoding="utf-8", newline="\n") as f:
        f.write('{"preset": "coin"}\n')

    def transform(t):
        head = "import { Label, events,"
        coin = '            events.emit("coin.collected", { score: score() }, { subject: player });'
        if head not in t or coin not in t:
            raise RuntimeError("the sprites script changed shape")
        t = t.replace(head, "import { Label, audio, events,", 1)
        return t.replace(coin, coin + '\n            audio.play("assets/chime.sfx");', 1)
    edit_main(project_dir, transform)


def coin_chime_check(env, answer):
    recipes = {f["path"] for f in env.command("assets.list", {}) if f.get("kind") == "audio" and f["path"].endswith(".sfx")}
    if not recipes:
        return False, "no sound recipe (.sfx) under assets/"
    chimes = lambda: [v["clip"] for v in env.command("audio.list", {}) if v["clip"] in recipes]  # noqa: E731
    env.command("step", {"ticks": 10})
    if chimes():
        return False, "the chime plays before any coin"
    env.command("input.hold", {"action": "move_x", "ticks": 600})
    r = env.command("step", {"ticks": 600, "until": {"event": "coin.collected"}})
    if not r["until"]["met"]:
        return False, "walking right collected no coin"
    env.command("step", {"ticks": 1})
    heard = chimes()
    if not heard:
        return False, f"nothing from {sorted(recipes)} plays after a coin ({len(env.command('audio.list', {}))} voices)"
    clips = {c["path"]: c for c in env.command("audio.clips", {}) if c.get("path") in recipes}
    secs = [c.get("seconds", 0) for c in clips.values()]
    if not secs or not all(0.03 < x < 1.5 for x in secs):
        return False, f"the recipes render to {secs} seconds: not a short chime"
    return True, f"{heard[0]} ({secs[0]:.2f} s) plays when a coin is collected, not before"


THEME_SONG = """{
  "bpm": 120,
  "instruments": {
    "lead": {"wave": "square", "duty": 0.25, "hold": 0.1, "decay": 0.1, "volume": 0.25},
    "bass": {"wave": "triangle", "hold": 0.15, "decay": 0.08, "volume": 0.4}
  },
  "tracks": [
    {"instrument": "lead", "notes": "C5 E5 G5 E5 F5 A5 G5 C6"},
    {"instrument": "bass", "notes": "C3 . G2 . F2 . G2 ."}
  ]
}
"""


def theme_song_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with open(os.path.join(project_dir, "assets", "theme.song"), "w", encoding="utf-8", newline="\n") as f:
        f.write(THEME_SONG)

    def transform(t):
        head = "import { Label, events,"
        start = "onStart(() => {\n"
        if head not in t or start not in t:
            raise RuntimeError("the sprites script changed shape")
        t = t.replace(head, "import { Label, audio, events,", 1)
        return t.replace(start, start + '    audio.play("assets/theme.song", { loop: true, volume: 0.5 });\n', 1)
    edit_main(project_dir, transform)


def theme_song_check(env, answer):
    songs = [f["path"] for f in env.command("assets.list", {}) if f["path"].endswith(".song")]
    if not songs:
        return False, "no score (.song) under assets/"
    playing = [v for v in env.command("audio.list", {}) if v["clip"] in songs]
    if not playing:
        return False, f"nothing from {songs} plays from the start"
    if not playing[0].get("loop"):
        return False, f"{playing[0]['clip']} plays once, not in a loop"
    song = json.loads(env.command("project.read", {"path": playing[0]["clip"]})["text"])
    tracks = song.get("tracks", [])
    notes = lambda tr: [n for n in str(tr.get("notes", "")).replace("|", " ").split() if n not in (".", "-")]  # noqa: E731
    if len(song.get("instruments", {})) < 2 or len(tracks) < 2:
        return False, f"{playing[0]['clip']} has {len(song.get('instruments', {}))} instruments and {len(tracks)} tracks, not two of each"
    if max(len(notes(t)) for t in tracks) < 8:
        return False, "no track of at least eight notes"
    secs = [c.get("seconds", 0) for c in env.command("audio.clips", {}) if c.get("path") == playing[0]["clip"]]
    if not secs or secs[0] < 2:
        return False, f"the score renders to {secs} seconds, under two"
    env.command("step", {"ticks": int(secs[0] * 60) + 30})
    if not [v for v in env.command("audio.list", {}) if v["clip"] == playing[0]["clip"]]:
        return False, "the music stops after its first time through"
    return True, f"{playing[0]['clip']}: {len(tracks)} tracks, {secs[0]:.1f} s, looping"


VAULT_ROWS = ["############", "#@.........#", "#..c.......#", "#....##....#", "#....##..c.#", "#..........#", "#.....c....#", "############"]


def vault_level_solve(env):
    legend = {"*": {"layer": "floor", "tile": 0}, "#": {"layer": "walls", "tile": 1}, ".": None, "@": {"object": "start"}, "c": {"object": "coin"}}
    env.command("tilemap.text", {"name": "maps/vault.tmj", "rows": VAULT_ROWS, "legend": legend, "tilesets": [{"image": "assets/dungeon_tiles.png"}], "layers": ["floor", {"name": "walls", "solid": True}]})
    env.command("world.spawn", {"name": "Vault", "components": {"Transform": {"position": {"x": 40, "y": 0, "z": 0}}, "TileMap": {"map": "maps/vault.tmj"}}})


def vault_level_check(env, answer):
    p = entity_pos(env, "Vault")
    if p is None or not near(p["x"], 40, 0.01) or not near(p["y"], 0, 0.01):
        return False, f"no entity Vault at x 40, y 0 ({p})"
    info = env.command("tilemap.info", {"entity": "Vault"})
    if info.get("width") != 12 or info.get("height") != 8:
        return False, f"the Vault's map is {info.get('width')} by {info.get('height')}, not 12 by 8"
    solid = lambda x, y: env.command("tilemap.solid", {"entity": "Vault", "tile_x": x, "tile_y": y})["solid"]  # noqa: E731
    border = [(x, 0) for x in range(12)] + [(x, 7) for x in range(12)] + [(0, y) for y in range(8)] + [(11, y) for y in range(8)]
    if not all(solid(x, y) for x, y in border):
        return False, "the walls around the vault are not all solid"
    if not all(solid(x, y) for x, y in ((5, 3), (6, 3), (5, 4), (6, 4))):
        return False, "the 2 by 2 pillar at the center (cells 5..6, 3..4) is not solid"
    open_cells = [(x, y) for x in range(1, 11) for y in range(1, 7) if not (5 <= x <= 6 and 3 <= y <= 4)]
    if sum(1 for x, y in open_cells if solid(x, y)) > 0:
        return False, "cells inside the vault besides the pillar are solid"
    objs = env.command("tilemap.objects", {"entity": "Vault"})
    coins = [o for o in objs if o.get("type") == "coin"]
    starts = [o for o in objs if o.get("type") == "start"]
    if len(coins) != 3 or len(starts) != 1:
        return False, f"the vault has {len(coins)} coins and {len(starts)} starts, not 3 and 1"
    for o in coins + starts:
        x, y = o["center"]["x"] - 40, -o["center"]["y"]
        cx, cy = int(x), int(y)
        if not (1 <= cx <= 10 and 1 <= cy <= 6) or solid(cx, cy):
            return False, f"{o['name']} is at cell ({cx}, {cy}), not on the vault's floor"
    return True, "a 12 by 8 vault, walled, a solid pillar, 3 coins and a start on its floor"


# ---- the conversation task
MERCHANT = """{
  "start": "hello",
  "nodes": {
    "hello": [
      { "say": "Merchant", "text": "Potions! Fresh potions, three gold apiece." },
      { "choice": [
        { "text": "Buy a potion (3 gold)", "if": "gold >= 3", "set": { "gold": "gold - 3", "potions": "potions + 1" }, "goto": "sold" },
        { "text": "Leave", "goto": "end" }
      ] }
    ],
    "sold": [ { "say": "Merchant", "text": "A fine choice. You have {potions} now." } ]
  }
}
"""

MERCHANT_TALK = """    const m = world.get(merchant, "Transform")!.position;
    if (input.pressed("talk") && Math.hypot(m.x - x, m.y - y) < 1.8) {
        talking = dialogue.start("dialogue/merchant.dialogue.json", { gold, potions });
        dialogue.show(talking, { onEnd: (c) => {
            gold = Number(c.vars.gold);
            potions = Number(c.vars.potions);
            talking = null;
        } });
        return;
    }
"""


def merchant_talk_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "dialogue"), exist_ok=True)
    with open(os.path.join(project_dir, "dialogue", "merchant.dialogue.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(MERCHANT)

    def transform(t):
        spawn = '    guard = world.spawn("Guard", { components: box(3.1, 1.2, 0.7, 0.9, "#c0392b") });'
        near_guard = '    const g = world.get(guard, "Transform")!.position;'
        expose = 'expose("gold", () => gold);'
        if spawn not in t or near_guard not in t or expose not in t or "let paid = false;" not in t:
            raise RuntimeError("the talk script changed shape")
        t = t.replace("let paid = false;", "let paid = false;\nlet potions = 0;\nlet merchant = 0;", 1)
        t = t.replace(spawn, spawn + '\n    merchant = world.spawn("Merchant", { components: box(-2, 2, 0.7, 0.9, "#8e44ad") });', 1)
        t = t.replace(near_guard, MERCHANT_TALK + near_guard, 1)
        return t.replace(expose, expose + '\nexpose("potions", () => potions);', 1)
    edit_main(project_dir, transform)


def converse(env, buy):
    """Stand beside the merchant, press talk and play the conversation as a player would: Space past
    the lines, a choice by its number key (the one that buys when `buy` and one is offered, else
    the last). Answers whether a buying choice was offered and whether a box showed at all."""
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": -2.8, "y": 2, "z": 0}}})
    env.command("step", {"ticks": 2})
    env.command("input.press", {"action": "talk"})
    env.command("step", {"ticks": 3})
    offered, shown, bought = False, False, False
    for _ in range(120):
        if not env.command("ui.query", {"name": "dialogue"}):
            break
        shown = True
        choices = [n["text"] for n in env.command("ui.query", {"type": "text"}) if re.match(r"^\d+\. ", n.get("text", ""))]
        if choices:
            buying = [c for c in choices if re.search(r"potion|buy", c, re.I)]
            offered = offered or bool(buying)
            # One potion a talk: a merchant who asks again after a sale is left the second time.
            leaving = [c for c in choices if c not in buying] or choices
            pick = buying[0] if buy and buying and not bought else leaving[-1]
            bought = bought or pick in buying
            env.command("input.press", {"key": pick.split(".")[0]})
        else:
            env.command("input.press", {"key": "Space"})
        env.command("step", {"ticks": 3})
    env.command("step", {"ticks": 2})
    return offered, shown


def merchant_talk_check(env, answer):
    p = entity_pos(env, "Merchant")
    if p is None or not near(p["x"], -2, 0.05) or not near(p["y"], 2, 0.05):
        return False, f"no entity Merchant at x -2, y 2 ({p})"
    try:
        script = json.loads(env.command("project.read", {"path": "dialogue/merchant.dialogue.json"})["text"])
    except Exception as e:  # noqa: BLE001
        return False, f"dialogue/merchant.dialogue.json does not read as JSON: {e}"
    if not isinstance(script.get("nodes"), dict) or not script["nodes"]:
        return False, "dialogue/merchant.dialogue.json has no nodes"
    st = lambda: env.command("state", {})["state"]  # noqa: E731
    if state_key(st(), "potions") is None:
        return False, "no state \"potions\" exposed"
    seen = []
    for want_gold, want_potions in ((4, 1), (1, 2)):
        offered, shown = converse(env, True)
        if not shown:
            return False, "pressing talk beside the merchant shows no dialogue box"
        if not offered:
            return False, f"with {want_gold + 3} gold the merchant offers no potion"
        s = st()
        seen.append((state_key(s, "gold"), state_key(s, "potions")))
        if seen[-1] != (want_gold, want_potions):
            return False, f"after buying, gold and potions are {seen[-1]}, not ({want_gold}, {want_potions})"
    offered, shown = converse(env, True)
    if offered:
        return False, "with 1 gold the merchant still offers a potion"
    s = st()
    if (state_key(s, "gold"), state_key(s, "potions")) != (1, 2):
        return False, f"a talk without buying left gold and potions at ({state_key(s, 'gold')}, {state_key(s, 'potions')})"
    if not env.command("events.since", {"seq": 0, "type": "dialogue.choice"}):
        return False, "no dialogue.choice events: the talk did not go through the SDK's dialogue"
    return True, f"two potions bought, gold and potions {seen}, then none offered at 1 gold"


def jump_sound_check(env, answer):
    # A sound file under assets/, silent until the player jumps, heard right after, and again on the next jump.
    st = lambda: env.command("state", {})["state"]  # noqa: E731
    wavs = {f["path"] for f in env.command("assets.list", {}) if f.get("kind") == "audio"}
    if not wavs:
        return False, "no sound file under assets/"
    voices = lambda: [v["clip"] for v in env.command("audio.list", {}) if v["clip"] in wavs]  # noqa: E731

    def landed():
        for _ in range(240):
            if state_key(st(), "player.grounded"):
                return True
            env.command("step", {"ticks": 1})
        return False

    if not landed():
        return False, "the player never landed"
    env.command("step", {"ticks": 30})
    if voices():
        return False, "the sound plays before any jump"
    env.command("input.press", {"action": "jump"})
    env.command("step", {"ticks": 3})
    first = voices()
    if not first:
        return False, f"nothing from {sorted(wavs)} plays after a jump ({len(env.command('audio.list', {}))} voices)"
    if not landed():
        return False, "the player never landed after the jump"
    env.command("step", {"ticks": 30})
    env.command("input.press", {"action": "jump"})
    env.command("step", {"ticks": 3})
    again = voices()
    return bool(again), f"{first[0]} plays after a jump and again after the next"


def lamp_prefab_solve(env, project_dir):
    # A prefab file beside the script, instantiated three times on start.
    os.makedirs(os.path.join(project_dir, "prefabs"), exist_ok=True)
    lamp = {
        "format": "pocket-scene", "version": 1,
        "entities": [{
            "name": "Lamp",
            "components": {"Transform": {"position": {"x": 0, "y": 1, "z": 0}}, "MeshRenderer": {"mesh": "sphere", "color": {"r": 1, "g": 0.9, "b": 0.2, "a": 1}}},
            "children": [{"name": "Glow", "components": {"Transform": {}, "Light": {"kind": 1, "intensity": 2}}}],
        }],
    }
    with open(os.path.join(project_dir, "prefabs", "lamp.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(lamp, f, indent=2)

    def transform(t):
        cam = "    world.spawn(\"Camera\", { components: { Transform: { position: { x: 0, y: 2.5, z: 7 }, rotation: { x: -0.13, y: 0, z: 0, w: 0.99 } }, Camera: { fov_degrees: 50 } } });"
        if cam not in t:
            raise RuntimeError("the hello script changed shape")
        return t.replace(cam, cam + "\n    for (let i = 0; i < 3; i++) world.instantiate(\"prefabs/lamp.json\", { name: `Lamp${i}`, components: { Transform: { position: { x: -2 + 2 * i, y: 1, z: 0 } } } });", 1)
    edit_main(project_dir, transform)


def lamp_prefab_check(env, answer):
    # The file is a scene fragment, and three lamps stand where asked with their glow underneath.
    try:
        text = env.command("project.read", {"path": "prefabs/lamp.json"})["text"]
        doc = json.loads(text)
    except Exception as e:  # noqa: BLE001
        return False, f"prefabs/lamp.json unreadable: {e}"
    if doc.get("format") != "pocket-scene" or not isinstance(doc.get("entities"), list):
        return False, "prefabs/lamp.json is not a pocket-scene fragment"
    for i in range(3):
        lamp = env.command("world.find", {"path": f"Lamp{i}"})
        if not isinstance(lamp, int):
            return False, f"no entity named Lamp{i}"
        mr = env.command("world.get", {"entity": lamp, "component": "MeshRenderer"})
        if not mr or mr.get("mesh") != "sphere" or not color_is(mr["color"], 1, 0.9, 0.2):
            return False, f"Lamp{i} does not draw a yellow sphere"
        pos = env.command("world.get", {"entity": lamp, "component": "Transform"})["position"]
        if not (near(pos["x"], -2 + 2 * i) and near(pos["y"], 1) and near(pos["z"], 0)):
            return False, f"Lamp{i} at {pos}"
        kids = env.command("world.describe", {"entity": lamp}).get("children", [])
        glow = next((k for k in kids if k["name"] == "Glow"), None)
        if glow is None:
            return False, f"Lamp{i} has no child named Glow"
        light = env.command("world.get", {"entity": glow["id"], "component": "Light"})
        if not light or light.get("kind") != 1 or not near(light.get("intensity", 0), 2):
            return False, f"Lamp{i}'s Glow is not a point light of intensity 2"
    return True, "three lamps from the prefab, yellow spheres with a point light under each"


def night_lamp_solve(env):
    env.command("world.set", {"entity": "Sun", "component": "Light", "value": {"intensity": 0.05}})
    env.command("world.spawn", {"name": "Lamp", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 0}},
                "Light": {"kind": 1, "color": {"r": 1, "g": 0.7, "b": 0.4, "a": 1}, "intensity": 3, "range": 6, "shadows": True}}})
    env.command("step", {"ticks": 1})
    return env.command("render.stats", {})["light_shadows"]["lights"]


def night_lamp_check(env, answer):
    sun = env.command("world.get", {"entity": "Sun", "component": "Light"})
    if not sun or sun.get("intensity", 1) > 0.051:
        return False, f"the Sun's intensity is {sun and sun.get('intensity')}"
    lamp = env.command("world.find", {"path": "Lamp"})
    if not isinstance(lamp, int):
        return False, "no entity named Lamp"
    light = env.command("world.get", {"entity": lamp, "component": "Light"})
    if not light or light.get("kind") != 1 or not light.get("shadows"):
        return False, f"Lamp's light is {light}"
    if not (near(light.get("intensity", 0), 3) and near(light.get("range", 0), 6) and color_is(light["color"], 1, 0.7, 0.4)):
        return False, f"Lamp's light is {light}"
    pos = env.command("world.get", {"entity": lamp, "component": "Transform"})["position"]
    if not (near(pos["x"], 0) and near(pos["y"], 2) and near(pos["z"], 0)):
        return False, f"Lamp at {pos}"
    env.command("step", {"ticks": 1})
    shadowed = env.command("render.stats", {})["light_shadows"]["lights"]
    if answer != shadowed:
        return False, f"answered {answer!r}, {shadowed} lights cast shadows"
    return True, f"night: the sun at 0.05, a warm lamp at (0, 2, 0) casting shadows, answered {answer}"


def walker_sprint_solve(env, project_dir):
    # A new action in project.toml, and the speed it picks in the script.
    toml = os.path.join(project_dir, "project.toml")
    with open(toml, encoding="utf-8") as f:
        text = f.read()
    line = 'jump = ["Space", "pad:a"]\n'
    if line not in text:
        raise RuntimeError("the walker project.toml changed shape")
    with open(toml, "w", encoding="utf-8", newline="\n") as f:
        f.write(text.replace(line, line + 'sprint = ["LShift", "pad:left_shoulder"]\n', 1))

    def transform(t):
        a = 'const speed = crouching ? SPEED * 0.4 : SPEED;'
        if a not in t:
            raise RuntimeError("the walker script changed shape")
        return t.replace(a, 'const speed = crouching ? SPEED * 0.4 : input.down("sprint") ? 9 : SPEED;', 1)
    edit_main(project_dir, transform)


def fps_shotgun_solve(env, project_dir):
    # Six pellets a shot, each a hitscan turned a little off the view; a magazine of six.
    def transform(t):
        mag = "const MAG = 12;"
        one = "    const shot = combat.hitscan(from, dir, { damage: DAMAGE, knockback: 3, team: 1, shooter: player, range: 120 });"
        if mag not in t or one not in t:
            raise RuntimeError("the fps script changed shape")
        t = t.replace(mag, "const MAG = 6;", 1)
        pellets = """    let shot = null as ReturnType<typeof combat.hitscan>;
    for (let k = 0; k < 6; k++) {
        const a = random() * Math.PI * 2, r = Math.sqrt(random()) * 0.05;
        const off = forward(v.yaw + Math.cos(a) * r, v.pitch + Math.sin(a) * r);
        const hit = combat.hitscan(from, off, { damage: 8, knockback: 1, team: 1, shooter: player, range: 120 });
        if (hit && !shot) shot = hit;
    }"""
        t = t.replace(one, pellets, 1)
        return t.replace('import { Label, audio, combat,', 'import { Label, audio, combat, random,', 1)
    edit_main(project_dir, transform)


def fps_shotgun_check(env, answer):
    def held(name, at):
        env.command("world.set", {"entity": name, "component": "Behavior", "value": {"enabled": False}})
        env.command("world.set", {"entity": name, "component": "NavAgent", "value": {"mode": 0}})
        env.command("world.set", {"entity": name, "component": "Transform", "value": {"position": at}})
    def aim(at):
        eye = env.command("world.get", {"entity": "Player/Eye", "component": "WorldTransform"})["position"]
        dx, dy, dz = at["x"] - eye["x"], at["y"] - eye["y"], at["z"] - eye["z"]
        yaw, pitch = math.atan2(-dx, -dz), math.atan2(dy, math.hypot(dx, dz))
        env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"rotation": {"x": 0, "y": math.sin(yaw / 2), "z": 0, "w": math.cos(yaw / 2)}}})
        env.command("world.set", {"entity": "Player/Eye", "component": "Transform", "value": {"rotation": {"x": math.sin(pitch / 2), "y": 0, "z": 0, "w": math.cos(pitch / 2)}}})
    env.command("step", {"ticks": 3})
    p = entity_pos(env, "Player")
    target = {"x": p["x"], "y": 0, "z": p["z"] - 5}
    held("Raider0", target)
    env.command("step", {"ticks": 2})
    aim({"x": target["x"], "y": 1.15, "z": target["z"]})
    seq = env.command("events.last_seq", {})["seq"]
    env.command("input.press", {"action": "fire"})
    env.command("step", {"ticks": 2})
    hits = [e for e in env.command("events.since", {"seq": seq, "type": "hit", "limit": 50})["events"] if e.get("data", {}).get("to") == "/Raider0"]
    if len(hits) < 4:
        return False, f"one shot at a raider five ahead hit it {len(hits)} times, not with most of six pellets"
    if any(abs(float(h["data"].get("damage", 0)) - 8) > 1e-3 for h in hits):
        return False, f"pellets did {[h['data'].get('damage') for h in hits]} damage, not 8 each"
    points = {(round(h["data"]["point"]["x"], 3), round(h["data"]["point"]["y"], 3)) for h in hits if "point" in h.get("data", {})}
    if len(points) < 3:
        return False, "the pellets all hit the same place: no spread"
    # Every point within the spread: within 3 degrees of the aim at 5 units is under 0.27 off it.
    for h in hits:
        q = h["data"].get("point", {})
        if math.hypot(q.get("x", 0) - target["x"], q.get("y", 0) - 1.15) > 0.6:
            return False, f"a pellet landed at {q}, far outside a 3 degree spread"
    st = env.command("state", {})["state"]
    if state_key(st, "ammo") != 5:
        return False, f"after one shot the magazine has {state_key(st, 'ammo')}, not 5 of 6"
    # Six shots empty it.
    env.command("world.set", {"entity": "Raider0", "component": "Health", "value": {"current": 70, "dead": False}})
    env.command("step", {"ticks": 12})   # past the gap between shots
    for _ in range(5):
        env.command("input.press", {"action": "fire"})
        env.command("step", {"ticks": 12})
    st = env.command("state", {})["state"]
    if state_key(st, "ammo") not in (0, 6) and not state_key(st, "reloading"):
        return False, f"after six shots the state says ammo {state_key(st, 'ammo')}, reloading {state_key(st, 'reloading')}"
    return True, f"one shot is {len(hits)} pellet hits of 8 spread over {len(points)} points; six shots to a magazine"


def guards_footsteps_solve(env, project_dir):
    # Walking feet are heard too, closer: a noise that carries 4 every half second.
    def transform(t):
        a = """        events.emit("noise", { radius: 10 }, { subject: player });
        footfall = 1 / 3;
    }"""
        if a not in t:
            raise RuntimeError("the guards script changed shape")
        return t.replace(a, a + """ else if (!running && Math.hypot(vx, vz) > 0.1 && footfall === 0) {
        events.emit("noise", { radius: 4 }, { subject: player });
        footfall = 0.5;
    }""", 1)
    edit_main(project_dir, transform)


def farm_rain_solve(env, project_dir):
    # The weather answers the field: rain while three or more sown plots are dry, until none is.
    def transform(t):
        a = """    spell += dt;
    if (spell >= DRY_SPELL + RAIN_SPELL) spell -= DRY_SPELL + RAIN_SPELL;
    const into = spell - DRY_SPELL;
    const rain = into <= 0 ? 0 : Math.min(1, into / 4, (RAIN_SPELL - into) / 4);"""
        if a not in t or "let raining = false;" not in t:
            raise RuntimeError("the farm script changed shape")
        t = t.replace("let raining = false;", "let raining = false;\nlet rainOn = false;", 1)
        return t.replace(a, """    const dry = plots().filter((r) => r.Plot!.stage >= 0 && r.Plot!.water <= 0).length;
    if (dry >= 3) rainOn = true;
    else if (dry === 0) rainOn = false;
    const rain = rainOn ? 1 : 0;""", 1)
    edit_main(project_dir, transform)


def farm_rain_check(env, answer):
    def rain():
        return env.command("world.get", {"entity": "Weather", "component": "Weather"})["rain"]
    def sow(n, water):
        env.command("world.set", {"entity": f"Farm/Plot{n}", "component": "Plot", "value": {"stage": 0, "water": water, "growth": 0}})
    # Nothing sown: no rain for 110 seconds (the old timer rained after 75).
    for _ in range(22):
        env.command("step", {"ticks": 300})
        if rain() > 0.01:
            return False, f"it rains ({rain():.2f}) with nothing sown, at tick {env.command('state', {}).get('tick')}"
    # Two dry sown plots are not enough.
    sow(0, 0.0)
    sow(1, 0.0)
    env.command("step", {"ticks": 120})
    if rain() > 0.01:
        return False, f"it rains ({rain():.2f}) with only two sown plots dry"
    # A third: rain within two seconds.
    sow(2, 0.0)
    for _ in range(12):
        env.command("step", {"ticks": 10})
        if rain() >= 0.99:
            break
    else:
        return False, f"three sown plots are dry and the rain is {rain():.2f} two seconds later"
    # The rain waters them; once none is dry it stops.
    for _ in range(60):
        env.command("step", {"ticks": 30})
        if rain() <= 0.01:
            break
    else:
        return False, f"the rain is {rain():.2f} thirty seconds on"
    plots = [env.command("world.get", {"entity": f"Farm/Plot{n}", "component": "Plot"}) for n in range(3)]
    if any(p["water"] <= 0 for p in plots):
        return False, f"the rain stopped with a sown plot still dry: {plots}"
    return True, "no rain on a timer nor with two dry plots; rain within two seconds of the third, stopping once all three were watered"


def guards_footsteps_check(env, answer):
    def state(name):
        return env.command("world.get", {"entity": name, "component": "Behavior"})["state"]
    env.command("step", {"ticks": 5})
    # Standing still: no noise.
    seq = env.command("events.last_seq", {})["seq"]
    env.command("step", {"ticks": 60})
    if env.command("events.since", {"seq": seq, "type": "noise"})["events"]:
        return False, "the player standing still makes noise"
    # Walking (no sprint) far from everyone: noises that carry about 4.
    env.command("input.hold", {"action": "move_x", "ticks": 60, "value": 1})
    env.command("step", {"ticks": 60})
    walked = env.command("events.since", {"seq": seq, "type": "noise"})["events"]
    if len(walked) < 1:
        return False, "walking makes no noise"
    radii = {float(e.get("data", {}).get("radius", 0)) for e in walked}
    if not all(3 <= r <= 5 for r in radii):
        return False, f"walking noises carry {sorted(radii)}, not about 4"
    if not all(state(g) == "patrol" for g in ("Guard1", "Guard2", "Guard3")):
        return False, "a guard left its round while the player walked far away"
    # A guard three away hears the walk; one eight away does not.
    near = env.command("world.get", {"entity": "Guard2", "component": "Transform"})["position"]
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": near["x"], "y": 0.9, "z": near["z"] + 3}}})
    env.command("input.hold", {"action": "move_x", "ticks": 40, "value": 1})
    env.command("step", {"ticks": 40})
    if state("Guard2") == "patrol":
        return False, "a guard three units from the walking player did not hear it"
    return True, f"walking makes noises that carry {sorted(radii)} and a guard three away comes; standing makes none"


def knockout_solve(env, project_dir):
    # Health on the player at start; when it runs out, the hero goes limp as R makes it.
    def transform(t):
        start = "    seen = events.lastSeq();\n});"
        limp = "    if (limp) {\n        world.set(player, \"Character\""
        if start not in t or limp not in t:
            raise RuntimeError("the walker script changed shape")
        t = t.replace(start, "    seen = events.lastSeq();\n    if (player) world.set(player, \"Health\", { current: 30, max: 30 });\n});", 1)
        return t.replace(limp, "    if (!limp && hero && world.get(player, \"Health\")?.dead) {\n        limp = true;\n        world.set(hero, \"Ragdoll\", { active: true });\n    }\n" + limp, 1)
    edit_main(project_dir, transform)


def knockout_check(env, answer):
    health = env.command("world.get", {"entity": "Player", "component": "Health"})
    if not health or health.get("max") != 30 or health.get("current") != 30:
        return False, f"the Player's Health is {health}, not 30 of 30"
    at = entity_pos(env, "Player")
    env.command("world.spawn", {"name": "Spikes", "components": {"Transform": {"position": {"x": at["x"] + 1.5, "y": at["y"], "z": at["z"]}}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box", "size": {"x": 0.5, "y": 1, "z": 0.5}, "is_trigger": True}, "Hitbox": {"damage": 40}}})
    env.command("input.hold", {"action": "move_x", "ticks": 90})
    r = env.command("step", {"ticks": 90, "until": {"event": "health.depleted"}})
    if not r["until"]["met"]:
        return False, "walking into 40 damage of spikes did not use up the player's health"
    env.command("step", {"ticks": 150})
    rag = env.command("world.get", {"entity": "Player/Hero", "component": "Ragdoll"})
    if not rag or not rag.get("active") or rag.get("bodies", 0) < 5:
        return False, f"the hero is not limp after the player's health ran out: Ragdoll {rag}"
    head = next((j for j in env.command("animation.pose", {"entity": "Player/Hero"})["joints"] if j["name"] == "Head"), None)
    if head is None or head["position"]["y"] > 0.8:
        return False, f"the hero's head is at {head and head['position']['y']}, not on the ground"
    before = entity_pos(env, "Player")
    env.command("input.hold", {"action": "move_x", "ticks": 60})
    env.command("input.hold", {"action": "move_z", "ticks": 60})
    env.command("step", {"ticks": 60})
    after = entity_pos(env, "Player")
    moved = math.hypot(after["x"] - before["x"], after["z"] - before["z"])
    if moved > 0.3:
        return False, f"the move actions still move the player after it fell ({moved:.2f} units in a second)"
    return True, f"health used up, the hero limp on {rag['bodies']} bodies with its head at {head['position']['y']:.2f}, the player still ({moved:.2f})"


def walker_sprint_check(env, answer):
    actions = env.command("input.describe", {})
    if "sprint" not in actions or "LShift" not in (actions["sprint"].get("positive") or []):
        return False, f"no sprint action on LShift: {sorted(actions)}"
    st = lambda: env.command("state", {})["state"]  # noqa: E731
    for _ in range(60):
        if state_key(st(), "player.grounded"):
            break
        env.command("step", {"ticks": 1})

    def walk(sprint):
        x0 = state_key(st(), "player.x")
        env.command("input.hold", {"action": "move_x", "ticks": 60})
        if sprint:
            env.command("input.hold", {"action": "sprint", "ticks": 60})
        env.command("step", {"ticks": 60})
        x1 = state_key(st(), "player.x")
        env.command("step", {"ticks": 5})
        return x1 - x0
    plain = walk(False)
    fast = walk(True)
    ok = abs(plain - 5) < 0.35 and abs(fast - 9) < 0.35
    return ok, f"a second of walking covers {plain:.2f}, with sprint held {fast:.2f} (5 and 9 wanted)"


# ---- the terrain, scattering and vehicle tasks
HILL_AT = (20.0, -20.0)
HILL_FAR = [(HILL_AT[0] + 12, HILL_AT[1]), (HILL_AT[0] - 12, HILL_AT[1]), (HILL_AT[0], HILL_AT[1] + 12), (HILL_AT[0], HILL_AT[1] - 12)]


def hill_raise_before(env):
    h = env.command("terrain.height", {"x": HILL_AT[0], "z": HILL_AT[1]})["height"]
    env.far_heights = [env.command("terrain.height", {"x": x, "z": z})["height"] for x, z in HILL_FAR]
    return h < 9.5, f"the ground at {HILL_AT} stands {h:.2f} high"


def hill_raise_solve(env):
    h = 0.0
    for _ in range(40):
        r = env.command("terrain.sculpt", {"x": HILL_AT[0], "z": HILL_AT[1], "radius": 6, "amount": 1})
        h = r["height"]
        if h >= 10:
            break
    return env.command("terrain.height", {"x": HILL_AT[0], "z": HILL_AT[1]})["height"]


def hill_raise_check(env, answer):
    h = env.command("terrain.height", {"x": HILL_AT[0], "z": HILL_AT[1]})["height"]
    if h < 10:
        return False, f"the ground at {HILL_AT} stands {h:.2f} high"
    far = [env.command("terrain.height", {"x": x, "z": z})["height"] for x, z in HILL_FAR]
    moved = max(abs(a - b) for a, b in zip(far, env.far_heights))
    if moved > 0.01:
        return False, f"the ground 12 units away moved by {moved:.3f}"
    if not isinstance(answer, (int, float)) or abs(answer - h) > 0.05:
        return False, f"answered {answer!r}, the ground stands {h:.3f} high"
    return True, f"the ground at {HILL_AT} raised to {h:.2f}, nothing moved 12 units away"


# ---- the cloth task
def yard_flag_solve(env):
    env.command("world.spawn", {"name": "Pole", "components": {"Transform": {"position": {"x": -6, "y": 1.5, "z": 4}, "scale": {"x": 0.08, "y": 3, "z": 0.08}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.4, "g": 0.4, "b": 0.4, "a": 1}}}})
    env.command("world.spawn", {"name": "Flag", "components": {"Transform": {"position": {"x": -5.2, "y": 3, "z": 4}}, "MeshRenderer": {"mesh": "cube", "color": {"r": 0.9, "g": 0.1, "b": 0.1, "a": 1}},
                "Cloth": {"size": {"x": 1.6, "y": 1.0}, "segments": {"x": 16, "y": 10}, "pin": "left", "weight": 0.2}}})
    env.command("world.spawn", {"name": "Breeze", "components": {"Wind": {"direction": 0, "speed": 6}}})
    env.command("step", {"ticks": 180})
    c = env.command("physics.cloth", {"entity": "Flag"})
    return round(c["bounds"]["max"]["x"] - (-6), 2)


def yard_flag_check(env, answer):
    pole = entity_pos(env, "Pole")
    if pole is None or not near(pole["x"], -6, 0.1) or not near(pole["z"], 4, 0.1):
        return False, f"no entity Pole at x -6, z 4 ({pole})"
    flag = env.command("world.find", {"path": "Flag"})
    cloth = env.command("world.get", {"entity": "Flag", "component": "Cloth"}) if isinstance(flag, int) else None
    if not cloth:
        return False, "no entity Flag with a Cloth"
    if cloth.get("pin") != 2 or not near(cloth["size"]["x"], 1.6, 0.05) or not near(cloth["size"]["y"], 1.0, 0.05):
        return False, f"the Flag's Cloth is {cloth['size']} pinned by {cloth.get('pin')}, not 1.6 by 1 pinned along its left edge"
    color = (env.command("world.get", {"entity": "Flag", "component": "MeshRenderer"}) or {}).get("color", {})
    if not (color.get("r", 0) > 0.6 and color.get("g", 1) < 0.35 and color.get("b", 1) < 0.35):
        return False, f"the flag is not red ({color})"
    winds = [w for w in env.command("world.query", {"with": ["Wind"], "fields": ["Wind"]})["entities"]]
    blowing = [w for w in winds if (w.get("Wind") or {}).get("speed", 0) >= 5 and (w.get("Wind") or {}).get("enabled", True)]
    if not blowing:
        return False, f"no Wind of at least 5 units a second ({winds})"
    env.command("step", {"ticks": 180})
    c = env.command("physics.cloth", {"entity": "Flag", "points": True})
    across = c["across"]
    edge = [c["points"][j * across] for j in range(c["down"])]
    top = max(p[1] for p in edge)
    if any(abs(p[0] - pole["x"]) > 0.15 or abs(p[2] - pole["z"]) > 0.15 for p in edge):
        return False, f"the flag's pinned edge is not on the pole: {edge[0]} .. {edge[-1]}"
    pole_top = pole["y"] + 1.5
    if abs(top - pole_top) > 0.4:
        return False, f"the flag's top is at {top:.2f}, not at the pole's top ({pole_top:.2f})"
    reach = c["bounds"]["max"]["x"] - pole["x"]
    if reach < 1.2:
        return False, f"the flag reaches {reach:.2f} past the pole: it does not stream out downwind"
    if not isinstance(answer, (int, float)) or not 0.8 <= answer <= 1.7:
        return False, f"answered {answer}, not how far the flag reaches past the pole (about {reach:.2f})"
    return True, f"a red flag on the pole streams out {reach:.2f} in the wind (answered {answer})"


def flowers_solve(env):
    env.command("world.spawn", {"name": "Flowers", "components": {"Transform": {"scale": {"x": 0.3, "y": 0.3, "z": 0.3}}, "MeshRenderer": {"mesh": "sphere", "color": {"r": 0.9, "g": 0.1, "b": 0.15, "a": 1}},
                "Scatter": {"count": 400, "area": {"x": 94, "y": 94}, "on": "Hills", "max_slope": 20}}})


def flowers_check(env, answer):
    fid = env.command("world.find", {"path": "Flowers"})
    if not isinstance(fid, int):
        return False, "no entity named Flowers"
    mr = env.command("world.get", {"entity": fid, "component": "MeshRenderer"})
    if not mr or mr.get("mesh") != "sphere" or not color_is(mr["color"], 1, 0, 0.1):
        return False, f"Flowers draws {mr}"
    copies = env.command("scatter.copies", {"entity": fid, "limit": 20000})
    placed = copies["placed"]
    if placed < 50:
        return False, f"{placed} flowers stand"
    import math
    steepest = 0.0
    for c in copies["copies"]:
        g = env.command("terrain.height", {"x": c["x"], "z": c["z"]})
        if abs(g["height"] - c["y"]) > 0.3:
            return False, f"a flower at ({c['x']:.1f}, {c['z']:.1f}) stands at {c['y']:.2f} over ground at {g['height']:.2f}"
        steepest = max(steepest, math.degrees(math.acos(max(-1.0, min(1.0, g["normal"]["y"])))))
    if steepest > 20.5:
        return False, f"a flower stands on ground {steepest:.1f} degrees steep"
    return True, f"{placed} red flowers on the hills, none on ground steeper than {steepest:.1f} degrees"


def car_speed_solve(env):
    env.command("world.set", {"entity": "Car", "component": "Vehicle", "value": {"top_speed": 35, "power": 14}})
    env.command("input.hold", {"action": "throttle", "ticks": 300})
    env.command("step", {"ticks": 300})
    return env.command("world.get", {"entity": "Car", "component": "Vehicle"})["speed"]


def car_speed_check(env, answer):
    v = env.command("world.get", {"entity": "Car", "component": "Vehicle"})
    if not near(v.get("top_speed", 0), 35) or not near(v.get("power", 0), 14):
        return False, f"top_speed {v.get('top_speed')}, power {v.get('power')}"
    if not isinstance(answer, (int, float)):
        return False, f"answered {answer!r}"
    if answer < 26:
        return False, f"answered {answer:.2f}: not past the old top speed of 25"
    if abs(answer - v["speed"]) > 3:
        return False, f"answered {answer:.2f}, the car is going {v['speed']:.2f}"
    return True, f"top speed 35, power 14, driven to {answer:.2f}"


RAFT_AT = (-33, -22)


def raft_solve(env):
    level = env.command("water.height", {"x": RAFT_AT[0], "z": RAFT_AT[1]})["height"]
    # 2 by 0.2 by 1 is 0.4 cubic units: in water of density 2 it floats while lighter than 0.8.
    env.command("world.spawn", {"name": "Raft", "components": {
        "Transform": {"position": {"x": RAFT_AT[0], "y": level + 1, "z": RAFT_AT[1]}, "scale": {"x": 2, "y": 0.2, "z": 1}},
        "MeshRenderer": {"mesh": "cube", "color": {"r": 0.6, "g": 0.4, "b": 0.2, "a": 1}},
        "RigidBody": {"kind": 0, "mass": 0.3}, "Collider": {"shape": 0, "size": {"x": 1, "y": 0.1, "z": 0.5}}}})
    env.command("step", {"ticks": 180})
    p = env.command("world.get", {"entity": "Raft", "component": "Transform"})["position"]
    return env.command("water.height", {"x": p["x"], "z": p["z"]})["height"]


def raft_check(env, answer):
    rid = env.command("world.find", {"path": "Raft"})
    if not isinstance(rid, int):
        return False, "no entity named Raft"
    rb = env.command("world.get", {"entity": rid, "component": "RigidBody"}) or {}
    col = env.command("world.get", {"entity": rid, "component": "Collider"}) or {}
    if rb.get("kind") != 0:
        return False, f"the Raft's RigidBody is {rb}"
    size = col.get("size", {})
    if col.get("shape") != 0 or not (near(size.get("x", 0), 1, 0.05) and near(size.get("y", 0), 0.1, 0.02) and near(size.get("z", 0), 0.5, 0.05)):
        return False, f"the Raft's collider is {col}"
    env.command("step", {"ticks": 120})
    p = env.command("world.get", {"entity": rid, "component": "Transform"})["position"]
    s = env.command("water.height", {"x": p["x"], "z": p["z"]})
    if s.get("path") != "/Lake":
        return False, f"the raft drifted off the lake to ({p['x']:.1f}, {p['z']:.1f})"
    if abs(p["y"] - s["height"]) > 0.3:
        return False, f"the raft's centre is at {p['y']:.2f}, the surface at {s['height']:.2f}"
    if not isinstance(answer, (int, float)) or abs(answer - s["height"]) > 0.25:
        return False, f"answered {answer!r}, the surface is at {s['height']:.2f}"
    return True, f"the raft floats at {p['y']:.2f} on the surface at {s['height']:.2f}"


def calm_lake_solve(env):
    env.command("world.set", {"entity": "Lake", "component": "Water", "value": {"wave_height": 0, "clarity": 8}})
    env.command("world.set", {"entity": "Lake", "component": "Transform", "value": {"position": {"x": 0, "y": 3.7, "z": 0}}})
    env.command("step", {"ticks": 1})
    return env.command("water.height", {"x": 0, "z": 0})["height"]


def calm_lake_check(env, answer):
    wa = env.command("world.get", {"entity": "Lake", "component": "Water"}) or {}
    if wa.get("wave_height", 1) > 0.001:
        return False, f"the lake's waves are {wa.get('wave_height')} high"
    if not near(wa.get("clarity", 0), 8, 0.05):
        return False, f"clarity {wa.get('clarity')}"
    t = env.command("world.get", {"entity": "Lake", "component": "Transform"})["position"]
    if not near(t["y"], 3.7, 0.01) or not near(t["x"], 0, 0.01) or not near(t["z"], 0, 0.01):
        return False, f"the lake stands at {t}"
    h = env.command("water.height", {"x": 0, "z": 0})["height"]
    if not isinstance(answer, (int, float)) or abs(answer - h) > 0.01:
        return False, f"answered {answer!r}, the surface is at {h:.3f}"
    return True, f"a still, clear lake at {h:.2f}"


def dirt_share(env, x, z):
    for l in env.command("terrain.height", {"x": x, "z": z}).get("layers", []):
        if l["name"] == "dirt":
            return l["share"]
    return None


def dirt_patch_solve(env):
    info = env.command("world.get", {"entity": "Hills", "component": "Terrain"})
    layers = info["layers"]
    layers[1]["height"] = {"x": 0, "y": 0.2}
    env.command("world.set", {"entity": "Hills", "component": "Terrain", "value": {"layers": layers}})
    env.command("terrain.paint", {"x": 10, "z": -10, "layer": "dirt", "radius": 4, "amount": 1})
    return dirt_share(env, 10, -10)


def dirt_patch_check(env, answer):
    t = env.command("world.get", {"entity": "Hills", "component": "Terrain"}) or {}
    layers = t.get("layers", [])
    if [l.get("name") for l in layers] != ["grass", "sand", "rock", "dirt"]:
        return False, f"the layers are {[l.get('name') for l in layers]}"
    sand = layers[1]
    if not near(sand["height"]["y"], 0.2, 0.005) or not near(sand["height"]["x"], 0, 0.005):
        return False, f"sand lies at heights {sand['height']}"
    if not near(layers[2]["slope"]["x"], 32, 0.01) or layers[3].get("cover", 1) != 0 or not layers[0]["texture"].endswith("grass.png"):
        return False, f"the other layers changed: {layers}"
    here = dirt_share(env, 10, -10)
    if here is None or here < 0.9:
        return False, f"the dirt's share at x 10, z -10 is {here}"
    if not isinstance(answer, (int, float)) or abs(answer - here) > 0.01:
        return False, f"answered {answer!r}, the dirt's share there is {here:.3f}"
    for x, z in ((16, -10), (10, -16), (4, -10), (10, -4)):
        far = dirt_share(env, x, z)
        if far is None or far > 0.05:
            return False, f"dirt at ({x}, {z}), six units out: {far}"
    return True, f"sand below 0.2 of the height and a patch of dirt, {here:.2f} at its centre"


def glass_window_solve(env):
    env.command("world.spawn", {"name": "Window", "components": {"Transform": {"position": {"x": 0, "y": 1.5, "z": 3}, "scale": {"x": 2, "y": 1.5, "z": 0.05}},
                "MeshRenderer": {"mesh": "cube", "transmission": 1, "ior": 1.52}}})
    env.command("world.set", {"entity": "Ball", "component": "MeshRenderer", "value": {"clearcoat": 1, "clearcoat_roughness": 0.05}})
    env.command("step", {"ticks": 1})
    return env.command("render.stats", {})["glass"]


def glass_window_check(env, answer):
    w = env.command("world.find", {"path": "Window"})
    if not isinstance(w, int):
        return False, "no entity named Window"
    mr = env.command("world.get", {"entity": w, "component": "MeshRenderer"}) or {}
    if mr.get("mesh") != "cube" or mr.get("transmission", 0) < 0.99 or not near(mr.get("ior", 0), 1.52, 0.005):
        return False, f"the Window draws {mr}"
    t = env.command("world.get", {"entity": w, "component": "Transform"})
    if not (near(t["position"]["x"], 0) and near(t["position"]["y"], 1.5) and near(t["position"]["z"], 3) and near(t["scale"]["x"], 2) and near(t["scale"]["y"], 1.5) and near(t["scale"]["z"], 0.05)):
        return False, f"the Window stands at {t['position']} scaled {t['scale']}"
    ball = env.command("world.get", {"entity": "Ball", "component": "MeshRenderer"}) or {}
    if ball.get("clearcoat", 0) < 0.99 or not near(ball.get("clearcoat_roughness", -1), 0.05, 0.005):
        return False, f"the Ball's coat is {ball.get('clearcoat')} at {ball.get('clearcoat_roughness')}"
    env.command("step", {"ticks": 1})
    glass = env.command("render.stats", {})["glass"]
    if answer != glass or glass != 1:
        return False, f"answered {answer!r}, the renderer drew {glass} glass"
    return True, "a window of glass (ior 1.52) and a lacquered ball; one glass drawn"


PERSIAN = {
    "hud": {"score": "امتیاز {score, number}", "health": "سلامت {health}", "coins": "{count, plural, =0 {هنوز سکه‌ای نیست} one {# سکه} other {# سکه}}",
            "hint": "جعبه را با دکمه‌ها بچرخانید یا با WASD راه بروید.", "key": "کلید {key}"},
    "controls": {"spin_left": "چرخش به چپ", "spin_right": "چرخش به راست", "score_up": "امتیاز +10", "hurt": "آسیب", "heal": "درمان", "pause": "مکث", "resume": "ادامه"},
    "menu": {"title": "متوقف شد", "name": "نام شما", "name_placeholder": "نام", "volume": "صدا", "difficulty": "سختی", "easy": "آسان", "normal": "معمولی",
             "hard": "سخت", "show_bar": "نمایش نوار سلامت", "language": "زبان", "resume": "ادامه"},
    "language": {"en": "English", "zh": "中文", "ar": "العربية", "fa": "فارسی"},
}


def persian_locale_solve(env, project_dir):
    locales = os.path.join(project_dir, "locales")
    with open(os.path.join(locales, "fa.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(PERSIAN, f, ensure_ascii=False, indent=2)
    for lang in ("en", "zh", "ar"):
        path = os.path.join(locales, lang + ".json")
        with open(path, encoding="utf-8") as f:
            doc = json.load(f)
        doc["language"]["fa"] = "فارسی"
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            json.dump(doc, f, ensure_ascii=False, indent=2)


def persian_locale_check(env, answer):
    check = env.command("locale.check", {})
    fa = check.get("languages", {}).get("fa")
    if fa is None:
        return False, f"no Persian file: {sorted(check.get('languages', {}))}"
    if not check.get("complete"):
        return False, f"not complete: {json.dumps(check, ensure_ascii=False)[:300]}"
    env.command("locale.set", {"lang": "fa"})
    env.command("step", {"ticks": 2})
    texts = [n for n in env.command("ui.query", {"name": "score"}) if n.get("text")]
    if not texts:
        return False, "no score text in the interface"
    score = texts[0]["text"]
    if not any("\u0600" <= ch <= "\u06ff" for ch in score) or not any(ch.isdigit() for ch in score):
        return False, f"the score reads {score!r} in Persian"
    table = env.command("locale.table", {"lang": "fa"})["strings"]
    if not all(any("\u0600" <= ch <= "\u06ff" for ch in table[k]) for k in ("controls.pause", "menu.title", "hud.health")):
        return False, "the Persian texts are not in Persian"
    # Persian, not the Arabic file copied: letters only Persian has, and texts of its own.
    if not any(ch in "\u067e\u0686\u0698\u06a9\u06af\u06cc" for text in table.values() for ch in text):
        return False, "the Persian file has none of the letters Persian writes (پ چ ژ ک گ ی)"
    arabic = env.command("locale.table", {"lang": "ar"})["strings"]
    same = [k for k in table if k in arabic and table[k] == arabic[k] and not k.startswith("language.")]
    if len(same) > len(table) // 2:
        return False, f"{len(same)} of {len(table)} Persian texts are the Arabic ones"
    for lang in ("en", "zh", "ar", "fa"):
        name = env.command("locale.table", {"lang": lang})["strings"].get("language.fa", "")
        if not any("\u0600" <= ch <= "\u06ff" for ch in name):
            return False, f"{lang}.json names Persian {name!r}"
    return True, f"Persian complete, named in every language; the score reads {score}"


# Blender for the blender_level task and its reference solution: POCKET_BLENDER, as the engine reads it
# (engine/assets/src/import.cpp), else where macOS installs it.
BLENDER = os.environ.get("POCKET_BLENDER") or "/Applications/Blender.app/Contents/MacOS/Blender"


def blender_level_solve(env, project_dir):
    script = os.path.join(project_dir, "make_level.py")
    with open(script, "w", encoding="utf-8", newline="\n") as f:
        f.write("""import bpy, sys
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.mesh.primitive_plane_add(size=10, location=(0, 0, 1))
floor = bpy.context.active_object
floor.name = 'Floor'
floor['pocket'] = '{"RigidBody": {"kind": 1}, "Collider": {"shape": 3}}'
bpy.ops.mesh.primitive_cube_add(size=1, location=(3, 0, 2.5))
pillar = bpy.context.active_object
pillar.name = 'Pillar'
pillar.scale = (1, 1, 3)
pillar['pocket.RigidBody'] = '{"kind": 1}'
pillar['pocket.Collider'] = '{"shape": 0, "size": {"x": 0.5, "y": 1.5, "z": 0.5}}'
pillar['pocket.Health'] = '{"max": 50, "current": 50}'
bpy.ops.wm.save_as_mainfile(filepath=sys.argv[-1])
""")
    subprocess.run([BLENDER, "-b", "--factory-startup", "--python", script, "--", os.path.join(project_dir, "assets", "level.blend")], capture_output=True, check=True, timeout=300)
    os.remove(script)
    return drop_on_level(env)


def drop_on_level(env):
    env.command("world.instantiate", {"mesh": "assets/level.blend", "position": {"x": 60, "y": 0, "z": 0}, "name": "Level"})
    env.command("world.spawn", {"name": "Probe", "components": {"Transform": {"position": {"x": 58, "y": 8, "z": 1}}, "MeshRenderer": {"mesh": "sphere"},
                "RigidBody": {"kind": 0, "mass": 1}, "Collider": {"shape": 1, "size": {"x": 0.5, "y": 0.5, "z": 0.5}}}})
    env.command("step", {"ticks": 180})
    return env.command("world.get", {"entity": "Probe", "component": "Transform"})["position"]["y"]


def blender_level_check(env, answer):
    d = env.command("assets.describe", {"path": "assets/level.blend"})
    props = d.get("properties") or {}
    if not {"Floor", "Pillar"} <= set(props):
        return False, f"assets/level.blend has custom properties on {sorted(props)}"
    # A fresh level where the check puts it: the components from the properties, a ball on the floor.
    for name in ("Level", "Probe"):
        found = env.command("world.find", {"path": name})
        if isinstance(found, int):
            env.command("world.destroy", {"entity": found})
    inst = env.command("world.instantiate", {"mesh": "assets/level.blend", "position": {"x": -60, "y": 0, "z": 0}, "name": "Checked"})
    root = inst["roots"][0]
    kids = {env.command("world.describe", {"entity": c})["name"]: c for c in env.command("world.children", {"entity": root})}
    if "Floor" not in kids or "Pillar" not in kids:
        return False, f"the level's objects are {sorted(kids)}"
    rb = env.command("world.get", {"entity": kids["Floor"], "component": "RigidBody"}) or {}
    col = env.command("world.get", {"entity": kids["Floor"], "component": "Collider"}) or {}
    if rb.get("kind") != 1 or col.get("shape") != 3:
        return False, f"the Floor is RigidBody {rb} Collider {col}"
    hp = env.command("world.get", {"entity": kids["Pillar"], "component": "Health"}) or {}
    prb = env.command("world.get", {"entity": kids["Pillar"], "component": "RigidBody"}) or {}
    pcol = env.command("world.get", {"entity": kids["Pillar"], "component": "Collider"}) or {}
    size = pcol.get("size", {})
    if not near(hp.get("max", 0), 50) or not near(hp.get("current", 0), 50) or prb.get("kind") != 1 or pcol.get("shape") != 0 \
            or not all(near(size.get(k, 0), v) for k, v in (("x", 0.5), ("y", 1.5), ("z", 0.5))):
        return False, f"the Pillar has Health {hp}, RigidBody {prb} and Collider {pcol}"
    # One ball near the middle and one near a corner, 10 across: both rest on the floor.
    balls = {"CheckBall": (-62, 1), "CornerBall": (-64.5, -4.5)}
    for name, (x, z) in balls.items():
        env.command("world.spawn", {"name": name, "components": {"Transform": {"position": {"x": x, "y": 8, "z": z}}, "MeshRenderer": {"mesh": "sphere"},
                    "RigidBody": {"kind": 0, "mass": 1}, "Collider": {"shape": 1, "size": {"x": 0.5, "y": 0.5, "z": 0.5}}}})
    env.command("step", {"ticks": 180})
    for name, (x, z) in balls.items():
        y = env.command("world.get", {"entity": name, "component": "Transform"})["position"]["y"]
        if not near(y, 1.5, 0.05):
            return False, f"a ball dropped on the Floor at x {x}, z {z} rests at {y:.3f}, not 1.5"
    if not isinstance(answer, (int, float)) or not near(answer, 1.5, 0.05):
        return False, f"answered {answer!r}, a ball rests at 1.5 on the floor"
    return True, f"a level from Blender: a static floor it collides with at 1.5, a pillar with 50 health"


SAND = (0.85, 0.78, 0.55)
ROAD_FAR = [(x, z) for x in range(-28, 29, 4) for z in (-6.5, 6.5)]


def sand_road_before(env):
    env.far_paint = [env.command("terrain.height", {"x": x, "z": z}).get("paint", {}).get("a", 0.0) for x, z in ROAD_FAR]
    return True, ""


def sand_road_solve(env):
    points = [{"x": x, "z": 0} for x in range(-30, 31, 5)]
    env.command("terrain.paint", {"points": points, "color": {"r": SAND[0], "g": SAND[1], "b": SAND[2]}, "radius": 2.5, "amount": 1})
    return env.command("terrain.height", {"x": 0, "z": 0})["paint"]["a"]


def sand_road_check(env, answer):
    for x in range(-28, 29, 2):
        for z in (-1, 0, 1):
            p = env.command("terrain.height", {"x": x, "z": z}).get("paint")
            if not p or p["a"] < 0.8:
                return False, f"the ground at x {x}, z {z} is covered {p['a'] if p else 0:.2f}"
            if abs(p["r"] - SAND[0]) > 0.08 or abs(p["g"] - SAND[1]) > 0.08 or abs(p["b"] - SAND[2]) > 0.08:
                return False, f"the paint at x {x}, z {z} is {p}"
    for (x, z), was in zip(ROAD_FAR, env.far_paint):
        a = env.command("terrain.height", {"x": x, "z": z}).get("paint", {}).get("a", 0.0)
        if abs(a - was) > 0.02:
            return False, f"the ground at x {x}, z {z}, 6.5 from the road, changed its paint from {was:.2f} to {a:.2f}"
    a = env.command("terrain.height", {"x": 0, "z": 0})["paint"]["a"]
    if not isinstance(answer, (int, float)) or abs(answer - a) > 0.01:
        return False, f"answered {answer!r}, the coverage at x 0, z 0 is {a:.3f}"
    return True, "a sand road from x -30 to 30 along z 0, the ground beside it as it was"


def reed_field_solve(env):
    env.command("world.spawn", {"name": "Reeds", "components": {
        "Transform": {"position": {"x": 0, "y": 20, "z": 10}, "scale": {"x": 0.1, "y": 1.5, "z": 0.1}},
        "MeshRenderer": {"mesh": "cube", "color": {"r": 0.8, "g": 0.75, "b": 0.4, "a": 1}},
        "Scatter": {"count": 400, "area": {"x": 20, "y": 20}, "on": "Hills", "scale": {"x": 1, "y": 1}, "sway": 0.3, "collide": 0.1}}})
    env.command("step", {"ticks": 1})
    return env.command("scatter.copies", {"entity": "Reeds"})["placed"]


def reed_field_check(env, answer):
    rid = env.command("world.find", {"path": "Reeds"})
    if not isinstance(rid, int):
        return False, "no entity named Reeds"
    sc = env.command("world.get", {"entity": rid, "component": "Scatter"}) or {}
    if abs(sc.get("sway", 0) - 0.3) > 0.01:
        return False, f"the reeds sway {sc.get('sway')}"
    env.command("step", {"ticks": 1})
    cp = env.command("scatter.copies", {"entity": rid, "limit": 20000})
    if cp["placed"] < 100:
        return False, f"{cp['placed']} reeds stand"
    for c in cp["copies"]:
        if abs(c["x"]) > 10.01 or abs(c["z"] - 10) > 10.01:
            return False, f"a reed stands at ({c['x']:.1f}, {c['z']:.1f}), outside the 20 by 20 area around x 0, z 10"
    # Each a collider (a ray down onto a reed meets the Reeds above the ground), 0.1 in radius: the
    # Scatter's collide (world units) times the copy's own size.
    c = cp["copies"][0]
    hit = env.command("physics.raycast", {"origin": {"x": c["x"], "y": c["y"] + 30, "z": c["z"]}, "direction": {"x": 0, "y": -1, "z": 0}})
    if not hit or hit.get("entity") != rid or hit["point"]["y"] < c["y"] + 0.5:
        return False, f"a ray down onto the first reed meets {hit}"
    scale = (env.command("world.get", {"entity": rid, "component": "Transform"}) or {}).get("scale", {})
    for c in cp["copies"]:
        size = c["size"] / max(abs(scale.get("y", 1)), 1e-6)   # scatter.copies' size is the copy's height scale
        r = sc.get("collide", 0) * size
        if abs(r - 0.1) > 0.005:
            return False, f"a reed's collider is {r:.3f} in radius (collide {sc.get('collide')}, the copy's size {size:.2f})"
    if answer != cp["placed"]:
        return False, f"answered {answer!r}, {cp['placed']} reeds stand"
    return True, f"{cp['placed']} reeds that sway 0.3 and stop what runs into them"


def stormy_dusk_solve(env):
    import math
    env.command("world.set", {"entity": "Sky", "component": "Sky", "value": {"mode": 3, "clouds": 0.8}})
    # The sun 4 degrees up in the west (-x): its light shines toward +x and down.
    e = math.radians(4)
    to_sun = (-math.cos(e), math.sin(e), 0.0)
    d = (-to_sun[0], -to_sun[1], -to_sun[2])
    yaw = math.atan2(-d[0], -d[2]) / 2
    pitch = math.atan2(d[1], math.hypot(d[0], d[2])) / 2
    rot = {"x": math.cos(yaw) * math.sin(pitch), "y": math.sin(yaw) * math.cos(pitch), "z": -math.sin(yaw) * math.sin(pitch), "w": math.cos(yaw) * math.cos(pitch)}
    env.command("world.set", {"entity": "Sun", "component": "Transform", "value": {"rotation": rot}})
    env.command("world.set", {"entity": "Breeze", "component": "Wind", "value": {"direction": 0, "speed": 12}})
    env.command("step", {"ticks": 1})
    return env.command("render.stats", {})["sun_light"]["r"]


def stormy_dusk_check(env, answer):
    sky = env.command("world.get", {"entity": "Sky", "component": "Sky"}) or {}
    if sky.get("mode") != 3:
        return False, f"the sky is in mode {sky.get('mode')}"
    if abs(sky.get("clouds", 0) - 0.8) > 0.01:
        return False, f"clouds {sky.get('clouds')}"
    air = env.command("wind.at", {"x": 0, "z": 0})
    if not air.get("entity") or abs(air.get("direction", {}).get("x", 0) - 1) > 0.02 or abs(air["direction"].get("z", 1)) > 0.02:
        return False, f"the wind is {air}"
    first = env.command("world.get", {"entity": air["entity"], "component": "Wind"})
    if abs(first.get("speed", 0) - 12) > 0.01:
        return False, f"the wind blows at {first.get('speed')}"
    env.command("step", {"ticks": 1})
    st = env.command("render.stats", {})
    light = st["sun_light"]
    # A sun 4 degrees up: reddened (more than twice as red as blue) and dimmed below its own intensity.
    sun = env.command("world.get", {"entity": "Sun", "component": "Light"})
    if not (light["r"] > 2 * light["b"] and light["r"] < sun["intensity"] * 0.9):
        return False, f"the sun light reaching the ground is {light}"
    tr = env.command("world.get", {"entity": "Sun", "component": "Transform"})["rotation"]
    x, y, z, w = tr["x"], tr["y"], tr["z"], tr["w"]
    # -Z turned by the rotation: where the light shines.
    fx = -(2 * (x * z + w * y)); fy = -(2 * (y * z - w * x)); fz = -(1 - 2 * (x * x + y * y))
    import math
    elev = math.degrees(math.asin(max(-1, min(1, -fy))))
    if abs(elev - 4) > 0.5 or fx < 0.9:
        return False, f"the sun stands {elev:.1f} degrees up, its light shining toward ({fx:.2f}, {fy:.2f}, {fz:.2f})"
    if not isinstance(answer, (int, float)) or abs(answer - light["r"]) > 0.01:
        return False, f"answered {answer!r}, the sun light's red is {light['r']:.3f}"
    return True, f"a cloudy sunset from the west, the sun light {light['r']:.2f}, {light['g']:.2f}, {light['b']:.2f}, a wind of 12 toward +x"


def snowy_evening_solve(env):
    env.command("world.spawn", {"name": "Weather", "components": {"Weather": {"snow": 0.7, "cover": 1}}})
    env.command("world.set", {"entity": "Sky", "component": "Sky", "value": {"time_of_day": 17.5, "day_length": 600}})
    env.command("step", {"ticks": 600})
    return env.command("world.get", {"entity": "Sky", "component": "Sky"})["time_of_day"]


def snowy_evening_check(env, answer):
    # The first enabled Weather by id is the world's.
    found = env.command("world.query", {"with": ["Weather"], "fields": ["Weather"]}).get("entities", [])
    weathers = sorted((e for e in found if e["Weather"].get("enabled", True)), key=lambda e: e["id"])
    if not weathers:
        return False, "no Weather in the world"
    w = weathers[0]["Weather"]
    if abs(w.get("snow", 0) - 0.7) > 0.01 or w.get("rain", 0) > 0.01:
        return False, f"the weather is {w}"
    if w.get("cover", 0) < 0.95:
        return False, f"the snow lies {w.get('cover')} deep, not wholly white"
    skies = env.command("world.query", {"with": ["Sky"], "fields": ["Sky"]}).get("entities", [])
    sky = next((e["Sky"] for e in sorted(skies, key=lambda e: e["id"]) if e["Sky"].get("enabled", True)), None)
    if not sky or abs(sky.get("day_length", 0) - 600) > 0.5:
        return False, f"the sky's day is {sky and sky.get('day_length')} seconds long"
    # Started at 17.5 and run ten seconds of a ten-minute day: 0.4 of an hour on.
    if not isinstance(answer, (int, float)) or abs(answer - 17.9) > 0.06:
        return False, f"answered {answer!r}; ten seconds from 17.5 in a ten-minute day is 17.9"
    if sky["time_of_day"] < answer - 0.01:
        return False, f"the sky says {sky['time_of_day']:.2f}, before the answer {answer}"
    env.command("step", {"ticks": 1, "render": "each"})
    st = env.command("render.stats", {})
    if st.get("weather_drops", 0) != 4900:
        return False, f"{st.get('weather_drops')} flakes drawn, not 4900 (0.7 of 7000)"
    return True, f"snow at 0.7 on ground wholly white, the day running from 17:30 (answered {answer:.2f}), {st['weather_drops']} flakes drawn"


def sea_around_solve(env):
    env.command("world.set", {"entity": "Lake", "component": "Water", "value": {"ocean": True}})
    env.command("world.set", {"entity": "Sky", "component": "Sky", "value": {"clouds": 0.5}})
    env.command("step", {"ticks": 1})
    return env.command("water.height", {"x": 400, "z": 0})["level"]


def sea_around_check(env, answer):
    h = env.command("water.height", {"x": 400, "z": 0})
    if not h.get("inside"):
        return False, "no water 400 units east of the origin"
    if abs(h.get("level", 0) - 3.2) > 0.01:
        return False, f"the water there rests at {h.get('level')}, not the lake's 3.2"
    if env.command("water.height", {"x": -900, "z": 1500}).get("inside") is not True:
        return False, "no water far to the north-west: not a sea every way"
    if not isinstance(answer, (int, float)) or abs(answer - 3.2) > 0.05:
        return False, f"answered {answer!r}, the sea rests at 3.2"
    skies = env.command("world.query", {"with": ["Sky"], "fields": ["Sky"]}).get("entities", [])
    sky = next((e["Sky"] for e in sorted(skies, key=lambda e: e["id"]) if e["Sky"].get("enabled", True)), None)
    if not sky or not 0.35 <= sky.get("clouds", 0) <= 0.65:
        return False, f"the sky's clouds cover {sky and sky.get('clouds')}, not about half"
    env.command("step", {"ticks": 1, "render": "each"})
    st = env.command("render.stats", {})
    if not st.get("clouds"):
        return False, "the clouds are not marched as a volume (render.stats.clouds false)"
    return True, f"a sea every way at 3.2 (answered {answer}), clouds over {sky['clouds']} of the sky, volumetric"


def meadow_solve(env):
    env.command("world.set", {"entity": "Hills", "component": "Grass", "value": {"layer": "grass", "min_height": 3.5, "reach": 30}})
    env.command("step", {"ticks": 1, "render": "each"})
    return None


def meadow_check(env, answer):
    g = env.command("world.get", {"entity": "Hills", "component": "Grass"})
    if not isinstance(g, dict) or not g.get("enabled", True):
        return False, "no Grass on the terrain Hills"
    if g.get("layer", "") not in ("grass", ""):
        return False, f"the grass grows on the layer {g.get('layer')!r}, not the grass layer"
    if not 3.0 <= g.get("min_height", -1000) <= 4.0:
        return False, f"the grass grows from {g.get('min_height')} up, not from about 3.5"
    if abs(g.get("reach", 0) - 30) > 1:
        return False, f"drawn out to {g.get('reach')}, not 30"
    env.command("step", {"ticks": 1, "render": "each"})
    st = env.command("render.stats", {})
    if st.get("grass_blades", 0) <= 0:
        return False, "no blades drawn"
    return True, f"grass on the grass layer from {g['min_height']} up, out to {g['reach']}, {st['grass_blades']} cells round the camera"


def sailboat_solve(env):
    env.command("world.destroy", {"entity": "Ground"})
    env.command("world.spawn", {"name": "Sea", "components": {"Transform": {}, "Water": {"ocean": True, "depth": 20}}})
    env.command("world.spawn", {"name": "Breeze", "components": {"Wind": {"direction": 0, "speed": 6}}})
    env.command("world.spawn", {"name": "Sloop", "components": {
        "Transform": {"position": {"x": 0, "y": 0.3, "z": 0}, "rotation": {"yaw": -90}},
        "MeshRenderer": {"mesh": "boat"},
        "RigidBody": {"kind": "dynamic", "mass": 0.9},
        "Collider": {"shape": "box", "size": {"x": 0.32, "y": 0.3, "z": 1.6}, "offset": {"x": 0, "y": 0.3, "z": 0}},
        "Boat": {"sail": 1}}})
    env.command("step", {"ticks": 1})
    return None


def sailboat_check(env, answer):
    b = env.command("world.get", {"entity": "Sloop", "component": "Boat"})
    if not isinstance(b, dict):
        return False, "no Boat named Sloop"
    if abs(b.get("throttle", 0)) > 1e-6:
        return False, f"its throttle is {b.get('throttle')}: the wind alone should drive it"
    wind = env.command("wind.at", {"x": 0, "z": 0})
    if not wind.get("on", True) or wind.get("speed", 0) < 5.5:
        return False, f"the wind is {wind}"
    p0 = env.command("world.get", {"entity": "Sloop", "component": "Transform"})["position"]
    env.command("step", {"ticks": 300})
    p1 = env.command("world.get", {"entity": "Sloop", "component": "Transform"})["position"]
    b = env.command("world.get", {"entity": "Sloop", "component": "Boat"})
    if not b.get("afloat"):
        return False, f"the boat is not afloat (at y {p1['y']:.2f})"
    if p1["x"] - p0["x"] < 5:
        return False, f"five seconds sailing took it {p1['x'] - p0['x']:.1f} along +x, not 5 or more"
    return True, f"the Sloop sails before the wind: {p1['x'] - p0['x']:.1f} units along +x in five seconds, afloat"


def spike_trap_solve(env):
    at = env.command("world.get", {"entity": "Player", "component": "Transform"})["position"]
    env.command("world.set", {"entity": "Player", "component": "Health", "value": {"current": 60, "max": 60, "invulnerable": 0.5}})
    env.command("world.spawn", {"name": "Spikes", "components": {"Transform": {"position": at}, "Area2D": {"size": {"x": 1, "y": 0.5}},
                "Hitbox": {"damage": 15, "repeat": 0.5}}})
    r = env.command("step", {"ticks": 1200, "until": {"event": "health.depleted"}})
    return r["until"]["event"]["tick"]


def spike_trap_check(env, answer):
    spikes = env.command("world.find", {"path": "Spikes"})
    if not isinstance(spikes, int):
        return False, "no entity named Spikes"
    area = env.command("world.get", {"entity": spikes, "component": "Area2D"})
    box = env.command("world.get", {"entity": spikes, "component": "Hitbox"})
    if not area or not (near(area["size"]["x"], 1) and near(area["size"]["y"], 0.5)):
        return False, f"Spikes' Area2D is {area}"
    if not box or not (near(box["damage"], 15) and near(box["repeat"], 0.5)):
        return False, f"Spikes' Hitbox is {box}"
    hp = env.command("world.get", {"entity": "Player", "component": "Health"})
    if not hp or not near(hp["max"], 60) or not near(hp["invulnerable"], 0.5) or not hp["dead"]:
        return False, f"the Player's Health is {hp}"
    player = env.command("world.find", {"path": "Player"})
    depleted = [e for e in env.command("events.since", {"seq": 0, "type": "health.depleted", "limit": 4000})["events"] if e.get("subject") == player]
    if not depleted:
        return False, "no health.depleted event for the Player"
    hits = [e for e in env.command("events.since", {"seq": 0, "type": "hit", "limit": 4000})["events"] if e.get("subject") == player]
    if len(hits) != 4:
        return False, f"{len(hits)} hits on the Player, not the four that take 60 at 15 each"
    if answer != depleted[0]["tick"]:
        return False, f"answered {answer!r}, the health was depleted at tick {depleted[0]['tick']}"
    return True, f"spikes took 60 in four hits; depleted at tick {answer}"


def roof_mesh_solve(env):
    pts = [[-1, 0, -1], [1, 0, -1], [1, 0, 1], [-1, 0, 1], [0, 1, 0]]
    made = env.command("mesh.create", {"name": "roof", "positions": pts, "indices": [0, 4, 1, 1, 4, 2, 2, 4, 3, 3, 4, 0]})
    env.command("world.spawn", {"name": "Roof", "components": {"Transform": {"position": {"x": 0, "y": 2, "z": 0}}, "MeshRenderer": {"mesh": "mesh:roof", "color": {"r": 1, "g": 0.5, "b": 0.1, "a": 1}}}})
    return made["triangles"]


def roof_mesh_check(env, answer):
    meshes = [m["mesh"] for m in env.command("mesh.list", {})["meshes"]]
    if "mesh:roof" not in meshes:
        return False, f"no mesh named roof (made: {meshes})"
    roof = env.command("world.find", {"path": "Roof"})
    if not isinstance(roof, int):
        return False, "no entity named Roof"
    mr = env.command("world.get", {"entity": roof, "component": "MeshRenderer"})
    if mr.get("mesh") != "mesh:roof":
        return False, f"Roof draws {mr.get('mesh')!r}"
    c = mr["color"]
    if not (c["r"] > 0.8 and 0.25 < c["g"] < 0.75 and c["b"] < 0.35):
        return False, f"Roof's color is {c}, not orange"
    env.command("step", {"ticks": 1})
    b = env.command("world.get", {"entity": roof, "component": "Bounds"})
    lo, hi = b["min"], b["max"]
    if not (near(lo["y"], 2, 0.02) and near(hi["y"], 3, 0.02) and near(lo["x"], -1, 0.02) and near(hi["x"], 1, 0.02) and near(lo["z"], -1, 0.02) and near(hi["z"], 1, 0.02)):
        return False, f"Roof spans {lo} to {hi}, not a 2 by 2 base at y 2 rising to 3"
    if not isinstance(answer, int) or answer < 4 or answer > 6:
        return False, f"answered {answer!r} triangles"
    return True, f"a {answer}-triangle roof spanning y 2..3"


def walled_room_solve(env):
    env.command("tilemap.create", {"name": "maps/room.tmj", "width": 10, "height": 8, "tile_width": 16, "layers": [{"name": "walls", "solid": True}], "tilesets": [{"image": "assets/tiles.png"}]})
    env.command("world.spawn", {"name": "Room", "components": {"Transform": {"position": {"x": 30, "y": 0, "z": 0}}, "TileMap": {"map": "maps/room.tmj"}}})
    for args in ({"tile_x": 0, "tile_y": 0, "width": 10, "height": 1}, {"tile_x": 0, "tile_y": 7, "width": 10, "height": 1}, {"tile_x": 0, "tile_y": 0, "width": 1, "height": 8}, {"tile_x": 9, "tile_y": 0, "width": 1, "height": 8}):
        env.command("tilemap.fill", {"entity": "Room", "layer": "walls", "id": 0, **args})
    env.command("world.spawn", {"name": "Pebble", "components": {"Transform": {"position": {"x": 35, "y": -2, "z": 0}}, "RigidBody2D": {}, "Collider2D": {"shape": "circle", "radius": 0.25}}})
    env.command("step", {"ticks": 120})
    return env.command("world.get", {"entity": "Pebble", "component": "Transform"})["position"]["y"]


def walled_room_check(env, answer):
    room = env.command("world.find", {"path": "Room"})
    if not isinstance(room, int):
        return False, "no entity named Room"
    tm = env.command("world.get", {"entity": room, "component": "TileMap"})
    if tm.get("map") != "maps/room.tmj":
        return False, f"Room draws {tm.get('map')!r}"
    info = env.command("tilemap.info", {"entity": room})
    if info.get("width") != 10 or info.get("height") != 8:
        return False, f"the map is {info.get('width')} by {info.get('height')}"
    for x in range(10):
        for y in range(8):
            border = x in (0, 9) or y in (0, 7)
            solid = env.command("tilemap.solid", {"entity": room, "tile_x": x, "tile_y": y})["solid"]
            if solid != border:
                return False, f"cell ({x}, {y}) is {'solid' if solid else 'open'}"
    t = env.command("world.get", {"entity": room, "component": "Transform"})["position"]
    if not (near(t["x"], 30) and near(t["y"], 0)):
        return False, f"Room is at {t}"
    pebble = env.command("world.find", {"path": "Pebble"})
    if not isinstance(pebble, int):
        return False, "no entity named Pebble"
    col = env.command("world.get", {"entity": pebble, "component": "Collider2D"})
    if not col or col["shape"] != 1 or not near(col["radius"], 0.25):
        return False, f"Pebble's Collider2D is {col}"
    y = env.command("world.get", {"entity": pebble, "component": "Transform"})["position"]["y"]
    if not near(y, -6.75, 0.05):
        return False, f"the Pebble rests at y {y}, not on the floor's top at -7"
    if not isinstance(answer, (int, float)) or not near(answer, y, 0.05):
        return False, f"answered {answer!r}, the Pebble is at {y}"
    return True, f"a walled 10 by 8 room; the pebble rests at {y:.3f}"


def plank_bridge_solve(env):
    for i in range(5):
        joint = {"kind": "revolute", "anchor": {"x": -0.5, "y": 0}}
        if i == 0:
            joint["other_anchor"] = {"x": -4.5, "y": 4}
        else:
            joint["body"] = env.command("world.find", {"path": f"Plank{i - 1}"})
            joint["other_anchor"] = {"x": 0.5, "y": 0}
        env.command("world.spawn", {"name": f"Plank{i}", "components": {"Transform": {"position": {"x": -4 + i, "y": 4, "z": 0}}, "RigidBody2D": {}, "Collider2D": {"size": {"x": 0.5, "y": 0.1}}, "Joint2D": joint}})
    # The sixth hinge, from a body of its own: an entity holds one Joint2D.
    env.command("world.spawn", {"name": "Post", "components": {"Transform": {"position": {"x": 0.5, "y": 4, "z": 0}}, "RigidBody2D": {"kind": "static"}, "Collider2D": {"size": {"x": 0.05, "y": 0.05}, "mask": 0},
                "Joint2D": {"kind": "revolute", "body": env.command("world.find", {"path": "Plank4"}), "other_anchor": {"x": 0.5, "y": 0}}}})
    env.command("step", {"ticks": 120})
    return min(env.command("world.get", {"entity": f"Plank{i}", "component": "Transform"})["position"]["y"] for i in range(5))


def plank_bridge_check(env, answer):
    import math
    ends = []
    ys = []
    for i in range(5):
        p = env.command("world.find", {"path": f"Plank{i}"})
        if not isinstance(p, int):
            return False, f"no entity named Plank{i}"
        col = env.command("world.get", {"entity": p, "component": "Collider2D"})
        if not col or col["shape"] != 0 or not (near(col["size"]["x"], 0.5) and near(col["size"]["y"], 0.1)):
            return False, f"Plank{i}'s Collider2D is {col}"
        if not env.command("world.get", {"entity": p, "component": "RigidBody2D"}):
            return False, f"Plank{i} has no RigidBody2D"
        t = env.command("world.get", {"entity": p, "component": "Transform"})
        a = 2 * math.atan2(t["rotation"]["z"], t["rotation"]["w"])
        c = t["position"]
        ends.append(((c["x"] - 0.5 * math.cos(a), c["y"] - 0.5 * math.sin(a)), (c["x"] + 0.5 * math.cos(a), c["y"] + 0.5 * math.sin(a))))
        ys.append(c["y"])
    def gap(p, q):
        return math.hypot(p[0] - q[0], p[1] - q[1])
    if gap(ends[0][0], (-4.5, 4)) > 0.05:
        return False, f"Plank0's left end is at {ends[0][0]}, not held at (-4.5, 4)"
    if gap(ends[4][1], (0.5, 4)) > 0.05:
        return False, f"Plank4's right end is at {ends[4][1]}, not held at (0.5, 4)"
    for i in range(4):
        if gap(ends[i][1], ends[i + 1][0]) > 0.05:
            return False, f"Plank{i} and Plank{i + 1} came apart ({gap(ends[i][1], ends[i + 1][0]):.3f})"
    low = min(ys)
    if not (low < 4.0 and low > 3.0):
        return False, f"the lowest plank is at {low}: the bridge does not hang"
    if not isinstance(answer, (int, float)) or not near(answer, low, 0.05):
        return False, f"answered {answer!r}, the lowest plank is at {low}"
    return True, f"five planks hang between the points, lowest at {low:.3f}"


# ---- whole games from a brief (docs/agent-eval.md, Whole games): a blank project, a contract of
# names, actions, exposed values and events, and a check that plays the game through it.
GAME_TOML = """name = "{name}"
entry = "scripts/main.ts"
scene = "scene.json"

[window]
width = 960
height = 540

[input.actions]
move_x = {{ negative = ["A", "Left"], positive = ["D", "Right"] }}
move_y = {{ negative = ["S", "Down"], positive = ["W", "Up"] }}
"""
EMPTY_SCENE = '{"format": "pocket-scene", "entities": []}\n'

DODGE_TS = """import { events, expose, input, onStart, onTick, random, world } from "pocket";
let player = 0;
let alive = true;
let time = 0;
let next = 0;
let count = 0;
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 8 } } });
    player = world.spawn("Player", { components: { Transform: { position: { x: 0, y: -5, z: 0 } }, Sprite: { color: { r: 0.3, g: 0.7, b: 1, a: 1 } } } });
});
onTick((t) => {
    if (!alive) return;
    time += t.dt;
    const p = world.get(player, "Transform")!.position;
    world.set(player, "Transform", { position: { x: p.x + input.axis("move_x") * 6 * t.dt, y: p.y + input.axis("move_y") * 6 * t.dt } });
    next -= t.dt;
    if (next <= 0) {
        next += 1;
        count++;
        world.spawn(`Rock_${count}`, { components: { Transform: { position: { x: random() * 16 - 8, y: 6, z: 0 } }, Sprite: { color: { r: 0.5, g: 0.4, b: 0.3, a: 1 } } } });
    }
    const me = world.get(player, "Transform")!.position;
    for (const row of world.query({ name: "Rock_*", with: ["Transform"] })) {
        const r = row.Transform!.position;
        const y = r.y - 4 * t.dt;
        if (y < -7) { world.destroy(row.id); continue; }
        world.set(row.id, "Transform", { position: { y } });
        if (alive && Math.hypot(r.x - me.x, y - me.y) < 0.6) {
            alive = false;
            events.emit("game.over", { time: Number(time.toFixed(3)) });
        }
    }
});
expose("alive", () => alive);
expose("time_alive", () => Number(time.toFixed(3)));
"""

KEY_DOOR_TS = """import { events, expose, input, onStart, onTick, world } from "pocket";
let player = 0;
let hasKey = false;
let done = false;
let time = 0;
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 6, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 6 } } });
    player = world.spawn("Player", { components: { Transform: { position: { x: 0, y: 0, z: 0 } }, Sprite: { color: { r: 0.3, g: 0.7, b: 1, a: 1 } } } });
    world.spawn("Key", { components: { Transform: { position: { x: 4, y: 0, z: 0 } }, Sprite: { size: { x: 0.5, y: 0.5 }, color: { r: 1, g: 0.85, b: 0.2, a: 1 } } } });
    world.spawn("Door", { components: { Transform: { position: { x: 8, y: 0, z: 0 } }, Sprite: { size: { x: 0.4, y: 3 }, color: { r: 0.5, g: 0.3, b: 0.2, a: 1 } } } });
    world.spawn("Exit", { components: { Transform: { position: { x: 12, y: 0, z: 0 } }, Sprite: { color: { r: 0.3, g: 1, b: 0.4, a: 1 } } } });
});
onTick((t) => {
    time += t.dt;
    const p = world.get(player, "Transform")!.position;
    let x = p.x + input.axis("move_x") * 5 * t.dt;
    const y = p.y + input.axis("move_y") * 5 * t.dt;
    if (!hasKey && x > 7.5) x = Math.min(x, Math.max(p.x, 7.5));
    world.set(player, "Transform", { position: { x, y } });
    const key = world.find("Key");
    if (key !== undefined && !hasKey) {
        const k = world.get(key, "Transform")!.position;
        if (Math.hypot(k.x - x, k.y - y) < 0.7) {
            hasKey = true;
            world.destroy(key);
            events.emit("key.taken", {});
            const door = world.find("Door");
            if (door !== undefined) world.destroy(door);
        }
    }
    if (!done && Math.hypot(12 - x, 0 - y) < 0.7) {
        done = true;
        events.emit("level.complete", { seconds: Number(time.toFixed(3)) });
    }
});
expose("has_key", () => hasKey);
"""


SNAKE_TS = """import { events, expose, input, onStart, onTick, random, world } from "pocket";
const W = 20, H = 15, STEP = 0.15;
const segs: number[] = [];
const cells: Array<[number, number]> = [[5, 7], [4, 7], [3, 7]];
let dir: [number, number] = [1, 0];
let want: [number, number] = [1, 0];
let food = 0;
let alive = true;
let acc = 0;
const place = (e: number, x: number, y: number) => world.set(e, "Transform", { position: { x, y, z: 0 } });
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 9.5, y: 7, z: 10 } }, Camera: { orthographic: true, ortho_size: 8.5 } } });
    cells.forEach(([x, y], i) => segs.push(world.spawn(`Segment_${i}`, { components: { Transform: { position: { x, y, z: 0 } }, Sprite: { size: { x: 0.9, y: 0.9 }, color: i === 0 ? "#80ff80" : "#30c030" } } })));
    food = world.spawn("Food", { components: { Transform: { position: { x: 12, y: 3, z: 0 } }, Sprite: { size: { x: 0.7, y: 0.7 }, color: "#ff5050" } } });
});
onTick((t) => {
    if (!alive) return;
    const ax = input.axis("move_x"), ay = input.axis("move_y");
    if (ax !== 0 && dir[0] === 0) want = [Math.sign(ax), 0];
    else if (ay !== 0 && dir[1] === 0) want = [0, Math.sign(ay)];
    acc += t.dt;
    if (acc < STEP - 1e-9) return;
    acc -= STEP;
    dir = want;
    const nx = cells[0][0] + dir[0], ny = cells[0][1] + dir[1];
    const f = world.get(food, "Transform")!.position;
    const eat = Math.round(f.x) === nx && Math.round(f.y) === ny;
    const body = eat ? cells : cells.slice(0, -1);
    if (nx < 0 || ny < 0 || nx >= W || ny >= H || body.some(([x, y]) => x === nx && y === ny)) {
        alive = false;
        events.emit("game.over", { length: cells.length });
        return;
    }
    cells.unshift([nx, ny]);
    if (eat) {
        segs.push(world.spawn(`Segment_${segs.length}`, { components: { Transform: {}, Sprite: { size: { x: 0.9, y: 0.9 }, color: "#30c030" } } }));
        events.emit("food.eaten", { length: cells.length });
        for (let k = 0; k < 1000; k++) {
            const x = Math.floor(random() * W), y = Math.floor(random() * H);
            if (!cells.some(([cx, cy]) => cx === x && cy === y)) { place(food, x, y); break; }
        }
    } else {
        cells.pop();
    }
    cells.forEach(([x, y], i) => place(segs[i], x, y));
});
expose("length", () => cells.length);
expose("alive", () => alive);
"""


def write_game(project_dir, name, script):
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(GAME_TOML.format(name=os.path.basename(project_dir)))
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(EMPTY_SCENE)
    with open(os.path.join(project_dir, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(script)


def dodge_solve(env, project_dir):
    write_game(project_dir, "dodge", DODGE_TS)
    return None


def entity_pos(env, name):
    e = env.command("world.find", {"path": name})
    if not isinstance(e, int):
        return None
    t = env.command("world.get", {"entity": e, "component": "Transform"})
    return t["position"] if t else None


def dodge_check(env, answer):
    actions = env.command("input.actions", {})
    for a in ("move_x", "move_y"):
        if a not in actions:
            return False, f"no {a} action (actions: {sorted(actions)[:8]})"
    p0 = entity_pos(env, "Player")
    if p0 is None:
        return False, "no entity named Player with a Transform"
    env.command("input.hold", {"action": "move_x", "ticks": 60})
    env.command("step", {"ticks": 60})
    p1 = entity_pos(env, "Player")
    if not (near(p1["x"] - p0["x"], 6, 0.45) and near(p1["y"], p0["y"], 0.15)):
        return False, f"holding move_x for a second moved the Player from {p0} to {p1}, not 6 along x"
    # Out of the rocks' way (they fall between x -8 and 8), so a random hit cannot end the game
    # before the checks below; the brief has the game read the Player's Transform every tick.
    env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": 20, "y": p1["y"]}}})
    env.command("step", {"ticks": 150})
    rocks = env.command("world.query", {"name": "Rock*", "with": ["Transform"]})["entities"]
    if len(rocks) < 3:
        return False, f"{len(rocks)} rocks after 3.5 seconds, not one a second"
    r0 = rocks[-1]
    y0 = r0["Transform"]["position"]["y"]
    env.command("step", {"ticks": 30})
    r1 = env.command("world.get", {"entity": r0["id"], "component": "Transform"})
    if not r1 or not near(y0 - r1["position"]["y"], 2, 0.2):
        return False, f"a rock fell from {y0} to {r1 and r1['position']['y']} in half a second, not 4 a second"
    st = env.command("state", {})["state"]
    if st.get("alive") is not True or not isinstance(st.get("time_alive"), (int, float)) or st["time_alive"] < 3:
        return False, f"before a hit the state is {st}"
    seq = env.command("events.last_seq", {})["seq"]
    me = entity_pos(env, "Player")
    env.command("world.set", {"entity": r0["id"], "component": "Transform", "value": {"position": {"x": me["x"], "y": me["y"]}}})
    env.command("step", {"ticks": 2})
    over = env.command("events.since", {"seq": seq, "type": "game.over"})["events"]
    if not over:
        return False, "a rock put on the Player brought no game.over"
    st = env.command("state", {})["state"]
    if st.get("alive") is not False:
        return False, f"after game.over the state is {st}"
    t_end = st.get("time_alive")
    before = entity_pos(env, "Player")
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    after = entity_pos(env, "Player")
    if abs(after["x"] - before["x"]) > 0.01:
        return False, "the Player still moves after the game is over"
    if env.command("state", {})["state"].get("time_alive") != t_end:
        return False, "time_alive goes on after the game is over"
    return True, f"moves at 6, rocks fall at 4 one a second, a hit ends it at {t_end}"


def key_door_solve(env, project_dir):
    write_game(project_dir, "key_door", KEY_DOOR_TS)
    return None


def key_door_check(env, answer):
    for name in ("Player", "Key", "Door", "Exit"):
        if entity_pos(env, name) is None:
            return False, f"no entity named {name}"
    player = env.command("world.find", {"path": "Player"})
    # Without the key the door holds: put the player between the key and the door and walk right.
    env.command("world.set", {"entity": player, "component": "Transform", "value": {"position": {"x": 6, "y": 0}}})
    env.command("input.hold", {"action": "move_x", "ticks": 90})
    env.command("step", {"ticks": 90})
    p = entity_pos(env, "Player")
    # Into the door is not past it: the brief gives the door's place, not its width.
    if p["x"] > 8.05:
        return False, f"without the key the Player walked through the door to x {p['x']:.2f}"
    if p["x"] < 6.5:
        return False, f"the Player did not walk up to the door (x {p['x']:.2f})"
    seq = env.command("events.last_seq", {})["seq"]
    # The key, taken where it lies.
    env.command("world.set", {"entity": player, "component": "Transform", "value": {"position": {"x": 4, "y": 0}}})
    env.command("step", {"ticks": 3})
    if not env.command("events.since", {"seq": seq, "type": "key.taken"})["events"]:
        return False, "standing on the Key brought no key.taken"
    if env.command("state", {})["state"].get("has_key") is not True:
        return False, "has_key is not true after the key was taken"
    if entity_pos(env, "Key") is not None:
        return False, "the Key is still there after it was taken"
    env.command("input.hold", {"action": "move_x", "ticks": 150})
    r = env.command("step", {"ticks": 240, "until": {"event": "level.complete"}})
    if not r["until"]["met"]:
        p = entity_pos(env, "Player")
        return False, f"walking right with the key never reached the Exit (the Player is at {p})"
    return True, f"the door held without the key; with it the Exit was reached at tick {r['until']['tick']}"


def snake_solve(env, project_dir):
    write_game(project_dir, "snake", SNAKE_TS)
    return None


def snake_check(env, answer):
    head = lambda: entity_pos(env, "Segment_0")  # noqa: E731
    segs = [entity_pos(env, f"Segment_{i}") for i in range(3)]
    if any(s is None for s in segs):
        return False, f"no Segment_0, Segment_1 and Segment_2 with Transforms ({segs})"
    h = segs[0]
    head_ok = near(h["y"], 7, 0.05) and (near(h["x"], 5, 0.05) or near(h["x"], 6, 0.05))   # at most one move made
    if not head_ok or not all(near(s["y"], 7, 0.05) for s in segs) or not (segs[0]["x"] > segs[1]["x"] > segs[2]["x"]):
        return False, f"the snake does not start as three cells at (5, 7), (4, 7), (3, 7) heading +x: {segs}"
    st = env.command("state", {})["state"]
    if st.get("length") != 3 or st.get("alive") is not True:
        return False, f"at the start the state is {st}"
    if entity_pos(env, "Food") is None:
        return False, "no entity named Food"
    # The Food put in front of the head is eaten on the next move.
    env.command("world.set", {"entity": "Food", "component": "Transform", "value": {"position": {"x": round(h["x"]) + 1, "y": 7}}})
    seq = env.command("events.last_seq", {})["seq"]
    r = env.command("step", {"ticks": 30, "until": {"event": "food.eaten"}})
    if not r["until"]["met"]:
        return False, f"the Food put in front of the head was not eaten (the head is at {head()})"
    eaten = env.command("events.since", {"seq": seq, "type": "food.eaten"})["events"][0]["data"]
    if eaten.get("length") != 4 or env.command("state", {})["state"].get("length") != 4 or entity_pos(env, "Segment_3") is None:
        return False, f"after eating: food.eaten {eaten}, length {env.command('state', {})['state'].get('length')}, Segment_3 {entity_pos(env, 'Segment_3')}"
    env.command("world.set", {"entity": "Food", "component": "Transform", "value": {"position": {"x": 0, "y": 0}}})
    # A cell every 0.15 s: five in 45 ticks.
    a = head()
    env.command("step", {"ticks": 45})
    b = head()
    if not (abs(b["x"] - a["x"] - 5) <= 1 and near(b["y"], a["y"], 0.05)):
        return False, f"in 0.75 s the head went from {a} to {b}, not 5 cells along +x"
    # Never straight back.
    env.command("input.hold", {"action": "move_x", "value": -1, "ticks": 18})
    env.command("step", {"ticks": 18})
    c = head()
    if c["x"] <= b["x"]:
        return False, f"holding move_x back turned the snake around (head from {b} to {c})"
    # Up, then out through the top edge.
    env.command("input.hold", {"action": "move_y", "ticks": 300})
    env.command("step", {"ticks": 20})
    d = head()
    if not (d["y"] > c["y"] + 0.5 and abs(d["x"] - c["x"]) <= 1.05):
        return False, f"holding move_y did not turn the head up (from {c} to {d})"
    seq = env.command("events.last_seq", {})["seq"]
    r = env.command("step", {"ticks": 240, "until": {"event": "game.over"}})
    if not r["until"]["met"]:
        return False, f"leaving the grid at the top brought no game.over (head at {head()})"
    over = env.command("events.since", {"seq": seq, "type": "game.over"})["events"][0]["data"]
    st = env.command("state", {})["state"]
    if over.get("length") != 4 or st.get("alive") is not False:
        return False, f"game.over {over}, state {st}"
    e = head()
    env.command("step", {"ticks": 30})
    if head() != e:
        return False, "the snake still moves after the game is over"
    return True, f"starts right, eats and grows to 4, a cell every 0.15 s, no reversing, turns, ends at the top edge"



PAUSE_TOML = GAME_TOML + 'pause = {{ positive = ["Escape"] }}\n'

PAUSE_TS = """import { Button, Label, events, expose, h, input, mount, onStart, onTick, signal, world } from "pocket";
type Screen = "title" | "playing" | "paused";
const screen = signal<Screen>("title");
let box = 0;
let time = 0;
function go(next: Screen, event: string) {
    screen.set(next);
    events.emit(event, {});
}
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 6 } } });
    box = world.spawn("Box", { components: { Transform: { position: { x: 0, y: 0, z: 0 } }, Sprite: { color: { r: 1, g: 0.6, b: 0.2, a: 1 } } } });
    mount(() => {
        const s = screen();
        if (s === "playing") return null;
        const panel = { position: "absolute", left: 0, right: 0, top: 0, bottom: 0, align: "center", justify: "center", direction: "column", gap: 12, background: "#00000080" };
        if (s === "title") return h("box", panel, Label({ text: "Box", size: 32 }), Button({ label: "Start", name: "start", onClick: () => go("playing", "game.started") }));
        return h("box", panel, Label({ text: "Paused", size: 28 }),
            Button({ label: "Resume", name: "resume", onClick: () => go("playing", "game.resumed") }),
            Button({ label: "Restart", name: "restart", onClick: () => {
                world.set(box, "Transform", { position: { x: 0, y: 0, z: 0 } });
                time = 0;
                go("playing", "game.restarted");
            } }));
    });
});
onTick((t) => {
    if (screen() !== "playing") return;
    if (input.pressed("pause")) {
        go("paused", "game.paused");
        return;
    }
    time += t.dt;
    const p = world.get(box, "Transform")!.position;
    world.set(box, "Transform", { position: { x: p.x + input.axis("move_x") * 4 * t.dt, y: p.y } });
});
expose("screen", () => screen());
expose("time", () => Number(time.toFixed(3)));
"""


def pause_menu_solve(env, project_dir):
    write_game(project_dir, "pause_menu", PAUSE_TS)
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(PAUSE_TOML.format(name=os.path.basename(project_dir)))
    return None


def pause_menu_check(env, answer):
    actions = env.command("input.actions", {})
    for a in ("move_x", "pause"):
        if a not in actions:
            return False, f"no {a} action (actions: {sorted(actions)[:8]})"
    state = lambda: env.command("state", {})["state"]  # noqa: E731
    box = lambda: entity_pos(env, "Box")  # noqa: E731
    p0 = box()
    if p0 is None or not (near(p0["x"], 0, 0.01) and near(p0["y"], 0, 0.01)):
        return False, f"no Box at (0, 0) at the start ({p0})"
    if state().get("screen") != "title":
        return False, f"the game does not open on the title screen: {state()}"
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    if not near(box()["x"], 0, 0.01):
        return False, f"the Box moved on the title screen (to {box()})"

    def click(name):
        try:
            env.command("ui.click", {"id": name})
        except Exception as e:   # noqa: BLE001 (the runtime's answer, said in the detail)
            return f"no button named {name} to click: {e}"
        env.command("step", {"ticks": 2})
        return None

    def happened(seq, event):
        return bool(env.command("events.since", {"seq": seq, "type": event})["events"])

    seq = env.command("events.last_seq", {})["seq"]
    if (err := click("start")) is not None:
        return False, err
    if state().get("screen") != "playing" or not happened(seq, "game.started"):
        return False, f"clicking start: screen {state().get('screen')}, game.started {happened(seq, 'game.started')}"
    a = box()
    t0 = state().get("time", 0)
    env.command("input.hold", {"action": "move_x", "ticks": 60})
    env.command("step", {"ticks": 60})
    b = box()
    t1 = state().get("time", 0)
    if not near(b["x"] - a["x"], 4, 0.3):
        return False, f"holding move_x for a second while playing moved the Box from {a} to {b}, not 4 along x"
    if not near(t1 - t0, 1, 0.1):
        return False, f"the time went from {t0} to {t1} in a second of play"
    seq = env.command("events.last_seq", {})["seq"]
    env.command("input.press", {"action": "pause"})
    env.command("step", {"ticks": 2})
    if state().get("screen") != "paused" or not happened(seq, "game.paused"):
        return False, f"pressing pause: screen {state().get('screen')}, game.paused {happened(seq, 'game.paused')}"
    c, tc = box(), state().get("time")
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    if not near(box()["x"], c["x"], 0.01) or state().get("time") != tc:
        return False, f"while paused the Box went from {c} to {box()} and the time from {tc} to {state().get('time')}"
    seq = env.command("events.last_seq", {})["seq"]
    if (err := click("resume")) is not None:
        return False, err
    if state().get("screen") != "playing" or not happened(seq, "game.resumed"):
        return False, f"clicking resume: screen {state().get('screen')}, game.resumed {happened(seq, 'game.resumed')}"
    d = box()
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    if not near(box()["x"] - d["x"], 2, 0.3):
        return False, f"after resume, half a second of move_x moved the Box from {d} to {box()}"
    env.command("input.press", {"action": "pause"})
    env.command("step", {"ticks": 2})
    seq = env.command("events.last_seq", {})["seq"]
    if (err := click("restart")) is not None:
        return False, err
    e, st = box(), state()
    if st.get("screen") != "playing" or not happened(seq, "game.restarted"):
        return False, f"clicking restart: screen {st.get('screen')}, game.restarted {happened(seq, 'game.restarted')}"
    if not (near(e["x"], 0, 0.05) and near(e["y"], 0, 0.05)) or st.get("time", 1) > 0.1:
        return False, f"after restart the Box is at {e} and the time {st.get('time')}"
    return True, "title, start, a second of play (4 units, 1 s), pause holding the Box and the clock, resume, restart at (0, 0)"


BREAKOUT_TS = """import { events, expose, input, onStart, onTick, world } from "pocket";
const R = 0.15;
const SPEED = 6;
let paddle = 0;
let ball = 0;
let score = 0;
let lives = 3;
let over = false;
let cleared = false;
const bricksLeft = () => world.query({ name: "Brick_*" }).length;
function serve() {
    const px = world.get(paddle, "Transform")!.position.x;
    world.set(ball, "Transform", { position: { x: px, y: -5, z: 0 } });
    world.set(ball, "Velocity", { linear: { x: SPEED * Math.SQRT1_2, y: SPEED * Math.SQRT1_2, z: 0 } });
}
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 8 } } });
    paddle = world.spawn("Paddle", { components: { Transform: { position: { x: 0, y: -6, z: 0 } }, Sprite: { size: { x: 2, y: 0.3 }, color: { r: 0.8, g: 0.8, b: 0.9, a: 1 } } } });
    ball = world.spawn("Ball", { components: { Transform: { position: { x: 0, y: -5, z: 0 } }, Velocity: { linear: { x: SPEED * Math.SQRT1_2, y: SPEED * Math.SQRT1_2, z: 0 } }, Sprite: { size: { x: 0.3, y: 0.3 }, color: { r: 1, g: 1, b: 1, a: 1 } } } });
    for (let row = 0; row < 4; row++)
        for (let col = 0; col < 8; col++)
            world.spawn(`Brick_${row * 8 + col}`, { components: { Transform: { position: { x: -7 + col * 2, y: 3 + row, z: 0 } }, Sprite: { size: { x: 1.8, y: 0.6 }, color: { r: 0.9, g: 0.3 + row * 0.15, b: 0.2, a: 1 } } } });
});
onTick((t) => {
    if (over || cleared) {
        world.set(ball, "Velocity", { linear: { x: 0, y: 0, z: 0 } });
        return;
    }
    const pp = world.get(paddle, "Transform")!.position;
    world.set(paddle, "Transform", { position: { x: Math.max(-7, Math.min(7, pp.x + input.axis("move_x") * 10 * t.dt)), y: pp.y } });
    const p = world.get(ball, "Transform")!.position;
    const v = { ...world.get(ball, "Velocity")!.linear };
    if ((p.x < -8 + R && v.x < 0) || (p.x > 8 - R && v.x > 0)) v.x = -v.x;
    if (p.y > 7 - R && v.y > 0) v.y = -v.y;
    const q = world.get(paddle, "Transform")!.position;
    if (v.y < 0 && Math.abs(p.x - q.x) < 1 + R && Math.abs(p.y - q.y) < 0.15 + R) v.y = Math.abs(v.y);
    for (const b of world.query({ name: "Brick_*", with: ["Transform"] })) {
        const c = b.Transform!.position;
        const dx = p.x - c.x, dy = p.y - c.y;
        if (Math.abs(dx) < 0.9 + R && Math.abs(dy) < 0.3 + R) {
            world.destroy(b.id);
            score += 10;
            events.emit("brick.broken", { score });
            if (0.9 + R - Math.abs(dx) < 0.3 + R - Math.abs(dy)) v.x = Math.sign(dx) * Math.abs(v.x);
            else v.y = Math.sign(dy) * Math.abs(v.y);
            break;
        }
    }
    world.set(ball, "Velocity", { linear: v });
    if (p.y < -8) {
        lives--;
        events.emit("life.lost", { lives });
        if (lives <= 0) {
            over = true;
            events.emit("game.over", { score });
            world.set(ball, "Velocity", { linear: { x: 0, y: 0, z: 0 } });
        } else serve();
    }
    if (bricksLeft() === 0 && !cleared) {
        cleared = true;
        events.emit("level.clear", { score });
        world.set(ball, "Velocity", { linear: { x: 0, y: 0, z: 0 } });
    }
});
expose("score", () => score);
expose("lives", () => lives);
expose("bricks", bricksLeft);
"""


def breakout_solve(env, project_dir):
    write_game(project_dir, "breakout", BREAKOUT_TS)
    return None


def breakout_check(env, answer):
    if "move_x" not in env.command("input.actions", {}):
        return False, "no move_x action"
    state = lambda: env.command("state", {})["state"]  # noqa: E731
    bricks = lambda: env.command("world.query", {"name": "Brick_*", "with": ["Transform"]})["entities"]  # noqa: E731
    put = lambda name, pos: env.command("world.set", {"entity": name, "component": "Transform", "value": {"position": {"x": pos[0], "y": pos[1]}}})  # noqa: E731
    aim = lambda vel: env.command("world.set", {"entity": "Ball", "component": "Velocity", "value": {"linear": {"x": vel[0], "y": vel[1], "z": 0}}})  # noqa: E731
    vel = lambda: (env.command("world.get", {"entity": "Ball", "component": "Velocity"}) or {}).get("linear")  # noqa: E731
    if entity_pos(env, "Paddle") is None or entity_pos(env, "Ball") is None:
        return False, "no Paddle and Ball with Transforms"
    bs = bricks()
    st = state()
    if len(bs) != 32 or st.get("bricks") != 32 or st.get("lives") != 3 or st.get("score") != 0:
        return False, f"at the start {len(bs)} Brick_ entities and the state {st}, not 32 bricks, 3 lives, score 0"
    v = vel()
    if not v or not near((v["x"] ** 2 + v["y"] ** 2) ** 0.5, 6, 0.3):
        return False, f"the Ball's Velocity is {v}, not 6 units a second"
    # The paddle: 10 a second, kept within x -7..7.
    put("Paddle", (0, -6))
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    px = entity_pos(env, "Paddle")["x"]
    if not near(px, 5, 0.3):
        return False, f"half a second of move_x moved the Paddle from 0 to {px}, not 5"
    env.command("input.hold", {"action": "move_x", "ticks": 60})
    env.command("step", {"ticks": 60})
    if entity_pos(env, "Paddle")["x"] > 7.01:
        return False, f"the Paddle went past x 7 (to {entity_pos(env, 'Paddle')['x']})"
    # A brick from below.
    target = next((b for b in bs if near(b["Transform"]["position"]["x"], -1, 0.05) and near(b["Transform"]["position"]["y"], 3, 0.05)), None)
    if target is None:
        return False, "no brick centered at (-1, 3)"
    put("Paddle", (-6, -6))
    put("Ball", (-1, 2))
    aim((0, 6))
    seq = env.command("events.last_seq", {})["seq"]
    r = env.command("step", {"ticks": 30, "until": {"event": "brick.broken"}})
    if not r["until"]["met"]:
        return False, f"the Ball sent up into the brick at (-1, 3) broke nothing (it is at {entity_pos(env, 'Ball')})"
    env.command("step", {"ticks": 2})
    ev = env.command("events.since", {"seq": seq, "type": "brick.broken"})["events"][0]["data"]
    st = state()
    if any(b["id"] == target["id"] for b in bricks()) or st.get("score") != 10 or st.get("bricks") != 31 or ev.get("score") != 10:
        return False, f"after the hit: brick.broken {ev}, state {st}"
    if not (vel() or {}).get("y", 0) < 0:
        return False, f"the Ball did not bounce down off the brick (Velocity {vel()})"
    # A side wall and the paddle.
    put("Ball", (7.5, 0))
    aim((6, 0))
    env.command("step", {"ticks": 10})
    if not (vel() or {}).get("x", 0) < 0:
        return False, f"the Ball did not bounce off the wall at x 8 (Velocity {vel()})"
    put("Paddle", (0, -6))
    put("Ball", (0, -5.2))
    aim((0, -6))
    env.command("step", {"ticks": 15})
    if not (vel() or {}).get("y", 0) > 0 or state().get("lives") != 3:
        return False, f"the Ball dropped on the Paddle did not bounce up (Velocity {vel()}, lives {state().get('lives')})"
    # A miss.
    put("Paddle", (-6, -6))
    put("Ball", (5, -7))
    aim((0, -6))
    seq = env.command("events.last_seq", {})["seq"]
    r = env.command("step", {"ticks": 40, "until": {"event": "life.lost"}})
    if not r["until"]["met"]:
        return False, f"the Ball fell past the Paddle and no life.lost (Ball at {entity_pos(env, 'Ball')})"
    env.command("step", {"ticks": 1})
    lost = env.command("events.since", {"seq": seq, "type": "life.lost"})["events"][0]["data"]
    b = entity_pos(env, "Ball")
    if lost.get("lives") != 2 or state().get("lives") != 2 or b["y"] < -6.5:
        return False, f"after a miss: life.lost {lost}, lives {state().get('lives')}, the Ball at {b}"
    # The last brick.
    rest = bricks()
    for x in rest[1:]:
        env.command("world.destroy", {"entity": x["id"]})
    last = rest[0]["Transform"]["position"]
    put("Ball", (last["x"], last["y"] - 0.8))
    aim((0, 6))
    seq = env.command("events.last_seq", {})["seq"]
    r = env.command("step", {"ticks": 30, "until": {"event": "level.clear"}})
    if not r["until"]["met"]:
        return False, f"breaking the last brick brought no level.clear (state {state()})"
    if state().get("bricks") != 0:
        return False, f"with no bricks left the state is {state()}"
    # Again from the start, three misses.
    env.command("project.reload", {})
    env.command("step", {"ticks": 1})
    seq = env.command("events.last_seq", {})["seq"]
    for i in range(3):
        put("Paddle", (-6, -6))
        put("Ball", (5, -7.5))
        aim((0, -6))
        env.command("step", {"ticks": 20})
    over = env.command("events.since", {"seq": seq, "type": "game.over"})["events"]
    if not over or state().get("lives") != 0:
        return False, f"three misses: game.over {over}, lives {state().get('lives')}"
    a, pa = entity_pos(env, "Ball"), entity_pos(env, "Paddle")
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    if entity_pos(env, "Ball") != a or entity_pos(env, "Paddle") != pa:
        return False, "the Ball or the Paddle still moves after the game is over"
    return True, "32 bricks, a paddle at 10 a second held in, a brick broken from below (score 10), walls and the paddle bounce, a miss costs a life, the last brick clears the level, three misses end it"



WATCHMAN_TS = """import { nav, onStart, world } from "pocket";
onStart(() => {
    nav.bake({ min: { x: -10, y: -1, z: -10 }, max: { x: 10, y: 2, z: 10 }, cell: 0.5 });
    world.spawn("Beat", { components: { Transform: {}, Path: { points: [{ x: -6, y: 0, z: 6 }, { x: 6, y: 0, z: 6 }], smooth: false } } });
    world.spawn("Watchman", { components: {
        Transform: { position: { x: -6, y: 0, z: 6 } },
        MeshRenderer: { mesh: "capsule", color: { r: 0.8, g: 0.3, b: 0.2, a: 1 } },
        NavAgent: { speed: 2 },
        Behavior: {
            target: "Player",
            sight: 6,
            states: [
                { name: "patrol", move: "patrol", path: "Beat", speed: 2 },
                { name: "chase", move: "follow", speed: 4 },
                { name: "search", move: "follow", speed: 4 },
            ],
            transitions: [
                { from: "patrol", to: "chase", when: "sees" },
                { from: "chase", to: "search", when: "not sees" },
                { from: "search", to: "chase", when: "sees" },
                { from: "search", to: "patrol", when: "time > 2" },
            ],
        },
    } });
});
"""


def watchman_solve(env, project_dir):
    write_game(project_dir, "watchman", WATCHMAN_TS)
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"format": "pocket-scene", "entities": [
            {"name": "Ground", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 40}}, "MeshRenderer": {"mesh": "cube"}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box"}}},
            {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0.5, "z": -6}}, "MeshRenderer": {"mesh": "sphere"}}},
            {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 14, "z": 14}, "rotation": {"x": -0.38, "y": 0, "z": 0, "w": 0.92}}, "Camera": {}}},
        ]}))
    return None


PLATFORMER_ROWS = [
    "........................",
    "........................",
    "..................F.....",
    "...............######...",
    "........###.............",
    "P.......................",
    "#######...######...#####",
    "#######...######...#####",
]
PLATFORMER_TOML = GAME_TOML + 'jump = ["Space", "W", "Up"]\n'
PLATFORMER_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#6b4f3a"/><rect width="16" height="4" fill="#79b04a"/></svg>\n'
PLATFORMER_TS = """import { events, expose, input, onStart, onTick, tilemap, world } from "pocket";
const ROWS = %s;
const SPEED = 6;
const JUMP = 11;
let player = 0;
let camera = 0;
let deaths = 0;
let complete = false;
const cell = (ch: string) => {
    const row = ROWS.findIndex((r) => r.includes(ch));
    return { x: ROWS[row].indexOf(ch) + 0.5, y: -(row + 0.5) };
};
const start = cell("P");
const flag = cell("F");
onStart(() => {
    tilemap.fromText("maps/level.tmj", {
        rows: ROWS,
        legend: { "#": { layer: "ground", tile: 0 }, ".": null, P: null, F: null },
        tilesets: [{ image: "assets/tiles.svg", tile_width: 32, tile_height: 32 }],
        layers: [{ name: "ground", solid: true }],
        tile_width: 32,
        tile_height: 32,
    });
    world.spawn("Level", { components: { Transform: { position: { x: 0, y: 0, z: 0 } }, TileMap: { map: "maps/level.tmj" } } });
    world.spawn("Flag", { components: { Transform: { position: { x: flag.x, y: flag.y, z: 0 } }, Sprite: { size: { x: 0.3, y: 1 }, color: { r: 1, g: 0.85, b: 0.2, a: 1 } } } });
    player = world.spawn("Player", { components: { Transform: { position: { x: start.x, y: start.y, z: 0 } }, Sprite: { size: { x: 0.8, y: 0.9 }, color: { r: 0.3, g: 0.6, b: 1, a: 1 } }, Body2D: { size: { x: 0.4, y: 0.45 } } } });
    camera = world.spawn("Camera", { components: { Transform: { position: { x: start.x, y: -4, z: 10 } }, Camera: { orthographic: true, ortho_size: 5 } } });
});
onTick(() => {
    const body = world.get(player, "Body2D")!;
    if (complete) {
        world.set(player, "Body2D", { velocity: { x: 0, y: body.velocity.y } });
        return;
    }
    const vy = input.pressed("jump") && body.grounded ? JUMP : body.velocity.y;
    world.set(player, "Body2D", { velocity: { x: input.axis("move_x") * SPEED, y: vy } });
    const p = world.get(player, "Transform")!.position;
    world.set(camera, "Transform", { position: { x: p.x, y: -4, z: 10 } });
    if (p.y < -12) {
        deaths++;
        events.emit("player.died", { deaths });
        world.set(player, "Transform", { position: { x: start.x, y: start.y, z: 0 } });
        world.set(player, "Body2D", { velocity: { x: 0, y: 0 } });
    } else if (Math.hypot(p.x - flag.x, p.y - flag.y) < 0.8) {
        complete = true;
        events.emit("level.complete", { deaths });
    }
});
expose("deaths", () => deaths);
expose("complete", () => complete);
""" % json.dumps(PLATFORMER_ROWS)


def platformer_solve(env, project_dir):
    write_game(project_dir, "platformer", PLATFORMER_TS)
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(PLATFORMER_TOML.format(name=os.path.basename(project_dir)))
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with open(os.path.join(project_dir, "assets", "tiles.svg"), "w", encoding="utf-8", newline="\n") as f:
        f.write(PLATFORMER_SVG)
    return None


def platformer_check(env, answer):
    actions = env.command("input.actions", {})
    for a in ("move_x", "jump"):
        if a not in actions:
            return False, f"no {a} action (actions: {sorted(actions)[:8]})"
    try:
        solid = env.command("tilemap.rows", {"entity": "Level", "solid": True})
    except Exception as e:   # noqa: BLE001 (no Level or no map: said)
        return False, f"no tile map entity Level: {e}"
    rows = solid.get("rows") or (solid.get("layers") or [{}])[0].get("rows") or []
    want = ["".join("#" if c == "#" else "." for c in r) for r in PLATFORMER_ROWS]
    if [r[:24] for r in rows[:8]] != want:
        return False, f"the Level's solid cells read {rows[:8]}, not the rows of the brief"
    lo = env.command("world.get", {"entity": "Level", "component": "Transform"}) or {}
    if not (near(lo.get("position", {}).get("x", 0), 0, 0.01) and near(lo.get("position", {}).get("y", 0), 0, 0.01)):
        return False, f"the Level's corner is at {lo.get('position')}, not (0, 0)"
    body = env.command("world.get", {"entity": "Player", "component": "Body2D"})
    if not body:
        return False, "no Player with a Body2D"
    state = lambda: env.command("state", {})["state"]  # noqa: E731
    put = lambda x, y: (env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": x, "y": y}}}),  # noqa: E731
                        env.command("world.set", {"entity": "Player", "component": "Body2D", "value": {"velocity": {"x": 0, "y": 0}}}))
    env.command("step", {"ticks": 60})
    p = entity_pos(env, "Player")
    body = env.command("world.get", {"entity": "Player", "component": "Body2D"})
    if not body["grounded"] or not near(p["x"], 0.5, 0.2) or not (-6 < p["y"] < -5):
        return False, f"a second after the start the Player is at {p}, grounded {body['grounded']}, not standing at the start"
    rest = p["y"]
    # Walking: 6 units a second.
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    q = entity_pos(env, "Player")
    if not near(q["x"] - p["x"], 3, 0.35):
        return False, f"half a second of move_x moved the Player from x {p['x']:.2f} to {q['x']:.2f}, not 3 units"
    # A jump from the ground: up about 2.5 units and down again.
    put(3.5, rest)
    env.command("step", {"ticks": 5})
    env.command("input.press", {"action": "jump"})
    r = env.command("step", {"ticks": 70, "watch": ["Player:Transform.position.y"]})
    w = r.get("watch") or {}
    w = w.get("Player:Transform.position.y", w) if isinstance(w, dict) else (w[0] if w else {})
    top = w.get("max")
    if top is None or not near(top - rest, 2.52, 0.4):
        return False, f"a jump from y {rest:.2f} rose to {top}, not about 2.5 units higher (watch {w})"
    if not near(entity_pos(env, "Player")["y"], rest, 0.05):
        return False, f"a jump on flat ground came down at y {entity_pos(env, 'Player')['y']:.2f}, not {rest:.2f}"
    # A pit: below y -12 the Player dies and starts again.
    seq = env.command("events.last_seq", {})["seq"]
    put(8.5, -5.5)
    env.command("step", {"ticks": 90})
    died = env.command("events.since", {"seq": seq, "type": "player.died"})["events"]
    p = entity_pos(env, "Player")
    if not died or state().get("deaths") != 1 or not near(p["x"], 0.5, 0.3):
        return False, f"dropped into the pit at x 8.5: player.died {bool(died)}, deaths {state().get('deaths')}, the Player at {p}"
    # The flag ends the level, and then the Player no longer walks.
    seq = env.command("events.last_seq", {})["seq"]
    put(17.2, -2.55)
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    done = env.command("events.since", {"seq": seq, "type": "level.complete"})["events"]
    if not done or state().get("complete") is not True:
        return False, f"walking right on the top platform from x 17.2 to the flag: level.complete {bool(done)}, complete {state().get('complete')}"
    x = entity_pos(env, "Player")["x"]
    env.command("input.hold", {"action": "move_x", "ticks": 30})
    env.command("step", {"ticks": 30})
    if abs(entity_pos(env, "Player")["x"] - x) > 0.05:
        return False, "after level.complete move_x still moves the Player"
    return True, f"the level's solid cells, standing at the start, walking at 6, a jump of {top - rest:.2f}, a death in the pit, the flag"


SOKOBAN_ROWS = [
    "######",
    "#.x..#",
    "#@$$x#",
    "#....#",
    "######",
]
SOKOBAN_TOML = GAME_TOML + 'undo = ["Z"]\n'
SOKOBAN_TS = """import { events, expose, input, onStart, onTick, world } from "pocket";
const ROWS = %s;
type Cell = { c: number; r: number };
const walls = new Set<string>();
const goals: Cell[] = [];
const key = (c: number, r: number) => `${c},${r}`;
let player: Cell = { c: 0, r: 0 };
const boxes: Cell[] = [];
const ids: number[] = [];
let playerId = 0;
let moves = 0;
let solved = false;
const history: { player: Cell; boxes: Cell[] }[] = [];
const place = (id: number, at: Cell) => world.set(id, "Transform", { position: { x: at.c, y: -at.r, z: 0.1 } });
function draw() {
    place(playerId, player);
    boxes.forEach((b, i) => place(ids[i], b));
}
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 2.5, y: -2, z: 10 } }, Camera: { orthographic: true, ortho_size: 4 } } });
    ROWS.forEach((row, r) => [...row].forEach((ch, c) => {
        if (ch === "#") {
            walls.add(key(c, r));
            world.spawn(`Wall_${c}_${r}`, { components: { Transform: { position: { x: c, y: -r, z: 0 } }, Sprite: { size: { x: 1, y: 1 }, color: { r: 0.35, g: 0.35, b: 0.4, a: 1 } } } });
        } else if (ch === "x") {
            goals.push({ c, r });
            world.spawn(`Goal_${goals.length - 1}`, { components: { Transform: { position: { x: c, y: -r, z: 0 } }, Sprite: { size: { x: 0.5, y: 0.5 }, color: { r: 0.9, g: 0.8, b: 0.2, a: 1 } } } });
        } else if (ch === "@") player = { c, r };
        else if (ch === "$") boxes.push({ c, r });
    }));
    playerId = world.spawn("Player", { components: { Transform: { position: { x: player.c, y: -player.r, z: 0.1 } }, Sprite: { size: { x: 0.8, y: 0.8 }, color: { r: 0.3, g: 0.6, b: 1, a: 1 } } } });
    boxes.forEach((b, i) => ids.push(world.spawn(`Box_${i}`, { components: { Transform: { position: { x: b.c, y: -b.r, z: 0.1 } }, Sprite: { size: { x: 0.9, y: 0.9 }, color: { r: 0.7, g: 0.45, b: 0.2, a: 1 } } } })));
});
function step(dc: number, dr: number) {
    const to = { c: player.c + dc, r: player.r + dr };
    if (walls.has(key(to.c, to.r))) return;
    const hit = boxes.findIndex((b) => b.c === to.c && b.r === to.r);
    const before = { player: { ...player }, boxes: boxes.map((b) => ({ ...b })) };
    if (hit >= 0) {
        const past = { c: to.c + dc, r: to.r + dr };
        if (walls.has(key(past.c, past.r)) || boxes.some((b) => b.c === past.c && b.r === past.r)) return;
        boxes[hit] = past;
    }
    history.push(before);
    player = to;
    moves++;
    draw();
    events.emit("player.moved", { moves });
    if (!solved && goals.every((g) => boxes.some((b) => b.c === g.c && b.r === g.r))) {
        solved = true;
        events.emit("level.solved", { moves });
    }
}
onTick(() => {
    if (input.pressed("undo") && history.length) {
        const last = history.pop()!;
        player = last.player;
        last.boxes.forEach((b, i) => (boxes[i] = b));
        moves--;
        draw();
        return;
    }
    if (input.pressed("move_x")) step(Math.sign(input.axis("move_x")), 0);
    else if (input.pressed("move_y")) step(0, -Math.sign(input.axis("move_y")));
});
expose("moves", () => moves);
expose("solved", () => solved);
""" % json.dumps(SOKOBAN_ROWS)


def sokoban_solve(env, project_dir):
    write_game(project_dir, "sokoban", SOKOBAN_TS)
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(SOKOBAN_TOML.format(name=os.path.basename(project_dir)))
    return None


def sokoban_check(env, answer):
    actions = env.command("input.actions", {})
    for a in ("move_x", "move_y", "undo"):
        if a not in actions:
            return False, f"no {a} action (actions: {sorted(actions)[:8]})"
    state = lambda: env.command("state", {})["state"]  # noqa: E731

    def cell(name):
        p = entity_pos(env, name)
        return None if p is None else (round(p["x"]), round(-p["y"]))

    def where():
        return {n: cell(n) for n in ("Player", "Box_0", "Box_1")}

    start = {"Player": (1, 2), "Box_0": (2, 2), "Box_1": (3, 2)}
    env.command("step", {"ticks": 2})
    if where() != start:
        return False, f"at the start the cells are {where()}, not {start}"

    def press(action, sign=1):
        env.command("input.press", {"action": action, "sign": sign})
        env.command("step", {"ticks": 3})

    seq = env.command("events.last_seq", {})["seq"]
    press("move_x", -1)   # a wall
    press("move_x", 1)    # two boxes in a row
    if where() != start or state().get("moves") != 0:
        return False, f"pressing into the wall and into two boxes in a row moved something: {where()}, moves {state().get('moves')}"
    press("move_y", -1)
    moved = env.command("events.since", {"seq": seq, "type": "player.moved"})["events"]
    if cell("Player") != (1, 3) or state().get("moves") != 1 or not moved:
        return False, f"a press of down: the Player at {cell('Player')}, moves {state().get('moves')}, player.moved {bool(moved)}; not (1, 3), 1 move"
    press("undo")
    if where() != start or state().get("moves") != 0:
        return False, f"undo after one move left {where()} and moves {state().get('moves')}, not the start and 0"
    seq = env.command("events.last_seq", {})["seq"]
    for action, sign in (("move_y", -1), ("move_x", 1), ("move_y", 1)):
        press(action, sign)
    want = {"Player": (2, 2), "Box_0": (2, 1), "Box_1": (3, 2)}
    if where() != want or state().get("moves") != 3:
        return False, f"down, right, up (a push): {where()} with moves {state().get('moves')}, not {want} and 3"
    press("move_y", 1)    # the box against the top wall
    if where() != want or state().get("moves") != 3:
        return False, f"pushing a box into a wall moved something: {where()}, moves {state().get('moves')}"
    if env.command("events.since", {"seq": seq, "type": "level.solved"})["events"] or state().get("solved"):
        return False, "level.solved before every box is on a goal"
    press("move_x", 1)
    solved = env.command("events.since", {"seq": seq, "type": "level.solved"})["events"]
    data = solved[0].get("data", solved[0]) if solved else {}
    if cell("Box_1") != (4, 2) or not solved or state().get("solved") is not True or data.get("moves") != 4:
        return False, f"the last push: Box_1 at {cell('Box_1')}, level.solved {solved[:1]}, solved {state().get('solved')}; not (4, 2), solved in 4 moves"
    return True, "walls and two boxes in a row stop a move, a move and its undo, a push, a box against a wall, solved in 4 moves"


VILLAGERS_TS = """import { input, nav, onStart, onTick, repro, world } from "pocket";
const SHIRTS = ["red", "green", "yellow", "purple", "orange"];
const villagers: number[] = [];
let player = 0;
onStart(() => {
    nav.bake({ min: { x: -12, y: -1, z: -12 }, max: { x: 12, y: 2, z: 12 }, cell: 0.5 });
    SHIRTS.forEach((shirt, i) => {
        const a = (i / SHIRTS.length) * Math.PI * 2;
        villagers.push(world.spawn(`Villager_${i + 1}`, { components: {
            Transform: { position: { x: Math.cos(a) * 3, y: 0, z: Math.sin(a) * 3 } },
            MeshRenderer: { mesh: `humanoid?shirt=${shirt}` },
            Animator: { locomotion: true },
            NavAgent: { speed: 1.3, face: true },
            Behavior: {
                target: "Player",
                states: [{ name: "wander", move: "wander", radius: 2.5, speed: 1.3, clip: "idle" }, { name: "greet", move: "stay", clip: "wave" }],
                transitions: [{ from: "wander", to: "greet", when: "distance < 3" }, { from: "greet", to: "wander", when: "distance > 4" }],
            },
        } }));
    });
    player = world.find("Player") ?? 0;
});
onTick((t) => {
    const p = world.get(player, "Transform")!.position;
    world.set(player, "Transform", { position: { x: p.x + input.axis("move_x") * 4 * t.dt, y: p.y, z: p.z + input.axis("move_z") * 4 * t.dt } });
    for (const v of villagers) {
        if (world.get(v, "Behavior")!.state !== "greet") continue;
        const q = world.get(v, "Transform")!.position;
        const yaw = repro.atan2(-(p.x - q.x), -(p.z - q.z));   // its -Z toward the player
        world.set(v, "Transform", { rotation: { x: 0, y: repro.sin(yaw / 2), z: 0, w: repro.cos(yaw / 2) } });
    }
});
"""


def villagers_solve(env, project_dir):
    write_game(project_dir, "villagers", VILLAGERS_TS)
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"format": "pocket-scene", "entities": [
            {"name": "Ground", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 40}}, "MeshRenderer": {"mesh": "cube"}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box"}}},
            {"name": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0, "z": 15}}, "MeshRenderer": {"mesh": "capsule"}}},
            {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 12, "z": 14}, "rotation": {"x": -0.33, "y": 0, "z": 0, "w": 0.94}}, "Camera": {}}},
            {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0}}}]}, indent=1))
    return None


WOLVES_TS = """// Wolves and sheep: each wolf chases the nearest sheep (Behavior.targets); one within a unit is caught.
import { events, onTick, world } from "pocket";

onTick(() => {
    for (const wolf of ["Wolf1", "Wolf2"]) {
        const w = world.find(wolf);
        if (!w) continue;
        const at = world.get(w, "Transform")!.position;
        for (const row of world.query({ name: "Sheep*", with: ["Transform"] })) {
            const p = row.Transform!.position;
            if (Math.hypot(p.x - at.x, p.z - at.z) < 1) {
                events.emit("sheep.caught", { sheep: row.path }, { subject: w });
                world.destroy(row.id);
            }
        }
    }
});
"""
WOLF_SHEEP = [(7, 0, 5), (-6, 0, 8), (5, 0, -7), (-8, 0, -6)]


def wolves_solve(env, project_dir):
    write_game(project_dir, "wolves", WOLVES_TS)
    sheep = [{"name": f"Sheep{i + 1}", "components": {"Transform": {"position": {"x": x, "y": y, "z": z}}, "MeshRenderer": {"mesh": "sphere", "color": "#f0f0e8"}}} for i, (x, y, z) in enumerate(WOLF_SHEEP)]
    wolf = lambda name, x: {"name": name, "components": {"Transform": {"position": {"x": x, "y": 0, "z": 0}}, "MeshRenderer": {"mesh": "capsule", "color": "#555560"},  # noqa: E731
                                                       "NavAgent": {"speed": 3}, "Behavior": {"targets": "Sheep*", "states": [{"name": "hunt", "move": "follow", "speed": 3}]}}}
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"format": "pocket-scene", "entities": [
            {"name": "Ground", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 40}}, "MeshRenderer": {"mesh": "cube"}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box"}}},
            *sheep, wolf("Wolf1", -1), wolf("Wolf2", 1),
            {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 18, "z": 16}, "rotation": {"x": -0.38, "y": 0, "z": 0, "w": 0.92}}, "Camera": {}}},
            {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0}}}]}, indent=1))
    return None


def wolves_check(env, answer):
    import math
    # Only what happens while the check runs: the agent's own tries left events in the log too.
    start = env.command("events.last_seq", {})["seq"]

    def pos(name):
        try:
            return entity_pos(env, name)
        except Exception:   # noqa: BLE001 (gone)
            return None
    sheep = [f"Sheep{i + 1}" for i in range(4)]
    for n, (x, _, z) in zip(sheep, WOLF_SHEEP):
        p = pos(n)
        if not p or math.hypot(p["x"] - x, p["z"] - z) > 0.3:
            return False, f"{n} is at {p}, not ({x}, 0, {z})"
    for n, x in (("Wolf1", -1), ("Wolf2", 1)):
        p = pos(n)
        if not p or math.hypot(p["x"] - x, p["z"]) > 0.3:
            return False, f"{n} is at {p}, not ({x}, 0, 0)"
    def nearest(wolf):
        w = pos(wolf)
        alive = [(n, pos(n)) for n in sheep]
        alive = [(n, p) for n, p in alive if p]
        return min(alive, key=lambda np: math.hypot(np[1]["x"] - w["x"], np[1]["z"] - w["z"]))[0] if alive else None
    # Half a second in: each wolf has closed on its nearest sheep at about 3 a second.
    targets = {wolf: nearest(wolf) for wolf in ("Wolf1", "Wolf2")}
    before = {wolf: pos(wolf) for wolf in targets}
    env.command("step", {"ticks": 30})
    for wolf, sheep_name in targets.items():
        a, b, sp = before[wolf], pos(wolf), pos(sheep_name)
        d0 = math.hypot(sp["x"] - a["x"], sp["z"] - a["z"])
        d1 = math.hypot(sp["x"] - b["x"], sp["z"] - b["z"])
        if d0 - d1 < 0.9:
            return False, f"{wolf} closed only {d0 - d1:.2f} on its nearest sheep {sheep_name} in half a second"
    # A sheep put a little behind Wolf2: it turns to that one.
    w2 = pos("Wolf2")
    far = [n for n in sheep if pos(n) and n != targets["Wolf2"] and n != targets["Wolf1"]]
    if not far:
        return False, "no sheep left to move"
    moved = far[0]
    env.command("world.set", {"entity": moved, "component": "Transform", "value": {"position": {"x": w2["x"], "y": 0, "z": w2["z"] + 3}}})
    env.command("step", {"ticks": 30})
    w2b, sp = pos("Wolf2"), pos(moved)
    if sp is None:
        return False, f"{moved} vanished before Wolf2 could reach it"
    if math.hypot(sp["x"] - w2b["x"], sp["z"] - w2b["z"]) > 3 - 0.9:
        return False, f"{moved}, put 3 behind Wolf2, is still {math.hypot(sp['x'] - w2b['x'], sp['z'] - w2b['z']):.2f} from it half a second later"
    # Ten seconds on: the sheep are caught, each a sheep.caught event naming a wolf, the sheep gone.
    env.command("step", {"ticks": 600})
    caught = env.command("events.since", {"seq": start, "type": "sheep.caught"})["events"]
    wolves = {env.command("world.find", {"path": n}) for n in ("Wolf1", "Wolf2")}
    if len(caught) < 3 or any(e.get("subject") not in wolves for e in caught):
        return False, f"{len(caught)} sheep.caught events, subjects {[e.get('subject') for e in caught]} (the wolves are {sorted(wolves)})"
    left = [n for n in sheep if pos(n)]
    if len(left) > 1:
        return False, f"sheep still standing ten seconds on: {left}"
    return True, f"each wolf ran at its nearest sheep and turned to one put behind it; {len(caught)} caught, {len(left)} left"


def village_weather_solve(env, project_dir):
    write_game(project_dir, "village_weather", "// A village square (the scene is the game).\nexport {};\n")
    houses = [{"name": f"House{i + 1}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": "house"}}} for i, (x, z) in enumerate([(-7, -5), (7, -5), (0, 8)])]
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({"format": "pocket-scene", "entities": [
            {"name": "Ground", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 40}}, "MeshRenderer": {"mesh": "cube", "texture": "pattern:cobble", "texture_tile": 3}, "RigidBody": {"kind": "static"}, "Collider": {"shape": "box"}}},
            *houses,
            {"name": "Well", "components": {"Transform": {}, "MeshRenderer": {"mesh": "well"}}},
            {"name": "Sky", "components": {"Sky": {"mode": "atmosphere"}}},
            {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89}}, "Light": {"kind": 0}}},
            {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 14, "z": 20}, "rotation": {"x": -0.3, "y": 0, "z": 0, "w": 0.95}}, "Camera": {}}}]}, indent=1))
    return None


def village_weather_followup(env, project_dir):
    path = os.path.join(project_dir, "scene.json")
    scene = json.load(open(path, encoding="utf-8"))
    for e in scene["entities"]:
        if e["name"] == "Sky":
            e["components"]["Sky"]["time_of_day"] = 20.0
    scene["entities"].append({"name": "Weather", "components": {"Weather": {"rain": 0.8}}})
    for i, (x, z) in enumerate([(-5, -3), (5, -3), (-2, 6), (2, 6)]):
        scene["entities"].append({"name": f"Lamp{i + 1}", "components": {"Transform": {"position": {"x": x, "y": 0, "z": z}}, "MeshRenderer": {"mesh": "lamp"}},
                                  "children": [{"name": "Glow", "components": {"Transform": {"position": {"x": 0, "y": 2.8, "z": 0}}, "Light": {"kind": "point", "color": "#ffb060", "intensity": 3, "range": 8, "after_dark": True}}}]})
    json.dump(scene, open(path, "w", encoding="utf-8", newline="\n"), indent=1)
    return None


def village_weather_check(env, answer):
    import math
    def pos(name):
        try:
            return entity_pos(env, name)
        except Exception:   # noqa: BLE001 (missing)
            return None
    houses = [pos(f"House{i}") for i in (1, 2, 3)]
    if any(h is None for h in houses) or any(math.hypot(h["x"], h["z"]) > 10.5 for h in houses):
        return False, f"the first request's houses are not all there within 10 of the centre: {houses}"
    if pos("Well") is None:
        return False, "the first request's Well is gone"
    weathers = sorted((e for e in env.command("world.query", {"with": ["Weather"], "fields": ["Weather"]})["entities"] if e["Weather"].get("enabled", True)), key=lambda e: e["id"])
    if not weathers or abs(weathers[0]["Weather"].get("rain", 0) - 0.8) > 0.05:
        return False, f"the weather is {weathers[0]['Weather'] if weathers else None}, not rain at 0.8"
    skies = sorted((e for e in env.command("world.query", {"with": ["Sky"], "fields": ["Sky"]})["entities"] if e["Sky"].get("enabled", True)), key=lambda e: e["id"])
    t = skies[0]["Sky"].get("time_of_day", -1) if skies else -1
    if not (19.9 <= t <= 20.6):
        return False, f"the sky's time of day is {t}, not 20:00"
    lamps = []
    for i in range(1, 5):
        p = pos(f"Lamp{i}")
        if p is None:
            return False, f"no Lamp{i}"
        lid = env.command("world.find", {"path": f"Lamp{i}"})
        lights = [r for r in env.command("world.query", {"with": ["Light"], "fields": ["Light"], "under": lid})["entities"]]
        own = env.command("world.get", {"entity": lid, "component": "Light"})
        if own:
            lights.append({"Light": own})
        if not any(l["Light"].get("kind") == 1 and l["Light"].get("after_dark") for l in lights):
            return False, f"Lamp{i} has no point light that is on only after dark ({[l['Light'] for l in lights]})"
        near = min(math.hypot(p["x"] - h["x"], p["z"] - h["z"]) for h in houses)
        if near > 8:
            return False, f"Lamp{i} stands {near:.1f} from the nearest house"
        lamps.append(p)
    return True, "the square kept, rain at 0.8, 20:00, four lamps that light after dark by the houses"


def villagers_check(env, answer):
    import math

    def get(name, comp):
        try:
            return env.command("world.get", {"entity": name, "component": comp})
        except Exception:   # noqa: BLE001 (missing: said below)
            return None
    names = [f"Villager_{i}" for i in range(1, 6)]
    meshes = []
    for n in names:
        mr = get(n, "MeshRenderer")
        if not mr or not str(mr.get("mesh", "")).startswith("humanoid"):
            return False, f"{n} is not drawn by the built-in humanoid ({mr and mr.get('mesh')})"
        meshes.append(mr["mesh"])
    if len(set(meshes)) < 5:
        return False, f"the villagers' looks are not all different: {meshes}"
    if get("Player", "Transform") is None:
        return False, "no Player with a Transform"
    put_player = lambda x, z: env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": x, "y": 0, "z": z}}})  # noqa: E731
    put_player(20, 20)
    start = {n: entity_pos(env, n) for n in names}
    for n, p in start.items():
        if math.hypot(p["x"], p["z"]) > 4.2:
            return False, f"{n} starts at {p}, not within 4 of the centre"
    walking, moved = set(), set()
    for _ in range(8):
        env.command("step", {"ticks": 45})
        for n in names:
            a = get(n, "Animator") or {}
            if a.get("clip") in ("walk", "run"):
                walking.add(n)
            p = entity_pos(env, n)
            if math.hypot(p["x"], p["z"]) > 6.3:
                return False, f"{n} wandered to {p}, more than 6 from the centre"
            if math.hypot(p["x"] - start[n]["x"], p["z"] - start[n]["z"]) > 0.5:
                moved.add(n)
    if len(moved) < 4 or len(walking) < 4:
        return False, f"in six seconds {len(moved)} villagers moved and {len(walking)} played a walk ({sorted(walking)})"
    # The player beside one: it stops, faces the player and waves.
    v = entity_pos(env, "Villager_1")
    put_player(v["x"] + 1.5, v["z"] + 1.0)
    env.command("step", {"ticks": 40})
    a = get("Villager_1", "Animator") or {}
    here = entity_pos(env, "Villager_1")
    env.command("step", {"ticks": 30})
    there = entity_pos(env, "Villager_1")
    if a.get("clip") != "wave":
        return False, f"with the Player 1.8 away Villager_1 plays {a.get('clip')!r}, not wave"
    if math.hypot(there["x"] - here["x"], there["z"] - here["z"]) > 0.3:
        return False, f"waving, Villager_1 still walks ({here} -> {there})"
    rot = (get("Villager_1", "Transform") or {}).get("rotation", {})
    x, y, z, w = rot.get("x", 0), rot.get("y", 0), rot.get("z", 0), rot.get("w", 1)
    fwd = (-(2 * (x * z + w * y)), -(1 - 2 * (x * x + y * y)))   # the rotation applied to -Z, in XZ
    pl = entity_pos(env, "Player")
    to = (pl["x"] - there["x"], pl["z"] - there["z"])
    cosang = (fwd[0] * to[0] + fwd[1] * to[1]) / max(1e-6, math.hypot(*fwd) * math.hypot(*to))
    if cosang < math.cos(math.radians(35)):
        return False, f"Villager_1 faces {fwd}, not the Player at {to} from it (within 35 degrees)"
    put_player(20, 20)
    env.command("step", {"ticks": 150})
    a = get("Villager_1", "Animator") or {}
    if a.get("clip") == "wave":
        return False, "with the Player gone Villager_1 still waves"
    return True, f"five humanoids in five looks wander within 6 ({len(walking)} walking), one stops, faces the Player and waves, then wanders again"


FIREWORKS_TOML = GAME_TOML + 'launch = ["Space"]\n'
FIREWORKS_TS = """import { events, expose, input, onStart, onTick, particles, world } from "pocket";
const RISE = 12;
let launched = 0;
let bursts = 0;
let sparks = 0;
const rockets: Array<{ id: number; n: number; age: number }> = [];
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 10, z: 30 } }, Camera: { fov_degrees: 55 } } });
    sparks = world.spawn("Sparks", { components: { Transform: {}, ParticleEmitter: {
        emitting: false, max: 4000, lifetime: [1.5, 2.5], speed: [5, 9], spread: 180, gravity: { x: 0, y: -6, z: 0 }, drag: 0.4,
        size: [0.25, 0.05], color: { r: 1, g: 0.8, b: 0.3, a: 1 }, color_end: { r: 1, g: 0.2, b: 0.6, a: 0 }, additive: true } } });
});
onTick((t) => {
    if (input.pressed("launch")) {
        launched++;
        const id = world.spawn(`Rocket_${launched}`, { components: {
            Transform: { position: { x: 0, y: 0, z: 0 }, scale: { x: 0.2, y: 0.4, z: 0.2 } },
            MeshRenderer: { mesh: "capsule", emissive: { r: 1, g: 0.7, b: 0.3, a: 1 } },
            Trail: { time: 0.6, width: 0.3, color: { r: 1, g: 0.75, b: 0.3, a: 1 }, color_end: { r: 1, g: 0.3, b: 0.1, a: 0 }, additive: true } } });
        rockets.push({ id, n: launched, age: 0 });
    }
    for (let i = rockets.length - 1; i >= 0; i--) {
        const r = rockets[i];
        r.age += t.dt;
        const p = world.get(r.id, "Transform")!.position;
        if (r.age >= 1) {
            bursts++;
            particles.burst(sparks, 150, { at: p });
            events.emit("firework.burst", { n: r.n, x: p.x, y: p.y, z: p.z });
            world.destroy(r.id);
            rockets.splice(i, 1);
        } else {
            world.set(r.id, "Transform", { position: { x: p.x, y: p.y + RISE * t.dt, z: p.z } });
        }
    }
});
expose("launched", () => launched);
expose("bursts", () => bursts);
"""


def fireworks_solve(env, project_dir):
    write_game(project_dir, "fireworks", FIREWORKS_TS)
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(FIREWORKS_TOML.format(name=os.path.basename(project_dir)))
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write('{"format": "pocket-scene", "entities": [{"name": "Night", "components": {"Sky": {"mode": "off"}}}]}\n')
    return None


def fireworks_check(env, answer):
    if "launch" not in env.command("input.actions", {}):
        return False, "no launch action"
    state = lambda: env.command("state", {})["state"]  # noqa: E731
    stats = lambda: env.command("particles.stats", {})  # noqa: E731
    env.command("step", {"ticks": 5})
    seq = env.command("events.last_seq", {})["seq"]
    spawned0 = stats().get("spawned", 0)
    env.command("input.press", {"action": "launch"})
    env.command("step", {"ticks": 30})
    try:
        trail = env.command("world.get", {"entity": "Rocket_1", "component": "Trail"})
    except Exception as e:   # noqa: BLE001 (missing: said)
        return False, f"no Rocket_1 half a second after a launch: {e}"
    if not trail:
        return False, "Rocket_1 has no Trail"
    p = entity_pos(env, "Rocket_1")
    if not (4.5 < p["y"] < 7.5) or abs(p["x"]) > 0.3 or abs(p["z"]) > 0.3:
        return False, f"half a second after its launch Rocket_1 is at {p}, not about 6 up from the origin"
    env.command("step", {"ticks": 36})
    burst = env.command("events.since", {"seq": seq, "type": "firework.burst"})["events"]
    if not burst:
        return False, "no firework.burst a second after the launch"
    data = burst[0].get("data", {})
    if data.get("n") != 1 or not (10.5 < float(data.get("y", 0)) < 13.5):
        return False, f"the burst says {data}, not rocket 1 at about 12 up"
    try:
        gone = env.command("world.find", {"path": "Rocket_1"})
    except Exception:   # noqa: BLE001
        gone = None
    if isinstance(gone, int):
        return False, "Rocket_1 is still there after it burst"
    s = stats()
    if s.get("spawned", 0) - spawned0 < 100:
        return False, f"the burst made {s.get('spawned', 0) - spawned0} particles, not at least 100"
    pools = s.get("pools", [])
    additive = False
    for pool in pools:
        try:
            em = env.command("world.get", {"entity": pool["entity"], "component": "ParticleEmitter"})
        except Exception:   # noqa: BLE001
            em = None
        if em and em.get("additive"):
            additive = True
    if not additive:
        return False, "no emitter of the sparks glows (additive)"
    # Two rockets up at once.
    env.command("input.press", {"action": "launch"})
    env.command("step", {"ticks": 6})
    env.command("input.press", {"action": "launch"})
    env.command("step", {"ticks": 6})
    for n in (2, 3):
        if entity_pos(env, f"Rocket_{n}") is None:
            return False, f"no Rocket_{n} after two more launches"
    env.command("step", {"ticks": 70})
    st = state()
    if st.get("launched") != 3 or st.get("bursts") != 3:
        return False, f"after three launches the state says {st}, not launched 3 and bursts 3"
    return True, "a rocket with a trail rises 6 in half a second, bursts at 12 into 100 or more glowing sparks, three in all"


GLADE_TOML = GAME_TOML + 'stoke = ["F"]\n'
GLADE_TS = """import { audio, input, onStart, onTick, particles, world } from "pocket";
const at = (x: number, z: number) => ({ position: { x, y: 0, z } });
onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 4, z: 12 }, rotation: { x: -0.15, y: 0, z: 0, w: 0.989 } }, Camera: {} } });
    world.spawn("Sun", { components: { Transform: { rotation: { x: -0.3, y: 0.3, z: 0.1, w: 0.9 } }, Light: { kind: "directional", intensity: 0.6 } } });
    world.spawn("Ground", { components: { Transform: { position: { x: 0, y: -0.5, z: 0 }, scale: { x: 40, y: 1, z: 40 } }, MeshRenderer: { mesh: "cube", texture: "pattern:grass", texture_tile: 3 }, RigidBody: { kind: "static" }, Collider: { size: { x: 20, y: 0.5, z: 20 } } } });
    [[-6, -4], [5, -5], [-3, -9], [7, 2]].forEach(([x, z], i) => world.spawn(`Tree_${i}`, { components: { Transform: at(x, z), MeshRenderer: { mesh: i % 2 ? "pine?seed=" + i : "tree?seed=" + i } } }));
    [[2, 2], [-2.5, 1.5], [3, -2]].forEach(([x, z], i) => world.spawn(`Rock_${i}`, { components: { Transform: at(x, z), MeshRenderer: { mesh: "rock?seed=" + i } } }));
    const fire = world.spawn("Campfire", { components: { Transform: at(0, 0), Light: { kind: "point", color: { r: 1, g: 0.6, b: 0.3, a: 1 }, intensity: 5, range: 6 } } });
    particles.preset(fire, "fire");
    particles.preset(world.spawn("Smoke", { components: { Transform: { position: { x: 0, y: 1, z: 0 } } } }), "smoke");
    world.spawn("Sparks", { components: { Transform: { position: { x: 0, y: 0.3, z: 0 } } } });
    particles.preset("Sparks", "sparks");
});
onTick(() => {
    if (input.pressed("stoke")) {
        audio.play("sfx:hit");
        particles.burst("Sparks", 25);
    }
});
"""


def glade_solve(env, project_dir):
    write_game(project_dir, "glade", GLADE_TS)
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(GLADE_TOML.format(name=os.path.basename(project_dir)))
    return None


def glade_check(env, answer):
    if "stoke" not in env.command("input.actions", {}):
        return False, "no stoke action"
    # No files of its own: no image, model or sound under the project.
    listed = env.command("assets.list", {})
    files = [a.get("path", "") for a in (listed.get("assets", []) if isinstance(listed, dict) else listed)]
    own = [p for p in files if os.path.splitext(p)[1].lower() in (".png", ".jpg", ".jpeg", ".svg", ".glb", ".gltf", ".obj", ".wav", ".ogg", ".mp3", ".flac", ".sfx")]
    if own:
        return False, f"the glade brought files of its own: {own[:5]}"
    env.command("step", {"ticks": 90})
    found = env.command("world.query", {"with": ["MeshRenderer"], "fields": ["MeshRenderer.mesh", "MeshRenderer.texture", "Transform.scale"], "limit": 500})
    ents = found.get("entities", [])
    ground = [e for e in ents if str(e.get("MeshRenderer", {}).get("texture", "")).startswith("pattern:grass") and min(e.get("Transform", {}).get("scale", {}).get("x", 0), e.get("Transform", {}).get("scale", {}).get("z", 0)) >= 20]
    if not ground:
        textures = sorted({str(e.get("MeshRenderer", {}).get("texture", "")) for e in ents})
        return False, f"no ground 20 or more across textured with grass (textures: {textures[:8]})"
    meshes = [str(e.get("MeshRenderer", {}).get("mesh", "")).split("?")[0] for e in ents]
    trees = sum(1 for m in meshes if m in ("tree", "pine"))
    rocks = sum(1 for m in meshes if m == "rock")
    if trees < 4 or rocks < 3:
        return False, f"{trees} trees and {rocks} rocks, not at least four and three"
    # The campfire: at the origin, a warm point light, glowing particles that rise.
    p = entity_pos(env, "Campfire")
    if p is None or abs(p["x"]) > 0.5 or abs(p["z"]) > 0.5:
        return False, f"no Campfire at the origin ({p})"
    camp = env.command("world.find", {"path": "Campfire"})
    def near(comp, within):
        hits = env.command("world.query", {"with": [comp, "Transform"], "limit": 200}).get("entities", [])
        out = []
        for h in hits:
            q = env.command("world.get", {"entity": h["id"], "component": "WorldTransform"})["position"]
            if abs(q["x"] - p["x"]) <= within and abs(q["z"] - p["z"]) <= within:
                out.append(h["id"])
        return out
    lights = [i for i in near("Light", 1.0) if env.command("world.get", {"entity": i, "component": "Light"}).get("kind") == 1]
    if not lights:
        return False, "no point light at the campfire"
    stats = env.command("particles.stats", {})
    rising_glow, smoke = False, False
    for pool in stats.get("pools", []):
        em = env.command("world.get", {"entity": pool["entity"], "component": "ParticleEmitter"})
        q = env.command("world.get", {"entity": pool["entity"], "component": "WorldTransform"})["position"]
        if abs(q["x"] - p["x"]) > 1.0 or abs(q["z"] - p["z"]) > 1.0 or pool.get("alive", 0) < 5:
            continue
        lst = env.command("particles.list", {"entity": pool["entity"], "limit": 0})
        up = lst.get("bounds", {}).get("max", {}).get("y", -1e9) > q["y"] + 0.3
        if em.get("additive") and up:
            rising_glow = True
        elif up and not em.get("additive"):
            smoke = True
    if not rising_glow:
        return False, "no glowing (additive) particles rising from the campfire"
    if not smoke:
        return False, "no smoke (particles that are not additive) rising over the campfire"
    # Stoking: a sound and a burst of sparks.
    seq = env.command("events.last_seq", {})["seq"]
    spawned0 = env.command("particles.stats", {}).get("spawned", 0)
    env.command("input.press", {"action": "stoke"})
    env.command("step", {"ticks": 3})
    played = env.command("events.since", {"seq": seq, "type": "audio.started"})["events"]
    if not played:
        return False, "stoking played no sound"
    if env.command("particles.stats", {}).get("spawned", 0) - spawned0 < 10:
        return False, "stoking burst no sparks"
    lint = env.command("world.lint", {})
    if lint.get("errors", 0):
        return False, f"world.lint finds errors: {[x.get('problem') for x in lint.get('problems', []) if x.get('severity') == 'error'][:3]}"
    return True, f"a grass ground, {trees} trees and {rocks} rocks without files, a campfire with a point light, glowing flames and smoke, a sound and sparks on stoke"


def watchman_check(env, answer):
    def get(name, comp):
        try:
            return env.command("world.get", {"entity": name, "component": comp})
        except Exception:   # noqa: BLE001 (a missing entity: said below)
            return None
    for name, comp in (("Watchman", "Behavior"), ("Watchman", "NavAgent"), ("Beat", "Path"), ("Player", "Transform")):
        if get(name, comp) is None:
            return False, f"no {name} with a {comp}"
    def state():
        b = get("Watchman", "Behavior")
        s = next((x for x in b["states"] if x["name"] == b["state"]), None)
        return b, s
    def put_player(x, z):
        env.command("world.set", {"entity": "Player", "component": "Transform", "value": {"position": {"x": x, "y": 0.5, "z": z}}})
    put_player(8, -8)
    env.command("step", {"ticks": 30})
    b, s = state()
    if not s or s["move"] != 5:
        return False, f"with the Player far away the Watchman is in state {b['state']!r}, not walking its beat (a patrol state): {b.get('error')}"
    a = get("Watchman", "NavAgent")
    ends = [(-6, 6), (6, 6)]
    if not any(abs(a["goal"]["x"] - x) < 1 and abs(a["goal"]["z"] - z) < 1 for x, z in ends) or not near(a["speed"], 2, 0.05):
        return False, f"on its beat the Watchman heads for {a['goal']} at {a['speed']}, not an end of the beat at 2"
    w = entity_pos(env, "Watchman")
    put_player(w["x"] + 3, w["z"] - 1.5)
    env.command("step", {"ticks": 10})
    b, s = state()
    a = get("Watchman", "NavAgent")
    if not s or s["move"] != 1 or not near(a["speed"], 4, 0.05):
        return False, f"with the Player 3.4 away in the open the Watchman is in {b['state']!r} at speed {a['speed']}, not running after it at 4"
    put_player(w["x"] + 7.5, w["z"] - 7.5)   # out of sight: 10 away
    env.command("step", {"ticks": 30})
    b, s = state()
    if s and s["move"] == 5:
        return False, "half a second after losing the Player the Watchman was back on its beat already"
    put_player(9, -9)
    env.command("step", {"ticks": 150})
    b, s = state()
    if not s or s["move"] != 5:
        return False, f"three seconds after losing the Player the Watchman is in {b['state']!r}, not back on its beat"
    return True, f"walks its beat at 2, chases at 4 what it sees, back on the beat after the Player is out of sight 2 s ({len(b['states'])} states)"


def grey_flashback_solve(env):
    code = "fn effect(uv: vec2f) -> vec4f { let c = sample_frame(uv); let g = dot(c.rgb, vec3f(0.299, 0.587, 0.114)); return vec4f(g, g, g, c.a); }"
    env.command("render.post", {"effects": [{"code": code, "name": "grey"}]})
    return None


def frame_pixel(env, point):
    env.command("step", {"ticks": 1})
    r = env.command("render.project", {"point": point})
    return env.command("capture", {"pixel": {"x": r["x"], "y": r["y"]}})["pixel"]


def grey_flashback_check(env, answer):
    running = env.command("render.post", {})["effects"]
    if not running:
        return False, "no post effect is running"
    # A crate's brown and the car's red, grey now.
    for name in ("Crate_0_0", "Car"):
        e = env.command("world.find", {"path": name})
        p = env.command("world.get", {"entity": e, "component": "Transform"})["position"]
        px = frame_pixel(env, p)
        if max(px[:3]) - min(px[:3]) > 6:
            return False, f"{name} is drawn {px[:3]}, not grey"
        if max(px[:3]) < 30:
            return False, f"{name} is drawn black ({px[:3]})"
    return True, f"{len(running)} effect(s); the crates and the car are grey"


COIN_SVG = """<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">
  <circle cx="16" cy="16" r="14" fill="#f5c542" stroke="#b8860b" stroke-width="4"/>
</svg>
"""


def svg_coin_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with open(os.path.join(project_dir, "assets", "coin.svg"), "w", encoding="utf-8", newline="\n") as f:
        f.write(COIN_SVG)
    edit_main(project_dir, lambda t: t.replace("onStart(() => {\n", 'onStart(() => {\n    world.spawn("CoinBadge", { components: { Transform: { position: { x: 2, y: 2.5, z: 0 } }, Sprite: { texture: "assets/coin.svg", size: { x: 1, y: 1 } } } });\n', 1))
    return None


def svg_coin_check(env, answer):
    sp = env.command("world.get", {"entity": "CoinBadge", "component": "Sprite"})
    if not sp:
        return False, "no entity CoinBadge with a Sprite after the game starts again"
    tex = sp["texture"]
    if not tex.split("?")[0].endswith(".svg"):
        return False, f"CoinBadge's texture is {tex!r}, not an SVG of the project's"
    if not (near(sp["size"]["x"], 1, 0.05) and near(sp["size"]["y"], 1, 0.05)):
        return False, f"CoinBadge is {sp['size']} across, not 1 by 1"
    d = env.command("assets.describe", {"path": tex})
    if not d or d.get("kind") not in (None, "image") or d.get("error"):
        return False, f"the engine cannot read {tex}: {d}"
    p = env.command("world.get", {"entity": "CoinBadge", "component": "Transform"})["position"]
    if not (near(p["x"], 2, 0.05) and near(p["y"], 2.5, 0.05) and near(p["z"], 0, 0.05)):
        return False, f"CoinBadge is at {p}, not (2, 2.5, 0)"
    mid = frame_pixel(env, {"x": 2, "y": 2.5, "z": 0.01})
    rim = frame_pixel(env, {"x": 2.42, "y": 2.5, "z": 0.01})
    gold = mid[0] > 190 and mid[1] > 140 and mid[2] < 120
    darker = rim[0] + rim[1] + rim[2] < mid[0] + mid[1] + mid[2] - 40
    if not gold or not darker:
        return False, f"the coin's middle draws {mid} and its rim {rim}: not a gold disc with a darker rim"
    return True, f"an SVG coin, gold {mid[:3]} with a darker rim {rim[:3]}"


def orange_ball_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "materials"), exist_ok=True)
    with open(os.path.join(project_dir, "materials", "orange.wgsl"), "w", encoding="utf-8", newline="\n") as f:
        f.write("fn material(lit: vec4f, s: Surface) -> vec4f { return vec4f(1.0, 0.133, 0.0, 1.0); }\n")
    main = os.path.join(project_dir, "scripts", "main.ts")
    with open(main, encoding="utf-8") as f:
        text = f.read()
    text = text.replace('MeshRenderer: { mesh: "sphere", color:', 'MeshRenderer: { mesh: "sphere", material: "materials/orange.wgsl", color:', 1)
    with open(main, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    return None


def orange_ball_check(env, answer):
    mr = env.command("world.get", {"entity": "Ball", "component": "MeshRenderer"})
    path = mr.get("material", "")
    if not path:
        return False, "the Ball has no material"
    for pr in env.command("world.lint", {})["problems"]:
        if pr.get("component") == "MeshRenderer" and pr.get("severity") == "error":
            return False, f"the lint says: {pr.get('problem', pr)}"
    p = env.command("world.get", {"entity": "Ball", "component": "Transform"})["position"]
    px = frame_pixel(env, p)
    if not (px[0] > 200 and 30 < px[1] < 150 and px[2] < 50):
        return False, f"the Ball is drawn {px[:3]}, not orange"
    # Flat: the same orange at its edge as at its centre (no light falling off).
    q = frame_pixel(env, {"x": p["x"] + 0.35, "y": p["y"], "z": p["z"]})
    if abs(q[0] - px[0]) > 12 or abs(q[1] - px[1]) > 12:
        return False, f"the Ball is {px[:3]} at its centre and {q[:3]} toward its edge, not flat"
    return True, f"the Ball is drawn {px[:3]} through {path}"


def brute_enemies_solve(env):
    rows = env.command("world.query", {"with": ["Enemy"]})["entities"]
    for row in rows:
        env.command("world.set", {"entity": row["id"], "component": "Enemy", "value": {"kind": "brute", "damage": 25}})
    return len(rows)


def brute_enemies_check(env, answer):
    rows = env.command("world.query", {"with": ["Enemy"], "fields": ["Enemy"]})["entities"]
    if not rows:
        return False, "no enemies left to check"
    wrong = [r["path"] for r in rows if r["Enemy"]["kind"] != 1 or not near(r["Enemy"]["damage"], 25)]
    if wrong:
        return False, f"not brutes doing 25: {wrong}"
    if answer != len(rows):
        return False, f"answered {answer!r}, {len(rows)} enemies"
    return True, f"{len(rows)} enemies, all brutes doing 25"


def wait_for_coin_solve(env):
    env.command("input.hold", {"action": "move_x", "ticks": 600})
    r = env.command("step", {"ticks": 600, "until": {"event": "coin.collected"}})
    return r["until"]["event"]["tick"]


def wait_for_coin_check(env, answer):
    coins = env.command("events.since", {"seq": 0, "type": "coin.collected", "limit": 4000})["events"]
    if not coins:
        return False, "no coin was collected"
    if answer != coins[0]["tick"]:
        return False, f"answered {answer!r}, the first coin went at tick {coins[0]['tick']}"
    return True, f"the first coin went at tick {answer}"


def night_level_solve(env):
    env.command("world.set", {"entity": "Level", "component": "TileMap", "value": {"lit": True}})
    env.command("world.set", {"entity": "Player", "component": "Sprite", "value": {"lit": True}})
    env.command("render.ambient", {"color": "#202840", "intensity": 0.3})
    env.command("world.spawn", {"name": "Torchlight", "parent": "Player", "components": {"Transform": {"position": {"x": 0, "y": 0, "z": 0.5}}, "Light": {"kind": "point", "color": "#ffb060", "range": 5, "intensity": 2}}})


def night_level_check(env, answer):
    tm = env.command("world.get", {"entity": "Level", "component": "TileMap"})
    sp = env.command("world.get", {"entity": "Player", "component": "Sprite"})
    if not (tm and tm.get("lit")) or not (sp and sp.get("lit")):
        return False, f"the map lit {tm and tm.get('lit')}, the player's sprite lit {sp and sp.get('lit')}"
    amb = env.command("render.ambient", {})
    want = (0x20 / 255, 0x28 / 255, 0x40 / 255)
    if not (near(amb["intensity"], 0.3, 0.02) and all(near(amb["color"][k], w, 0.02) for k, w in zip("rgb", want))):
        return False, f"the ambient is {amb}, not #202840 at 0.3"
    env.command("input.hold", {"action": "move_x", "ticks": 60})
    env.command("step", {"ticks": 60})
    p = entity_pos(env, "Player")
    lights = env.command("world.query", {"with": ["Light", "WorldTransform"], "fields": ["Light", "WorldTransform"]})["entities"]
    for e in lights:
        l, w = e["Light"], e["WorldTransform"]["position"]
        if l["kind"] != 1 or not color_is(l["color"], 1.0, 0xb0 / 255, 0x60 / 255) or not near(l["range"], 5, 0.01):
            continue
        if abs(w["x"] - p["x"]) < 1.0 and abs(w["y"] - p["y"]) < 1.0 and w["z"] > p["z"]:
            return True, f"a warm light at {w} by the player at {p} after it walked"
    return False, f"no warm point light of range 5 in front of the player at {p} after it walked: {[(e['Light']['kind'], e['WorldTransform']['position']) for e in lights][:6]}"


def guard_view_solve(env):
    p = entity_pos(env, "Player")
    t = entity_pos(env, "Torch_6")
    cells = env.command("tilemap.fov", {"entity": "Level", "from": {"x": p["x"], "y": p["y"]}, "radius": 6})["count"]
    sees = env.command("tilemap.sight", {"entity": "Level", "from": {"x": t["x"], "y": t["y"]}, "to": {"x": p["x"], "y": p["y"]}})["visible"]
    return {"cells": cells, "sees": sees}


def guard_view_check(env, answer):
    want = guard_view_solve(env)
    if not isinstance(answer, dict):
        return False, f"answered {answer!r}, not an object with cells and sees"
    if answer.get("cells") != want["cells"] or answer.get("sees") is not want["sees"]:
        return False, f"answered {answer}, the map says {want}"
    return True, f"{want['cells']} cells in view, the torch {'sees' if want['sees'] else 'does not see'} the player"



# Diagnose and fix: a game with something wrong in it, a report of what a player sees, and a fix
# that has to leave the rest of the game as it was. The check compares the fixed game with the
# broken one where they should agree (measured in the same runtime before the agent starts).

def write_project(project_dir, script, scene=EMPTY_SCENE, toml=GAME_TOML):
    with open(os.path.join(project_dir, "project.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write(toml.format(name=os.path.basename(project_dir)))
    with open(os.path.join(project_dir, "scene.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(scene)
    with open(os.path.join(project_dir, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(script)


# A swarm: bees fly to the nearest flower, take a grain of pollen and carry it home; a flower that
# has given five grains wilts and grows again further round. Every bee queries every flower every
# tick, which is where the time goes.
SWARM_TS = """import { expose, onStart, onTick, world } from "pocket";

const BEES = 120;
const FLOWERS = 16;
const SPEED = 4;
const carrying: boolean[] = [];
const target: number[] = [];
const given: number[] = [];
const turn: number[] = [];
let pollen = 0;

function flowerAt(i: number): { x: number; y: number; z: number } {
    const a = (i / FLOWERS) * Math.PI * 2 + turn[i] * 0.7;
    const r = turn[i] % 2 === 0 ? 12 : 8;
    return { x: Math.cos(a) * r, y: 0, z: Math.sin(a) * r };
}

onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 22, z: 18 }, rotation: { x: -0.5, y: 0, z: 0, w: 0.866 } }, Camera: {} } });
    world.spawn("Sun", { components: { Transform: { rotation: { x: -0.4, y: 0.3, z: 0.1, w: 0.86 } }, Light: { kind: 0, intensity: 2 } } });
    world.spawn("Hive", { components: { Transform: { position: { x: 0, y: 0.5, z: 0 } }, MeshRenderer: { mesh: "cube", color: [0.6, 0.45, 0.2] } } });
    for (let i = 0; i < FLOWERS; i++) {
        turn.push(0);
        given.push(0);
        world.spawn(`Flower_${i}`, { components: { Transform: { position: flowerAt(i), scale: { x: 0.6, y: 0.6, z: 0.6 } }, MeshRenderer: { mesh: "sphere", color: [1, 0.4, 0.7] } } });
    }
    for (let i = 0; i < BEES; i++) {
        carrying.push(false);
        target.push(-1);
        world.spawn(`Bee_${i}`, { components: { Transform: { position: { x: (i % 12) - 5.5, y: 1, z: Math.floor(i / 12) - 4.5 }, scale: { x: 0.2, y: 0.2, z: 0.2 } }, MeshRenderer: { mesh: "sphere", color: [1, 0.85, 0.1] } } });
    }
});

// Each bee heads for the nearest flower, or home with a grain.
onTick(function fly({ dt }) {
    for (let i = 0; i < BEES; i++) {
        const bee = `Bee_${i}`;
        const p = world.get(bee, "Transform")!.position;
        let goal = { x: 0, y: 1, z: 0 };
        if (!carrying[i]) {
            let best = Infinity;
            for (const f of world.query({ name: "Flower_*", with: ["Transform"] })) {
                const q = f.Transform!.position;
                const d = Math.hypot(q.x - p.x, q.z - p.z);
                if (d < best) {
                    best = d;
                    goal = { x: q.x, y: 1, z: q.z };
                    target[i] = Number(f.path.slice("/Flower_".length));
                }
            }
        }
        const dx = goal.x - p.x;
        const dz = goal.z - p.z;
        const d = Math.hypot(dx, dz);
        if (d <= SPEED * dt) {
            world.set(bee, "Transform", { position: goal });
            if (carrying[i]) {
                carrying[i] = false;
                pollen += 1;
            } else {
                carrying[i] = true;
                const k = target[i];
                given[k] += 1;
                if (given[k] % 5 === 0) {
                    turn[k] += 1;
                    world.set(`Flower_${k}`, "Transform", { position: flowerAt(k) });
                }
            }
        } else {
            world.set(bee, "Transform", { position: { x: p.x + (dx / d) * SPEED * dt, y: 1, z: p.z + (dz / d) * SPEED * dt } });
        }
    }
});

expose("pollen", () => pollen);
expose("carrying", () => carrying.filter((c) => c).length);
"""

# The same game with the flowers read once a tick (and a flower that moves read again, as the
# game read it): what the reference solution writes.
SWARM_FAST_TS = SWARM_TS.replace("""    for (let i = 0; i < BEES; i++) {
        const bee = `Bee_${i}`;""", """    const flowers = world.query({ name: "Flower_*", with: ["Transform"] }).map((f) => ({ k: Number(f.path.slice("/Flower_".length)), p: f.Transform!.position }));
    for (let i = 0; i < BEES; i++) {
        const bee = `Bee_${i}`;""").replace("""            for (const f of world.query({ name: "Flower_*", with: ["Transform"] })) {
                const q = f.Transform!.position;""", """            for (const f of flowers) {
                const q = f.p;""").replace("""                    target[i] = Number(f.path.slice("/Flower_".length));""", """                    target[i] = f.k;""").replace("""                    world.set(`Flower_${k}`, "Transform", { position: flowerAt(k) });""", """                    world.set(`Flower_${k}`, "Transform", { position: flowerAt(k) });
                    for (const f of flowers) if (f.k === k) f.p = world.get(`Flower_${k}`, "Transform")!.position;""")
assert SWARM_FAST_TS.count("world.query") == 1


def swarm_setup(project_dir):
    write_project(project_dir, SWARM_TS)


def swarm_look(env, ticks):
    """Reload the game, run it `ticks`, and answer where it got to and what its script cost a tick
    over the last 60 of them."""
    env.command("project.reload", {})
    env.command("step", {"ticks": ticks - 60})
    env.command("script.profile", {"reset": True})
    env.command("step", {"ticks": 60})
    cost = env.command("script.profile", {})["script_ms_per_tick"]
    st = env.command("state", {})["state"]
    bees = {r["path"]: r["Transform"]["position"] for r in env.command("world.query", {"name": "Bee_*", "with": ["Transform"]})["entities"]}
    flowers = {r["path"]: r["Transform"]["position"] for r in env.command("world.query", {"name": "Flower_*", "with": ["Transform"]})["entities"]}
    return {"pollen": st.get("pollen"), "carrying": st.get("carrying"), "bees": bees, "flowers": flowers, "ms": cost}


def swarm_before(env):
    env.task_state = swarm_look(env, 360)
    env.command("project.reload", {})
    s = env.task_state
    if not s["pollen"] or s["ms"] <= 0:
        return False, f"the swarm did not run: {s['pollen']} pollen, {s['ms']} ms a tick"
    return True, ""


def swarm_solve(env, project_dir):
    with open(os.path.join(project_dir, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(SWARM_FAST_TS)
    return None


def same_places(a, b, tol=1e-3):
    if set(a) != set(b):
        return f"entities {sorted(set(a) ^ set(b))[:4]} are in one run and not the other"
    for k in a:
        if any(abs(a[k][c] - b[k][c]) > tol for c in ("x", "y", "z")):
            return f"{k} is at {a[k]} instead of {b[k]}"
    return ""


def swarm_check(env, answer):
    was = getattr(env, "task_state", None)
    if not was:
        return False, "no measurement of the game before the fix"
    now = swarm_look(env, 360)
    if now["pollen"] != was["pollen"] or now["carrying"] != was["carrying"]:
        return False, f"the game plays differently: {now['pollen']} pollen and {now['carrying']} carrying after 6 seconds, not {was['pollen']} and {was['carrying']}"
    for what in ("bees", "flowers"):
        why = same_places(now[what], was[what])
        if why:
            return False, f"the game plays differently: {why}"
    if now["ms"] > was["ms"] * 0.25:
        return False, f"the scripts take {now['ms']:.3f} ms a tick, not under a quarter of the {was['ms']:.3f} ms they took"
    return True, f"same swarm, {was['ms']:.2f} -> {now['ms']:.2f} ms of script a tick"


# A cannon: a pellet every tenth of a second, arcing up and landing; a landed pellet is meant to
# be cleared away, but the sweep looks for pellets below the ground, and a landed one rests on it.
CANNON_TS = """import { events, expose, onStart, onTick, world } from "pocket";
import type { Entity } from "pocket";

interface Pellet { id: Entity; x: number; y: number; vx: number; vy: number; landed: boolean }
const pellets: Pellet[] = [];
let fired = 0;
let landed = 0;

onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 4, z: 14 } }, Camera: { orthographic: true, ortho_size: 7 } } });
    world.spawn("Ground", { components: { Transform: { position: { x: 0, y: -0.1, z: 0 } }, Sprite: { size: { x: 30, y: 0.2 }, color: { r: 0.3, g: 0.6, b: 0.3, a: 1 } } } });
    world.spawn("Cannon", { components: { Transform: { position: { x: 0, y: 0.3, z: 0 } }, Sprite: { size: { x: 0.8, y: 0.6 }, color: { r: 0.2, g: 0.2, b: 0.25, a: 1 } } } });
});

// Every sixth tick a pellet, sent up at an angle that sweeps from left to right and back.
onTick(function fire({ tick }) {
    if (tick % 6 !== 0) return;
    fired += 1;
    const a = Math.PI / 2 + Math.sin(fired * 0.37) * 0.6;
    const id = world.spawn(`Pellet_${fired}`, { components: { Transform: { position: { x: 0, y: 0.5, z: 0 } }, Sprite: { size: { x: 0.2, y: 0.2 }, color: { r: 1, g: 0.6, b: 0.2, a: 1 } } } });
    pellets.push({ id, x: 0, y: 0.5, vx: Math.cos(a) * 8, vy: Math.sin(a) * 8, landed: false });
});

// Pellets fly under gravity until they reach the ground, where they stop.
onTick(function fly({ dt }) {
    for (const p of pellets) {
        if (p.landed) continue;
        p.vy -= 9.8 * dt;
        p.x += p.vx * dt;
        p.y += p.vy * dt;
        if (p.y <= 0) {
            p.y = 0;
            p.landed = true;
            landed += 1;
            events.emit("pellet.landed", { x: p.x });
        }
        world.set(p.id, "Transform", { position: { x: p.x, y: p.y, z: 0 } });
    }
});

// Landed pellets are cleared away.
onTick(function sweep() {
    for (let i = pellets.length - 1; i >= 0; i--) {
        if (pellets[i].y < 0) {
            world.destroy(pellets[i].id);
            pellets.splice(i, 1);
        }
    }
});

expose("fired", () => fired);
expose("landed", () => landed);
"""

CANNON_FIXED_TS = CANNON_TS.replace("        if (pellets[i].y < 0) {", "        if (pellets[i].landed) {")


def cannon_setup(project_dir):
    write_project(project_dir, CANNON_TS)


def cannon_look(env, ticks):
    env.command("project.reload", {})
    seq = env.command("events.last_seq", {})["seq"]
    env.command("step", {"ticks": ticks})
    st = env.command("state", {})["state"]
    flying = {r["path"]: r["Transform"]["position"] for r in env.command("world.query", {"name": "Pellet_*", "with": ["Transform"], "limit": 100000})["entities"] if r["Transform"]["position"]["y"] > 0}
    pellets = env.command("world.query", {"name": "Pellet_*", "limit": 100000})["count"]
    landings = env.command("events.since", {"seq": seq, "type": "pellet.landed", "limit": 100000})["events"]
    return {"fired": st.get("fired"), "landed": st.get("landed"), "flying": flying, "pellets": pellets, "landings": [round(e["data"]["x"], 4) for e in landings]}


def cannon_before(env):
    env.task_state = cannon_look(env, 1800)
    env.command("project.reload", {})
    s = env.task_state
    if s["pellets"] < 250:
        return False, f"the cannon did not leave its pellets about: {s['pellets']} after 30 seconds"
    return True, ""


def cannon_solve(env, project_dir):
    with open(os.path.join(project_dir, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(CANNON_FIXED_TS)
    return None


def cannon_check(env, answer):
    was = getattr(env, "task_state", None)
    if not was:
        return False, "no measurement of the game before the fix"
    now = cannon_look(env, 1800)
    if now["fired"] != was["fired"] or now["landed"] != was["landed"]:
        return False, f"the cannon plays differently: fired {now['fired']}, landed {now['landed']} after 30 seconds, not {was['fired']} and {was['landed']}"
    if now["landings"] != was["landings"]:
        return False, "the pellets land in other places than they did"
    why = same_places(now["flying"], was["flying"])
    if why:
        return False, f"the pellets in the air fly differently: {why}"
    if now["pellets"] > len(now["flying"]) + 2:
        return False, f"{now['pellets']} pellets after 30 seconds, {len(now['flying'])} of them in the air: the landed ones still pile up"
    return True, f"{was['pellets']} pellets left about before, {now['pellets']} now, the same {now['landed']} landings"


# Crates dropped on a floor; the blue one falls through it. The floor shares a negative collision
# group with the ghost, which drifts through it on purpose, and the blue crate was put in that
# group too.
def crate_scene():
    def box(name, x, y, color, group=0, extra=None):
        c = {"Transform": {"position": {"x": x, "y": y, "z": 0}}, "MeshRenderer": {"mesh": "cube", "color": color},
             "RigidBody": {"kind": "dynamic"}, "Collider": {"size": {"x": 0.5, "y": 0.5, "z": 0.5}, "group": group}}
        c.update(extra or {})
        return {"name": name, "components": c}
    ents = [
        {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 3, "z": 12}}, "Camera": {}}},
        {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.4, "y": 0.3, "z": 0.1, "w": 0.86}}, "Light": {"kind": 0, "intensity": 2}}},
        {"name": "Floor", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 16, "y": 1, "z": 6}}, "MeshRenderer": {"mesh": "cube", "color": [0.5, 0.5, 0.5]},
                                         "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 8, "y": 0.5, "z": 3}, "group": -2}}},
        box("RedCrate", -3, 3, [0.9, 0.2, 0.2]),
        box("GreenCrate", 0, 4, [0.2, 0.8, 0.3], 0, {"Collider": {"size": {"x": 0.5, "y": 0.5, "z": 0.5}, "layer": 1, "mask": 4294967295}}),
        box("BlueCrate", 3, 3.5, [0.2, 0.3, 0.9], -2),
        {"name": "Ghost", "components": {"Transform": {"position": {"x": 6, "y": 3, "z": 0}, "scale": {"x": 0.8, "y": 0.8, "z": 0.8}}, "MeshRenderer": {"mesh": "sphere", "color": [0.9, 0.9, 1.0]},
                                         "RigidBody": {"kind": "dynamic"}, "Collider": {"shape": 1, "size": {"x": 0.4, "y": 0.4, "z": 0.4}, "group": -2}}},
    ]
    return json.dumps({"format": "pocket-scene", "entities": ents}, indent=1) + "\n"


CRATES_TS = """import { expose, onTick, world } from "pocket";

// How many crates are resting on the floor (within a hand of it, hardly moving).
let resting = 0;
onTick(() => {
    resting = 0;
    for (const name of ["RedCrate", "GreenCrate", "BlueCrate"]) {
        const t = world.get(name, "Transform");
        if (t && Math.abs(t.position.y - 0.5) < 0.1) resting += 1;
    }
});
expose("resting", () => resting);
"""


def sinking_setup(project_dir):
    write_project(project_dir, CRATES_TS, scene=crate_scene())


def sinking_solve(env, project_dir):
    path = os.path.join(project_dir, "scene.json")
    with open(path, encoding="utf-8") as f:
        scene = json.load(f)
    for e in scene["entities"]:
        if e["name"] == "BlueCrate":
            e["components"]["Collider"]["group"] = 0
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(scene, f, indent=1)
    return "group"


def sinking_check(env, answer):
    env.command("project.reload", {})
    env.command("step", {"ticks": 180})
    def pos(name):
        return env.command("world.get", {"entity": name, "component": "Transform"})["position"]
    blue = pos("BlueCrate")
    if not (near(blue["y"], 0.5, 0.06) and near(blue["x"], 3, 0.1)):
        return False, f"BlueCrate is at {blue} three seconds in, not resting on the floor where it fell (x 3, y 0.5)"
    for name, x in (("RedCrate", -3), ("GreenCrate", 0)):
        p = pos(name)
        if not (near(p["y"], 0.5, 0.06) and near(p["x"], x, 0.1)):
            return False, f"{name} is at {p}, no longer resting where it was"
    ghost = pos("Ghost")
    if ghost["y"] > -2:
        return False, f"the Ghost no longer drifts through the floor (it is at y {ghost['y']:.2f})"
    floor = env.command("world.get", {"entity": "Floor", "component": "Collider"})
    if floor.get("group") != -2:
        return False, f"the Floor's collision group was changed to {floor.get('group')}"
    if not isinstance(answer, str) or "group" not in answer.lower():
        return False, f"BlueCrate lands now, but the answer {answer!r} does not name the field that was wrong (its Collider's group)"
    return True, f"BlueCrate rests at y {blue['y']:.3f}, the Ghost still sinks; answered {answer!r}"



# Coins in a row: the player walks right and takes them; the next coin glows when one is taken, and
# the last one taken ends the level. Taking the last throws (there is no next coin to light), which
# stops the game: a player sees it freeze.
COINS_TS = """import { events, expose, input, onStart, onTick, world } from "pocket";
import type { Entity } from "pocket";

const coins: Entity[] = [];
let player = 0;
let taken = 0;
let done = false;

onStart(() => {
    world.spawn("Camera", { components: { Transform: { position: { x: 5, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 5 } } });
    player = world.spawn("Player", { components: { Transform: { position: { x: 0, y: 0, z: 0 } }, Sprite: { color: { r: 0.3, g: 0.7, b: 1, a: 1 } } } });
    for (let i = 1; i <= 4; i++) {
        coins.push(world.spawn(`Coin_${i}`, { components: { Transform: { position: { x: i * 2, y: 0, z: 0 }, scale: { x: 0.5, y: 0.5, z: 0.5 } }, Sprite: { color: { r: 0.6, g: 0.5, b: 0.1, a: 1 } } } }));
    }
    glow(coins[0]);
});

// The coin to go for next shines brighter.
function glow(coin: Entity): void {
    world.set(coin, "Sprite", { color: { r: 1, g: 0.85, b: 0.2, a: 1 } });
}

onTick(({ dt }) => {
    if (done) return;
    const p = world.get(player, "Transform")!.position;
    world.set(player, "Transform", { position: { x: p.x + input.axis("move_x") * 4 * dt, y: p.y, z: 0 } });
    const coin = coins[taken];
    const c = world.get(coin, "Transform")!.position;
    if (Math.abs(c.x - p.x) < 0.5) {
        world.destroy(coin);
        taken += 1;
        events.emit("coin.taken", { taken });
        glow(coins[taken]);
        if (taken === coins.length) {
            done = true;
            events.emit("level.complete", { coins: taken });
        }
    }
});

expose("taken", () => taken);
expose("done", () => done);
"""

COINS_FIXED_TS = COINS_TS.replace("""        glow(coins[taken]);
        if (taken === coins.length) {""", """        if (taken < coins.length) glow(coins[taken]);
        if (taken === coins.length) {""")


def coins_setup(project_dir):
    write_project(project_dir, COINS_TS)


def coins_solve(env, project_dir):
    with open(os.path.join(project_dir, "scripts", "main.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write(COINS_FIXED_TS)
    return None


def coins_check(env, answer):
    env.command("project.reload", {})
    seq = env.command("events.last_seq", {})["seq"]
    colors = []
    env.command("input.hold", {"action": "move_x", "ticks": 420})
    for _ in range(14):
        env.command("step", {"ticks": 30})
        st = env.command("state", {})
        if not st.get("ok", True):
            return False, f"the game stopped with a script error: {st.get('errors')}"
        colors.append({r["path"]: r["Sprite"]["color"] for r in env.command("world.query", {"name": "Coin_*", "with": ["Sprite"]})["entities"]})
    taken = env.command("events.since", {"seq": seq, "type": "coin.taken"})["events"]
    done = env.command("events.since", {"seq": seq, "type": "level.complete"})["events"]
    if [e["data"]["taken"] for e in taken] != [1, 2, 3, 4]:
        return False, f"coin.taken came as {[e['data'] for e in taken]}, not once for each of the four coins"
    if len(done) != 1 or done[0]["data"].get("coins") != 4:
        return False, f"level.complete came {len(done)} times ({[e['data'] for e in done]}), not once with coins 4"
    # The next coin still glows: each time, the first coin left is the bright one and the rest are dim.
    for c in colors:
        left = sorted(c, key=lambda k: int(k.split("_")[1]))
        for i, k in enumerate(left):
            bright = c[k]["r"] > 0.9 if isinstance(c[k], dict) else c[k][0] > 0.9
            if bright != (i == 0):
                return False, f"the glow is wrong: with {left} left, {k} is {'bright' if bright else 'dim'}"
    if env.command("state", {})["state"].get("done") is not True:
        return False, "done is not true after the last coin"
    return True, "four coins taken, the next one glowing each time, level.complete once, no script error"


# A house drawn as voxels: a text file the agent writes, read by the engine like any model.
HOUSE_VOXELS = {
    "voxel": 0.25,
    "palette": {"w": "#e8dcc0", "r": "#a8322d", "d": "#5a3a22", "g": "#9cc9e8"},
    "layers": [
        ["wwwwww", "w....w", "w....w", "w....w", "w....w", "wwddww"],
        ["wwwwww", "w....w", "w....w", "w....w", "w....w", "wwddww"],
        ["wwwwww", "w....w", "g....g", "w....w", "w....w", "wgwwgw"],
        ["wwwwww", "w....w", "w....w", "w....w", "w....w", "wwwwww"],
        ["rrrrrr", "rrrrrr", "rrrrrr", "rrrrrr", "rrrrrr", "rrrrrr"],
        ["......", "rrrrrr", "rrrrrr", "rrrrrr", "rrrrrr", "......"],
        ["......", "......", "rrrrrr", "rrrrrr", "......", "......"],
    ],
}


def house_solve(env, project_dir):
    os.makedirs(os.path.join(project_dir, "assets"), exist_ok=True)
    with open(os.path.join(project_dir, "assets", "house.voxels"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(HOUSE_VOXELS, f, indent=1)
    edit_main(project_dir, lambda s: s.replace("onStart(() => {", 'onStart(() => {\n    world.spawn("House", { components: { Transform: { position: { x: 10, y: 0, z: 0 } }, MeshRenderer: { mesh: "assets/house.voxels" } } });', 1))
    return None


def house_check(env, answer):
    # Where it stands in the world: an instantiated model's parts are children at the origin of a root.
    found = [r for r in env.command("world.query", {"with": ["MeshRenderer", "WorldTransform"], "fields": ["MeshRenderer", "WorldTransform"]})["entities"] if r["MeshRenderer"].get("mesh") == "assets/house.voxels"]
    if not found:
        return False, "no entity draws assets/house.voxels"
    p = found[0]["WorldTransform"]["position"]
    if not (near(p["x"], 10, 0.01) and near(p["y"], 0, 0.01) and near(p["z"], 0, 0.01)):
        return False, f"the house is at {p}, not (10, 0, 0)"
    d = env.command("assets.describe", {"path": "assets/house.voxels"})
    if d.get("error") or d.get("importer") != "voxels":
        return False, f"the engine does not read assets/house.voxels as voxels: {d.get('error') or d.get('importer')}"
    text = env.command("project.read", {"path": "assets/house.voxels"})["text"]
    v = json.loads("\n".join(l for l in text.splitlines() if not l.strip().startswith("//")))
    layers = v["layers"]
    width = max(len(r) for layer in layers for r in layer)
    depth = max(len(layer) for layer in layers)
    if len(layers) < 6 or width < 6 or depth < 6:
        return False, f"the house is {width} by {depth} cells and {len(layers)} high, not at least 6 each way"
    colours = {ch for layer in layers for row in layer for ch in row if ch not in ". "}
    if len(colours) < 3:
        return False, f"the house uses {len(colours)} colours, not walls, a roof and a door"
    if not any(ch != "." and ch != " " for row in layers[0] for ch in row):
        return False, "the bottom layer is empty: the house does not stand on the ground"
    tri = d.get("triangles", 0)
    return True, f"{width} by {depth} by {len(layers)} cells in {len(colours)} colours, {tri} triangles, at (10, 0, 0)"


# A festival: two rows of poles, each with a cloth flag woven far finer than a flag that size needs,
# so the cloth is where each tick goes. A fix keeps every flag where it is, its size, pin and colour,
# still flying in the wind, at a weave of 8 by 5 or more.
def festival_scene():
    ents = [
        {"name": "Camera", "components": {"Transform": {"position": {"x": 0, "y": 4, "z": 16}, "rotation": {"x": -0.1, "y": 0, "z": 0, "w": 0.995}}, "Camera": {"fov_degrees": 60}}},
        {"name": "Sun", "components": {"Transform": {"rotation": {"x": -0.42, "y": 0.28, "z": 0.12, "w": 0.85}}, "Light": {"kind": 0, "intensity": 2.4}}},
        {"name": "Square", "components": {"Transform": {"position": {"x": 0, "y": -0.5, "z": 0}, "scale": {"x": 40, "y": 1, "z": 20}}, "MeshRenderer": {"mesh": "cube", "color": [0.55, 0.5, 0.45]}, "RigidBody": {"kind": "static"}, "Collider": {"size": {"x": 20, "y": 0.5, "z": 10}}}},
        {"name": "Wind", "components": {"Wind": {"direction": 10, "speed": 7, "gusts": 0.3}}},
    ]
    colors = [[0.85, 0.15, 0.15], [0.15, 0.45, 0.85], [0.95, 0.8, 0.1], [0.2, 0.7, 0.3]]
    for i in range(24):
        x, z = -11.5 + (i % 12) * 2.0, -2.0 if i < 12 else 2.0
        ents.append({"name": f"Pole_{i}", "components": {"Transform": {"position": {"x": x, "y": 1.6, "z": z}, "scale": {"x": 0.08, "y": 3.2, "z": 0.08}}, "MeshRenderer": {"mesh": "cube", "color": [0.35, 0.35, 0.35]}}})
        ents.append({"name": f"Flag_{i}", "components": {"Transform": {"position": {"x": x + 0.8, "y": 2.9, "z": z}}, "MeshRenderer": {"mesh": "cube", "color": colors[i % 4]},
                                                          "Cloth": {"size": {"x": 1.6, "y": 1.0}, "segments": {"x": 48, "y": 30}, "pin": "left", "weight": 0.15}}})
    return json.dumps({"format": "pocket-scene", "entities": ents}, indent=1) + "\n"


def festival_setup(project_dir):
    write_project(project_dir, "export {};\n", scene=festival_scene())


def festival_look(env):
    env.command("project.reload", {})
    env.command("step", {"ticks": 60})
    env.command("perf", {"reset": True})
    env.command("step", {"ticks": 60})
    perf = env.command("perf", {})
    flags = {}
    for r in env.command("world.query", {"name": "Flag_*", "with": ["Cloth"], "fields": ["Transform", "Cloth", "MeshRenderer"]})["entities"]:
        pole_x = r["Transform"]["position"]["x"] - 0.8
        reach = env.command("physics.cloth", {"entity": r["path"]})["bounds"]["max"]["x"] - pole_x
        flags[r["path"]] = {"at": r["Transform"]["position"], "cloth": r["Cloth"], "color": r["MeshRenderer"]["color"], "reach": reach}
    return {"tick_ms": perf["tick"]["avg_ms"], "systems": perf.get("systems", [])[:3], "flags": flags}


def festival_before(env):
    env.task_state = festival_look(env)
    env.command("project.reload", {})
    s = env.task_state
    if len(s["flags"]) != 24 or not s["systems"] or s["systems"][0]["system"] != "cloth":
        return False, f"the festival did not come up as meant: {len(s['flags'])} flags, costliest systems {s['systems']}"
    return True, ""


def festival_solve(env, project_dir):
    path = os.path.join(project_dir, "scene.json")
    with open(path, encoding="utf-8") as f:
        scene = json.load(f)
    for e in scene["entities"]:
        if e["name"].startswith("Flag_"):
            e["components"]["Cloth"]["segments"] = {"x": 16, "y": 10}
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(scene, f, indent=1)
    return None


def festival_check(env, answer):
    was = getattr(env, "task_state", None)
    if not was:
        return False, "no measurement of the level before the fix"
    now = festival_look(env)
    if set(now["flags"]) != set(was["flags"]):
        return False, f"the flags are not the same: {sorted(set(now['flags']) ^ set(was['flags']))[:4]}"
    for k, f in now["flags"].items():
        w = was["flags"][k]
        if any(abs(f["at"][c] - w["at"][c]) > 1e-3 for c in ("x", "y", "z")):
            return False, f"{k} moved from {w['at']} to {f['at']}"
        for field in ("size", "pin", "weight"):
            if f["cloth"].get(field) != w["cloth"].get(field):
                return False, f"{k}'s Cloth {field} changed from {w['cloth'].get(field)} to {f['cloth'].get(field)}"
        if f["color"] != w["color"]:
            return False, f"{k}'s colour changed"
        seg = f["cloth"].get("segments", {})
        if seg.get("x", 0) < 8 or seg.get("y", 0) < 5:
            return False, f"{k} is woven {seg.get('x')} by {seg.get('y')}, coarser than 8 by 5"
        if f["reach"] < 1.2:
            return False, f"{k} no longer flies: it reaches {f['reach']:.2f} past its pole"
    if now["tick_ms"] > was["tick_ms"] * 0.25:
        return False, f"a tick takes {now['tick_ms']:.2f} ms, not under a quarter of the {was['tick_ms']:.2f} ms it took"
    return True, f"the same 24 flags flying, a tick {was['tick_ms']:.2f} -> {now['tick_ms']:.2f} ms"


TASKS = [
    {"name": "spawn_named", "project": "hello", "ticks": 0, "solve": spawn_named_solve, "check": spawn_named_check,
     "task": "Spawn an entity named Beacon at x 2, y 1, z -3 that draws a red cube: a MeshRenderer with mesh \"cube\" and color r 1, g 0, b 0."},
    {"name": "recolor", "project": "hello", "ticks": 0, "solve": recolor_solve, "check": recolor_check,
     "task": "Make the entity named Ball green: its MeshRenderer color r 0, g 1, b 0."},
    {"name": "tile_edit", "project": "sprites", "ticks": 0, "solve": tile_edit_solve, "check": tile_edit_check,
     "task": "On the tile map of the entity named Level, put tile id 0 of its first tileset into the cell tile_x 3, tile_y 2 of the layer named \"ground\"."},
    {"name": "clear_enemies", "project": "playground", "ticks": 900, "before": clear_enemies_before, "solve": clear_enemies_solve, "check": clear_enemies_check,
     "task": "Destroy every entity named Enemy and nothing else."},
    {"name": "play_clip", "project": "assets", "ticks": 0, "solve": play_clip_solve, "check": play_clip_check,
     "task": "Make the entity named Arm play its animation clip named \"turn\", looping."},
    {"name": "path_length", "project": "playground", "ticks": 10, "solve": path_length_solve, "check": path_length_check,
     "task": "The project has baked a navigation grid. Answer with the length of the navigation path from x -8, y 0, z -8 to x 8, y 0, z 8, as the number \"answer\"."},
    {"name": "count_overlap", "project": "physics", "ticks": 120, "solve": count_overlap_solve, "check": count_overlap_check,
     "task": "Answer with the number of entities the physics overlap query finds within radius 4 of the origin (x 0, y 0, z 0), as the integer \"answer\"."},
    {"name": "event_cause", "project": "playground", "ticks": 1500, "before": event_cause_before, "solve": event_cause_solve, "check": event_cause_check,
     "task": "Find the most recent event of type player.hit and answer with the type of the event at the root of its causes, as the string \"answer\"."},
    {"name": "night_lamp", "project": "hello", "ticks": 0, "solve": night_lamp_solve, "check": night_lamp_check,
     "task": "Make it night: set the intensity of the Light on the entity named Sun to 0.05, and spawn an entity named Lamp at x 0, y 2, z 0 with a warm point light that casts shadows (a Light of kind 1, color r 1, g 0.7, b 0.4, intensity 3, range 6). Then step the simulation one tick so a frame is drawn, and answer with the number of lights casting shadows in that frame as the renderer reports it, as the integer \"answer\"."},
    {"name": "expose_count", "project": "hello", "ticks": 0, "script": True, "solve": expose_count_solve, "check": expose_count_check,
     "task": "Edit scripts/main.ts in the project directory so that the project exposes a state key named \"answer\" whose value is the number of entities in the world (the SDK's expose and world.summary)."},
    {"name": "ball_lower", "project": "hello", "ticks": 0, "script": True, "solve": ball_lower_solve, "check": ball_lower_check,
     "task": "Edit scripts/main.ts in the project directory so that the ball starts its fall from a height of 1.0 instead of 3.0."},
    {"name": "spawn_stars", "project": "hello", "ticks": 0, "script": True, "solve": spawn_stars_solve, "check": spawn_stars_check,
     "task": "Edit scripts/main.ts in the project directory so that, on start, the project also spawns five entities named Star0 to Star4 at x 0 to 4, y 2, z 0, each drawing a sphere (a MeshRenderer with mesh \"sphere\")."},
    {"name": "double_jump", "project": "sprites", "ticks": 0, "script": True, "entry": "scripts/main.tsx", "solve": double_jump_solve, "check": double_jump_check,
     "task": "Edit scripts/main.tsx in the project directory to give the player a double jump: pressing the jump action while in the air, once per time off the ground, gives the same upward speed again (a jump from the ground rises 2.3 units today; with a second jump pressed a fifth of a second later it must rise more than 3)."},
    {"name": "coin_respawn", "project": "sprites", "ticks": 0, "script": True, "entry": "scripts/main.tsx", "solve": coin_respawn_solve, "check": coin_respawn_check,
     "task": "Edit scripts/main.tsx in the project directory so that a collected coin comes back where it was three seconds of game time after it was collected (drawn and collectable again, counted in the exposed \"coins\" state), and not before."},
    {"name": "jump_sound", "project": "sprites", "ticks": 0, "script": True, "entry": "scripts/main.tsx", "solve": jump_sound_solve, "check": jump_sound_check,
     "task": "Give the jump a sound: write a short WAV file (mono 16-bit PCM, at least a tenth of a second, any tone) under assets/ in the project directory, and edit scripts/main.tsx so that it plays through the SDK's audio.play each time the player jumps from the ground, and at no other time."},
    {"name": "coin_chime", "project": "sprites", "ticks": 0, "script": True, "solve": coin_chime_solve, "check": coin_chime_check, "edits": "a new sound recipe",
     "task": "Give the sprites game a sound for picking up a coin, made as a sound recipe: a .sfx file of your own under assets/, a short bright chime that plays every time a coin is collected. Answer null."},
    {"name": "theme_song", "project": "sprites", "ticks": 0, "script": True, "solve": theme_song_solve, "check": theme_song_check, "edits": "a new score",
     "task": "Give the sprites game background music written as a score: a .song file of your own under assets/ with at least two instruments, a melody of at least eight notes and a bass line, playing in a loop from the start. Answer null."},
    {"name": "merchant_talk", "project": "talk", "ticks": 0, "script": True, "solve": merchant_talk_solve, "check": merchant_talk_check, "edits": "scripts/main.ts and a new dialogue script",
     "task": "Add a potion merchant to the talk game. Spawn an entity named Merchant drawn as a sprite at x -2, y 2, who talks when the player presses the talk action within 1.8 units of it, the way the guard does. Write the conversation as a dialogue script, dialogue/merchant.dialogue.json, shown with the SDK's dialogue box: the merchant greets the player and offers a choice to buy a potion for 3 gold, offered only while the player has at least 3 gold, or to leave. Buying takes 3 gold and gives one potion. Expose the player's potions as the state \"potions\"; the gold already exposed as \"gold\" must stay the player's gold, the same in both conversations. Answer null."},
    {"name": "lamp_prefab", "project": "hello", "ticks": 0, "script": True, "solve": lamp_prefab_solve, "check": lamp_prefab_check,
     "task": "Write a prefab file prefabs/lamp.json in the project directory: a pocket-scene fragment (format \"pocket-scene\", version 1, an \"entities\" list) whose one root entity named Lamp draws a yellow sphere (a MeshRenderer with mesh \"sphere\" and color r 1, g 0.9, b 0.2) and has a child named Glow with a Light of kind 1 (a point light) and intensity 2. Then edit scripts/main.ts so that on start the project instantiates that prefab three times through the SDK's world.instantiate with the file's path, named Lamp0, Lamp1 and Lamp2, at x -2, 0 and 2, y 1, z 0."},
    {"name": "hill_raise", "project": "hills", "ticks": 2, "before": hill_raise_before, "solve": hill_raise_solve, "check": hill_raise_check,
     "task": "Raise the ground of the terrain named Hills at x 20, z -20 until it stands at least 10 units high there, without changing the ground 12 or more units away from that point; then answer with the ground's height at x 20, z -20 as the number \"answer\"."},
    {"name": "yard_flag", "project": "walker", "ticks": 2, "solve": yard_flag_solve, "check": yard_flag_check,
     "task": "Raise a flag in the walker's yard: an entity named Pole drawn as a thin grey box 3 units tall standing on the ground at x -6, z 4, and an entity named Flag that is a red sheet of cloth 1.6 wide and 1 tall, held along its left edge down the pole from the pole's top, streaming out in a wind (add one) that blows toward +x at 6 units a second. Then let 3 seconds of game time pass and answer with how far the flag's far edge reaches past the pole along x, as the number \"answer\"."},
    {"name": "flowers", "project": "hills", "ticks": 2, "solve": flowers_solve, "check": flowers_check,
     "task": "Strew red flowers over the terrain named Hills: spawn an entity named Flowers that draws small red spheres (a MeshRenderer with mesh \"sphere\" and color r 1, g 0, b 0.1) with a Scatter placing copies on Hills over the whole terrain, only where the ground is no steeper than 20 degrees. At least 50 flowers must stand."},
    {"name": "car_speed", "project": "drive", "ticks": 120, "solve": car_speed_solve, "check": car_speed_check,
     "task": "Make the car named Car faster: set its Vehicle's top_speed to 35 and power to 14, then drive it with the throttle action held for 300 ticks (5 seconds of game time) and answer with the car's speed at the end as the number \"answer\" (the game's script sets the car's throttle from the throttle action every tick)."},
    {"name": "knockout", "project": "walker", "ticks": 0, "script": True, "solve": knockout_solve, "check": knockout_check,
     "task": "In the walker game, give the Player entity a Health of 30 (current and max) when the game starts, and when the player's health is used up make the hero (the entity Player/Hero, the player's body) go limp as a ragdoll: it falls where it stands and lies there, and from then on the move actions no longer move the player. Answer null."},
    {"name": "fps_shotgun", "project": "fps", "ticks": 0, "script": True, "solve": fps_shotgun_solve, "check": fps_shotgun_check,
     "task": "Turn the fps game's gun into a shotgun: each shot fires 6 pellets, each going off within 3 degrees of where the view looks (randomly) and doing 8 damage to what it hits, instead of one bullet of 25; the magazine holds 6 shots instead of 12. Keep everything else as it is. Answer null."},
    {"name": "farm_rain", "project": "farm", "ticks": 0, "script": True, "solve": farm_rain_solve, "check": farm_rain_check,
     "task": "In the farm game it rains on a timer (thirty seconds of rain every hundred and five). Make the weather answer the field instead: it rains (the rain of the Weather on the entity named Weather at 1) as soon as three or more sown plots are dry, and stops (rain 0) once no sown plot is dry; it never rains on a timer. A plot is the project's own Plot component (components.toml): sown when its stage is 0 or more, dry when its water is 0. Answer null."},
    {"name": "guards_footsteps", "project": "guards", "ticks": 0, "script": True, "solve": guards_footsteps_solve, "check": guards_footsteps_check,
     "task": "In the guards game the guards hear the player only while it runs. Make them hear it walking too, but from nearer: while the player walks (moving, not running) its footsteps are a noise that carries 4 units, every half second; standing still makes none. Running stays as it is. Answer null."},
    {"name": "walker_sprint", "project": "walker", "ticks": 0, "script": True, "solve": walker_sprint_solve, "check": walker_sprint_check,
     "task": "Give the walker a sprint: add an input action named sprint bound to the left Shift key (LShift) in project.toml, and edit scripts/main.ts so that while sprint is held the player walks at 9 units per second instead of 5 (and at 5 otherwise)."},
    {"name": "raft", "project": "hills", "ticks": 2, "solve": raft_solve, "check": raft_check,
     "task": "Float a raft on the lake: spawn an entity named Raft, a dynamic rigid body with a box collider 2 units long (x), 0.2 thick (y) and 1 wide (z), drawn as a brown box of that size, and drop it into the lake from a little above the water at x -33, z -22. It must float: run the game for 180 ticks and check that the raft's centre stays within 0.3 of the water's surface. Answer with the height of the water's surface under the raft after those ticks as the number \"answer\"."},
    {"name": "sand_road", "project": "hills", "ticks": 2, "before": sand_road_before, "solve": sand_road_solve, "check": sand_road_check,
     "task": "Paint a sand road on the terrain named Hills: a straight road along z 0 from x -30 to x 30, its colour r 0.85, g 0.78, b 0.55, covering the ground fully (a coverage of at least 0.8) at least a unit to either side of the line, and leaving the ground 6.5 or more units from the line as it was. Answer with the paint's coverage at x 0, z 0 as the number \"answer\"."},
    {"name": "reed_field", "project": "hills", "ticks": 2, "solve": reed_field_solve, "check": reed_field_check,
     "task": "Plant a field of reeds that sway and block: spawn an entity named Reeds drawing thin boxes (a MeshRenderer with mesh \"cube\"; its Transform scale x 0.1, y 1.5, z 0.1) with a Scatter placing copies on the terrain named Hills over a 20 by 20 area centred on x 0, z 10, at least 100 of them standing, all the entity's own size (none bigger or smaller), whose tops sway 0.3 units in the wind, and each copy a collider 0.1 in radius. Answer with the number of reeds standing as the integer \"answer\"."},
    {"name": "stormy_dusk", "project": "hills", "ticks": 2, "solve": stormy_dusk_solve, "check": stormy_dusk_check,
     "task": "Make the hills a windy, cloudy sunset: the sky computed by the atmosphere with clouds covering 0.8 of it, the sun (the entity named Sun) standing 4 degrees above the horizon in the west (toward -x, so its light shines toward +x), and the wind (the entity named Breeze) blowing toward +x at 12 units a second. Then step one tick and answer with the red of the sun light as it reaches the ground, as the renderer reports it, as the number \"answer\"."},
    {"name": "sea_around", "project": "hills", "ticks": 2, "solve": sea_around_solve, "check": sea_around_check,
     "task": "Make the hills an island: turn their lake (the entity Lake) into a sea that runs to the horizon every way, resting where the lake does, and give the sky clouds over about half of it (the engine's volumetric ones). Then answer with the height the water rests at 400 units east of the origin (x 400, z 0), as the number \"answer\"."},
    {"name": "meadow", "project": "hills", "ticks": 2, "solve": meadow_solve, "check": meadow_check,
     "task": "Grow grass on the hills through the running game: blades of grass over the terrain (the entity Hills), on its grass layer only and none below 3.5 high (the beach), drawn out to 30 units from the camera. Answer null."},
    {"name": "sailboat", "project": "blank", "ticks": 0, "solve": sailboat_solve, "check": sailboat_check,
     "task": "Through the running game, put a sailing boat to sea in this blank project: an ocean at height 0 running to the horizon (the project's ground taken away), a steady wind of 6 units a second blowing toward +x, and a boat named Sloop afloat near the origin, bow toward +x, with its sail set and no engine or oars driving it, so the wind alone carries it along. Answer null."},
    {"name": "snowy_evening", "project": "hills", "ticks": 2, "solve": snowy_evening_solve, "check": snowy_evening_check,
     "task": "Make the hills a snowy winter evening that goes on by itself: snow falling at 0.7 of its heaviest onto ground already wholly white, and the sky's time of day at half past five in the afternoon with a whole day lasting ten minutes, so the sun (the entity named Sun) goes down on its own. Then step 600 ticks (ten seconds) and answer with the time of day the sky then says, in hours, as the number \"answer\"."},
    {"name": "dirt_patch", "project": "hills", "ticks": 2, "solve": dirt_patch_solve, "check": dirt_patch_check,
     "task": "The terrain named Hills is drawn from four textured layers (grass, sand, rock, dirt). Make its sand layer lie only below 0.2 of the terrain's height instead of where it lies now, leaving the other layers as they are, and paint its dirt layer at full strength in a patch of radius 4 around x 10, z -10. Answer with the dirt layer's share of the ground at x 10, z -10 as the number \"answer\"."},
    {"name": "glass_window", "project": "hello", "ticks": 0, "solve": glass_window_solve, "check": glass_window_check,
     "task": "Spawn an entity named Window: a cube scaled x 2, y 1.5, z 0.05 at x 0, y 1.5, z 3, drawn as clear glass that lets all the light through, with an index of refraction of 1.52. Give the entity named Ball a clear coat at full strength with a roughness of 0.05. Then step one tick and answer with the number of glass meshes the renderer drew in that frame as the integer \"answer\"."},
    {"name": "persian_locale", "project": "ui", "ticks": 0, "script": True, "entry": "scripts/main.tsx", "edits": "the files in locales/", "solve": persian_locale_solve, "check": persian_locale_check,
     "task": "The interface speaks English, Chinese and Arabic (locales/en.json, zh.json, ar.json). Add Persian: write locales/fa.json with every text the English file has, translated into Persian (keep the placeholders and plural forms working), and give the language choice its name by adding language.fa (\"فارسی\") to every language file. Answer with anything; the check switches to Persian and reads the interface."},
    {"name": "blender_level", "project": "assets", "ticks": 0, "script": True, "edits": "assets/level.blend (made by Blender)", "solve": blender_level_solve, "check": blender_level_check,
     "task": "Make a small level in Blender, which is installed at " + BLENDER + " (run it headless with a Python script, -b --factory-startup --python). Save it as assets/level.blend in the project with: a plane named Floor, 10 across, lying flat 1 unit above the ground (Blender z 1), that the engine will make a static body colliding with its own triangles; and a box named Pillar, 1 by 1 by 3 standing on it at Blender x 3, y 0, that becomes a static body with a box collider 0.5 by 1.5 by 0.5 in half extents and has 50 health (the Health component, max and current 50). Give them those engine components through Blender custom properties so that world.instantiate of the file brings them. Then instantiate it at x 60 and drop a dynamic sphere of radius 0.5 (mass 1) from 8 up onto the floor at x 58, z 1; answer with the height it comes to rest at after 180 ticks as the number \"answer\"."},
    {"name": "calm_lake", "project": "hills", "ticks": 2, "solve": calm_lake_solve, "check": calm_lake_check,
     "task": "Calm the lake: make the water of the entity named Lake perfectly still (no waves) and clearer, so that one sees 8 units into it, and raise its surface by half a unit (it stands at 3.2), leaving it centred where it is. Answer with the water's surface height at x 0, z 0 as the number \"answer\"."},
    {"name": "spike_trap", "project": "sprites", "ticks": 30, "solve": spike_trap_solve, "check": spike_trap_check,
     "task": "Put a spike trap under the player: give the entity named Player 60 health (current and max) with half a second of invulnerability after each hit, and spawn an entity named Spikes where the player stands that notices 2D bodies in a box of half extents 1 by 0.5 and hurts what is in it by 15, again every half second while it stays. Then run the game, leaving the player where it is, until its health is used up, and answer with the tick at which that was reported, as the integer \"answer\"."},
    {"name": "brute_enemies", "project": "playground", "ticks": 600, "before": clear_enemies_before, "solve": brute_enemies_solve, "check": brute_enemies_check,
     "task": "The playground declares a component of its own for its enemies. Make every enemy now in the world a brute that does 25 damage, without stepping the game, and answer with how many enemies there are, as the integer \"answer\"."},
    {"name": "roof_mesh", "project": "hello", "ticks": 0, "solve": roof_mesh_solve, "check": roof_mesh_check,
     "task": "Make a mesh named roof from numbers: a square pyramid with its base corners at (-1, 0, -1), (1, 0, -1), (1, 0, 1) and (-1, 0, 1) and its apex at (0, 1, 0), its four sloping sides facing outward (a base is up to you). Spawn an entity named Roof that draws it in orange, placed at (0, 2, 0). Answer with how many triangles the mesh has, as the integer \"answer\"."},
    {"name": "walled_room", "project": "sprites", "ticks": 0, "solve": walled_room_solve, "check": walled_room_check,
     "task": "Make a tile map by code named maps/room.tmj: 10 tiles wide and 8 high, 16-pixel tiles, with assets/tiles.png as its tileset and one tile layer named walls whose tiles are all solid. Put the tileset's first tile in every cell of the map's border and leave the inside empty. Show it with an entity named Room at x 30, y 0. Then drop a 2D rigid body named Pebble, a circle of radius 0.25, from x 35, y -2 inside the room, run the game for two seconds, and answer with the Pebble's y, as the number \"answer\"."},
    {"name": "plank_bridge", "project": "crates", "ticks": 0, "solve": plank_bridge_solve, "check": plank_bridge_check,
     "task": "Hang a bridge in the crates game: five planks named Plank0 to Plank4, each a dynamic 2D rigid body with a box shape of half extents 0.5 by 0.1, laid end to end at height 4 with their centers at x -4, -3, -2, -1 and 0. Hinge each plank to the next where their ends meet, hinge Plank0's left end to a fixed point in the world at (-4.5, 4), and Plank4's right end to a fixed point at (0.5, 4). Run the game for two seconds and answer with the lowest plank center's y, as the number \"answer\"."},
    {"name": "dodge", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": dodge_solve, "check": dodge_check,
     "task": "Make a small game in this blank project, replacing its example. A dodge game in the XY plane (x across, y up), seen from the front: the player is an entity named Player that the actions move_x and move_y (bound to A/D and S/W, and to the arrow keys) move at 6 units a second. Every second a rock appears: an entity named Rock_1, Rock_2, ... at a random x between -8 and 8 and y = 6, falling at 4 units a second; rocks below y = -7 are removed. A rock within 0.6 units of the player ends the game: emit an event game.over with {time}, after which nothing moves the player and the clock stops. Expose alive (true until the game is over) and time_alive (seconds alive). The game must read where the player and the rocks are from their Transforms every tick, so that moving one with world.set moves it in the game. Answer null."},
    {"name": "snake", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": snake_solve, "check": snake_check,
     "task": "Make a snake game in this blank project, replacing its example. In the XY plane (x across, y up), on a grid of cells one unit wide, x from 0 to 19 and y from 0 to 14: the snake is a row of entities named Segment_0 (the head), Segment_1, and so on, starting as three at (5, 7), (4, 7) and (3, 7), heading +x, moving one cell every 0.15 seconds. The actions move_x and move_y (A/D, S/W and the arrow keys) turn it, never straight back. An entity named Food sits on a cell: when the head moves onto it the snake grows by one segment, the game emits food.eaten with {length}, and the Food moves to a random free cell. Leaving the grid or running into itself ends the game: emit game.over with {length}, after which nothing moves. Expose length and alive. The game must read where the Food is from its Transform every tick, so that moving it with world.set moves it in the game. Answer null."},
    {"name": "pause_menu", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": pause_menu_solve, "check": pause_menu_check,
     "task": "Make a small game in this blank project with a title screen and a pause menu, replacing its example. In the XY plane: an entity named Box at (0, 0) that move_x (A/D and the arrow keys) moves along x at 4 units a second, only while playing. The game opens on a title screen with a Pocket UI button named start; clicking it starts play (emit game.started). The action pause (Escape) while playing pauses: the Box and the clock stop and a menu shows buttons named resume and restart (emit game.paused). resume plays on (game.resumed); restart puts the Box back at (0, 0) and the clock at 0 and plays again (game.restarted). Expose screen (\"title\", \"playing\" or \"paused\") and time, the seconds played. The game must read the Box's position from its Transform every tick. Answer null."},
    {"name": "breakout", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": breakout_solve, "check": breakout_check,
     "task": "Make a breakout game in this blank project, replacing its example. In the XY plane (x across, y up): a Paddle 2 wide at (0, -6) that move_x (A/D and the arrow keys) moves at 10 units a second, kept between x -7 and 7; a Ball 0.3 across starting at (0, -5), moving up and right at 6 units a second, its motion its Velocity component; walls at x -8 and 8 and a ceiling at y 7 it bounces off; it bounces up off the paddle. Bricks 1.8 by 0.6 named Brick_0 to Brick_31, in 4 rows of 8, centers at x -7, -5, ..., 7 and y 3, 4, 5, 6: a hit removes the brick, bounces the ball, adds 10 to the score and emits brick.broken with {score}. Below y -8 the ball costs a life (3 at the start; emit life.lost with {lives}) and starts again above the paddle; at none, emit game.over and nothing moves after. With no bricks left emit level.clear and stop the ball. Expose score, lives and bricks (how many Brick_ entities the world still has). The game must read the Ball's and the Paddle's Transforms and the Ball's Velocity every tick. Answer null."},
    {"name": "platformer", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": platformer_solve, "check": platformer_check,
     "task": "Make a 2D platformer in this blank project, replacing its example. In the XY plane (x right, y up), one unit a tile: a tile map entity named Level, its top-left corner at (0, 0), made from these rows ('#' a solid tile, '.' empty, 'P' where the Player starts and 'F' the flag, each at its cell's center), drawn with tiles of your own: " + json.dumps(PLATFORMER_ROWS) + ". A Player with a Body2D (half extents 0.4 by 0.45, the default gravity) that move_x (A/D and the arrows) walks at 6 units a second; a jump action (Space, W, Up) leaves the ground at 11 units a second. Below y -12 the Player dies: emit player.died with {deaths} and put it back at the start. Coming within 0.8 of the flag's center emits level.complete once, and the Player answers no more input. Expose deaths and complete. A camera follows the Player. Answer null."},
    {"name": "sokoban", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": sokoban_solve, "check": sokoban_check,
     "task": "Make a Sokoban puzzle in this blank project, replacing its example. In the XY plane, one unit a cell: the cell in column c and row r (rows counting down) is centered at (c, -r). The level is these rows ('#' a wall, '.' floor, '@' the Player, '$' a box, 'x' a goal): " + json.dumps(SOKOBAN_ROWS) + ". The Player and the boxes are entities named Player, Box_0, Box_1 (boxes in reading order) at their cells' centers. Each press of move_x or move_y (A/D/W/S and the arrows) moves the Player one cell, unless a wall is there; a box in the way is pushed one cell when the cell past it is free (not a wall, not a box), else nothing moves. A move adds one to moves and emits player.moved with {moves}. When every box is on a goal, emit level.solved with {moves} once. An undo action (Z) takes back the last move (and its count). Expose moves and solved. Answer null."},
    {"name": "villagers", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": villagers_solve, "check": villagers_check,
     "task": "Make a village square in this blank project, replacing its example: a ground at y 0 and five villagers, entities named Villager_1 to Villager_5, each the engine's built-in humanoid with a shirt of its own colour, standing within 4 units of (0, 0, 0) at the start. They wander at walking pace (about 1.3 units a second), never more than 6 units from the centre, walking when they move and standing idle when they stop. A Player entity (drawn however you like, its feet at y 0) moves with move_x and move_z at 4 units a second. When the Player comes within 3 units of a villager, that villager stops, turns to face the Player and waves; once the Player is more than 4 units away it wanders again. The game reads the Player's place from its Transform every tick (the check moves it with world.set). Answer null."},
    {"name": "village_weather", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": village_weather_solve, "check": village_weather_check,
     "task": "Make a village square in this blank project, replacing its example: a ground at y 0, three houses named House1, House2 and House3 standing within 10 units of (0, 0, 0), a well named Well at the centre, the sky computed by the atmosphere, and a camera that sees the square. Answer null.",
     "followups": [{"task": "This village square was made earlier; the players now ask for an evening in the rain. Keep what is there and make it rain at 0.8 of the heaviest, set the time of day to 20:00, and put four lamps named Lamp1 to Lamp4 near the houses (within 8 units of one), each with a warm point light that is lit only after dark. Answer null.", "solve": village_weather_followup}]},
    {"name": "wolves", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": wolves_solve, "check": wolves_check,
     "task": "Make a chase in this blank project, replacing its example: a ground at y 0, four sheep named Sheep1 to Sheep4 standing still at (7, 0, 5), (-6, 0, 8), (5, 0, -7) and (-8, 0, -6), and two wolves named Wolf1 at (-1, 0, 0) and Wolf2 at (1, 0, 0) that each run at 3 units a second toward whichever sheep is nearest to it at the time (so a wolf turns to another sheep that comes nearer). A sheep that a wolf comes within a unit of is caught: it is destroyed, and a sheep.caught event is emitted with that wolf as its subject. Answer null."},
    {"name": "fireworks", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": fireworks_solve, "check": fireworks_check,
     "task": "Make a fireworks show in this blank project, replacing its example, under a dark night sky. Each press of a launch action (Space) sends up a rocket: an entity named Rocket_1, Rocket_2 ... (counting launches) starting at (0, 0, 0) and rising straight up at 12 units a second with a glowing trail behind it (the engine's Trail). One second after its launch it bursts where it is: the rocket is destroyed, an event firework.burst with {n, x, y, z} is emitted (n its number), and at least 100 sparks fly out from there in every direction, falling and fading over about two seconds and glowing (additive). Several rockets may be up at once. Expose launched and bursts (how many so far). The game reads each rocket's place from its Transform. Answer null."},
    {"name": "glade", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": glade_solve, "check": glade_check,
     "task": "Make a forest glade in this blank project, replacing its example, without adding any image, model or sound file to the project: a ground about 40 by 40 units textured with grass, at least four trees and three rocks around it, and in the middle a campfire: an entity named Campfire at (0, 0, 0) with flames (glowing particles that rise), a warm point light, and smoke rising above it. Pressing a stoke action (F) plays a sound and bursts sparks from the fire. A camera looks at the glade. Answer null."},
    {"name": "watchman", "project": "blank", "ticks": 0, "script": True, "solve": watchman_solve, "check": watchman_check,
     "task": "In this blank project, replacing its example, add a guard with the engine's Behavior component. Bake a navigation grid over the ground at the start (x and z from -10 to 10). An entity named Watchman starting at (-6, 0, 6) walks back and forth between (-6, 0, 6) and (6, 0, 6), a Path named Beat, at 2 units a second; when it sees the Player within 6 units it runs after it at 4 units a second; when it has not seen the Player for 2 seconds it goes back to walking its beat. Keep an entity named Player that the game does not move by itself (it may stand still): it is moved with world.set. Answer null."},
    {"name": "key_door", "project": "blank", "ticks": 0, "script": True, "game": True, "solve": key_door_solve, "check": key_door_check,
     "task": "Make a small game in this blank project, replacing its example. In the XY plane (x across, y up): an entity named Player at (0, 0) that the actions move_x and move_y (bound to A/D and S/W, and to the arrow keys) move at 5 units a second; a Key at (4, 0); a Door at (8, 0) that the player cannot pass going right until it has the key; an Exit at (12, 0). Coming within 0.7 units of the key takes it: emit key.taken, remove the Key and open the way through the door. Coming within 0.7 units of the exit emits level.complete with {seconds}. Expose has_key. The game must read where the player is from its Transform every tick, so that moving it with world.set moves it in the game. Answer null."},
    {"name": "grey_flashback", "project": "crates", "ticks": 30, "solve": grey_flashback_solve, "check": grey_flashback_check,
     "task": "Make the whole picture of the crates game grey, as for a flashback: a post effect that turns every pixel of the frame to its grey (luminance), leaving the interface as it is. Do it through the running game (no file edits needed). Answer null."},
    {"name": "svg_coin", "project": "hello", "ticks": 0, "script": True, "solve": svg_coin_solve, "check": svg_coin_check, "edits": "a new SVG file",
     "task": "Draw a coin for the hello game as an SVG file of your own in the project: a gold disc (#f5c542) with a darker rim (#b8860b), 32 by 32. Show it in the game from the start as a sprite: an entity named CoinBadge at x 2, y 2.5, z 0, 1 unit wide and high, its texture your SVG. Answer null."},
    {"name": "orange_ball", "project": "hello", "ticks": 0, "script": True, "solve": orange_ball_solve, "check": orange_ball_check, "edits": "a new material file",
     "task": "Draw the hello game's ball through a material of your own: a WGSL file in the project that makes the ball a flat orange, #ff6600, everywhere, ignoring the light. The game should draw its ball (the entity named Ball, which its script spawns) through that material from the start. Answer null."},
    {"name": "night_level", "project": "sprites", "ticks": 0, "solve": night_level_solve, "check": night_level_check,
     "task": "Make the sprites game night, through the running game: its tile map (the entity Level) and the Player's sprite lit by the scene's lights, the ambient light #202840 at intensity 0.3, and a warm point light (colour #ffb060, range 5) that goes wherever the Player goes, half a unit in front of it toward the camera. Answer null."},
    {"name": "guard_view", "project": "dungeon", "ticks": 5, "solve": guard_view_solve, "check": guard_view_check,
     "task": "In the dungeon game, about where the Player stands now: how many map cells can it see within 6 cells, as the map's field of view counts them, and does the torch Torch_6 have a clear line of sight from where it is to the Player? Answer with an object {\"cells\": number, \"sees\": boolean}."},
    {"name": "vault_level", "project": "dungeon", "ticks": 2, "solve": vault_level_solve, "check": vault_level_check,
     "task": "Add a vault to the running dungeon game: a new tile map named maps/vault.tmj, 12 by 8 cells, with the dungeon's tiles (assets/dungeon_tiles.png: tile 0 floor, tile 1 wall), solid walls all around its edge and a solid 2 by 2 pillar at its center (cells 5 and 6 across, 3 and 4 down), floor everywhere else, and objects on the floor: three of type coin and one of type start. Show it as an entity named Vault at x 40, y 0. Answer null."},
    {"name": "slow_swarm", "project": "blank", "ticks": 0, "script": True, "setup": swarm_setup, "before": swarm_before, "solve": swarm_solve, "check": swarm_check,
     "task": "Players say this game has become slow: a swarm of bees flies between flowers and a hive, and each tick takes far too long. Find what makes it slow and make the scripts take under a quarter of the time a tick they take now, without changing what the game does: the same bees and flowers in the same places and the same pollen, tick for tick. Answer null."},
    {"name": "leaky_cannon", "project": "blank", "ticks": 0, "script": True, "setup": cannon_setup, "before": cannon_before, "solve": cannon_solve, "check": cannon_check,
     "task": "Players say this game gets slower the longer it runs. A cannon fires pellets that arc up and land. Find what is wrong and fix it, without changing how many pellets are fired or how they fly and where they land. Answer null."},
    {"name": "sinking_crate", "project": "blank", "ticks": 0, "script": True, "edits": "scene.json", "setup": sinking_setup, "solve": sinking_solve, "check": sinking_check,
     "task": "In this level three crates drop onto the floor, but the blue one (BlueCrate) falls straight through it. Find why and fix the level's scene so that BlueCrate lands on the floor like the other crates, leaving the rest as it is (the Ghost drifts through the floor on purpose). Answer the name of the component field that was wrong."},
    {"name": "voxel_house", "project": "hello", "ticks": 0, "script": True, "edits": "a new voxel model in assets/ and scripts/main.ts", "solve": house_solve, "check": house_check,
     "task": "Draw a small house as a voxel model of your own, written as text in assets/house.voxels: walls of one colour, a roof of another and a door, at least 6 cells wide, 6 deep and 6 high, standing on its bottom layer. Show it in the game from the start at (10, 0, 0). Answer null."},
    {"name": "frozen_coins", "project": "blank", "ticks": 0, "script": True, "setup": coins_setup, "solve": coins_solve, "check": coins_check,
     "task": "Players say this game freezes when they take the last coin: they walk right (the move_x action) and take the coins one by one, the next coin glowing each time, and taking the last should complete the level. Find what is wrong and fix it, leaving the rest of the game as it is. Answer null."},
    {"name": "festival_flags", "project": "blank", "ticks": 0, "script": True, "edits": "scene.json", "setup": festival_setup, "before": festival_before, "solve": festival_solve, "check": festival_check,
     "task": "Players say this festival level runs slowly: each tick takes far too long. Find what makes it slow and make a tick take under a quarter of the time it takes now, changing the level's scene and not how it looks to a player: the same flags on the same poles, the same sizes and colours, still flying in the wind. Answer null."},
    {"name": "wait_for_coin", "project": "sprites", "ticks": 0, "solve": wait_for_coin_solve, "check": wait_for_coin_check,
     "task": "Hold the move_x action toward +x and run the game until the player collects its first coin; answer with the tick at which the coin was collected, as the integer \"answer\"."},
]


def bundle(project_dir):
    """Bundle a project's TypeScript with the tool; the bundle lands in build/ts/<dir name>.js."""
    proc = subprocess.run([POCKET, "ts", project_dir], capture_output=True, text=True, cwd=ROOT, env={**os.environ, "POCKET_ROOT": ROOT}, encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        raise RuntimeError(f"bundling {project_dir} failed: {(proc.stderr or proc.stdout).strip()[-400:]}")


def eval_dir():
    """The run's directory outside the repository, with a copy of the documentation set and an
    index of it (one line a file: its title, what it covers, its size)."""
    global EVAL_DIR
    if EVAL_DIR:
        return EVAL_DIR
    EVAL_DIR = os.path.realpath(tempfile.mkdtemp(prefix="pocket-eval-"))
    lines = ["# The documentation", "", "One line a file: what it covers and how big it is. Read the ones the task needs.", ""]
    for d in DOCS:
        src = os.path.join(ROOT, d)
        dst = os.path.join(EVAL_DIR, d)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
        with open(src, encoding="utf-8") as f:
            text = f.read()
        title = next((l.lstrip("# ").strip() for l in text.splitlines() if l.startswith("#")), d)
        # The first paragraph of prose (its lines joined: the docs are wrapped), to its first stop;
        # not a heading, a table, a list, code or markup.
        paras = [p for p in re.split(r"\n\s*\n", text) if p.strip()]
        prose = next((p for p in paras[1:] if not p.lstrip().startswith(("#", "|", "-", "```", "<")) and not re.match(r"\s*\d+[.)]\s", p)), "")
        first = re.split(r"(?<=[.:])\s", " ".join(l.strip() for l in prose.splitlines()), maxsplit=1)[0][:200]
        lines.append(f"- `{d}` ({len(text) // 1024 + 1} KB): {title}. {first}")
    with open(os.path.join(EVAL_DIR, "docs", "INDEX.md"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    return EVAL_DIR


def doc_paths():
    base = eval_dir()
    return [os.path.join(base, "docs", "INDEX.md")] + [os.path.join(base, d) for d in DOCS]


def scratch_copy(task):
    """A copy of the task's sample in the run's directory, with its own bundle name; its guide
    (AGENTS.md) points at the copied documentation rather than the repository's."""
    # A name that says nothing of the task: an agent that searched for it found the harness's checks.
    name = f"game-{os.getpid()}-{TASKS.index(task) if task in TASKS else 0:02d}"
    base = eval_dir()
    dst = os.path.join(base, name)
    shutil.rmtree(dst, ignore_errors=True)
    source = [] if task["project"] == "blank" else ["--from", task["project"]]
    proc = subprocess.run([POCKET, "new", name, *source, "--dir", base], capture_output=True, text=True, cwd=ROOT, env={**os.environ, "POCKET_ROOT": ROOT}, encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        raise RuntimeError(f"copying {task['project']} failed: {(proc.stderr or proc.stdout).strip()[-400:]}")
    guide = os.path.join(dst, "AGENTS.md")
    if os.path.exists(guide):
        with open(guide, encoding="utf-8") as f:
            text = f.read()
        text = text.replace(os.path.join(ROOT, "docs"), os.path.join(base, "docs")).replace("`mcp.md` for the commands", "`INDEX.md` lists them, `mcp.md` for the commands")
        with open(guide, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
    bundle(dst)
    return dst


def scratch_remove(project_dir):
    shutil.rmtree(project_dir, ignore_errors=True)
    name = os.path.basename(project_dir)
    for suffix in (".js", ".js.project.json", ".js.lines.json"):
        try:
            os.remove(os.path.join(ROOT, "build", "ts", name + suffix))
        except FileNotFoundError:
            pass


def run_external(cmd, env, task, timeout, project_dir):
    """One external runner: the task as JSON on stdin, the last JSON line of its output as the answer."""
    payload = {"name": task["name"], "task": task["task"], "project": task["project"], "project_dir": project_dir, "rpc_url": env.url, "docs": doc_paths(),
               "notes": "POST {\"id\": 1, \"method\": \"<command>\", \"params\": {...}} to rpc_url + \"/rpc\"; the runtime is paused; `commands` lists every method."
                        + (f" This task edits files: change {task.get('edits', task.get('entry', 'scripts/main.ts'))} under project_dir; after an edit the command project.apply (pocket_apply in pi, project_apply over MCP) bundles and type-checks it, reloads the project (a fresh world from the scene, the script started again) and steps it, so you can see what it does; the harness bundles and reloads it once more when you are done." if task.get("script") else "")}
    # The agent's own tools start runtimes from the copy under test too, not a build of the checkout.
    runtime = os.environ.get("POCKET_EVAL_RUNTIME")
    extra = {"POCKET_RUNTIME": os.path.abspath(runtime)} if runtime else {}
    proc = subprocess.run(cmd, input=json.dumps(payload), capture_output=True, text=True, shell=True, timeout=timeout, env={**os.environ, "POCKET_RPC_URL": env.url, **extra}, encoding="utf-8", errors="replace")
    answer = None
    metrics = {}
    for line in reversed(proc.stdout.strip().splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                last = json.loads(line)
                answer = last.get("answer")
                # Whatever else the runner reports about the attempt (tokens, cost, tool calls) rides along.
                metrics = {k: v for k, v in last.items() if k != "answer"}
                break
            except json.JSONDecodeError:
                continue
    return answer, proc.returncode, proc.stderr[-2000:], metrics


def merge_metrics(a, b):
    """Two runs' metrics as one: numbers added, nested counts (tokens, tools) added key by key."""
    out = dict(a)
    for k, v in b.items():
        if isinstance(v, bool) or k not in out:
            out[k] = v
        elif isinstance(v, (int, float)) and isinstance(out[k], (int, float)):
            out[k] = out[k] + v
        elif isinstance(v, dict) and isinstance(out[k], dict):
            out[k] = merge_metrics(out[k], v)
        elif isinstance(v, list) and isinstance(out[k], list):
            out[k] = out[k] + v
    return out


def trace_summary(name):
    """What the task's trace says (POCKET_AGENT_TRACES, tools/scripts/trace_report.py): turns,
    tokens, calls, what was read, and the calls before and after the first edit; `stuck` when the
    agent made many more calls after its first edit than before it, the shape of a fix that did not
    take for a reason the agent had to dig for."""
    traces = os.environ.get("POCKET_AGENT_TRACES")
    path = os.path.join(traces, f"{name}.jsonl") if traces else ""
    if not path or not os.path.exists(path):
        return None
    import trace_report  # noqa: E402 - beside this file
    r = trace_report.task_report(path)
    calls = r["calls"]
    first_edit = next((i for i, c in enumerate(calls) if c["tool"] in ("edit", "write") or c["tool"].endswith("project_apply")), None)
    s = {"turns": r["turns"], "tokens": r["total_tokens"], "calls": len(calls),
         "read_kb": round(sum(c["bytes"] for c in calls if c["tool"] in ("read", "grep")) / 1024, 1),
         "failed_calls": sum(1 for c in calls if c["error"])}
    if first_edit is not None:
        s["calls_before_edit"] = first_edit
        s["calls_after_edit"] = len(calls) - first_edit - 1
        s["stuck"] = s["calls_after_edit"] >= 15 and s["calls_after_edit"] > 2 * max(first_edit, 1)
    return s


def run(runner="reference", tasks=None, timeout=300, log=print, project_root=None, rows_to=None):
    chosen = [t for t in TASKS if not tasks or t["name"] in tasks]
    # POCKET_EVAL_TARGET=ios-sim: the game runs as an app in the iOS Simulator (pocket run --ios),
    # the agent and the checks talking to it from the Mac. The app carries its own copy of the game,
    # so the tasks that edit the project's files are left out.
    ios = os.environ.get("POCKET_EVAL_TARGET") == "ios-sim"
    if ios:
        skipped = [t["name"] for t in chosen if t.get("script") or "setup" in t]
        chosen = [t for t in chosen if not (t.get("script") or "setup" in t)]
        if skipped:
            log(f"on the iOS Simulator: {len(skipped)} tasks that edit files left out ({', '.join(skipped[:6])}{', ...' if len(skipped) > 6 else ''})")
    results = []
    started = time.time()
    for t in chosen:
        t0 = time.time()
        answer = None
        metrics = {}
        detail = ""
        ok = False
        error = None
        project_dir = None
        env = None
        try:
            # Every task on a copy outside the repository (the reference and null runners need none).
            outside = runner not in ("reference", "null") or t.get("script") or t["project"] == "blank"   # a blank project is made new
            project_dir = scratch_copy(t) if outside else os.path.join(project_root or ROOT, "samples", t["project"])
            if "setup" in t:
                # The game as the task finds it, written into the copy before the runtime starts.
                t["setup"](project_dir)
                bundle(project_dir)
            # POCKET_EVAL_RUNTIME: a copy of the runtime to test (so a long run is not changed by a rebuild).
            if ios:
                env = IosEnv(project_dir, root=project_root, device=os.environ.get("POCKET_EVAL_DEVICE") or None)
            else:
                env = PocketEnv(project_dir, root=project_root, runtime=os.environ.get("POCKET_EVAL_RUNTIME") or None)
            if t["ticks"]:
                env.command("step", {"ticks": t["ticks"]})
            if "before" in t:
                # The setup must have made the task real (enemies to clear, a hit to explain).
                ready, why = t["before"](env)
                if not ready:
                    raise RuntimeError(f"setup: {why}")
            if runner == "reference":
                answer = t["solve"](env, project_dir) if t.get("script") else t["solve"](env)
            elif runner == "null":
                pass
            else:
                answer, code, err, metrics = run_external(runner, env, t, timeout, project_dir)
                if code != 0:
                    error = f"runner exited {code}: {err.strip()[-300:]}"
            # Follow-ups (docs/agent-eval.md, Follow-ups): later requests on the same project, each
            # to an agent of its own that finds the project as the one before left it.
            for k, fu in enumerate(t.get("followups", []), start=2):
                if t.get("script"):
                    bundle(project_dir)
                    env.command("project.reload", {})
                    env.command("step", {"ticks": 2})
                if runner == "reference":
                    answer = fu["solve"](env, project_dir) if t.get("script") else fu["solve"](env)
                elif runner != "null":
                    sub = {**t, "name": f"{t['name']}-{k}", "task": fu["task"]}
                    answer, code, err, more = run_external(runner, env, sub, timeout, project_dir)
                    metrics = merge_metrics(metrics, more)
                    if code != 0 and not error:
                        error = f"follow-up {k}: runner exited {code}: {err.strip()[-300:]}"
            if t.get("script"):
                # The edit takes effect through the tool's bundler and a project reload: a fresh
                # world from the scene and the edited script started over it.
                bundle(project_dir)
                reloaded = env.command("project.reload", {})
                if not reloaded.get("ok", False):
                    raise RuntimeError(f"the reloaded project has errors: {reloaded}")
                env.command("step", {"ticks": 2})
            ok, detail = t["check"](env, answer)
        except Exception as e:  # noqa: BLE001 - the report carries the failure
            error = f"{type(e).__name__}: {e}"
        finally:
            if env is not None:
                env.close()
            if project_dir and EVAL_DIR and project_dir.startswith(EVAL_DIR):
                scratch_remove(project_dir)
        seconds = round(time.time() - t0, 1)
        row = {"name": t["name"], "project": t["project"], "ok": bool(ok), "seconds": seconds, "detail": detail, "answer": answer, "error": error}
        if metrics:
            row["metrics"] = metrics
        trace = trace_summary(t["name"])
        if trace:
            row["trace"] = trace
        results.append(row)
        if rows_to:
            # Each task's row as it is done, so a long run that is cut short keeps what it did.
            with open(rows_to, "a", encoding="utf-8", newline="\n") as f:
                f.write(json.dumps(row) + "\n")
        spent = f"  {metrics.get('tool_calls', '?')} calls, {metrics.get('tokens', {}).get('total', '?')} tokens, ${metrics.get('cost_usd', 0):.4f}" if metrics else ""
        if metrics.get("peeked"):
            spent += f"  PEEKED at the harness: {metrics['peeked'][:3]}"
        if row.get("trace", {}).get("stuck"):
            spent += f"  STUCK after its first edit: {row['trace']['calls_after_edit']} calls after against {row['trace']['calls_before_edit']} before"
        log(f"{'pass' if ok else 'FAIL'}  {t['name']:<14} {t['project']:<11} {seconds:5.1f} s  {detail}{spent}{('  [' + error + ']') if error else ''}")
    passed = sum(1 for r in results if r["ok"])
    if EVAL_DIR:
        shutil.rmtree(EVAL_DIR, ignore_errors=True)
    log(f"{passed}/{len(results)} passed with runner {runner!r} in {time.time() - started:.1f} s")
    report = {"runner": runner, "passed": passed, "total": len(results), "seconds": round(time.time() - started, 1), "tasks": results}
    spent = [r["metrics"] for r in results if "metrics" in r]
    if spent:
        report["totals"] = {
            "cost_usd": round(sum(m.get("cost_usd", 0) for m in spent), 4),
            "tokens": sum(m.get("tokens", {}).get("total", 0) for m in spent),
            "tool_calls": sum(m.get("tool_calls", 0) for m in spent),
        }
        log(f"spent ${report['totals']['cost_usd']:.4f}, {report['totals']['tokens']} tokens, {report['totals']['tool_calls']} tool calls")
    return report


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="run the agent benchmark against a runner")
    ap.add_argument("--runner", default="reference", help="reference, null, or a shell command that solves a task from stdin")
    ap.add_argument("--tasks", help="comma-separated task names (default: all)")
    ap.add_argument("--timeout", type=int, default=300, help="seconds an external runner may take per task")
    ap.add_argument("--json", action="store_true", help="print the report as JSON instead of lines")
    ap.add_argument("--rows", help="append each task's result as a JSON line to this file as it finishes")
    a = ap.parse_args()
    report = run(a.runner, a.tasks.split(",") if a.tasks else None, a.timeout, log=(lambda *_: None) if a.json else print, rows_to=a.rows)
    if a.json:
        print(json.dumps(report, indent=2))
    sys.exit(0 if report["passed"] == report["total"] else 1)
