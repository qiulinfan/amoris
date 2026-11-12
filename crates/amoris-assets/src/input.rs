//! A person's input (docs/spec/input.md): the names of the devices' inputs, the standard gamepad's
//! layout and dead zones, the project's input map, and the seat that turns a frame of devices into
//! `player.act` actions of a first- or third-person seat. Pure and built for `wasm32`: the native window
//! (amoris-app) and the player page (through amoris-viewport's `seat_frame`) run this one copy, so
//! a binding means the same thing in both. Devices never reach the world; the actions do, and a
//! replay records only them.

use std::collections::{BTreeMap, BTreeSet};

use amoris_contract::{Candidate, Pointer, Problem, codes, detail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The W3C Gamepad API's standard buttons, in its order (input.md 2.1).
pub const PAD_BUTTONS: [&str; 17] = [
    "PadSouth",
    "PadEast",
    "PadWest",
    "PadNorth",
    "PadL1",
    "PadR1",
    "PadL2",
    "PadR2",
    "PadSelect",
    "PadStart",
    "PadL3",
    "PadR3",
    "PadUp",
    "PadDown",
    "PadLeft",
    "PadRight",
    "PadHome",
];

/// The sticks' half-axes: the left stick's, then the right's.
pub const PAD_STICKS: [&str; 8] = [
    "PadLStickLeft",
    "PadLStickRight",
    "PadLStickUp",
    "PadLStickDown",
    "PadRStickLeft",
    "PadRStickRight",
    "PadRStickUp",
    "PadRStickDown",
];

/// The touch stick's directions (input.md 5).
pub const TOUCH_STICK: [&str; 4] = [
    "TouchStickLeft",
    "TouchStickRight",
    "TouchStickUp",
    "TouchStickDown",
];

/// The mouse's buttons.
pub const MOUSE: [&str; 3] = ["MouseLeft", "MouseRight", "MouseMiddle"];

/// The wheel: a press each notch.
pub const WHEEL: [&str; 2] = ["WheelUp", "WheelDown"];

/// An on-screen touch button's input: `Touch:<logical input or game button>`.
pub const TOUCH_PREFIX: &str = "Touch:";

/// The first-person seat's logical inputs (input.md 3).
pub const FIRST_PERSON: [&str; 24] = [
    "move_forward",
    "move_back",
    "move_left",
    "move_right",
    "look_left",
    "look_right",
    "look_up",
    "look_down",
    "run",
    "rise",
    "jump",
    "primary",
    "secondary",
    "slot_next",
    "slot_prev",
    "slot_1",
    "slot_2",
    "slot_3",
    "slot_4",
    "slot_5",
    "slot_6",
    "slot_7",
    "slot_8",
    "slot_9",
];

/// The third-person seat's logical inputs (input.md 3.1).
pub const THIRD_PERSON: [&str; 18] = [
    "move_forward",
    "move_back",
    "move_left",
    "move_right",
    "look_left",
    "look_right",
    "look_up",
    "look_down",
    "run",
    "guard",
    "jump",
    "primary",
    "secondary",
    "dodge",
    "interact",
    "lock",
    "zoom_in",
    "zoom_out",
];

/// Which seat a map drives (input.md 3, 3.1): the project's `[player] actions`, `third_person` or
/// any other catalog, whose map is the first-person seat's (the `sailing` catalog has no device
/// bindings, and a game without a walker seat never uses its map).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    #[default]
    FirstPerson,
    ThirdPerson,
}

impl SeatKind {
    /// The seat of an action catalog's name.
    pub fn of_catalog(actions: &str) -> SeatKind {
        if actions == "third_person" {
            SeatKind::ThirdPerson
        } else {
            SeatKind::FirstPerson
        }
    }

    /// The seat's logical inputs.
    pub fn logical(self) -> &'static [&'static str] {
        match self {
            SeatKind::FirstPerson => &FIRST_PERSON,
            SeatKind::ThirdPerson => &THIRD_PERSON,
        }
    }
}

/// A third-person seat's zoom changes this much a press of `zoom_out` (`zoom_in` divides by it),
/// within [`ZOOM_RANGE`].
pub const ZOOM_STEP: f32 = 1.1;
/// The `third_person` catalog's `zoom` range.
pub const ZOOM_RANGE: [f32; 2] = [0.5, 2.0];
/// A stick's deflection below this is nothing.
pub const STICK_DEAD_ZONE: f32 = 0.15;
/// A stick's deflection from this out reads 1.
pub const STICK_FULL: f32 = 0.95;
/// A button's value (an analog trigger's travel) below this is nothing.
pub const BUTTON_DEAD_ZONE: f32 = 0.05;
/// An input is down, for a held toggle and a press edge, from this strength.
pub const DOWN: f32 = 0.5;
/// The view turns this far a count of the pointer's look.
pub const LOOK_RAD_PER_COUNT: f32 = 0.003;
/// The look stick's full turn rate, yaw and pitch, radians a second.
pub const STICK_LOOK_RAD_S: [f32; 2] = [3.0, 2.0];
/// The most buttons a game declares (the masks' 16 bits).
pub const MAX_BUTTONS: usize = 16;
/// The most on-screen touch buttons.
pub const MAX_TOUCH_BUTTONS: usize = 8;

/// The larger of two strengths. Presentation arithmetic, outside every tick, so the platform's
/// `f32::max` would do; the game crates' lint asks for comparisons spelled out (numeric.md 5).
fn larger(a: f32, b: f32) -> f32 {
    if b > a { b } else { a }
}

fn at_most_one(v: f32) -> f32 {
    if v > 1.0 { 1.0 } else { v }
}

