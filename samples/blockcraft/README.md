# Blockcraft

A voxel sandbox island in the style of the original block game: a 128 x 128 seeded island of
one-metre blocks in an open ocean, with grass, dirt, stone, coal, iron and gold ore, sand, gravel,
snow-capped peaks, oak and birch trees, tall grass, poppies, dandelions, water and flat blocky
clouds. Break any block but bedrock, and build with nine kinds from the hotbar. The water is the
engine's shallow water: break a lake's bank and it floods, place water and it pours a block's worth
that runs off, dig under it and it drains into the hole; swim by holding Space.

The textures are original 16 x 16 pixel art drawn by `textures.py`; no game's assets are copied.

## Playing

From the engine's repository root:

```sh
cargo run --release -p amoris-app --bin amoris -- play samples/blockcraft --walk
```

Click to capture the mouse. WASD moves, Shift runs, Space jumps (one block) and swims up, the
left button breaks the block in the crosshair, the right button places the hotbar's block against
it, 1 to 9 or the wheel choose the block. Escape releases the mouse; Escape again exits.

The window plays the game's `player` seat: every input is a `player.act` the engine records, so an
agent over MCP or the CLI (`amoris player act --seat player ...` against `amoris serve`) plays
the same way.

## How it is built

The island is the engine's block volume (`Voxels`): the engine meshes it into chunks, the
first-person body (`Walker`) collides with its solid blocks and swims in its water, and the scripts
edit it (`docs/spec/voxels.md`). The water is one `WaterRegion` over the island at two columns a
block (`docs/spec/water.md`): the volume's water blocks are liquids that flow (their palette entry
has a `flow`), which the engine turns into the region's water at load and whenever a script places
one, and `physics.water` moves it as a height field over the blocks' beds.

- `gen_world.py` writes everything the scene loads: `textures/` (16 x 16 pixel art written
  upscaled by nearest neighbour to 1024 x 1024, the size the engine resamples every texture to,
  so the pixels stay sharp), `blocks.gltf` (the blocks' materials, no geometry), `world.cells`
  (the volume's cells), `clouds/` and `scene.json` (the player placed where it sees farthest
  across open land; the water region draws 5 km of open sea around the island at its level).
- `scripts/main.ts` holds the rules: breaking and placing (a ray cast from the eye through water;
  bedrock stays; plants go with the block under them; a block never goes where the player
  stands; poppies only on grass or dirt), the hotbar's text, the return to the spawn after a fall
  into the void, and the clouds' drift.
- `perception.json` and `[player]` in `project.toml` declare the `first_person` seat.

Edit the generator (its constants set the seed, size, sea level and cloud height) and run:

```sh
python3 samples/blockcraft/gen_world.py
```
