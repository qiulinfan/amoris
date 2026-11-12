# A person's input: devices, the input map and on-screen controls

- Status: built 2026-11-12, track `input`. Charter 4.1 (winit; gilrs for gamepads), 2 (proof target
  2: agents act through the action layer; a person reaches the same controls through devices).
- Covers: the seat input of voxels.md 8 and web-export.md 3, the third-person seat of player.md 5.1,
  the game's buttons of animation.md 6, and the human side of player.md 5.

## 1. What a device becomes

A person plays a first-person seat (`[player] actions = "first_person"`) or a third-person one
(`"third_person"`, player.md 5.1) in the native window (`amoris play <project> --walk`) or in the
browser player (web-export.md). Devices never reach the world. Each drawn frame the presenter
gathers its devices into a device frame (2), the project's input map (4) turns the frame into the
seat's logical inputs, and the seat sends what changed as `player.act` `set` and `pulse` actions, as
the local human player (`Source::Player(0)`). An accepted act is recorded as the Write `player.act`
in its canonical form (player.md 5), so a run played with a gamepad or on a touch screen replays
exactly as one played with the keyboard: the recording names controls, never a device or a
binding.

Scripts read logical inputs only: the `Walker`'s controls (`forward`, `strafe`, `yaw`, `pitch`,
`run`, `rise`, `slot` and the one-tick `jump`, `primary`, `secondary`), a third-person seat's
`ThirdPerson` (`zoom`, `guard` and the one-tick `dodge`, `interact`, `lock_toggle`) and the game's
`Buttons` (`held`, `pressed`; animation.md 6). Agents act through the same controls with
`player.act` and never see devices or bindings. The `sailing` catalog has no device bindings: agents
and policies play it.

One implementation serves both presenters: `amoris_assets::input` (pure Rust, built for `wasm32`)
holds the vocabulary, the gamepad's dead zones, the input map and the seat (`Seat`: device frame in,
`player.act` actions out, of the seat the map is for: `SeatKind`, from the catalog's name). The
native window calls it from `amoris-app`; the player page calls it through the viewport
(`Viewport::seat_frame`), so the page's JavaScript gathers devices and draws the touch controls but
maps nothing.

## 2. Device inputs

Every input has a name. A device frame holds the inputs held now with their strength in (0, 1], the
inputs pressed since the last frame (press edges, in order), the pointer's look in mouse counts, the
active gamepad's state and the frame's seconds.

| Device | Names | Strength |
|---|---|---|
| Keyboard | The page's `KeyboardEvent.code` (`KeyW`, `Space`, `ShiftLeft`, `Digit1`), by key position; winit's `KeyCode` names are the same | 1 |
| Mouse buttons | `MouseLeft`, `MouseRight`, `MouseMiddle` | 1 |
| Wheel | `WheelUp`, `WheelDown`: a press each notch (a line, or 100 pixels in the page, 40 natively) | press only |
| Gamepad buttons | `PadSouth` … `PadHome` (the table below) | the button's value through the trigger dead zone |
| Gamepad sticks | `PadLStickLeft`, `PadLStickRight`, `PadLStickUp`, `PadLStickDown`, and `PadRStick…` | the half-axis through the stick's dead zone |
| Touch stick | `TouchStickLeft`, `TouchStickRight`, `TouchStickUp`, `TouchStickDown` | the knob's offset over the stick's radius |
| Touch buttons | `Touch:<input>`: an on-screen button bound to the logical input or game button it names (5) | 1 |

The pointer's look is not a named input: the mouse's movement while captured (raw device counts
natively, `movementX`/`movementY` in the page) and a touch drag (two counts per CSS pixel) turn the
view at 0.003 rad a count.

### 2.1 The standard gamepad

Gamepads are read in the W3C Gamepad API's standard layout, by position: `PadSouth` is Xbox's A and
PlayStation's cross wherever the pad prints them. The browser reads only pads the browser maps to
it (`Gamepad.mapping == "standard"`; Chrome, Edge, Firefox and Safari map common pads). Natively
gilrs maps a pad with its SDL mapping database and the table's gilrs column gives the layout.

