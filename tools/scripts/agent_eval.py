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
import os
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
from pocket_env import PocketEnv  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
POCKET = os.path.join(ROOT, ".pocket", "pocket")
DOCS = ["docs/sdk.md", "docs/mcp.md", "docs/design/world-model.md", "docs/design/tilemaps.md", "docs/design/navigation.md", "docs/design/physics.md", "docs/design/animation.md", "docs/generated/components.md"]


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
    with open(os.path.join(project_dir, "project.toml")) as f:
        for line in f:
            m = re.match(r'\s*entry\s*=\s*"([^"]+)"', line)
            if m:
                return m.group(1)
    return "scripts/main.ts"


def edit_main(project_dir, transform):
    path = os.path.join(project_dir, entry_script(project_dir))
    with open(path) as f:
        text = f.read()
    with open(path, "w") as f:
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
    with open(os.path.join(project_dir, "prefabs", "lamp.json"), "w") as f:
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
    {"name": "lamp_prefab", "project": "hello", "ticks": 0, "script": True, "solve": lamp_prefab_solve, "check": lamp_prefab_check,
     "task": "Write a prefab file prefabs/lamp.json in the project directory: a pocket-scene fragment (format \"pocket-scene\", version 1, an \"entities\" list) whose one root entity named Lamp draws a yellow sphere (a MeshRenderer with mesh \"sphere\" and color r 1, g 0.9, b 0.2) and has a child named Glow with a Light of kind 1 (a point light) and intensity 2. Then edit scripts/main.ts so that on start the project instantiates that prefab three times through the SDK's world.instantiate with the file's path, named Lamp0, Lamp1 and Lamp2, at x -2, 0 and 2, y 1, z 0."},
]


def bundle(project_dir):
    """Bundle a project's TypeScript with the tool; the bundle lands in build/ts/<dir name>.js."""
    proc = subprocess.run([POCKET, "ts", project_dir], capture_output=True, text=True, cwd=ROOT, env={**os.environ, "POCKET_ROOT": ROOT})
    if proc.returncode != 0:
        raise RuntimeError(f"bundling {project_dir} failed: {(proc.stderr or proc.stdout).strip()[-400:]}")


def scratch_copy(task):
    """A copy of the task's sample under build/agent-eval, with its own bundle name."""
    name = f"bench-{task['name']}-{os.getpid()}"
    dst = os.path.join(ROOT, "build", "agent-eval", name)
    shutil.rmtree(dst, ignore_errors=True)
    proc = subprocess.run([POCKET, "new", name, "--from", task["project"], "--dir", os.path.join("build", "agent-eval")], capture_output=True, text=True, cwd=ROOT, env={**os.environ, "POCKET_ROOT": ROOT})
    if proc.returncode != 0:
        raise RuntimeError(f"copying {task['project']} failed: {(proc.stderr or proc.stdout).strip()[-400:]}")
    bundle(dst)
    return dst


def scratch_remove(project_dir):
    shutil.rmtree(project_dir, ignore_errors=True)
    name = os.path.basename(project_dir)
    for suffix in (".js", ".js.project.json"):
        try:
            os.remove(os.path.join(ROOT, "build", "ts", name + suffix))
        except FileNotFoundError:
            pass


def run_external(cmd, env, task, timeout, project_dir):
    """One external runner: the task as JSON on stdin, the last JSON line of its output as the answer."""
    payload = {"task": task["task"], "project": task["project"], "project_dir": project_dir, "rpc_url": env.url, "docs": DOCS,
               "notes": "POST {\"id\": 1, \"method\": \"<command>\", \"params\": {...}} to rpc_url + \"/rpc\"; the runtime is paused; `commands` lists every method."
                        + (f" This task edits files: change {task.get('entry', 'scripts/main.ts')} under project_dir; the harness bundles the project again and reloads the project (a fresh world from the scene, the script started again) when you are done." if task.get("script") else "")}
    proc = subprocess.run(cmd, input=json.dumps(payload), capture_output=True, text=True, shell=True, timeout=timeout, env={**os.environ, "POCKET_RPC_URL": env.url})
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


def run(runner="reference", tasks=None, timeout=300, log=print, project_root=None):
    chosen = [t for t in TASKS if not tasks or t["name"] in tasks]
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
            project_dir = scratch_copy(t) if t.get("script") else os.path.join(project_root or ROOT, "samples", t["project"])
            # POCKET_EVAL_RUNTIME: a copy of the runtime to test (so a long run is not changed by a rebuild).
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
            if t.get("script") and project_dir:
                scratch_remove(project_dir)
        seconds = round(time.time() - t0, 1)
        row = {"name": t["name"], "project": t["project"], "ok": bool(ok), "seconds": seconds, "detail": detail, "answer": answer, "error": error}
        if metrics:
            row["metrics"] = metrics
        results.append(row)
        spent = f"  {metrics.get('tool_calls', '?')} calls, {metrics.get('tokens', {}).get('total', '?')} tokens, ${metrics.get('cost_usd', 0):.4f}" if metrics else ""
        log(f"{'pass' if ok else 'FAIL'}  {t['name']:<14} {t['project']:<11} {seconds:5.1f} s  {detail}{spent}{('  [' + error + ']') if error else ''}")
    passed = sum(1 for r in results if r["ok"])
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
    a = ap.parse_args()
    report = run(a.runner, a.tasks.split(",") if a.tasks else None, a.timeout, log=(lambda *_: None) if a.json else print)
    if a.json:
        print(json.dumps(report, indent=2))
    sys.exit(0 if report["passed"] == report["total"] else 1)
