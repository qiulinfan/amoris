# Behavior

What a guard, a shopkeeper or a wolf does is mostly a few states and the reasons to change between
them: wander until something comes near, chase it while it is in sight, run when hurt, go back home.
A `Behavior` says that as data on the entity, and the engine runs it every tick, so the state is in
the world (`world.query {with: ["Behavior"], fields: ["Behavior.state"]}` shows what every guard is
doing), every change is an event, and a scenario or an agent checks it the way it checks anything
else. A script that wants more writes the state itself, or sets the target.

```json
"Behavior": {
  "target": "Player",
  "states": [
    { "name": "wander", "move": "wander", "radius": 3 },
    { "name": "chase", "move": "follow", "speed": 4.5, "event": "guard.alerted" },
    { "name": "flee", "move": "flee", "radius": 8 }
  ],
  "transitions": [
    { "to": "flee", "when": "health < 30" },
    { "from": "wander", "to": "chase", "when": "sees and distance < 8" },
    { "from": "chase", "to": "wander", "when": "not sees and time > 3" }
  ]
}
```

## States

Each state says how the entity moves while in it, through its `NavAgent`
(`docs/design/navigation.md`, Agents), which does the walking: `stay` stands still; `follow` follows
the target; `flee` walks to `radius` away from the target and again whenever the target comes within
half of that of where it was going; `wander` walks to a point within `radius` of home, chosen from
the run's seed, and to another on arriving; `home` walks back to `home`; `patrol` walks the points
of the `Path` named by `path` (`docs/design/paths.md`) in turn, looping, `waypoint` saying which it
heads for; `seek` walks to `seen_at`, where it last saw the target; `investigate` walks to
`heard_at`, where it last heard a noise. `speed`, when not 0, is the agent's speed in the state. A
state that moves an entity without a `NavAgent` says so in `error` and moves nothing. `face: true`
turns the entity toward the target while in the state, its -Z forward about the vertical, at its
`NavAgent`'s `turn_speed` (540 degrees a second without one): a guard that stops to confront, a
shopkeeper who greets, a turret. `runtime_tests` (`[behavior][face]`): a keeper facing -Z turns
partway in a tenth of a second toward a mark on its +X and faces it within two thirds of a second.

Entering a state (the first at the start, one a transition goes to, or one written into `state` from
outside) emits `behavior.changed` with `{from, to}` and the entity as its subject, emits the state's
own `event` when it has one, plays its `clip` on the entity's `Animator` when it has one, and starts
`time` again from 0. `previous` is the state it left.

## Transitions

Every tick, after the hits and before the agents move, the transitions are tried in order and the
first whose `from` is the state (`*` for any but the one it goes to) and whose condition holds is
taken; at most one a tick. A condition is written as an `AnimationGraph`'s is (comparisons, `and`,
`or`, `not`, parentheses; a bare name is true when it is not 0), over what the entity perceives:

- `distance`: to the target (a billion with none), from the entities' origins;
- `sees`: 1 when the target is within `sight`, within `fov` degrees of where the entity faces (its
  -Z, turned about the vertical), and no collider stands on the line between `eye` above the two
  origins (the two themselves, their children and triggers aside). In a 2D game the line between the
  two origins in the XY plane must also be clear of an orthogonal tile map's cells that hide (solid
  ones, or tiles with an `opaque` property, as for `tilemap.sight`) and of the 2D bodies;
- `unseen`: seconds since it last saw the target (since it started, when it never has), kept in
  `unseen` with where the target was, `seen_at`; so "not seen for 2 seconds" is `unseen > 2`;
- `time`: seconds in the state;
- `health`, `health_max`: its `Health` (0 without one);
- `hit`: 1 when a hitbox hit it since the last tick (`docs/design/combat.md`);
- `arrived`, `stuck`: its agent's state;
- `random`: a number in [0, 1) new every tick, from the run's seed (so `random < 0.01` is about once
  in a hundred ticks, the same in every run);
- `has_target`: 1 while the target is alive;
- `heard`: 1 when an event of the transition's `on` type was emitted since the last tick; a
  transition with `on` needs one, whatever its condition;
- `noise`: 1 when a noise reached it since the last tick (Noises below), and `unheard`, seconds
  since one last did (since it started, when none has).

A condition that does not read, or a transition to a state that is not there, is said in `error` and
never taken. `home` is where the entity stood at its first tick unless it was given (any point, the
origin included: the default that means "not given" is a million units down; an agent that set its
villagers' home to the centre of a square had them wander round where each started, when zero meant
"not given"); `enabled: false` holds the state and leaves the agent alone.

