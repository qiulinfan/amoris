//! The native window's gamepads (docs/spec/input.md 6), read with gilrs in the standard layout of
//! `amoris_assets::input` (input.md 2.1). The pad that last sent an event is the active one.

use amoris_assets::input::{PAD_BUTTONS, Pad};
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

/// gilrs's buttons in the standard layout's order (`PAD_BUTTONS`).
pub const BUTTONS: [Button; 17] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::Select,
    Button::Start,
    Button::LeftThumb,
    Button::RightThumb,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::Mode,
];

/// What happened to the gamepads since the last poll.
#[derive(Clone, Debug, PartialEq)]
pub enum PadEvent {
    Connected(String),
    Disconnected(String),
    /// A button of the standard layout went down, by its input name (`PadStart`).
    Pressed(&'static str),
}

/// The gamepads, while a window plays a seat.
pub struct Pads {
    gilrs: Gilrs,
    active: Option<GamepadId>,
}

impl Pads {
    /// The platform's gamepads, or none (said once) where gilrs has no backend.
    pub fn new() -> Option<Pads> {
        match Gilrs::new() {
            Ok(gilrs) => {
                for (_, pad) in gilrs.gamepads() {
                    eprintln!("gamepad: {} connected", pad.name());
                }
                let active = gilrs.gamepads().map(|(id, _)| id).next();
                Some(Pads { gilrs, active })
            }
            Err(e) => {
                eprintln!("no gamepads: {e}");
                None
            }
        }
    }

    /// Drains gilrs's events: connections, the active pad, and the standard buttons pressed.
    pub fn poll(&mut self) -> Vec<PadEvent> {
        let mut out = Vec::new();
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            let name = || self.gilrs.gamepad(id).name().to_owned();
            match event {
                EventType::Connected => {
                    self.active.get_or_insert(id);
                    out.push(PadEvent::Connected(name()));
                }
                EventType::Disconnected => {
                    if self.active == Some(id) {
                        self.active = self.gilrs.gamepads().map(|(g, _)| g).find(|g| *g != id);
                    }
                    out.push(PadEvent::Disconnected(name()));
                }
                EventType::ButtonPressed(b, _) => {
                    self.active = Some(id);
                    if let Some(i) = BUTTONS.iter().position(|x| *x == b) {
                        out.push(PadEvent::Pressed(PAD_BUTTONS[i]));
                    }
                }
                EventType::ButtonChanged(..) | EventType::AxisChanged(..) => {
                    self.active = Some(id);
                }
                _ => {}
            }
        }
        out
    }

    /// The active pad's name.
    pub fn name(&self) -> Option<String> {
        let pad = self.gilrs.connected_gamepad(self.active?)?;
        Some(pad.name().to_owned())
    }

    /// The active pad's state in the standard layout: button values and the sticks, positive
    /// right and down (gilrs's y is up).
    pub fn state(&self) -> Option<Pad> {
        let pad = self.gilrs.connected_gamepad(self.active?)?;
        let buttons = BUTTONS
            .iter()
            .map(|b| pad.button_data(*b).map_or(0.0, |d| d.value()))
            .collect();
        let axes = vec![
            pad.value(Axis::LeftStickX),
            -pad.value(Axis::LeftStickY),
            pad.value(Axis::RightStickX),
            -pad.value(Axis::RightStickY),
        ];
        Some(Pad { buttons, axes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gilrs_buttons_cover_the_standard_layout_once_each() {
        assert_eq!(BUTTONS.len(), PAD_BUTTONS.len());
        for (i, b) in BUTTONS.iter().enumerate() {
            assert_eq!(BUTTONS.iter().position(|x| x == b), Some(i), "{b:?} twice");
        }
    }
}
