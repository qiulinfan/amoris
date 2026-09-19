# Input

Gameplay reads actions, not scancodes. An action map binds names to keys, gamepad buttons and axes; the runtime keeps each action's state (down, pressed this frame, released this frame, value in -1..1) and puts a snapshot into every tick, so scripts write `input.axis("move_x")` and `input.pressed("jump")` and the same code plays with a keyboard or a pad.

## Map

From `project.toml`:

```toml
[input.actions]
move_x = { negative = ["A", "Left", "pad:dpad_left"], positive = ["D", "Right", "pad:dpad_right"], axis = ["pad:leftx"] }
jump   = ["Space", "pad:a"]
```

or from a script (`input.map({...})`) or an agent (`input.map {actions}`); redefining keeps the live state of actions that already existed. Bindings are SDL scancode names (`Space`, `Left`, `A`), `pad:<button>` (`a`, `b`, `x`, `y`, `dpad_up`, `left_shoulder`, ...) and `pad:<axis>` (`leftx`, `lefty`, `rightx`, `righty`, `left_trigger`, `right_trigger`). A button contributes its sign; an axis contributes its value past the dead zone (0.15 by default, rescaled); the larger magnitude wins and the result is clamped to -1..1. `down` means any button is held or an axis is past half.

## Every frame

The platform layer turns SDL keyboard and gamepad events into the normalized event stream (`pad_added`, `pad_removed`, `pad_button {pad, button, pressed}`, `pad_axis {pad, axis, value}`), which the journal records and replays like everything else. The map consumes those events; the tick payload carries `actions`; `pressed` and `released` are true for exactly the first tick after the change, even when the input arrived between frames (an agent pressing while paused).

## Agents

`input.hold {key | action, ticks, sign}` presses a key (or an action's first positive key; `sign: -1` for the negative direction) now and releases it after N ticks, through the same path as real input and into the journal; `input.press` is a one-tick hold. A hold asked for by a script during a tick (a scenario, a bot) presses at the start of the next tick, so its pressed edge is what that tick's scripts see; a hold from outside (an agent between frames) presses at once. `input.actions`, `input.describe` and `input.state` show what is bound, what is down and what is held. So "walk right for a second" is `input.hold {action: "move_x", ticks: 60}` followed by `step {ticks: 60}` and a transcript.

## Not yet

Rebinding UI in the editor, rumble, per-player maps for local multiplayer, mouse-delta actions (mouse look reads `onInput` deltas directly), and touch.
