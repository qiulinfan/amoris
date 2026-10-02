# Paths

A line through points, and things that move along it: a racing line and the cars on it, a guard's
patrol, the lane creeps walk in a tower defense, a platform's run, a camera's rail. Two components
and three questions.

## Components

- `Path`: `points` (a list of `{x, y, z}` in the entity's own space, so its Transform moves, turns
  and scales the whole line), `closed` (the last point runs back to the first), `smooth` (a
  Catmull-Rom curve through every point; false runs straight from point to point), and `length`,
  which the engine writes. A smooth curve passes through its points and swells a little past the
  straight lines between them (through the corners of a 4 by 4 square it is 16.8 long, not 16).
- `PathFollower`: `path` (the entity with the Path, by name or path), `speed` (units a second along
  it; negative goes back), `distance` (how far along it is: the engine writes it, and writing it
  puts the follower elsewhere), `mode` once (stops at the end, `finished`), loop (round again: a
  closed path goes on round, an open one starts over) or pingpong (back and forth, the engine
  turning `speed` round at each end), `orient` none, forward (its -Z along the path, Y up) or flat
  (turned about Z so its +X points along, for 2D), `offset` (added to the point, unturned) and
  `playing`.

```ts
world.spawn("Lane", { components: { Transform: {}, Path: { points: [{ x: -8, y: 0, z: 0 }, { x: 0, y: 0, z: 4 }, { x: 8, y: 0, z: 0 }] } } });
world.spawn("Creep", { components: { Transform: {}, MeshRenderer: { mesh: "sphere" }, PathFollower: { path: "Lane", speed: 2, orient: "forward" } } });
```

## Every tick

Before the physics steps (with the timelines), every follower moves on by its speed, is put on the
path (plus its offset) and turned with it, in entity order. `path.arrived` comes at a once path's
end and at each end of a ping-pong (`{path, follower, end: "end" | "start"}`), `path.looped` when an
open path starts over. A follower is placed by its Transform as a root would be. A Path's curve is
kept between ticks while its points and its place are unchanged.

## Asking

| Command | What it answers |
|---|---|
| `path.info {entity}` | The length, whether it is closed, its two ends. |
| `path.sample {entity, distance \| fraction}` | The point that far along (wrapped round a closed path, held at the ends of an open one) and the way the path runs there. |
| `path.nearest {entity, point}` | How far along the path the point nearest a given one lies, that point, and how far away it is: how far a car is round the track, whether a runner left its lane. |

In scripts: `paths.info(path)`, `paths.sample(path, {distance})`, `paths.nearest(path, point)`.
`render.debug {paths: true}` draws every Path as its curve.

## From Tiled

A polyline or a polygon drawn in a Tiled object layer carries its points (`tilemap.objects` answers
them in the world as `points`, with `polyline` or `polygon`), and
`tilemap.paths {entity, layer?, smooth?}` makes a Path entity of each, named after the object, its
points where the map draws them, a polygon closed. `samples/sprites` has a patrol route drawn so
(the `routes` layer).

## Verification

`runtime_tests` (`[paths]`): on a straight ten-unit track lifted one unit, a follower at 5 a second
is halfway after a second and finished at the end after two, a ping-pong one at 4 is back at 8 after
three seconds with its speed turned round, a follower going round a closed curve through a square's
corners stays on it without a `path.looped`, and the track answers its quarter point, its direction
and the distance along it of a point beside it; the sprites level's patrol becomes a six-unit Path a
guard runs along.

## The defense sample

`samples/defense` is a tower defense on a `Path`: raiders (built-in humanoids) follow the road as
`PathFollower`s, each wave two more, hardier and quicker; towers (the `tower` prop) built beside the
road by the mouse or the cursor keys for gold shoot the nearest raider in range with `combat.shoot`;
a raider down pays gold, one that reaches the keep (its follower `finished`) costs a life.
`pocket scenario defense` checks that a tower costs fifty gold and is refused on the road and on
another tower, that two towers by the road's bend shoot three raiders down, and that a raider
carried to the road's end costs a life; `tests/evidence/rendering/defense.png` is three towers at
work.

## Not yet

A speed that follows a curve along the path (slowing for corners), a path's own roll for a rail
camera, and paths made from a navmesh's route (`nav.path` answers points a script can give a Path).
