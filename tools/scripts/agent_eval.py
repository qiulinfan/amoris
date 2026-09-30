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
DOCS = ["docs/design/assets.md", "docs/design/pocket-ui.md", "docs/sdk.md", "docs/mcp.md", "docs/design/world-model.md", "docs/design/rendering.md", "docs/design/tilemaps.md", "docs/design/navigation.md", "docs/design/physics.md", "docs/design/terrain.md", "docs/design/water.md", "docs/design/wind.md", "docs/design/timelines.md", "docs/design/cameras.md", "docs/design/localization.md", "docs/design/networking.md", "docs/design/animation.md", "docs/generated/components.md"]


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
    with open(toml) as f:
        text = f.read()
    line = 'jump = ["Space", "pad:a"]\n'
    if line not in text:
        raise RuntimeError("the walker project.toml changed shape")
    with open(toml, "w") as f:
        f.write(text.replace(line, line + 'sprint = ["LShift", "pad:left_shoulder"]\n', 1))

    def transform(t):
        a = 'world.set(player, "Character", { velocity: { x: input.axis("move_x") * SPEED, y: vy, z: input.axis("move_z") * SPEED } });'
        if a not in t:
            raise RuntimeError("the walker script changed shape")
        return t.replace(a, 'const speed = input.down("sprint") ? 9 : SPEED;\n    world.set(player, "Character", { velocity: { x: input.axis("move_x") * speed, y: vy, z: input.axis("move_z") * speed } });', 1)
    edit_main(project_dir, transform)


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
    with open(os.path.join(locales, "fa.json"), "w") as f:
        json.dump(PERSIAN, f, ensure_ascii=False, indent=2)
    for lang in ("en", "zh", "ar"):
        path = os.path.join(locales, lang + ".json")
        with open(path) as f:
            doc = json.load(f)
        doc["language"]["fa"] = "فارسی"
        with open(path, "w") as f:
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


BLENDER = "/Applications/Blender.app/Contents/MacOS/Blender"


