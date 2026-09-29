# Terrain

Open ground in a 3D game is a height field: a grid of heights over a square, hills from noise or from a picture, drawn as one mesh and walked on as one collider. In Pocket it is a component, so an agent makes a landscape with the same words it uses for everything else, asks where the ground is before it places a tree, and reshapes it with a command.

## The component

`Terrain` on an entity (with a `MeshRenderer` to draw it and a static `RigidBody` with a `Collider` of shape 3 to stand on it) makes a grid of `resolution` by `resolution` heights (129 by default) across `size.x` by `size.y` (x by z, 64 by 64) centred on the entity, from 0 up to `height` (8):

- **From a heightmap**: `heightmap` names a greyscale PNG in the project (8 or 16 bits, the first channel), black at 0 and white at `height`, its top row at -z, resampled to the grid.
- **From noise** when `heightmap` is empty: `octaves` layers of gradient noise (4), the first with features `scale` units across (24), each next twice as fine and half as tall, spread over 0..`height`, reproducible from `seed`: the same three numbers make the same hills on every machine.

The heights become a mesh the engine keeps in the asset store under `terrain:<entity>@<revision>` and hands to the renderer and the physics in place of `MeshRenderer.mesh` and `Collider.mesh` (the world's derived mesh for the entity), so the scene file holds only the settings. Its vertices are coloured from the ground: `grass` where it is gentle, `rock` where it is steeper than `rock_slope` degrees (35), `snow` above `snow_line` (a fraction of the height, 0.85; 1 for none), blended across their borders; the `MeshRenderer`'s color multiplies them, its texture repeats every `texture_tile` units (4) and its roughness and the rest apply as to any mesh. Every cell is two triangles split along the same diagonal everywhere, and every answer about the ground (below) follows those triangles, so what a query says is what is drawn and what a body rests on.

Changing the shape settings (heightmap, seed, scale, octaves, resolution, size, height) makes the heights again, dropping sculpting; changing only the colours meshes the same heights again. The entity's transform places the terrain (a turn about y and a scale apply; the queries below take them into account).

## Standing on it

The terrain's collider is a mesh collider of its triangles (`docs/design/physics.md`), so everything that collides with meshes collides with it: rigid bodies land and roll on it, raycasts and sweeps hit it, `nav.bake` maps its walkable cells from above, and a `Character` walks it, up the slopes it may and not up the rest, followed down the far side of a hill without leaving the ground. Ground that rises into a character (sculpted up under it, or a platform rising faster than a step) lifts it onto its top when that top is walkable.

## Scattering

`Scatter` on an entity with a `MeshRenderer` strews copies of that mesh over the ground below it: grass, bushes, stones, flowers. From `seed`, `count` places (500, up to 20000) are tried at random across `area` around the entity; each is dropped onto the ground there (the heights of the terrain `on` names, or the first static collider under the place when `on` is empty or names something else) and kept when that ground is no steeper than `max_slope` (35), within `min_height`..`max_height` (world y: above the water, below the snow) and at least `spacing` from the copies kept before it. Each copy turns at random about the vertical (within `yaw` degrees), leans toward the slope by `align` (0 upright, 1 square to it), sinks by `sink`, takes a size between `scale.x` and `scale.y` times the entity's scale and a brightness within `shade` of the colour. The same seed and settings place the same copies on every run; they are placed again when those settings, the entity's position or a terrain change (a sculpted hill carries its bushes up with it), and `Scatter.placed` says how many stand.

The copies are not entities: the renderer draws the entity's mesh once per copy at the copy's matrix (`World::derived_instances`), casting shadows and culled one by one, and equal meshes batch, so two thousand bushes cost a handful of draw calls. They do not collide; things that should stop the player are entities with colliders (the sample's trees). `scatter.copies {entity, limit}` lists where they stand (point, size, shade), and `render.stats` counts them as `scattered`.

## Commands

| Command | SDK | Purpose |
|---|---|---|
| `terrain.info {entity?}` | `terrain.info()` | Size, height range, resolution, source (`noise` or the heightmap), lowest and highest points, whether it was sculpted, its revision. Without `entity`, the first terrain. |
| `terrain.height {x, z, entity?}` | `terrain.height(x, z)` | The ground at world x, z: its height, the point and the normal there, whether the point is over the terrain (outside, the nearest edge answers). What a script places a tree or a spawn point with. |
| `terrain.sculpt {x, z, radius?, amount?, mode?, target?, entity?}` | `terrain.sculpt(x, z, options)` | A brush around world x, z, full at the centre and fading to nothing at `radius` (3) on a cosine: `raise` (the default) or `lower` by `amount` units at the centre (0.5), `flatten` toward `target` (the height at the centre by default) by `amount` of the way, `smooth` toward the neighbours' mean by `amount` of the way. Heights stay within 0..`height`. Emits `terrain.sculpted` and answers the samples changed, the new revision and the height at the centre. |
| `terrain.save {path, entity?}` | `terrain.save(path)` | The heights as a 16-bit greyscale PNG in the project (65535 is `height`); the terrain's `heightmap` becomes it, so the sculpted ground is what the scene loads next time. |
| `terrain.heights {entity?, heights?}` | `terrain.heights()`, `terrain.setHeights(heights)` | The grid itself, `resolution` squared numbers row after row along z: read, or set whole (the editor's undo; an agent's own generator). |
| `scatter.copies {entity, limit?}` | `command("scatter.copies", ...)` | Where a Scatter's copies stand: point, size and shade of each (the first `limit`, 100), and how many were placed. |
| `terrain.reset {entity?}` | `terrain.reset()` | Heights made again from the heightmap or the noise, sculpting dropped. |

## In the editor

With a terrain selected, the inspector's Sculpt section picks a mode (raise, lower, flatten, smooth), a radius and a strength; Sculpt turns the scene pane into the brush (the transform handles step aside), and a click or a drag on the ground calls `terrain.sculpt` where the ray from the camera through the pointer meets the terrain's collider (flatten levels toward the height the stroke began on). A stroke is one undo step (`terrain.heights` before and after). Save heightmap writes `assets/<name>-heights.png` and points the terrain at it; Reset returns to the noise or the heightmap, undoably. `runtime_tests` (`[editor][terrain]`) drag across the pane, undo, redo and lower.

## The sample

`samples/hills` is a valley of noise hills 96 units across and 12 high around a lake (a `Water` body, `docs/design/water.md`): the script finds the highest point with `terrain.height` and stands a beacon on it, plants trees (a trunk that stops the player and a crown) where the ground is gentle grass above the water, and puts the player on the shore; two Scatters strew about nine hundred bushes over the gentle grass between the water and the snow and some three hundred and fifty stones leaning with the slopes, rock included; WASD or a pad walks the character over the hills. Its scenarios wander a random bot over the hills for eight seconds with the character never closer to the ground than its half height, and raise a ridge of steep rock across the player's way with `terrain.sculpt` (it holds the player back), then flatten it (the player walks on); a third drops a light and a heavy crate into the lake. `assets_tests` (`[terrain]`) pin the noise (the same seed, the same hills), the heights the queries answer against the mesh's own triangles and normals, and the 16-bit PNG read back to within a step; `runtime_tests` (`[terrain]`) stand the player on it, find it with a ray, lift the player by sculpting under it, flatten and smooth, save and read it back, and return to the noise; `[scatter]` checks every bush's place against the ground's height, slope and the limits, the spacing, the same copies on a second run, a sculpt lifting the ones on it, and a count of 0 clearing them.

## Not yet

Detail beyond the grid (holes, caves and overhangs are meshes placed on it), levels of detail and streaming for terrains larger than one mesh (a 1025 by 1025 grid is the limit), texture splatting from painted weights (the colours come from height and slope), copies that collide (a scattered tree is only drawn), and copies that sway or fade out with distance.