| Index | Name | Xbox | PlayStation | gilrs |
|---|---|---|---|---|
| 0 | `PadSouth` | A | Cross | `South` |
| 1 | `PadEast` | B | Circle | `East` |
| 2 | `PadWest` | X | Square | `West` |
| 3 | `PadNorth` | Y | Triangle | `North` |
| 4 | `PadL1` | LB | L1 | `LeftTrigger` |
| 5 | `PadR1` | RB | R1 | `RightTrigger` |
| 6 | `PadL2` | LT | L2 | `LeftTrigger2` (its value) |
| 7 | `PadR2` | RT | R2 | `RightTrigger2` (its value) |
| 8 | `PadSelect` | View | Share / Create | `Select` |
| 9 | `PadStart` | Menu | Options | `Start` |
| 10 | `PadL3` | Left stick press | L3 | `LeftThumb` |
| 11 | `PadR3` | Right stick press | R3 | `RightThumb` |
| 12 | `PadUp` | D-pad up | Up | `DPadUp` |
| 13 | `PadDown` | D-pad down | Down | `DPadDown` |
| 14 | `PadLeft` | D-pad left | Left | `DPadLeft` |
| 15 | `PadRight` | D-pad right | Right | `DPadRight` |
| 16 | `PadHome` | Guide | PS | `Mode` |

Axes 0 to 3 are the left stick's x and y and the right stick's x and y, positive right and down
(gilrs's y, positive up, is negated).

### 2.2 Dead zones

- A stick has a radial dead zone: a deflection `r` below 0.15 is nothing, and above it the stick
  reads `(r − 0.15) / (0.95 − 0.15)` along its direction, 1 from 0.95 out. A radial zone keeps
  diagonals (a square one snaps a slightly-off diagonal to an axis), and rescaling keeps the motion
  continuous at its edge. Its half-axes are the result's four signed parts.
- A button's value (the analog triggers' travel; digital buttons read 0 or 1) below 0.05 is
  nothing and is rescaled the same way above it.
- An input counts as down, for a held toggle or a press edge, at strength 0.5 or more. A gamepad
  input's press is its crossing of 0.5 between frames, so a stick pushed past halfway presses its
  direction once.

## 3. The seat

The first-person seat's logical inputs, and how the seat turns them into controls each frame:

| Logical input | Control | Default inputs |
|---|---|---|
| `move_forward`, `move_back`, `move_left`, `move_right` | `forward` and `strafe`: the sums of the opposed strengths, scaled to length 1 when longer | `KeyW` `KeyS` `KeyA` `KeyD`; the left stick; the touch stick |
| `look_left`, `look_right`, `look_up`, `look_down` | `yaw` and `pitch`, with the pointer's look: the axis `v` turns at `v·|v|` times 3 rad/s (yaw) and 2 rad/s (pitch) | the right stick |
| `run` | `run` while down | `ShiftLeft`, `ShiftRight`, `PadL3` |
| `rise` | `rise` while down | `Space`, `PadSouth` |
| `jump` | pulse `jump` on a press | `Space`, `PadSouth` |
| `primary` | pulse `primary` on a press | `MouseLeft`, `PadR2` |
| `secondary` | pulse `secondary` on a press | `MouseRight`, `PadL2` |
| `slot_next`, `slot_prev` | `slot` one right or left a press, wrapping at nine | `WheelDown`, `PadR1`; `WheelUp`, `PadL1` |
| `slot_1` … `slot_9` | `slot` `"0"` … `"8"` on a press | `Digit1` … `Digit9` |

A logical input's strength is the largest of its inputs'. The seat sends one `set` of `forward`,
`strafe`, `run`, `yaw`, `pitch` and `rise` when any of them changed, a `set` of `slot` when it
changed, a `pulse` for each press, and the game's buttons as before (animation.md 6): `held`, the
mask of buttons any of whose inputs is down, when it changed, and `pressed`, the mask pressed this
frame, when not zero. While the seat is not playing (the mouse released, the page's veil up) it
holds every control at rest and acts on nothing, but still takes the gamepad's presses, so the press
that starts play does not also jump.

### 3.1 The third-person seat

The `third_person` catalog's seat (player.md 5.1) has its own logical inputs. The move, the look
and `run` are the first-person seat's, sent as one `set` of `forward`, `strafe`, `run`, `yaw` and
`pitch` when any of them changed (`frame` is left to the game and to agents); the look turns the
orbit, and a game camera's look is taken back as the window and the page take it (player.md 5.1,
Presentation).

