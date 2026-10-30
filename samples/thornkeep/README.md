# Thornkeep

A third-person action game in a ruined keep at dusk. The garrison is long gone; brigands hold the
gate and the courtyards, and the Thorn Warden sits on the broken throne at the top of the hill,
the thorns growing out of his armour and over every wall. Climb the road, fight through the outer
bailey and the inner ward, light the shrine, and break the Warden.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/thornkeep --walk
```

The browser edition is exported with:

```sh
cargo xtask export-web samples/thornkeep --out out/thornkeep-web --zip out/thornkeep-web.zip
```

Click to capture the mouse. WASD moves (relative to the camera), the mouse turns the camera, Shift
sprints. Left click swings (three swings chain into a combo), E is a heavy blow, Space rolls (you
pass through blows mid-roll), the right button guards, and raising the guard just before a blow
lands parries it, leaving the attacker open to a riposte. Q (or the middle button) locks on to the
nearest foe and back off. 1 throws a bolt of thorn-fire, 2 a fan of knives (both spend focus),
3 or C drinks a flask. Escape releases the mouse. Sound starts with the first click.

- Every swing, roll and guard spends stamina; it comes back when you stop spending it. A blow
  breaks a foe's poise and staggers it; a guard that runs out of stamina breaks.
- Brigands fight with swords, brutes behind shields (a heavy blow goes through a shield), archers
  keep their distance and back off when you close. One foe at a time takes a turn to attack; the
  rest circle.
- Two shrines burn in the keep: one just inside the outer gate, one in the inner ward to the left
  at the top of the stair. Each refills your flasks and, once lit, is where you rise if you fall.
- The Warden wakes when you step into the ring of pillars. At half his health the thorns awaken:
  he grows faster and casts. When he falls the keep is freed, and the game is done.

## How it is built

The game is a test of the engine's skeletal animation, added for it (docs/spec/animation.md):
clip libraries parsed from glTF and retargeted onto each body, state machines with crossfades,
blend spaces, montages with sections, notifies, root motion, sockets, the avatar that turns a
walker into a body facing its motion or its target, the spring-arm camera with its shake, the
navigation grid and the scripts' physics queries.

- `level.py` is the keep's layout as data (walls, towers, gates, stairs, slabs, houses, the ring of
  pillars, the keep's tower), the props, braziers and foes, the ground's terrain features with the
  engine's height arithmetic mirrored (`ground`), and the forest scattered on it.
- `build_level.py` models the keep in Blender from that layout with primitives and bmesh edits,
  Poly Haven's stone, slate, wood and iron textures box-projected in metres, and the braziers,
  banners and thorn vines; it exports `models/level.glb` and the ground's materials
  (`models/ground.glb`).
- `build_dressing.py` makes `models/dressing.glb`, one node per kind: firs modelled from crossed
  branch cards cut from Poly Haven's fir twig atlas, a stave barrel, and Poly Haven's crates,
  chest, rocks, boulders, trunks, stumps, branches, ferns and grass, decimated, with the plants'
  alpha folded into their colour.
- `build_characters.py` assembles the five characters from Quaternius's outfits, heads and
  hairstyles on one 65-joint skeleton, models the Thorn Warden's armour (a helm crowned with
  thorns and lit from inside, plates, back thorns), and writes the two animation libraries as
  clip-only glTF files (`models/anim/ual1.glb`, `ual2.glb`). `build_props.py` models the swords,
  greatsword, knife, axe, shield and the thorn bolt (`models/props.glb`).
- `build_anims.py` writes each character's animation graph (`anim/*.anim.json`): the locomotion
  blend, guard and death states, the upper-body and full-body slots, and the montages of every
  attack, roll, hit, stagger, cast, throw and drink, with their hit, chain, evade, step, swing and
  release notifies.
- `build_scene.py` writes `scene.json` and `perception.json`: the terrain, the keep and its
  colliders, the road's parapets, the dressing, the braziers' fire, light and sound, the
  navigation grid, the fighters (each a walker, its posed body, its weapon on a hand socket with
  its sparks, and the spray at its chest), the spring-arm camera and the HUD.
- `scripts/main.ts` holds the rules: the sellsword's controls, stamina, combos, rolls, guard and
  parry, lock-on, bolts, knives and flasks; the blade's swept hits from its notifies; the foes'
  brains (watch, alert, chase by the grid, circle, take turns, strike, back off, return); the
  Warden's phases; the shrine and the outcome; the sounds, sparks and camera shake.
- `build_audio.py` synthesizes every sound: swings, blows, clangs, guards and parries, rolls,
  steps, deaths, the alarm horn, the bolt, the Warden's roar, the shrine's chime, the wind and the
  braziers' fire.
- The Blender file the models were built in, one collection per part, is kept locally as
  `art/thornkeep.blend` (ignored, not in the repository); the scripts above rebuild every model
  from the sources.

Rebuild everything with (the Quaternius packs unpacked under `out/thornkeep-sources/ual1`, `ual2`,
`ubc` and `outfits`, as build_characters.py names them):

```sh
python samples/thornkeep/fetch_assets.py
blender -b --factory-startup --python samples/thornkeep/build_characters.py
blender -b --factory-startup --python samples/thornkeep/build_props.py
blender -b --factory-startup --python samples/thornkeep/build_level.py
blender -b --factory-startup --python samples/thornkeep/build_dressing.py
python samples/thornkeep/build_anims.py
python samples/thornkeep/build_audio.py
python samples/thornkeep/build_scene.py
```

`check.toml` runs the engine's determinism, fork and reload checks on the walk up the road to the
gate fight.

Credits and licences are in `CREDITS.txt`.