## Picking whom to attend to

`target` is one entity, set in the scene or by a script. `targets` picks it instead: the nearest
entity whose name matches a pattern (`"Sheep*"`, `*` any run of characters and `?` one) or, when it
names a component (`"Health"`, a project's own), the nearest that has it, other than the Behavior's
own entity and its children. It is looked for again every quarter second (each entity on its own
tick of the fifteen) and at once when the one it had is gone, and each change is a `behavior.target`
event (`{target, path}`). A wolf hunts whichever sheep is nearest, an enemy turns on the nearest of
two players, a villager greets whoever walks by. `runtime_tests` (`[behavior][targets]`): a wolf
picks the nearer of two sheep, the other at once when that one goes, a third that comes nearer
within a quarter second, and the nearest with `Health` when told to.

## Noises

A guard that only sees is easy to sneak past at a run. A `noise` event is a sound the Behaviors
hear: made where `data.at` says (`{x, y, z}`), or where its subject stands, and carrying
`data.radius` units (10 by default). Each Behavior within the radius times its `hearing` (1; 0 is
deaf), other than the subject itself, hears it that tick: `noise` is 1 in its conditions, `heard_at`
is where it was made and `unheard` starts again from 0. A wall between (a collider that is not a
trigger, nor the noise's maker or the listener, across the line from the noise to the listener's
eyes) halves how far the noise carries: through it, only from nearer. `Animator.footstep_noise`
makes one at each footfall of a walking character (`docs/design/animation.md`, Footsteps). A script
makes others as the game has it: footsteps while running, a door slammed, a stone thrown to draw a
guard away.

```ts
if (input.down("sprint")) events.emit("noise", { radius: 10 }, { subject: player });
events.emit("noise", { at: stone, radius: 6 });   // where a thrown stone lands
```

```json
{ "name": "listen", "move": "investigate", "speed": 3, "event": "guard.heard" },
{ "from": "patrol", "to": "listen", "when": "noise" },
{ "from": "listen", "to": "patrol", "when": "(arrived and time > 1) or unheard > 6" }
```

`runtime_tests` (`[behavior][noise]`): ten units off behind a wall, a noise that carries 12 is not
heard and one that carries 25 is, and with the wall gone 12 is; a guard does not hear a noise 12
units away that carries 8, hears one that carries 14 and walks to it, while a deaf one
(`hearing: 0`) stays; a noise with a subject and no `at` is heard where the subject stands.

## The sample

`samples/guards` is a walled courtyard with three guards, each a `Behavior` that patrols a `Path`,
chases the player it sees within 7 units across 150 degrees of where it faces (its `NavAgent` has
`face` on), goes to where it last saw it once it has lost sight of it (`seek`), and after 4 seconds
without seeing it (`unseen > 4`) goes back to its round. Running (Shift) makes a noise every third
of a second that carries 10 units, and a guard that hears one goes to look (`investigate`); a guard
that reaches the player sends it back to the start, and the vault at the north wall ends the level.
The guards and the player are the built-in humanoid (`docs/design/animation.md`, A character without
a file), red shirts and a blue one, each with `Animator.locomotion` on, so they walk on their rounds
and run in the chase with no clip chosen by the script; the player's body is turned toward where it
walks. `pocket scenario guards` checks that the guards keep their rounds while the player hides,
that one chases and catches a player that walks into view, that a guard comes to look when the
player runs near, and that the vault ends the level.

## Checking

`runtime_tests` (`[behavior]`), on the playground's grid: a guard wanders within 2 of home, chases a
mark brought within 5 (with `behavior.changed` and `guard.alerted`), flees 6 away when its health is
set under 30, takes the state written into it with that state's entry, and says a transition to a
state that does not exist; a watching guard sees a mark in the open but not behind a pillar, nor
past its sight, nor behind it with a field of view of 90 degrees; a patrol heads for a path's first
point, then its second once there. `[tilemap][sight]`: a guard in a tile room does not see a mark
behind a wall column and sees it once it is in the open.

## Not yet

No nested machines or behavior trees; the sight is of one target at a time (`targets` picks it), by
distance and a ray (what it remembers is where it last saw the target and when), and a wall muffles
a noise the same whatever it is made of (stone or a curtain). A field of view narrower than 360
degrees reads where the entity faces, so its `NavAgent` should have `face` on (it turns the entity
toward where it walks) or the game should turn it. Facing is about the vertical, so in a 2D game a
field of view does not apply (leave `fov` at 360).