| Logical input | Control | Default inputs |
|---|---|---|
| `move_…`, `look_…`, `run`, `jump`, `primary` | as in the first-person seat | as in the first-person seat |
| `guard` | `guard` while down, sent when it changed | `MouseRight`, `PadL2` |
| `secondary` | pulse `secondary` on a press | `KeyE`, `PadNorth` |
| `dodge` | pulse `dodge` on a press | `KeyC`, `PadEast` |
| `interact` | pulse `interact` on a press | `KeyF`, `PadWest` |
| `lock` | pulse `lock` on a press | `KeyQ`, `MouseMiddle`, `PadR3` |
| `zoom_out`, `zoom_in` | `zoom` times or over 1.1 a press, within 0.5 to 2, sent when it changed | `WheelDown`, `PadDown`; `WheelUp`, `PadUp` |

It has no `rise` and no slots: `Space` and `PadSouth` are `jump` alone, and the wheel zooms. Its
default touch layout is `Attack` (`primary`), `Dodge`, `Guard`, `Jump` and `Lock`.
`samples/thornkeep` keeps the defaults but for `interact` (`KeyF` alone: its `PadWest` drinks a
flask) and lays out its own touch buttons.

Escape and `PadStart` are the presenters' own: they release the mouse (natively) or pause and show
the veil (the page); while released, `PadStart` or any gamepad button starts play again. They are
not bindable, and the game never sees them.

## 4. The input map

The project's `[player]` declares the map; the defaults above apply where it says nothing.

```toml
[player]
actions = "first_person"
# The game's own buttons (animation.md 6): any input names of section 2.
buttons = [
  { name = "dodge", keys = ["Space", "PadEast"] },
  { name = "lock", keys = ["KeyQ", "MouseMiddle", "PadR3"] },
]

[player.bindings]            # replaces a logical input's default inputs
jump = ["Space", "PadSouth"]
primary = ["MouseLeft", "PadR2", "PadWest"]

[player.touch]               # the browser's on-screen controls (5)
stick = true                 # the move stick at the bottom left (default true)
look = true                  # a drag on the rest of the screen looks (default true)
buttons = [{ label = "Roll", input = "dodge" }, { label = "Swing", input = "primary" }]
```

The project's load checks the map: a binding names a logical input of its seat (3, or 3.1 for a
`third_person` catalog; any other catalog has the first-person seat's map), every input name
is one of section 2 (a keyboard name is any name outside the other devices' prefixes that starts
with an ASCII capital and is alphanumeric, as every `KeyboardEvent.code` is), a touch button's
`input` names a logical input or one of the game's buttons, and at most 16 buttons and 8 touch
buttons; a fault is `request.invalid_value` at its JSON pointer (`/player/bindings/jump/1`) with
the candidates, and too many buttons is `project.invalid`.