/// A stick through its radial dead zone, rescaled so it reads 0 at the zone's edge and 1 from
/// [`STICK_FULL`] out, along its own direction.
pub fn stick(x: f32, y: f32) -> [f32; 2] {
    let r = (x * x + y * y).sqrt();
    // A NaN from a broken driver is nothing too.
    if r.is_nan() || r <= STICK_DEAD_ZONE {
        return [0.0; 2];
    }
    let s = at_most_one((r - STICK_DEAD_ZONE) / (STICK_FULL - STICK_DEAD_ZONE));
    [x / r * s, y / r * s]
}

/// A button's value through its dead zone, rescaled to reach 1 at full travel.
pub fn button(v: f32) -> f32 {
    if v.is_nan() || v <= BUTTON_DEAD_ZONE {
        return 0.0;
    }
    at_most_one((v - BUTTON_DEAD_ZONE) / (1.0 - BUTTON_DEAD_ZONE))
}

/// A gamepad's state in the standard layout: [`PAD_BUTTONS`]' values (0 to 1) and the axes left x,
/// left y, right x, right y (-1 to 1, positive right and down). Missing entries are at rest.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    #[serde(default)]
    pub buttons: Vec<f32>,
    #[serde(default)]
    pub axes: Vec<f32>,
}

impl Pad {
    /// The pad's inputs with a strength above zero, through the dead zones.
    pub fn inputs(&self) -> Vec<(&'static str, f32)> {
        let mut out = Vec::new();
        for (name, v) in PAD_BUTTONS.iter().zip(&self.buttons) {
            let s = button(*v);
            if s > 0.0 {
                out.push((*name, s));
            }
        }
        let axis = |i: usize| self.axes.get(i).copied().unwrap_or(0.0);
        for (side, (x, y)) in [(0, (axis(0), axis(1))), (4, (axis(2), axis(3)))] {
            let [x, y] = stick(x, y);
            for (j, s) in [-x, x, -y, y].into_iter().enumerate() {
                if s > 0.0 {
                    out.push((PAD_STICKS[side + j], s));
                }
            }
        }
        out
    }
}

/// One frame of a seat's devices, as a presenter gathers it (input.md 2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceFrame {
    /// Inputs held now with their strength: keys and mouse buttons at 1, the touch stick's
    /// directions and touch buttons. The gamepad's come from [`DeviceFrame::pad`].
    #[serde(default)]
    pub held: BTreeMap<String, f32>,
    /// Inputs pressed since the last frame, in order (the wheel's notches as `WheelUp` and
    /// `WheelDown`). The gamepad's presses are found by the seat.
    #[serde(default)]
    pub pressed: Vec<String>,
    /// The pointer's look since the last frame, in counts: right and down positive.
    #[serde(default)]
    pub look: [f32; 2],
    /// The active gamepad now, if any.
    #[serde(default)]
    pub pad: Option<Pad>,
    /// Seconds since the last frame.
    #[serde(default)]
    pub dt_s: f32,
}

impl DeviceFrame {
    /// Adds a later frame to this one: its held inputs and pad replace these, its presses, look
    /// and time add to them.
    pub fn merge(&mut self, later: DeviceFrame) {
        self.held = later.held;
        self.pad = later.pad;
        self.pressed.extend(later.pressed);
        self.look[0] += later.look[0];
        self.look[1] += later.look[1];
        self.dt_s += later.dt_s;
    }
}

/// One of the game's own buttons (animation.md 6) and the inputs that hold it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ButtonDef {
    pub name: String,
    /// Input names (input.md 2): keys, mouse buttons, the wheel, the gamepad's buttons and sticks.
    pub keys: Vec<String>,
}

/// The browser player's on-screen controls (input.md 5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TouchLayout {
    /// The move stick at the bottom left.
    #[serde(default = "yes")]
    pub stick: bool,
    /// A drag on the rest of the screen looks.
    #[serde(default = "yes")]
    pub look: bool,
    /// The buttons at the bottom right, at most 8.
    #[serde(default = "default_touch_buttons")]
    pub buttons: Vec<TouchButton>,
}

/// An on-screen button: its label and the logical input or game button it presses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TouchButton {
    pub label: String,
    pub input: String,
}

fn yes() -> bool {
    true
}

fn touch_buttons(buttons: &[(&str, &str)]) -> Vec<TouchButton> {
    buttons
        .iter()
        .map(|(label, input)| TouchButton {
            label: (*label).into(),
            input: (*input).into(),
        })
        .collect()
}

impl TouchLayout {
    /// The default layout of a seat (input.md 5).
    pub fn default_for(seat: SeatKind) -> TouchLayout {
        let buttons = match seat {
            SeatKind::FirstPerson => {
                touch_buttons(&[("Jump", "jump"), ("Use", "primary"), ("Alt", "secondary")])
            }
            SeatKind::ThirdPerson => touch_buttons(&[
                ("Attack", "primary"),
                ("Dodge", "dodge"),
                ("Guard", "guard"),
                ("Jump", "jump"),
                ("Lock", "lock"),
            ]),
        };
        TouchLayout {
            stick: true,
            look: true,
            buttons,
        }
    }
}

fn default_touch_buttons() -> Vec<TouchButton> {
    TouchLayout::default_for(SeatKind::FirstPerson).buttons
}

impl Default for TouchLayout {
    fn default() -> TouchLayout {
        TouchLayout::default_for(SeatKind::FirstPerson)
    }
}

