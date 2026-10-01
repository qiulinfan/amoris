# A project's own components

`samples/playground/components.toml` declares `Enemy {damage, kind (grunt, brute), spawn_seq, spawn_tick}`; the playground's script keeps its chasers on that component instead of a script Map (ADR 0007).

`transcript.py` drives a debug runtime serving the playground with `--history 600` and writes `transcript.json`, the calls an agent would make and the answers:

1. `world.schema {component: "Enemy"}`: the fields, the value names of `kind`, `project: true`.
2. `step {ticks: 240}`, then `world.query {with: ["Enemy"], fields: ["Enemy"]}`: three enemies with their spawn events and ticks.
3. `world.set {kind: "brute", damage: 25}` answers the component as it now is (`kind: 1`); `world.set {damge: 5}` is refused with `Enemy has no field 'damge'; did you mean 'damage'?` and changes nothing.
4. `world.instantiate` of a scene fragment with `Enemy {kind: "brute", damage: 30}` (as a scene file or a Blender property would give it), read back with `world.get`.
5. `recorder.track` of the first enemy's `damage` over ticks 230 to 299: 10 until the set at tick 240, 25 from then, and nothing from tick 267, when it reached the player and was destroyed.
6. `world.save`, `world.load` of what was saved, and the query again: the enemies with their fields.
7. `world.tree {components: ["Enemy"], values: true}`.

Run (debug build, playground bundled, outside the sandbox):

```bash
./.pocket/pocket ts samples/playground
python3 tests/evidence/components/transcript.py
```