The map is presentation: it is not in the persisted `PlayerSpec`, so changing it changes no world
hash and no recording. The web package carries the resolved map as `player_input` (defaults with the
project's bindings, the buttons, the touch layout); the native window builds it from the manifest.

## 5. Touch controls

The player page shows its touch controls once a touch reaches it (the first touch `pointerdown`),
or at load with `?touch=1`. A tap on the veil then starts play without asking for pointer lock
(phones have none), as the page's keyboard mode does (web-export.md 3).

- The stick: a touch that lands in the bottom-left zone grabs the stick there; its knob follows the
  finger up to 60 CSS pixels, and the offset over 60 is the `TouchStick…` strengths (up is forward).
  It springs back when the finger lifts.
- The buttons, bottom right, one per touch button of the layout (default: `Jump` for `jump`,
  `Use` for `primary`, `Alt` for `secondary`; a third-person seat's, 3.1), each holding and pressing
  `Touch:<input>` while touched. A `[player.touch]` that names no buttons has the first-person
  seat's three. A pause button at the top right shows the veil.
- Touch as the pointer: a touch anywhere else on the game is a pointer, as an unlocked mouse is. Its
  drag looks (when `look`), and a tap that did not move (8 CSS pixels) presses `MouseLeft`, so it
  acts as a stationary click does. Touches are handled as Pointer Events (`pointerType == "touch"`)
  on the canvas with `touch-action: none`; an overlay above the canvas that takes a pointer event
  takes it from the look. The game's own UI (ui.md 6) takes a touch first: one that lands on an
  enabled button, on the game or the veil, neither looks nor clicks, and lifted over the same
  button it sends `ui.press`, as a click does; the touch controls keep their touches.

## 6. The native window

`amoris_render::app` reads gamepads with gilrs (feature `window`) while the window plays a seat
(`RunOptions::walk`). Each frame it drains gilrs's events: a pad that connects or disconnects is
logged and named in the window's status; the pad that last sent an event is the active one, and its
state is read in the standard layout (2.1) into the device frame. A disconnect of the active pad
lifts all of its inputs at once, so nothing stays held. While the window is focused and the mouse
released, `PadStart` or any other button captures the mouse as a click does; `PadStart` while
captured releases it as Escape does. Without gilrs (no backend on the platform) the window plays
without gamepads and says so once.

## 7. The browser

The page polls `navigator.getGamepads()` each frame, takes the standard-mapped pad with the latest
`timestamp` and names it in `window.amorisPlayer.pad`; a pad that is gone lifts its inputs. While
the page is ready and not playing, `PadStart` or any button starts play in the page's keyboard mode
(no pointer lock: a gamepad button is no user gesture for the lock); `PadStart` while playing
pauses. The mouse still looks while dragged. The page's sound starts with the first click or tap; a
session started from the gamepad alone stays silent until one, since browsers start audio only on a
user gesture.

## 8. Tests and checks

| What | Where |
|---|---|
| Dead zones (radial, rescaled, continuous at the edge, clamped), the standard layout's names, the default map, a project's overrides and its refusals, held and pressed resolution, stick look rates, touch buttons, and the seat's actions (only what changed, pulses on presses, nothing while released, the starting press swallowed) | `amoris-assets` `input::tests` |
| gilrs's buttons cover the standard layout once each | `amoris-render` `pad::tests` |
| A gamepad session (left stick, right stick, `PadSouth`, `PadR1`) drives the walker through the seat, records and replays identically, and the same session twice gives the same world hash | `amoris-runtime/tests/voxels.rs` `a_gamepad_session_moves_the_walker_and_replays` |
| A project's `[player.bindings]` and `[player.touch]` load, and a bad one is refused at its pointer | `amoris-runtime/tests/voxels.rs` `a_projects_input_map_loads_and_is_checked` |
| The third-person seat (3.1): its defaults, zoom by presses within its range, the guard held, its pulses, a first-person input refused for it and the other way round; Thornkeep's map loads as the seat's | `amoris-assets` `input::tests` `a_third_person_seat_sends_its_controls`; `amoris-runtime --features transpile --test third_person` `a_third_person_projects_input_map_is_its_seats` |
| A touch on a game UI button presses it and does not look (5) | `node tools/host/web_play_check.mjs <ui-hud folder> --ui` |
| In headless Chrome, a synthetic standard gamepad starts play without the lock, its stick moves the walker, its right stick turns the look and `PadSouth` jumps; touch events start play, show the controls, the touch stick moves the walker, a drag turns the look and the jump button jumps | `node tools/host/web_play_check.mjs <folder> --gamepad`, `--touch` (web-export.md 5) |

## 9. Not verified

A real gamepad on the native window (gilrs's backends: IOKit on macOS, Windows.Gaming.Input or
XInput, evdev on Linux), a real gamepad in a browser, and touch on a real phone or tablet (iOS
Safari, Android Chrome) have not been played here; the checks above drive the same code paths with
synthetic state. Gamepad vibration, gyro, more than one pad at once (local multiplayer) and
rebinding at run time are not built.
