# Timelines

A cutscene, a door that swings open and shut, the sun sinking over a minute: the fields of a few components changing over time, a few set at moments, and something happening at a time. A timeline is that written down, a JSON file in the project, and the `Timeline` component plays it on the simulation clock. An agent writes one as a file, checks it with a command before it plays, and reads where it is from the component; scripts can do the same with a line each.

## The file

```json
{
  "duration": 6,
  "tracks": [
    {"entity": "Door", "component": "Transform", "field": "position.y", "keys": [[0, 0], [1.5, 2.2, "cubicOut"], [4.5, 2.2], [6, 0, "sineInOut"]]},
    {"entity": "Door", "component": "Transform", "field": "rotation", "keys": [[0, [0, 0, 0]], [1.5, [0, 90, 0]]]},
    {"entity": "Lamp", "component": "Light", "field": "color", "keys": [[0, [1, 1, 1, 1]], [2, [1, 0.5, 0.2, 1]]]},
    {"entity": "Door", "component": "MeshRenderer", "field": "cast_shadows", "keys": [[0, true], [1.5, false]]}
  ],
  "events": [[0.2, "door.creak", {"loud": 2}], [6, "door.shut"]]
}
```

A **track** moves one field of one component of one entity: `entity` is a name or path (empty or `.` for the entity the Timeline is on), `component` a component's name, `field` a path into it (`position`, `position.y`, `color`, `layers.0.weight`, as `world.set` takes them). Its **keys** are `[time, value, ease]` (or `{time, value, ease}`), sorted by time: before the first key the field holds the first value, after the last the last; between two keys the value goes from the first to the second along the second's easing (`linear` by default; `step` holds the first value until the second key; `quadIn`, `quadOut`, `quadInOut`, `cubicIn`, `cubicOut`, `cubicInOut`, `sineIn`, `sineOut`, `sineInOut`, `expoOut`, `backOut`, `elasticOut`, `bounceOut`, the names the SDK's tweens use). Numbers and lists of numbers are interpolated; a list fills the field's parts in order (`x y z`, `r g b a`); three numbers into a rotation are degrees about x, y and z (so `[0, 0, 0]` to `[0, 360, 0]` turns a full circle), four are a quaternion, normalized. A value that is not a number (true, a string) is set at its key's time.

An **event** is `[time, type, data]` (or `{time, type, data}`): when the play passes that time, an event of that type is emitted on the Timeline's entity with the data, the timeline's `path` and the `time` added, for scripts and agents alike (`events.since`). `duration` is optional: by default the last key's or event's time.

## Playing

`Timeline` on an entity plays the file at `path`: its `time` advances by the tick's length times `speed` (negative plays backward) while `playing` is true, and every track is applied at the new time before the physics step, so a platform moved by a timeline is where the bodies meet it. A timeline that does not `loop` stops at its end (`playing` false, `finished` true) and emits `timeline.finished`; one that loops starts again, firing its events again. Events are emitted once as the time passes them (from the time before, included, to the time now, not included; at a stop, the end's included), across a loop's wrap too. Setting `time` seeks. The file is read again when it changes on disk, so an edited timeline plays its new keys at once.

A track that cannot apply (no such entity, the component missing, no such field, a value of the wrong shape) is skipped and the first such problem is the component's `error`; the others still play.

## Commands

| Command | SDK | Purpose |
|---|---|---|
| `timeline.play {entity, path?, time?, speed?, loop?}` | `timeline.play(entity, path, {time, speed, loop})` | The entity's Timeline set to play the file from `time` (0); answers the duration and every track that could not apply in this world. |
| `timeline.stop {entity}` | `timeline.stop(entity)` | Stop where it is. |
| `timeline.seek {entity, time}` | `timeline.seek(entity, time)` | Put it at `time` (within the duration) and apply its tracks there at once, with no tick and no events: a look at a moment of a cutscene, and the editor's playhead (the Timeline tab, `docs/editor.md`). |
| `timeline.info {path, entity?}` | `timeline.info(path)` | The file read and checked against the world: its duration, each track's target, key count and times, and its `problem`, the events, and all the `problems` together. A file that is not JSON, a key that is not a key or an easing that does not exist is refused with the reason. |
| | `timeline.write(path, doc)` | The file written into the project (`project.write`). |

## Samples and tests

`samples/showcase` has a `Dusk` entity playing `timelines/dusk.json` in a loop: over 24 seconds the sun sinks toward the horizon (its rotation keyed in degrees), dims and reddens, the sky darkens and the pavilion's lamp brightens, then all of it comes back, with a `dusk.deepest` event at the turn. `runtime_tests` (`[timeline]`) check a file against the world before it plays, then play it: a door halfway up at half a second (linear), a lamp at a sixteenth of its end (quadIn at a quarter), the door turned 45 degrees at one second from degrees in the keys, a colour halfway, shadows stepped off at 1.5 seconds, the door back down at the end with the timeline stopped and finished and its events each fired once; then looped from 2.5 seconds across the wrap (the end's event and the creak again), played backward, stopped, an easing that does not exist refused, and the three tracks of a stray file (no such entity, no such field, a value of the wrong shape) named before it plays and as its error after.

## Not yet

Curves beyond the easings (a key has no tangents of its own), tracks on scripts' state or on UI, a timeline that blends into another, and dragging keys along the editor's ruler or editing a key's value and easing there (the editor keys values from the scene; the file is also text an agent writes).