/// The default inputs of each of a seat's logical inputs (input.md 3, 3.1).
pub fn default_bindings(seat: SeatKind) -> BTreeMap<String, Vec<String>> {
    let shared: [(&str, &[&str]); 10] = [
        ("move_forward", &["KeyW", "PadLStickUp", "TouchStickUp"]),
        ("move_back", &["KeyS", "PadLStickDown", "TouchStickDown"]),
        ("move_left", &["KeyA", "PadLStickLeft", "TouchStickLeft"]),
        ("move_right", &["KeyD", "PadLStickRight", "TouchStickRight"]),
        ("look_left", &["PadRStickLeft"]),
        ("look_right", &["PadRStickRight"]),
        ("look_up", &["PadRStickUp"]),
        ("look_down", &["PadRStickDown"]),
        ("run", &["ShiftLeft", "ShiftRight", "PadL3"]),
        ("jump", &["Space", "PadSouth"]),
    ];
    let own: &[(&str, &[&str])] = match seat {
        SeatKind::FirstPerson => &[
            ("rise", &["Space", "PadSouth"]),
            ("primary", &["MouseLeft", "PadR2"]),
            ("secondary", &["MouseRight", "PadL2"]),
            ("slot_next", &["WheelDown", "PadR1"]),
            ("slot_prev", &["WheelUp", "PadL1"]),
        ],
        SeatKind::ThirdPerson => &[
            ("guard", &["MouseRight", "PadL2"]),
            ("primary", &["MouseLeft", "PadR2"]),
            ("secondary", &["KeyE", "PadNorth"]),
            ("dodge", &["KeyC", "PadEast"]),
            ("interact", &["KeyF", "PadWest"]),
            ("lock", &["KeyQ", "MouseMiddle", "PadR3"]),
            ("zoom_in", &["WheelUp", "PadUp"]),
            ("zoom_out", &["WheelDown", "PadDown"]),
        ],
    };
    let mut m: BTreeMap<String, Vec<String>> = shared
        .iter()
        .chain(own)
        .map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect()))
        .collect();
    if seat == SeatKind::FirstPerson {
        for i in 1..=9 {
            m.insert(format!("slot_{i}"), vec![format!("Digit{i}")]);
        }
    }
    m
}

/// A project's input map: every logical input's inputs, the game's buttons and the touch layout.
/// Presentation only: never world state (input.md 4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputMap {
    /// The seat the map drives.
    pub seat: SeatKind,
    pub bindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub buttons: Vec<ButtonDef>,
    #[serde(default)]
    pub touch: TouchLayout,
}

impl Default for InputMap {
    fn default() -> InputMap {
        InputMap {
            seat: SeatKind::FirstPerson,
            bindings: default_bindings(SeatKind::FirstPerson),
            buttons: Vec::new(),
            touch: TouchLayout::default(),
        }
    }
}

fn invalid(path: &Pointer, message: String) -> Problem {
    Problem::new(
        "project.invalid",
        message.clone(),
        detail([
            ("path", json!(path.to_string())),
            ("message", json!(message)),
        ]),
    )
}

fn candidates<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<Candidate<'a>> {
    names.into_iter().map(Candidate::new).collect()
}

/// Checks one input name (input.md 2) at `path`; `targets` are what a `Touch:` input may name.
fn check_input(path: &Pointer, name: &str, targets: &[&str]) -> Result<(), Problem> {
    let known = |list: &[&'static str]| {
        if list.contains(&name) {
            Ok(())
        } else {
            Err(codes::invalid_value(
                path,
                name,
                &candidates(list.iter().copied()),
            ))
        }
    };
    if let Some(target) = name.strip_prefix(TOUCH_PREFIX) {
        return if targets.contains(&target) {
            Ok(())
        } else {
            let valid: Vec<String> = targets
                .iter()
                .map(|t| format!("{TOUCH_PREFIX}{t}"))
                .collect();
            Err(codes::invalid_value(
                path,
                name,
                &candidates(valid.iter().map(String::as_str)),
            ))
        };
    }
    if name.starts_with("Pad") {
        let all: Vec<&'static str> = PAD_BUTTONS.iter().chain(&PAD_STICKS).copied().collect();
        return known(&all);
    }
    if name.starts_with("Touch") {
        return known(&TOUCH_STICK);
    }
    if name.starts_with("Mouse") {
        return known(&MOUSE);
    }
    if name.starts_with("Wheel") {
        return known(&WHEEL);
    }
    // A key: every `KeyboardEvent.code` is an ASCII capital followed by letters and digits.
    let mut chars = name.chars();
    if chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_alphanumeric())
    {
        return Ok(());
    }
    let mut valid: Vec<&str> = vec!["KeyW", "Space", "ShiftLeft", "Digit1", "ArrowUp"];
    valid.extend(MOUSE);
    valid.extend(WHEEL);
    valid.extend(PAD_BUTTONS);
    Err(codes::invalid_value(path, name, &candidates(valid)))
}

