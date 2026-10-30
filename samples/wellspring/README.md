# Wellspring

A first-person open-world adventure in a dry highland valley. The keepers who tended the valley's
spring closed it at three sluices the winter the lake turned to salt, and left. You come back to a
river of dry stone, a shrunken lake and an empty village. Find the bronze handles of the three
sluice wheels, fit them and turn the sluices open, and the water comes back: real water, running
down its bed, over two falls and into the lake. Follow it down, on the keepers' old raft if you
dare, and ring the bell in the shrine on the island.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/wellspring --walk
```

The browser edition is exported with:

```sh
cargo xtask export-web samples/wellspring --out out/wellspring-web --zip out/wellspring-web.zip
```

Click to capture the mouse. WASD walks, the mouse looks, Shift runs, Space jumps (and swims up in
water). Left-click uses what you face: takes a handle, fits it to a wheel, turns the wheel, reads a
stone, boards the raft or rings the bell. On the raft, W paddles, A / D steer and right-click steps
off it, onto the bank or into the water. Escape releases the mouse. Sound starts with the first
click, in the browser too.

- The three handles lie where the keepers last drank: by the village well, at a cairn on the west
  hill and in a ruined hut on the east plateau. Cairns mark the ways.
- The three sluices: the cave spring at the river's head under the northern tarn, the west stream
  on the ridge, and the aqueduct's intake on the east ridge, whose water crosses seven arches and
  falls from the spout into the river.
- Each quarter turn of a wheel lifts its gate and lets more water through; four turns open it.
  The day moves on with each sluice you open, toward evening.
- The keepers' raft is moored at the jetty below the village: paddle it across the lake to the
  island, or swim.
- Once the river reaches the lake, the shrine bell can be rung, and the valley greens. That is the
  end; it stays yours to walk.

## How it is built

- `terrain.py` declares the ground as one `Terrain` of features (slopes, ridges of overlapping
  hills, rolling noise, dry ravines down the flanks, the river's channel with its two falls and the
  west stream's), and mirrors the engine's height arithmetic so the generator can stand everything
  on the ground. The river's bed is worked out from the ground along its course, falling all the
  way.
- `build_scene.py` writes `scene.json` and `perception.json`: the ground and the far mountains,
  the river and lake as two joined GPU-resident water regions, the three sluices' `WaterSource`s
  (shut at first), the keepers' works with their colliders, the village, the cairns, the raft (a
  `Board` with floaters), the shrine and its bell, the lake gauge (a `WaterProbe`) and the HUD.
  `scatter.py` places the boulders, cliff outcrops, quiver-tree groves, shrub clumps, patches of
  dry grass (thick along the paths and the river) and grass and scree along the seams where grass
  meets rock, by the ground's slope and height, off the water, the paths and the works. The shrubs
  and grass are one entity, `models/vegetation.glb`, written from `plants.glb` with one node per
  plant (the renderer draws and culls each node as an instance), the scree likewise
  (`models/scree.glb`); the far mountains (`models/far.glb`, terrain.py) and the cloud dome
  (`models/clouds.glb`, sky.py) are generated meshes too.
- `scripts/main.ts` holds the rules: what the keeper faces and uses, the sluices' wheels, gates,
  water and spray, the gauge that tells when the river has reached the lake, the raft's mooring
  rope, the day's sun and sky, the bell's swing, the valley greening and the lanterns brightening
  toward evening, and the HUD.
- `build_structures.py` models every building and prop in Blender from primitives, modifiers and
  bmesh: the sluice houses, gates and wheels, the aqueduct's bays and spout, the shrine and its
  bell (a lathe profile), the raft and its jetty, the huts with their fallen beams and rubble, the
  well, the trough and the water jars, the steles, the cairns, the handles and the lanterns, with
  box-projected texture coordinates in metres. It exports `models/structures.glb`.
- `build_models.py` turns Poly Haven scans into game-ready models in Blender: decimated boulders,
  cliffs, shrubs and quiver trees, leaves alpha-tested, origins at the foot
  (`models/rocks.glb`, `models/plants.glb`), and the ground's four materials with their tiling
  (`models/ground.glb`). `fetch_assets.py` downloads its sources first.
- `build_textures.py` paints the grass card (dry blades and seed heads, alpha-tested) and the
  clouds (fractal cumulus around the azimuth).
- `build_audio.py` synthesizes every sound: the wind, the wheel, the gate and the rush of water,
  the handle's chime, the bell and the river's arrival.
- The Blender file the models were built in, one collection per exported file, is kept locally as
  `art/wellspring.blend` (ignored, not in the repository); the scans' textures it links are those
  `fetch_assets.py` downloads into `out/wellspring-sources/`.

Rebuild everything with:

```sh
python samples/wellspring/fetch_assets.py
python samples/wellspring/build_textures.py
blender -b --factory-startup --python samples/wellspring/build_models.py
blender -b --factory-startup --python samples/wellspring/build_structures.py
python samples/wellspring/build_audio.py
python samples/wellspring/build_scene.py
```

`checks/pilot.py` is a test pilot that plays the whole quest through the player seat's own controls
(walking by waypoints, facing and clicking, paddling the raft), against a running host or the
exported page in a browser. `check.toml` runs the engine's determinism, fork, replay and reload
checks on a short walk to the well.

Credits and licences are in `CREDITS.txt`.
