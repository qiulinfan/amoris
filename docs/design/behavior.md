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
heads for; `seek` walks to `seen_at`, where it last saw the target. `speed`, when not 0, is the
agent's speed in the state. A state that moves an entity without a `NavAgent` says so in `error` and
moves nothing.

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
  origins (the two themselves, their children and triggers aside);
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
  transition with `on` needs one, whatever its condition.

A condition that does not read, or a transition to a state that is not there, is said in `error` and
never taken. `home` is where the entity stood at its first tick unless it was given;
`enabled: false` holds the state and leaves the agent alone.

## The sample

`samples/guards` is a walled courtyard with three guards, each a `Behavior` that patrols a `Path`,
chases the player it sees within 7 units across 150 degrees of where it faces (its `NavAgent` has
`face` on), goes to where it last saw it once it has lost sight of it (`seek`), and after 4 seconds
without seeing it (`unseen > 4`) goes back to its round; a guard that reaches the player sends it
back to the start, and the vault at the north wall ends the level. `pocket scenario guards` checks
that the guards keep their rounds while the player hides, that one chases and catches a player that
walks into view, and that the vault ends the level.

## Checking

`runtime_tests` (`[behavior]`), on the playground's grid: a guard wanders within 2 of home, chases a
mark brought within 5 (with `behavior.changed` and `guard.alerted`), flees 6 away when its health is
set under 30, takes the state written into it with that state's entry, and says a transition to a
state that does not exist; a watching guard sees a mark in the open but not behind a pillar, nor
past its sight, nor behind it with a field of view of 90 degrees; a patrol heads for a path's first
point, then its second once there.

## Not yet

No nested machines or behavior trees; the perception is of one target, by distance and a ray (no
hearing radius; what it remembers is where it last saw the target and when). A field of view
narrower than 360 degrees reads where the entity faces, so its `NavAgent` should have `face` on (it
turns the entity toward where it walks) or the game should turn it.
