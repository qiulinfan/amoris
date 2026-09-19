# Transcripts of the three samples (300 headless ticks each, 2026-09-18)

Produced with `pocket_runtime --project samples/<name> --headless --frames 300 --json`; the `transcript` field of the report, also available as the `transcript` command and MCP tool. Each line is a segment in which every exposed value keeps its trend; events are grouped by type with up to four subjects.

## hello

```
transcript ticks 0-299 (14 segments)
t0-46 (47 ticks): ball.grounded = 0, ball.y falling 2.997->0, bounces rising 0->1, hue rising 0.001->0.039 | events: entity.spawnedx4(/Ground,/Ball,/Sun,/Camera)
t47-78 (32 ticks): ball.y rising 0.087->1.429, hue rising 0.04->0.066
t79-111 (33 ticks): ball.y falling 1.428->0, bounces rising 1->2, hue rising 0.067->0.093
t112-133 (22 ticks): ball.y rising 0.058->0.657, hue rising 0.094->0.112
t134-155 (22 ticks): ball.y falling 0.655->0, bounces rising 2->3, hue rising 0.113->0.13
t156-170 (15 ticks): ball.y rising 0.038->0.289, hue rising 0.131->0.142
t171-185 (15 ticks): ball.y falling 0.286->0, bounces rising 3->4, hue rising 0.143->0.155
t186-195 (10 ticks): ball.y rising 0.026->0.135, hue rising 0.156->0.163
t196-205 (10 ticks): ball.y falling 0.133->0, bounces rising 4->5, hue rising 0.164->0.172
t206-211 (6 ticks): ball.y rising 0.015->0.052, hue rising 0.172->0.177
t212-218 (7 ticks): ball.y falling 0.051->0, bounces rising 5->6, hue rising 0.177->0.182
t219-222 (4 ticks): ball.y rising 0.009->0.021, hue rising 0.183->0.186
t223-234 (12 ticks): ball.grounded rising 0->1, ball.y falling 0.019->0, bounces rising 6->8, hue rising 0.187->0.196
t235-299 (65 ticks): hue rising 0.197->0.25
```

## playground

```
transcript ticks 0-299 (1 segments)
t0-299 (300 ticks): enemies rising 0->3, hits rising 0->3, killed rising 0->3, player.health falling 100->70 | events: component.addedx6 enemy.spawnedx6 entity.destroyedx3(/Level/Enemy,/Level/Enemy_2,/Level/Enemy_3) entity.spawnedx12(/Level,/Level/Ground,/Level/Player,/Level/Player/Lamp) player.hitx3
```

## physics

```
transcript ticks 0-299 (11 segments)
t0-11 (12 ticks): contacts = 0, dropped = 0, inGoal = 0, lowest = 100, resting = 0 | events: entity.spawnedx9(/Ground,/Ramp,/Goal,/WallEast)
t12-95 (84 ticks): contacts rising 0->1, dropped rising 1->4, lowest falling 7.498->0.47 | events: collision.beginx2(/Crate) collision.endx2(/Crate) entity.spawnedx4(/Crate,/Ball,/Crate_2,/Ball_2)
t96-100 (5 ticks): lowest rising 0.503->0.53 | events: collision.beginx1@t96(/Crate) collision.endx1@t98(/Crate)
t101-122 (22 ticks): dropped rising 4->5, inGoal rising 0->1, lowest falling 0.529->0.399 | events: collision.beginx4(/Ball,/Crate) collision.endx4(/Ball,/Crate) entity.spawnedx1@t110(/Crate_3) goal.reachedx1@t112(/Crate) trigger.enterx1@t112(/Crate)
t123-191 (69 ticks): contacts rising 3->4, dropped rising 5->8, inGoal rising 1->2 | events: collision.beginx11(/Crate,/Crate_2,/Ball,/Ball_2) collision.endx9(/Crate_2,/Ball,/Ball_2,/Crate_3) entity.spawnedx3(/Ball_3,/Crate_4,/Ball_4) goal.reachedx1@t147(/Ball) trigger.enterx1@t147(/Ball)
t192-195 (4 ticks): lowest falling 0.397->0.343 | events: collision.beginx1@t193(/Crate_3) collision.endx1@t195(/Crate_3)
t196-200 (5 ticks): contacts falling 5->4, lowest rising 0.355->0.375 | events: collision.beginx3(/Ball,/Crate_3,/Ball_3) collision.endx3(/Ball,/Crate_3,/Ball_3)
t201-205 (5 ticks): contacts rising 4->5, inGoal = 3, lowest falling 0.374->0.339 | events: collision.beginx1@t202(/Crate_2) collision.endx1@t201(/Crate_2) goal.reachedx1@t201(/Crate_2) trigger.enterx1@t201(/Crate_2)
t206-210 (5 ticks): contacts = 6, dropped = 9, lowest rising 0.341->0.345 | events: collision.beginx1@t206(/Ball) entity.spawnedx1@t206(/Crate_5)
t211-298 (88 ticks): contacts rising 5->6, dropped rising 9->12, inGoal rising 3->6, resting rising 0->3, ray -> "/Ball_3 at 3.83" | events: collision.beginx27(/Crate_3,/Crate_4,/Ball_2,/Ball_4) collision.endx24(/Crate_3,/Crate_4,/Ball_2,/Ball_4) entity.spawnedx3(/Ball_5,/Crate_6,/Ball_6) goal.reachedx3(/Ball_2,/Ball_3,/Crate_4) trigger.enterx3(/Ball_2,/Ball_3,/Crate_4) trigger.exitx1@t211(/Ball)
t299-299 (1 ticks): lowest = 0.264
```