impl InputMap {
    /// The map of a project's `[player]` for `seat`: the seat's defaults with its `bindings` in
    /// their place, its buttons and its touch layout (the seat's default when none), each name
    /// checked (input.md 4).
    pub fn new(
        seat: SeatKind,
        bindings: &BTreeMap<String, Vec<String>>,
        buttons: &[ButtonDef],
        touch: Option<&TouchLayout>,
    ) -> Result<InputMap, Problem> {
        let root = Pointer::root().key("player");
        if buttons.len() > MAX_BUTTONS {
            return Err(invalid(
                &root.key("buttons"),
                format!(
                    "A game declares at most {MAX_BUTTONS} buttons; [player] buttons has {}.",
                    buttons.len()
                ),
            ));
        }
        let logical = seat.logical();
        let mut targets: Vec<&str> = logical.to_vec();
        targets.extend(buttons.iter().map(|b| b.name.as_str()));
        let mut map = default_bindings(seat);
        for (name, inputs) in bindings {
            let at = root.key("bindings").key(name);
            if !logical.contains(&name.as_str()) {
                return Err(codes::invalid_value(
                    &at,
                    name,
                    &candidates(logical.iter().copied()),
                ));
            }
            for (i, input) in inputs.iter().enumerate() {
                check_input(&at.index(i), input, &targets)?;
            }
            map.insert(name.clone(), inputs.clone());
        }
        for (i, b) in buttons.iter().enumerate() {
            for (j, input) in b.keys.iter().enumerate() {
                check_input(
                    &root.key("buttons").index(i).key("keys").index(j),
                    input,
                    &targets,
                )?;
            }
        }
        let touch = touch
            .cloned()
            .unwrap_or_else(|| TouchLayout::default_for(seat));
        if touch.buttons.len() > MAX_TOUCH_BUTTONS {
            return Err(invalid(
                &root.key("touch").key("buttons"),
                format!(
                    "The touch layout has at most {MAX_TOUCH_BUTTONS} buttons; it has {}.",
                    touch.buttons.len()
                ),
            ));
        }
        for (i, b) in touch.buttons.iter().enumerate() {
            if !targets.contains(&b.input.as_str()) {
                return Err(codes::invalid_value(
                    &root.key("touch").key("buttons").index(i).key("input"),
                    &b.input,
                    &candidates(targets.iter().copied()),
                ));
            }
        }
        Ok(InputMap {
            seat,
            bindings: map,
            buttons: buttons.to_vec(),
            touch,
        })
    }
}

/// One frame of the seat's logical inputs, resolved through the map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeatInput {
    /// Strafe (right positive) and forward, of length at most 1.
    pub motion: [f32; 2],
    /// The look in pointer counts: the pointer's and the look stick's.
    pub look: [f32; 2],
    pub run: bool,
    pub rise: bool,
    /// A third-person seat's guard, held.
    pub guard: bool,
    /// Presses this frame.
    pub jump: bool,
    pub primary: bool,
    pub secondary: bool,
    /// A third-person seat's presses this frame.
    pub dodge: bool,
    pub interact: bool,
    pub lock: bool,
    /// A third-person seat's `zoom_out` presses less its `zoom_in` presses.
    pub zoom_steps: i32,
    /// A slot chosen this frame (0 for `slot_1`), and steps right less steps left.
    pub slot: Option<u8>,
    pub slot_steps: i32,
    /// The game's buttons down now and pressed this frame, a bit each.
    pub held: u32,
    pub pressed: u32,
}

/// A first- or third-person seat as a presenter drives it: a frame of devices in, its
/// `player.act` actions out, sent only when something changed (input.md 3, 3.1).
#[derive(Clone, Debug)]
pub struct Seat {
    map: InputMap,
    /// The gamepad's inputs down after the last frame, for its press edges.
    pad_down: BTreeSet<&'static str>,
    sent: Option<(f32, f32, bool, f32, f32, bool)>,
    slot: u8,
    held: u32,
    /// A third-person seat's zoom and guard as last sent.
    zoom: f32,
    guard: bool,
}

impl Seat {
    pub fn new(map: InputMap) -> Seat {
        Seat {
            map,
            pad_down: BTreeSet::new(),
            sent: None,
            slot: 0,
            held: 0,
            zoom: 1.0,
            guard: false,
        }
    }

    pub fn map(&self) -> &InputMap {
        &self.map
    }

    /// Resolves a frame. While not `playing` every control is at rest and nothing is pressed, but
    /// the gamepad's presses are still taken, so the press that starts play does not also act.
    pub fn frame(&mut self, f: &DeviceFrame, playing: bool) -> SeatInput {
        let mut held: BTreeMap<&str, f32> = f
            .held
            .iter()
            .filter(|(_, s)| **s > 0.0)
            .map(|(k, s)| (k.as_str(), at_most_one(*s)))
            .collect();
        let mut pressed: Vec<&str> = f.pressed.iter().map(String::as_str).collect();
        let mut down = BTreeSet::new();
        for (name, s) in f.pad.as_ref().map(Pad::inputs).unwrap_or_default() {
            let h = held.entry(name).or_insert(0.0);
            *h = larger(*h, s);
            if s >= DOWN {
                down.insert(name);
                if !self.pad_down.contains(name) {
                    pressed.push(name);
                }
            }
        }
        self.pad_down = down;
        if !playing {
            return SeatInput::default();
        }
        let touch = |name: &str| format!("{TOUCH_PREFIX}{name}");
        let strength = |inputs: &[String], name: &str| {
            let t = touch(name);
            inputs
                .iter()
                .map(String::as_str)
                .chain([t.as_str()])
                .map(|i| held.get(i).copied().unwrap_or(0.0))
                .fold(0.0f32, larger)
        };
        let presses = |inputs: &[String], name: &str| {
            let t = touch(name);
            pressed
                .iter()
                .filter(|p| **p == t || inputs.iter().any(|i| i == *p))
                .count()
        };
        let empty = Vec::new();
        let inputs = |name: &str| self.map.bindings.get(name).unwrap_or(&empty);
        let level = |name: &str| strength(inputs(name), name);
        let count = |name: &str| presses(inputs(name), name);
        let x = level("move_right") - level("move_left");
        let z = level("move_forward") - level("move_back");
        let length = larger((x * x + z * z).sqrt(), 1.0);
        let dt = if f.dt_s.is_finite() {
            f.dt_s.clamp(0.0, 0.1)
        } else {
            0.0
        };
        let turn = |pos: &str, neg: &str, rate: f32| {
            let v = level(pos) - level(neg);
            v * v.abs() * rate * dt / LOOK_RAD_PER_COUNT
        };
        // The last slot chosen this frame wins, as the last key pressed does.
        let mut slot = None;
        for p in &pressed {
            for k in 1..=9u8 {
                let name = format!("slot_{k}");
                if *p == touch(&name) || inputs(&name).iter().any(|i| i == p) {
                    slot = Some(k - 1);
                }
            }
        }
        let (mut held_mask, mut pressed_mask) = (0, 0);
        for (i, b) in self.map.buttons.iter().enumerate().take(MAX_BUTTONS) {
            if strength(&b.keys, &b.name) >= DOWN {
                held_mask |= 1 << i;
            }
            if presses(&b.keys, &b.name) > 0 {
                pressed_mask |= 1 << i;
            }
        }
        SeatInput {
            motion: [x / length, z / length],
            look: [
                f.look[0] + turn("look_right", "look_left", STICK_LOOK_RAD_S[0]),
                f.look[1] + turn("look_down", "look_up", STICK_LOOK_RAD_S[1]),
            ],
            run: level("run") >= DOWN,
            rise: level("rise") >= DOWN,
            guard: level("guard") >= DOWN,
            jump: count("jump") > 0,
            primary: count("primary") > 0,
            secondary: count("secondary") > 0,
            dodge: count("dodge") > 0,
            interact: count("interact") > 0,
            lock: count("lock") > 0,
            zoom_steps: i32::try_from(count("zoom_out")).unwrap_or(0)
                - i32::try_from(count("zoom_in")).unwrap_or(0),
            slot,
            slot_steps: i32::try_from(count("slot_next")).unwrap_or(0)
                - i32::try_from(count("slot_prev")).unwrap_or(0),
            held: held_mask,
            pressed: pressed_mask,
        }
    }

