# Input

Gameplay reads actions, not scancodes. An action map binds names to keys, gamepad buttons and axes;
the runtime keeps each action's state (down, pressed this frame, released this frame, value in
-1..1) and puts a snapshot into every tick, so scripts write `input.axis("move_x")` and
`input.pressed("jump")` and the same code plays with a keyboard or a pad.

## Map

From `project.toml`:

```toml
[input.actions]
move_x = { negative = ["A", "Left", "pad:dpad_left"], positive = ["D", "Right", "pad:dpad_right"], axis = ["pad:leftx"] }
jump   = ["Space", "pad:a"]
```

or from a script (`input.map({...})`) or an agent (`input.map {actions}`); redefining keeps the live
state of actions that already existed. An `input.json` beside `project.toml` (`{"actions": {...}}`
in the same shape) replaces the map from `project.toml` when the project starts: the editor's Input
tab writes it (`docs/editor.md`), so bindings changed there hold without touching the TOML, and
deleting the file returns to the project's own. A `pad:` binding answers any gamepad; `pad0:`,
`pad1:` and so on answer one pad by its index, so local multiplayer is one map with an action per
player (`p1_jump = ["pad0:a"]`, `p2_jump = ["pad1:a"]`, `p2_move = { axis = ["pad1:leftx"] }`).
Bindings are SDL scancode names (`Space`, `Left`, `A`), `pad:<button>` (`a`, `b`, `x`, `y`,
`dpad_up`, `left_shoulder`, ...), `pad:<axis>` (`leftx`, `lefty`, `rightx`, `righty`,
`left_trigger`, `right_trigger`), the mouse's buttons, `mouse:left`, `mouse:right`, `mouse:middle`,
`mouse:x1` and `mouse:x2` (`fire = ["mouse:left", "pad:right_trigger"]`; a press that lands on an
interface element is the interface's and does not move the action), and the mouse as axes: `mouse:x`
and `mouse:y` are its motion over the tick (a hundred pixels is full deflection, +y down the
screen), `wheel:x` and `wheel:y` its wheel (a notch is full), all spent by the tick that read them,
so `look_x = { axis: ["mouse:x", "pad:rightx"], deadzone: 0 }` is mouse look on either device. A
button contributes its sign; an axis contributes its value past the dead zone (0.15 by default,
rescaled; set it to 0 for the mouse), turned over when its name starts with a minus or it is listed
under `negative` (a stick pushed up reads -1, so a game where up is +y binds `"-pad:lefty"`); the
larger magnitude wins and the result is clamped to -1..1. `down` means any button is held or an axis
is past half.

## Every frame

The platform layer turns SDL keyboard and gamepad events into the normalized event stream
(`pad_added`, `pad_removed`, `pad_button {pad, button, pressed}`, `pad_axis {pad, axis, value}`),
which the journal records and replays like everything else. The map consumes those events; the tick
payload carries `actions`; `pressed` and `released` are true for exactly the first tick after the
change, even when the input arrived between frames (an agent pressing while paused).

## The cursor

A first-person or mouse-orbit game wants the pointer captured: hidden, held in the window, and every
motion arriving as `mouse:x` / `mouse:y` however far it goes. `input.cursor {locked: true}` (the
SDK's `input.lockCursor()`, or `[input] cursor = "locked"` in `project.toml` from the start) asks
for that; `input.cursor {visible: false}` only hides it, for a game that draws its own. Escape lets
the pointer go and the next click in the window takes it again, so the player is never trapped:
natively the engine does this, and in a browser the page's pointer lock behaves the same way (the
browser releases it on Escape; SDL asks again on the click, which a browser requires). While the
pointer is held, its presses and motion are the game's and the interface does not see them.
`input.cursor {}` answers `locked` (what the game asked for), `held` (whether the pointer is
captured now, false after Escape until the click, and always false headless and in the editor, which
never captures it) and `visible`. Headless, the request is still remembered, so a scenario can check
that its game asked for it.

## Agents

`input.pad {pad, button, pressed}` and `input.pad {pad, axis, value}` put a gamepad's button or
stick through the same path as a real one (a focused interface takes the d-pad, A and B first:
`docs/design/pocket-ui.md`, Focus) (`input.touch` does the same for a finger, Touch below);
`input.rumble {pad, low, high, ms}` (the SDK's `input.rumble`) shakes a pad's motors (0..1 each) for
a while and answers `rumbled: false` without a pad that can;
`input.rumble {pad, pattern: [{low, high, ms}, ...], repeat}` (the SDK's `input.rumblePattern`)
plays steps one after another on the tick clock (a step with both motors at 0 is a pause: a
heartbeat is a strong, a pause and a weak), replacing whatever the pad was playing, and
`input.rumble {pad, stop: true}` (`input.stopRumble`) silences it. Every step is an `input.rumble`
world event with its strengths, length and index, whether or not a motor answered, so a headless
test or an agent sees the pattern a player would feel; `input.state` lists the patterns still
playing as `rumble`. `input.hold {key | action, ticks, sign}` presses a key (or an action's first
positive key; `sign: -1` for the negative direction) now and releases it after N ticks, through the
same path as real input and into the journal; `input.press` is a one-tick hold. A press of a key
that is still down (pressed a tick ago, or held) lets it up and presses it again, so presses a tick
apart are two presses, each with its edge; a hold of a key already held only holds it longer. A hold
asked for by a script during a tick (a scenario, a bot) presses at the start of the next tick, so
its pressed edge is what that tick's scripts see; a hold from outside (an agent between frames)
presses at once. `input.actions`, `input.describe` and `input.state` show what is bound, what is
down and what is held. So "walk right for a second" is `input.hold {action: "move_x", ticks: 60}`
followed by `step {ticks: 60}` and a transcript.

## Touch

Fingers reach the engine as their own events, `touch_down`, `touch_move` and `touch_up` with
`finger` (an index: the first finger down is 0 until it lifts), `x`, `y` in window points, the
move's `dx`, `dy` and `pressure` (0..1, from the screen where it can tell, 1 while down where it
cannot); scripts see them in `onInput` like any input event, tagged with `ui` when they land on the
interface. The first finger also acts as the mouse: the platform layer turns it into `mouse_down`,
`mouse_move` and `mouse_up` (SDL's own emulation is off, so the stream holds each thing once), so
interface buttons, drags and `mouse:x` / `mouse:y` bindings work under a thumb without a line of
code, and the journal records and replays both.
`input.touch {finger, x, y, phase: down | move | up, pressure}` puts a synthetic finger down for
tests and agents, through the same path, and `input.state` counts the fingers down as `fingers`. On
the web build touch comes through SDL's Emscripten port the same way.

## Gestures

Fingers also make gestures, recognized by the engine from the finger events on the tick clock, so a
replay makes the same ones: a `tap` (down and up within 0.3 s, moved less than 12 points; a second
one within 0.3 s in the same place is `count: 2`, a double tap), a `long_press` (down and still for
half a second, reported while the finger is still down; lifting it afterwards is not a tap), a
`swipe` (40 points or more of travel and lifted within half a second, with `dx`, `dy`, the dominant
`direction` of `left`, `right`, `up` or `down`, and `seconds`; one that starts within 24 points of a
side of the view and moves away from it also carries that side as `edge`, `left`, `right`, `top` or
`bottom`, for a drawer that slides out or a back gesture), and a `pinch` while two fingers are down
(`phase: "begin"` when the second lands, `"move"` as either moves, `"end"` when one lifts, with `x`,
`y` at their centre, `scale` against the spread they began with, `rotation` in degrees and
`distance`; the two fingers make no tap or swipe of their own). Each reaches scripts in `onInput` as
`{type: "gesture", gesture, ...}` after the finger events that made it, with `ui` when the finger
went down on an interface element, and is emitted as an `input.gesture` world event for agents
(`events.recent {type: "input.gesture"}`). The thresholds are a project's:
`[input] gestures = false` turns them off, `gesture_slop` (points), `gesture_tap`, `gesture_hold`,
`gesture_swipe` (points), `gesture_swipe_seconds` and `gesture_edge` (points, 0 for no edge swipes)
move them, and `input.state` reports them as `gestures`. Synthetic fingers (`input.touch`) make
gestures the same way, so "swipe right" in a test is a down, a few moves and an up. The mouse makes
none: a desktop game reads its clicks and drags, a phone game its gestures, and the first finger
still clicks buttons as the mouse. Times are simulation time, so a slowed clock (`time.scale`)
stretches them.

## A value set directly

`input.axis {action, value}` (`input.setAxis(action, value)` in scripts) sets an action's value from
-1 to 1 until it is set again, 0 letting go: a stick tilted part way, without a binding. It goes in
as an input event (a pad axis named `@` and the action), so it is journaled and replays, and from a
script it takes effect at the next tick as a hold does. Past 0.5 either way the action is down, with
its pressed and released edges; the action's dead zone is not applied a second time. An agent
pushing a car's stick halfway is `input.axis {action: "move_x", value: 0.5}`.

## On-screen controls

`touch.stick({x, y, left | right, top | bottom, radius, deadZone, invertY})` and
`touch.button({action, left | right, top | bottom, radius, label})` put a stick and buttons on a
touch screen, placed from a corner in points. A finger that comes down near the stick takes it until
it lifts, and its tilt (its own dead zone applied, clamped to the radius) sets the `x` and `y`
actions through `input.axis`; a finger on a button holds its action at 1. Any number of fingers at
once, each with its own control, from the fingers' own events (a finger also acts as the mouse, so
an action bound to `mouse:left` is pressed by a touch anywhere: bind fire buttons to keys and pads
instead). They draw themselves with the interface, without taking the pointer
(`pointerEvents: "none"`, an element named `touch-controls`), from the first touch, or from the
start with `touch.show()`. A game reads the same actions it reads from a keyboard and a pad;
`samples/crates` has a stick for the car and a button for the ball. `runtime_tests`
(`[touchcontrols]`): a value set part way and past half, a finger on the stick moved 50 points
tilting `move_x` to 0.80, a second finger holding the button meanwhile, both lifted and let go.

A game that makes no controls of its own gets them at its first touch, from its input map, so a game
written for a keyboard plays on a phone as it is: a stick for the actions bound to left and right
(`Left`/`Right`, `A`/`D`, the pad's d-pad or left stick) and to up and down (turned over when the
action's positive side is up), 110 points in from the bottom left corner, and a button for each
other action not bound to the mouse alone, up to six, in the bottom right corner, labelled with the
action's name. `touch.auto(false)` keeps the screen the game's own; a game that calls `touch.stick`
or `touch.button` before the first touch keeps only its own. Scenarios and the editor never get
them. `runtime_tests` (`[autotouch]`): the walker, which has none, walks right and forward from a
finger on the stick and crouches from a finger on the first button.

## The window

`window.info` answers the window's size in points and pixels, its pixel density, whether it is
fullscreen and its title; `window.set {fullscreen?, width?, height?, title?}` changes them
(`display.info()`, `display.fullscreen(on)`, `display.resize(w, h)`, `display.title(t)` in scripts),
and `[window] fullscreen = true` in `project.toml` starts that way. A browser grants fullscreen only
while the page handles a click or a key press, so a game asks for it from an input handler there.
Headless, the requests are remembered and answered.

## Not yet

Motion sensors (a pad's or a phone's gyroscope and accelerometer), pens (tilt, the eraser end), and
a phone's own vibration on the web build (rumble reaches gamepads only).