def blender_level_solve(env, project_dir):
    script = os.path.join(project_dir, "make_level.py")
    with open(script, "w") as f:
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
        "Scatter": {"count": 400, "area": {"x": 20, "y": 20}, "on": "Hills", "scale": {"x": 1, "y": 1}, "sway": 0.3, "collide": 1.0}}})
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
    # Scatter's collide times the entity's scale times the copy's size.
    c = cp["copies"][0]
    hit = env.command("physics.raycast", {"origin": {"x": c["x"], "y": c["y"] + 30, "z": c["z"]}, "direction": {"x": 0, "y": -1, "z": 0}})
    if not hit or hit.get("entity") != rid or hit["point"]["y"] < c["y"] + 0.5:
        return False, f"a ray down onto the first reed meets {hit}"
    scale = (env.command("world.get", {"entity": rid, "component": "Transform"}) or {}).get("scale", {})
    for c in cp["copies"]:
        size = c["size"] / max(abs(scale.get("y", 1)), 1e-6)   # scatter.copies' size is the copy's height scale
        r = sc.get("collide", 0) * scale.get("x", 1) * size
        if abs(r - 0.1) > 0.005:
            return False, f"a reed's collider is {r:.3f} in radius (collide {sc.get('collide')}, scale x {scale.get('x')}, size {size:.2f})"
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
    {"name": "lamp_prefab", "project": "hello", "ticks": 0, "script": True, "solve": lamp_prefab_solve, "check": lamp_prefab_check,
     "task": "Write a prefab file prefabs/lamp.json in the project directory: a pocket-scene fragment (format \"pocket-scene\", version 1, an \"entities\" list) whose one root entity named Lamp draws a yellow sphere (a MeshRenderer with mesh \"sphere\" and color r 1, g 0.9, b 0.2) and has a child named Glow with a Light of kind 1 (a point light) and intensity 2. Then edit scripts/main.ts so that on start the project instantiates that prefab three times through the SDK's world.instantiate with the file's path, named Lamp0, Lamp1 and Lamp2, at x -2, 0 and 2, y 1, z 0."},
    {"name": "hill_raise", "project": "hills", "ticks": 2, "before": hill_raise_before, "solve": hill_raise_solve, "check": hill_raise_check,
     "task": "Raise the ground of the terrain named Hills at x 20, z -20 until it stands at least 10 units high there, without changing the ground 12 or more units away from that point; then answer with the ground's height at x 20, z -20 as the number \"answer\"."},
    {"name": "flowers", "project": "hills", "ticks": 2, "solve": flowers_solve, "check": flowers_check,
     "task": "Strew red flowers over the terrain named Hills: spawn an entity named Flowers that draws small red spheres (a MeshRenderer with mesh \"sphere\" and color r 1, g 0, b 0.1) with a Scatter placing copies on Hills over the whole terrain, only where the ground is no steeper than 20 degrees. At least 50 flowers must stand."},
    {"name": "car_speed", "project": "drive", "ticks": 120, "solve": car_speed_solve, "check": car_speed_check,
     "task": "Make the car named Car faster: set its Vehicle's top_speed to 35 and power to 14, then drive it with the throttle action held for 300 ticks (5 seconds of game time) and answer with the car's speed at the end as the number \"answer\" (the game's script sets the car's throttle from the throttle action every tick)."},
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
    {"name": "dirt_patch", "project": "hills", "ticks": 2, "solve": dirt_patch_solve, "check": dirt_patch_check,
     "task": "The terrain named Hills is drawn from four textured layers (grass, sand, rock, dirt). Make its sand layer lie only below 0.2 of the terrain's height instead of where it lies now, leaving the other layers as they are, and paint its dirt layer at full strength in a patch of radius 4 around x 10, z -10. Answer with the dirt layer's share of the ground at x 10, z -10 as the number \"answer\"."},
    {"name": "glass_window", "project": "hello", "ticks": 0, "solve": glass_window_solve, "check": glass_window_check,
     "task": "Spawn an entity named Window: a cube scaled x 2, y 1.5, z 0.05 at x 0, y 1.5, z 3, drawn as clear glass that lets all the light through, with an index of refraction of 1.52. Give the entity named Ball a clear coat at full strength with a roughness of 0.05. Then step one tick and answer with the number of glass meshes the renderer drew in that frame as the integer \"answer\"."},
    {"name": "persian_locale", "project": "ui", "ticks": 0, "script": True, "entry": "scripts/main.tsx", "edits": "the files in locales/", "solve": persian_locale_solve, "check": persian_locale_check,
     "task": "The interface speaks English, Chinese and Arabic (locales/en.json, zh.json, ar.json). Add Persian: write locales/fa.json with every text the English file has, translated into Persian (keep the placeholders and plural forms working), and give the language choice its name by adding language.fa (\"فارسی\") to every language file. Answer with anything; the check switches to Persian and reads the interface."},
    {"name": "blender_level", "project": "assets", "ticks": 0, "script": True, "edits": "assets/level.blend (made by Blender)", "solve": blender_level_solve, "check": blender_level_check,
     "task": "Make a small level in Blender, which is installed at /Applications/Blender.app/Contents/MacOS/Blender (run it headless with a Python script, -b --factory-startup --python). Save it as assets/level.blend in the project with: a plane named Floor, 10 across, lying flat 1 unit above the ground (Blender z 1), that the engine will make a static body colliding with its own triangles; and a box named Pillar, 1 by 1 by 3 standing on it at Blender x 3, y 0, that becomes a static body with a box collider 0.5 by 1.5 by 0.5 in half extents and has 50 health (the Health component, max and current 50). Give them those engine components through Blender custom properties so that world.instantiate of the file brings them. Then instantiate it at x 60 and drop a dynamic sphere of radius 0.5 (mass 1) from 8 up onto the floor at x 58, z 1; answer with the height it comes to rest at after 180 ticks as the number \"answer\"."},
    {"name": "calm_lake", "project": "hills", "ticks": 2, "solve": calm_lake_solve, "check": calm_lake_check,
     "task": "Calm the lake: make the water of the entity named Lake perfectly still (no waves) and clearer, so that one sees 8 units into it, and raise its surface by half a unit (it stands at 3.2), leaving it centred where it is. Answer with the water's surface height at x 0, z 0 as the number \"answer\"."},
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
                        + (f" This task edits files: change {task.get('edits', task.get('entry', 'scripts/main.ts'))} under project_dir; the harness bundles the project again and reloads the project (a fresh world from the scene, the script started again) when you are done." if task.get("script") else "")}
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


def run(runner="reference", tasks=None, timeout=300, log=print, project_root=None, rows_to=None):
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
        if rows_to:
            # Each task's row as it is done, so a long run that is cut short keeps what it did.
            with open(rows_to, "a") as f:
                f.write(json.dumps(row) + "\n")
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
    ap.add_argument("--rows", help="append each task's result as a JSON line to this file as it finishes")
    a = ap.parse_args()
    report = run(a.runner, a.tasks.split(",") if a.tasks else None, a.timeout, log=(lambda *_: None) if a.json else print, rows_to=a.rows)
    if a.json:
        print(json.dumps(report, indent=2))
    sys.exit(0 if report["passed"] == report["total"] else 1)
