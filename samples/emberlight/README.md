# Emberlight

A first-person light puzzle in a drowned mountain observatory. The sun did not rise this year; one
ember still burns in the vault. You are the last keeper, and you carry no lamp of your own. In four
vaulted chambers, turn the brass mirrors and the half-silvered prism so the ember's beam reaches
every sleeping sun-lantern. Each lamp that wakes warms the blue stone a little; when the fifth
wakes, the sun returns through the crown chamber's oculus.

From the repository root:

```sh
cargo run --release -p amoris-app -- play samples/emberlight --walk
```

The browser edition is exported with:

```sh
cargo xtask export-web samples/emberlight --out out/emberlight-web --zip out/emberlight-web.zip
```

Serve that folder on a static host or upload its zip as an itch.io HTML5 game. In browsers that
refuse mouse capture, click to start keyboard play and drag to look; a click without dragging still
turns the nearest mirror. Browser audio is not available.

Click to capture the mouse. WASD walk, the mouse looks, Shift runs, Space jumps. Walk up to a
mirror or the prism and left-click to turn it ninety degrees; right-click puts every mirror in the
chamber back to the turn it started at. Escape releases the mouse.

- Four chambers: the threshold (one mirror), the crossing (three mirrors), the fork (a prism and two
  mirrors feeding two lamps), and the crown (a spiral around the altar).
- The beam, the lamps, the doors and the dawn are the rules' work: the beam is traced after every
  turn, a lamp wakes when a ray reaches its crystal, the chamber's door opens when all its lamps
  burn, and the sun rises when the fifth does. A woken lamp never goes out.
- Walk through a chamber's arch once its door has risen; the camera can keep looking around while
  the ending plays.

## How it is built

- `build_scene.py` writes `architecture.glb`, `scene.json` and `perception.json`: the four chambers
  on one axis, each a 12 × 12 m vault with a coffered ceiling, a moon oculus and its cold light,
  corner columns, pilasters, banners, braziers, brass wall sconces, and the interior wall cells the
  beam sees.
  The same file verifies every chamber's puzzle before writing anything: the starting turns must
  light no lamp, and exactly one configuration of the chamber's mirrors (and prism) must light
  them all. The assertion runs on every build.
- `textures.py` draws the two tileable 1024 × 1024 surface sets embedded in the mesh: dressed blue
  ashlar with chisel marks, damp stains, lichen and hairline cracks, and worn slate flags with
  grout and mineral veins. Their tangent-space normal maps come from the same height fields. Only
  the Python standard library is used.
- `build_audio.py` writes the original sound: a seamless 24 s D-minor observatory drone, a struck
  bronze lamp bell, a stone door rising and a dawn swell. The drone's every partial completes a
  whole number of cycles over the loop, so it joins without a fade. The scene plays the drone and a
  quiet wind loop; the project's `[sounds]` table rings the bell at the waking lamp, grinds the door
  at the door, and brings the dawn swell at the last lamp.
- `build_models.py` builds the props headless in Blender 4.5 and exports `models/*.glb`: the
  standing mirror, the caged sun-lantern (its crystal is a separate mesh so only the crystal
  lights), the ember bowl with its floating ringed crystal, the half-silvered prism on its
  pointer, the tripod brazier, the great crown beacon, and the sliding door with its bronze sun.
  Every prop's origin is at its foot. Rebuild with:

  ```sh
  /Applications/Blender.app/Contents/MacOS/Blender -b --python samples/emberlight/build_models.py
  ```

  The audio is generated with `python3 samples/emberlight/build_audio.py`.

- `scripts/main.ts` holds the rules: mirror and prism interaction, the grid ray trace with splitters
  and latching lamps, the beam's render pool, door opening, the finale and the HUD. The beam is
  drawn with `Model` segments whose scale, rotation, colour and emission the rules write each tick;
  a second small pool marks the turns and wall impacts with bright flares, and each waking lamp's
  light takes its chamber's colour. `Light` and the `Environment` are animated the same way for the
  lamp crystals, brazier flicker, sconces and the dawn. Those three components became script-visible
  in the engine for this game, so a look can be animated without an engine round trip. The room
  title also counts the chamber's own lamps, so the keeper can tell a two-lamp chamber from a
  one-lamp one.
- `check.toml` runs the deterministic, fork, replay and reload checks. Its input scenario turns
  every mirror and the prism through the player seat's own controls (the walk between them
  elided) and then runs out the dawn, so the checks cover the complete beam, lamp, door and finale
  chain rather than an idle scene.

Regenerate the scene and the architecture with:

```sh
python3 samples/emberlight/build_scene.py
```

The generated files are committed with their generators. Everything in this folder is original: the
geometry is built from primitives by `build_models.py` and `build_scene.py`, and the textures are
drawn by `textures.py`. No third-party assets are used.