    /// The `player.act` actions that say what changed, with the look (yaw, pitch) the presenter
    /// turned by [`SeatInput::look`].
    pub fn acts(&mut self, input: &SeatInput, yaw: f32, pitch: f32) -> Vec<Value> {
        let mut actions = match self.map.seat {
            SeatKind::FirstPerson => self.first_person_acts(input, yaw, pitch),
            SeatKind::ThirdPerson => self.third_person_acts(input, yaw, pitch),
        };
        // The game's own buttons (animation.md 6).
        if !self.map.buttons.is_empty() {
            if input.held != self.held {
                self.held = input.held;
                actions.push(json!({"do": "set", "controls": {"held": input.held}}));
            }
            if input.pressed != 0 {
                actions.push(json!({"do": "set", "controls": {"pressed": input.pressed}}));
            }
        }
        actions
    }

    /// A third-person seat's controls (input.md 3.1; player.md 5.1): the move, the orbit's look
    /// and sprint, the zoom a step a press, the guard while held and the verbs' pulses.
    fn third_person_acts(&mut self, input: &SeatInput, yaw: f32, pitch: f32) -> Vec<Value> {
        let mut actions = Vec::new();
        let now = (
            input.motion[1],
            input.motion[0],
            input.run,
            yaw,
            pitch,
            false,
        );
        if self.sent != Some(now) {
            actions.push(json!({"do": "set", "controls": {
                "forward": now.0, "strafe": now.1, "run": now.2, "yaw": yaw, "pitch": pitch}}));
            self.sent = Some(now);
        }
        if input.zoom_steps != 0 {
            // A step a press, multiplied in order (the game crates' lint refuses `powi`).
            let mut zoom = self.zoom;
            for _ in 0..input.zoom_steps.unsigned_abs() {
                zoom = if input.zoom_steps > 0 {
                    zoom * ZOOM_STEP
                } else {
                    zoom / ZOOM_STEP
                };
            }
            let zoom = zoom.clamp(ZOOM_RANGE[0], ZOOM_RANGE[1]);
            if zoom != self.zoom {
                self.zoom = zoom;
                actions.push(json!({"do": "set", "controls": {"zoom": zoom}}));
            }
        }
        if input.guard != self.guard {
            self.guard = input.guard;
            actions.push(json!({"do": "set", "controls": {"guard": input.guard}}));
        }
        for (on, control) in [
            (input.jump, "jump"),
            (input.primary, "primary"),
            (input.secondary, "secondary"),
            (input.dodge, "dodge"),
            (input.interact, "interact"),
            (input.lock, "lock"),
        ] {
            if on {
                actions.push(json!({"do": "pulse", "control": control}));
            }
        }
        actions
    }

    /// A first-person seat's controls (input.md 3; voxels.md 8): the walk, the look, the hotbar
    /// and the hands' pulses.
    fn first_person_acts(&mut self, input: &SeatInput, yaw: f32, pitch: f32) -> Vec<Value> {
        let mut actions = Vec::new();
        let now = (
            input.motion[1],
            input.motion[0],
            input.run,
            yaw,
            pitch,
            input.rise,
        );
        if self.sent != Some(now) {
            actions.push(json!({"do": "set", "controls": {
                "forward": now.0, "strafe": now.1, "run": now.2,
                "yaw": yaw, "pitch": pitch, "rise": now.5}}));
            self.sent = Some(now);
        }
        let slot = match (input.slot, input.slot_steps) {
            (Some(s), _) => s,
            (None, 0) => self.slot,
            (None, n) => u8::try_from((i32::from(self.slot) + n).rem_euclid(9)).unwrap_or(0),
        };
        if slot != self.slot {
            self.slot = slot;
            actions.push(json!({"do": "set", "controls": {"slot": slot.to_string()}}));
        }
        for (on, control) in [
            (input.jump, "jump"),
            (input.primary, "primary"),
            (input.secondary, "secondary"),
        ] {
            if on {
                actions.push(json!({"do": "pulse", "control": control}));
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn length(x: f32, y: f32) -> f32 {
        (x * x + y * y).sqrt()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn keys(held: &[&str], pressed: &[&str]) -> DeviceFrame {
        DeviceFrame {
            held: held.iter().map(|k| ((*k).to_owned(), 1.0)).collect(),
            pressed: pressed.iter().map(|k| (*k).to_owned()).collect(),
            dt_s: 1.0 / 60.0,
            ..Default::default()
        }
    }

    fn pad(buttons: &[(usize, f32)], axes: [f32; 4]) -> DeviceFrame {
        let mut b = vec![0.0; 17];
        for (i, v) in buttons {
            b[*i] = *v;
        }
        DeviceFrame {
            pad: Some(Pad {
                buttons: b,
                axes: axes.to_vec(),
            }),
            dt_s: 0.05,
            ..Default::default()
        }
    }

    #[test]
    fn the_stick_has_a_radial_rescaled_dead_zone() {
        assert_eq!(stick(0.1, 0.1), [0.0; 2], "inside the zone");
        assert_eq!(stick(f32::NAN, 0.0), [0.0; 2]);
        // Just past the edge reads nearly nothing: no jump at the zone's edge.
        let [x, y] = stick(STICK_DEAD_ZONE + 1e-4, 0.0);
        assert!(x > 0.0 && x < 1e-3 && y == 0.0);
        // Halfway between the zone and full deflection is half, along its own direction.
        let r = (STICK_DEAD_ZONE + STICK_FULL) / 2.0;
        let d = r / 2f32.sqrt();
        let [x, y] = stick(d, -d);
        assert!(close(length(x, y), 0.5) && close(x, -y), "{x} {y}");
        // A slightly-off diagonal stays diagonal (a square zone would snap it to an axis).
        let [x, y] = stick(0.7, 0.12);
        assert!(y > 0.1, "{y}");
        assert!(close(x / y, 0.7 / 0.12));
        // Full and past full read 1.
        assert!(close(stick(0.0, -STICK_FULL)[1], -1.0));
        assert!(close(length(stick(1.0, 1.0)[0], stick(1.0, 1.0)[1]), 1.0));
    }

    #[test]
    fn buttons_have_a_dead_zone_and_reach_one() {
        assert_eq!(button(0.04), 0.0);
        assert_eq!(button(f32::NAN), 0.0);
        assert!(close(button(1.0), 1.0));
        assert!(close(button(0.525), 0.5));
        assert!(button(2.0) <= 1.0);
    }

    #[test]
    fn a_pad_reads_in_the_standard_layout() {
        let p = Pad {
            buttons: vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3],
            axes: vec![0.0, -1.0, 1.0, 0.0],
        };
        let inputs = p.inputs();
        let names: Vec<&str> = inputs.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            ["PadSouth", "PadR2", "PadLStickUp", "PadRStickRight"],
            "y is positive down"
        );
        assert!(close(inputs[1].1, button(0.3)));
        assert_eq!(PAD_BUTTONS.iter().collect::<BTreeSet<_>>().len(), 17);
    }

    #[test]
    fn the_default_map_covers_every_logical_input() {
        assert_eq!(InputMap::default().seat, SeatKind::FirstPerson);
        for seat in [SeatKind::FirstPerson, SeatKind::ThirdPerson] {
            let m = InputMap::new(seat, &BTreeMap::new(), &[], None).unwrap();
            let logical = seat.logical();
            for l in logical {
                assert!(!m.bindings[*l].is_empty(), "{seat:?} {l}");
                for i in &m.bindings[*l] {
                    check_input(&Pointer::root(), i, logical).unwrap();
                }
            }
            assert_eq!(m.bindings.len(), logical.len(), "{seat:?}");
            for b in &m.touch.buttons {
                assert!(logical.contains(&b.input.as_str()), "{seat:?} {}", b.input);
            }
        }
        assert_eq!(
            InputMap::default(),
            InputMap::new(SeatKind::FirstPerson, &BTreeMap::new(), &[], None).unwrap()
        );
        assert_eq!(SeatKind::of_catalog("third_person"), SeatKind::ThirdPerson);
        assert_eq!(SeatKind::of_catalog("sailing"), SeatKind::FirstPerson);
    }

    #[test]
    fn a_project_replaces_bindings_and_is_refused_where_it_is_wrong() {
        let buttons = vec![ButtonDef {
            name: "dodge".into(),
            keys: vec!["KeyQ".into(), "PadEast".into()],
        }];
        let over: BTreeMap<String, Vec<String>> =
            [("jump".to_owned(), vec!["PadNorth".to_owned()])].into();
        let touch = TouchLayout {
            buttons: vec![TouchButton {
                label: "Roll".into(),
                input: "dodge".into(),
            }],
            ..TouchLayout::default()
        };
        let m = InputMap::new(SeatKind::FirstPerson, &over, &buttons, Some(&touch)).unwrap();
        assert_eq!(m.bindings["jump"], ["PadNorth"]);
        assert_eq!(
            m.bindings["rise"],
            ["Space", "PadSouth"],
            "others keep theirs"
        );
        assert_eq!(m.touch.buttons[0].input, "dodge");
        let refused =
            |b: &BTreeMap<String, Vec<String>>, buttons: &[ButtonDef], t: Option<&TouchLayout>| {
                let p = InputMap::new(SeatKind::FirstPerson, b, buttons, t).unwrap_err();
                (
                    p.code.clone(),
                    p.detail["path"].as_str().unwrap_or("").to_owned(),
                )
            };
        let one = |k: &str, v: &str| -> BTreeMap<String, Vec<String>> {
            [(k.to_owned(), vec!["Space".to_owned(), v.to_owned()])].into()
        };
        assert_eq!(
            refused(&one("jumpp", "Space"), &[], None),
            (
                "request.invalid_value".into(),
                "/player/bindings/jumpp".into()
            )
        );
        assert_eq!(
            refused(&one("jump", "PadA"), &[], None).1,
            "/player/bindings/jump/1"
        );
        assert_eq!(
            refused(&one("jump", "MouseBack"), &[], None).1,
            "/player/bindings/jump/1"
        );
        assert_eq!(
            refused(&one("jump", "space"), &[], None).1,
            "/player/bindings/jump/1"
        );
        assert_eq!(
            refused(&one("jump", "Touch:dodge"), &[], None).1,
            "/player/bindings/jump/1"
        );
        InputMap::new(
            SeatKind::FirstPerson,
            &one("jump", "Touch:dodge"),
            &buttons,
            None,
        )
        .unwrap();
        let bad = vec![ButtonDef {
            name: "x".into(),
            keys: vec!["PadZ".into()],
        }];
        assert_eq!(
            refused(&BTreeMap::new(), &bad, None).1,
            "/player/buttons/0/keys/0"
        );
        let bad_touch = TouchLayout {
            buttons: vec![TouchButton {
                label: "?".into(),
                input: "dodge".into(),
            }],
            ..TouchLayout::default()
        };
        assert_eq!(
            refused(&BTreeMap::new(), &[], Some(&bad_touch)).1,
            "/player/touch/buttons/0/input"
        );
        let many = vec![
            ButtonDef {
                name: "b".into(),
                keys: vec![],
            };
            MAX_BUTTONS + 1
        ];
        assert_eq!(refused(&BTreeMap::new(), &many, None).0, "project.invalid");
        // A third-person verb is no first-person input, and the other way round.
        assert_eq!(
            refused(&one("dodge", "KeyC"), &[], None).1,
            "/player/bindings/dodge"
        );
        let p =
            InputMap::new(SeatKind::ThirdPerson, &one("slot_1", "Digit1"), &[], None).unwrap_err();
        assert_eq!(p.detail["path"], "/player/bindings/slot_1");
        let m = InputMap::new(SeatKind::ThirdPerson, &one("interact", "KeyG"), &[], None).unwrap();
        assert_eq!(m.bindings["interact"], ["Space", "KeyG"]);
        assert_eq!(
            m.bindings["dodge"],
            ["KeyC", "PadEast"],
            "others keep theirs"
        );
    }

    #[test]
    fn a_third_person_seat_sends_its_controls() {
        let buttons = vec![ButtonDef {
            name: "flask".into(),
            keys: vec!["Digit3".into(), "PadWest".into()],
        }];
        let map = InputMap::new(SeatKind::ThirdPerson, &BTreeMap::new(), &buttons, None).unwrap();
        let mut seat = Seat::new(map);
        // W, Shift, the right button held; C, Q and two notches toward the player.
        let f = keys(
            &["KeyW", "ShiftLeft", "MouseRight"],
            &["KeyC", "KeyQ", "WheelDown", "WheelDown", "Digit3"],
        );
        let i = seat.frame(&f, true);
        assert!(i.run && i.guard && i.dodge && i.lock && !i.jump && !i.rise);
        assert_eq!((i.zoom_steps, i.slot, i.slot_steps), (2, None, 0));
        let a = seat.acts(&i, 0.5, -0.2);
        assert_eq!(
            a,
            [
                json!({"do": "set", "controls": {"forward": 1.0, "strafe": 0.0, "run": true,
                    "yaw": 0.5f32, "pitch": -0.2f32}}),
                json!({"do": "set", "controls": {"zoom": 1.1f32 * 1.1f32}}),
                json!({"do": "set", "controls": {"guard": true}}),
                json!({"do": "pulse", "control": "dodge"}),
                json!({"do": "pulse", "control": "lock"}),
                json!({"do": "set", "controls": {"pressed": 1}}),
            ]
        );
        // The gamepad: L2 guards, R2 swings, North is the heavy, East dodges, West interacts and
        // D-pad up zooms in; the zoom stops at its range.
        let p = pad(
            &[(6, 1.0), (7, 1.0), (3, 1.0), (1, 1.0), (2, 1.0), (12, 1.0)],
            [0.0; 4],
        );
        let i = seat.frame(&p, true);
        assert!(i.guard && i.primary && i.secondary && i.dodge && i.interact && !i.lock);
        assert_eq!(i.zoom_steps, -1);
        assert_eq!(i.held, 1, "PadWest also holds the game's flask");
        for _ in 0..20 {
            seat.frame(&pad(&[], [0.0; 4]), true);
            let i = seat.frame(&pad(&[(13, 1.0)], [0.0; 4]), true);
            seat.acts(&i, 0.5, -0.2);
        }
        assert_eq!(seat.zoom, ZOOM_RANGE[1]);
        // Released: nothing pulses, and the guard stays lowered.
        let i = seat.frame(&f, false);
        let a = seat.acts(&i, 0.5, -0.2);
        assert!(a.iter().all(|a| a["do"] == "set"), "{a:?}");
        assert!(!seat.guard);
        let i = seat.frame(&f, false);
        assert!(seat.acts(&i, 0.5, -0.2).is_empty());
    }

    #[test]
    fn keys_resolve_as_the_window_always_read_them() {
        let mut seat = Seat::new(InputMap::default());
        let i = seat.frame(
            &keys(
                &["KeyW", "KeyD", "ShiftRight", "Space"],
                &["Space", "Digit4", "WheelDown"],
            ),
            true,
        );
        assert!(close(length(i.motion[0], i.motion[1]), 1.0) && close(i.motion[0], i.motion[1]));
        assert!(i.run && i.rise && i.jump && !i.primary);
        assert_eq!((i.slot, i.slot_steps), (Some(3), 1));
        assert_eq!(i.look, [0.0; 2]);
        let i = seat.frame(&keys(&["KeyA", "KeyD"], &[]), true);
        assert_eq!(i.motion, [0.0; 2]);
        assert!(!i.jump && i.slot.is_none());
    }

    #[test]
    fn a_pad_moves_looks_and_presses_once() {
        let mut seat = Seat::new(InputMap::default());
        // Left stick half up, right stick full right, A down.
        let r = (STICK_DEAD_ZONE + STICK_FULL) / 2.0;
        let f = pad(&[(0, 1.0)], [0.0, -r, 1.0, 0.0]);
        let i = seat.frame(&f, true);
        assert!(
            close(i.motion[1], 0.5) && i.motion[0] == 0.0,
            "{:?}",
            i.motion
        );
        assert!(
            close(i.look[0], 3.0 * 0.05 / LOOK_RAD_PER_COUNT),
            "{:?}",
            i.look
        );
        assert!(i.jump && i.rise);
        // Held: no second press, still rising.
        let i = seat.frame(&f, true);
        assert!(!i.jump && i.rise);
        // The look's response is quadratic: half the deflection turns a quarter as fast.
        let f = pad(&[], [0.0, 0.0, 0.0, r]);
        let i = seat.frame(&f, true);
        assert!(
            close(i.look[1], 0.25 * 2.0 * 0.05 / LOOK_RAD_PER_COUNT),
            "{:?}",
            i.look
        );
        // A trigger past halfway presses once; R1 steps the slot.
        let i = seat.frame(&pad(&[(7, 0.6), (5, 1.0)], [0.0; 4]), true);
        assert!(i.primary && i.slot_steps == 1);
        let i = seat.frame(&pad(&[(7, 0.6), (5, 1.0)], [0.0; 4]), true);
        assert!(!i.primary && i.slot_steps == 0);
        // A disconnect lifts everything.
        let i = seat.frame(&DeviceFrame::default(), true);
        assert_eq!(i, SeatInput::default());
    }

    #[test]
    fn the_press_that_starts_play_does_not_act() {
        let mut seat = Seat::new(InputMap::default());
        let a = pad(&[(0, 1.0)], [0.0; 4]);
        assert_eq!(seat.frame(&a, false), SeatInput::default());
        let i = seat.frame(&a, true);
        assert!(!i.jump && i.rise, "held, not pressed");
        assert!(seat.frame(&pad(&[], [0.0; 4]), true) == SeatInput::default());
        assert!(seat.frame(&a, true).jump);
    }

    #[test]
    fn touch_buttons_and_the_touch_stick_reach_their_inputs() {
        let buttons = vec![ButtonDef {
            name: "dodge".into(),
            keys: vec!["KeyQ".into()],
        }];
        let mut seat = Seat::new(
            InputMap::new(SeatKind::FirstPerson, &BTreeMap::new(), &buttons, None).unwrap(),
        );
        let mut f = keys(
            &["Touch:dodge", "Touch:jump"],
            &["Touch:dodge", "Touch:jump", "Touch:slot_3"],
        );
        f.held.insert("TouchStickUp".into(), 0.4);
        f.held.insert("TouchStickRight".into(), 0.3);
        f.look = [12.0, -4.0];
        let i = seat.frame(&f, true);
        assert!(close(i.motion[0], 0.3) && close(i.motion[1], 0.4));
        assert!(i.jump && !i.rise, "Touch:jump is jump, not rise");
        assert_eq!((i.held, i.pressed, i.slot), (1, 1, Some(2)));
        assert_eq!(i.look, [12.0, -4.0]);
    }

    #[test]
    fn the_seat_sends_what_changed() {
        let buttons = vec![ButtonDef {
            name: "dodge".into(),
            keys: vec!["PadEast".into()],
        }];
        let mut seat = Seat::new(
            InputMap::new(SeatKind::FirstPerson, &BTreeMap::new(), &buttons, None).unwrap(),
        );
        let f = pad(&[(1, 1.0), (0, 1.0)], [0.0, -1.0, 0.0, 0.0]);
        let i = seat.frame(&f, true);
        let a = seat.acts(&i, 0.25, -0.1);
        assert_eq!(
            a,
            [
                json!({"do": "set", "controls": {"forward": 1.0, "strafe": 0.0, "run": false,
                    "yaw": 0.25f32, "pitch": -0.1f32, "rise": true}}),
                json!({"do": "pulse", "control": "jump"}),
                json!({"do": "set", "controls": {"held": 1}}),
                json!({"do": "set", "controls": {"pressed": 1}}),
            ]
        );
        let i = seat.frame(&f, true);
        assert!(seat.acts(&i, 0.25, -0.1).is_empty(), "nothing changed");
        // Released: everything at rest, sent once.
        let i = seat.frame(&f, false);
        let a = seat.acts(&i, 0.25, -0.1);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0]["controls"]["forward"], 0.0);
        assert_eq!(a[1], json!({"do": "set", "controls": {"held": 0}}));
    }

    #[test]
    fn frames_merge_presses_and_look() {
        let mut a = keys(&["KeyW"], &["Space"]);
        a.look = [1.0, 2.0];
        let mut b = keys(&["KeyS"], &["Digit1"]);
        b.look = [3.0, 4.0];
        a.merge(b);
        assert_eq!(a.pressed, ["Space", "Digit1"]);
        assert_eq!(a.look, [4.0, 6.0]);
        assert!(a.held.contains_key("KeyS") && !a.held.contains_key("KeyW"));
        assert!(close(a.dt_s, 2.0 / 60.0));
    }
}
