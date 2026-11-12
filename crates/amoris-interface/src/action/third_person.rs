//! The `third_person` catalog (docs/spec/player.md 5.1): one seat, `player`, whose body carries a
//! `Walker` and a `ThirdPerson`. Its controls write the move in a frame, the orbit's look and zoom,
//! sprint and guard; its pulses jump, act, dodge, interact and lock on, two of them at a target the
//! seat perceives. `physics.third_person` turns them into the walker's each tick. No intents and
//! no codes: the game's rules give the verbs their meaning.

use amoris_contract::Problem;
use amoris_physics::{MoveFrame, ThirdPerson, Walker};
use amoris_sim::EntityId;
use bevy_ecs::prelude::World;

use super::catalog::{ActionCatalog, Bodies, SeatDecl, ViewFactory};
use super::defs::{ControlBinding, ControlDef, ControlKind};
use super::first_person::{button_controls, flag, number, read, with_walker};
use super::state::ControlValue;
use crate::perception::PerceptionDefs;

/// The move's frames, as the `frame` control names them.
pub const FRAMES: [&str; 3] = ["camera", "world", "body"];

/// Whether the body takes the catalog's controls: a walker with a `ThirdPerson`.
fn has_seat(world: &World, body: EntityId) -> bool {
    amoris_sim::entity::entity(world, body)
        .is_some_and(|e| world.get::<Walker>(e).is_some() && world.get::<ThirdPerson>(e).is_some())
}

fn with_seat(
    world: &mut World,
    body: EntityId,
    f: impl FnOnce(&mut ThirdPerson),
) -> Result<(), Problem> {
    let e = amoris_sim::entity::require(world, body)?;
    let mut tp = world.get_mut::<ThirdPerson>(e).ok_or_else(|| {
        super::state::internal("third_person", "the seat's body has no ThirdPerson")
    })?;
    let mut next = *tp;
    f(&mut next);
    if *tp != next {
        *tp = next;
    }
    Ok(())
}

fn read_seat(
    world: &World,
    body: EntityId,
    f: fn(&ThirdPerson) -> ControlValue,
) -> Option<ControlValue> {
    let e = amoris_sim::entity::entity(world, body)?;
    world.get::<ThirdPerson>(e).map(f)
}

/// A pulse's target as an entity id.
fn target(v: Option<&ControlValue>) -> Result<EntityId, Problem> {
    let x = number(v)?;
    EntityId::from_f64(x)
        .map_err(|_| super::state::internal("third_person", "a pulse's target is no entity"))
}

/// The controls bound to the walker itself: the orbit's look, sprint and its pulses.
fn walker_controls() -> Vec<ControlDef> {
    let axis = |min: f64, max: f64| ControlKind::Axis {
        min,
        max,
        default: 0.0,
    };
    let pulse = ControlKind::Pulse { target_kinds: None };
    vec![
        ControlDef {
            name: "yaw".into(),
            doc: "The orbit camera's turn, radians about +y: 0 looks along -z (north), positive turns left (west).".into(),
            kind: axis(-std::f64::consts::PI, std::f64::consts::PI),
            channel: "look".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_walker(w, b, |walker| walker.yaw = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Number(walker.yaw)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "pitch".into(),
            doc: "The orbit camera's tilt, radians: positive looks up from below.".into(),
            kind: axis(
                -amoris_physics::walker::MAX_PITCH,
                amoris_physics::walker::MAX_PITCH,
            ),
            channel: "look".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_walker(w, b, |walker| walker.pitch = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Number(walker.pitch)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "run".into(),
            doc: "Sprint instead of walking.".into(),
            kind: ControlKind::Toggle { default: false },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = flag(v)?;
                    with_walker(w, b, |walker| walker.run = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.run)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "jump".into(),
            doc: "Jump, if standing on something (a game may give it another meaning).".into(),
            kind: pulse.clone(),
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_walker(w, b, |walker| walker.jump = true),
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.jump)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "primary".into(),
            doc: "The main action for one tick (an attack, a shot).".into(),
            kind: pulse.clone(),
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_walker(w, b, |walker| walker.primary = true),
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.primary)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "secondary".into(),
            doc: "The second action for one tick (a heavy attack).".into(),
            kind: pulse,
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_walker(w, b, |walker| walker.secondary = true),
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.secondary)),
                writable: has_seat,
            },
        },
    ]
}

/// The controls bound to the seat's `ThirdPerson`.
fn seat_controls(kinds: &[String]) -> Vec<ControlDef> {
    let axis = |min: f64, max: f64, default: f64| ControlKind::Axis { min, max, default };
    let pulse = ControlKind::Pulse { target_kinds: None };
    let mut all = vec![
        ControlDef {
            name: "forward".into(),
            doc: "Move ahead in the frame, -1 (back) to 1: away from the camera, north, or the way the body faces.".into(),
            kind: axis(-1.0, 1.0, 0.0),
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_seat(w, b, |t| t.forward = x)
                },
                read: |w, b| read_seat(w, b, |t| ControlValue::Number(t.forward)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "strafe".into(),
            doc: "Move sideways in the frame, -1 (left, or west) to 1 (right, or east).".into(),
            kind: axis(-1.0, 1.0, 0.0),
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_seat(w, b, |t| t.strafe = x)
                },
                read: |w, b| read_seat(w, b, |t| ControlValue::Number(t.strafe)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "frame".into(),
            doc: "What forward and strafe are relative to: \"camera\" (the look's yaw), \"world\" (north and east: a bearing b is forward cos b, strafe sin b) or \"body\" (the way the body faces).".into(),
            kind: ControlKind::Choice {
                values: FRAMES.iter().map(|s| (*s).to_owned()).collect(),
                default: "camera".into(),
            },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let frame = match v {
                        Some(ControlValue::Name(n)) if n == "camera" => MoveFrame::Camera,
                        Some(ControlValue::Name(n)) if n == "world" => MoveFrame::World,
                        Some(ControlValue::Name(n)) if n == "body" => MoveFrame::Body,
                        _ => {
                            return Err(super::state::internal(
                                "third_person",
                                "an unknown frame",
                            ));
                        }
                    };
                    with_seat(w, b, |t| t.frame = frame)
                },
                read: |w, b| {
                    read_seat(w, b, |t| {
                        ControlValue::Name(
                            match t.frame {
                                MoveFrame::Camera => "camera",
                                MoveFrame::World => "world",
                                MoveFrame::Body => "body",
                            }
                            .to_owned(),
                        )
                    })
                },
                writable: has_seat,
            },
        },
        ControlDef {
            name: "zoom".into(),
            doc: "The camera arm's length as a share of its rig's distance, 0.5 (near) to 2.".into(),
            kind: axis(0.5, 2.0, 1.0),
            channel: "look".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_seat(w, b, |t| t.zoom = x)
                },
                read: |w, b| read_seat(w, b, |t| ControlValue::Number(t.zoom)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "guard".into(),
            doc: "Held: guard (or aim), as the game's rules say.".into(),
            kind: ControlKind::Toggle { default: false },
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = flag(v)?;
                    with_seat(w, b, |t| t.guard = x)
                },
                read: |w, b| read_seat(w, b, |t| ControlValue::Bool(t.guard)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "dodge".into(),
            doc: "Dodge (a roll or a step) for one tick.".into(),
            kind: pulse.clone(),
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_seat(w, b, |t| t.dodge = true),
                read: |w, b| read_seat(w, b, |t| ControlValue::Bool(t.dodge)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "interact".into(),
            doc: "Interact with what is in front (talk, open, take) for one tick.".into(),
            kind: pulse.clone(),
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_seat(w, b, |t| t.interact = true),
                read: |w, b| read_seat(w, b, |t| ControlValue::Bool(t.interact)),
                writable: has_seat,
            },
        },
        ControlDef {
            name: "lock".into(),
            doc: "Lock on to the nearest lockable thing in front and in sight, or let go of the lock.".into(),
            kind: pulse,
            channel: "look".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_seat(w, b, |t| t.lock_toggle = true),
                read: |w, b| read_seat(w, b, |t| ControlValue::Bool(t.lock_toggle)),
                writable: has_seat,
            },
        },
    ];
    // The targeted pulses take any kind the game's perception declares; without kinds nothing
    // could be named, so they are left out.
    if !kinds.is_empty() {
        let targeted = ControlKind::Pulse {
            target_kinds: Some(kinds.to_vec()),
        };
        all.push(ControlDef {
            name: "lock_on".into(),
            doc: "Lock on to this entity (it must be lockable; another is ignored): the camera and the body turn to it.".into(),
            kind: targeted.clone(),
            channel: "look".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let t = target(v)?;
                    with_seat(w, b, |s| s.lock_request = Some(t))
                },
                read: |w, b| {
                    read_seat(w, b, |t| {
                        ControlValue::Number(t.lock_request.map_or(0.0, |id| id.to_f64()))
                    })
                },
                writable: has_seat,
            },
        });
        all.push(ControlDef {
            name: "interact_with".into(),
            doc: "Interact with this entity for one tick (the game's rules read it in ThirdPerson.interact_target).".into(),
            kind: targeted,
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let t = target(v)?;
                    with_seat(w, b, |s| s.interact_target = Some(t))
                },
                read: |w, b| {
                    read_seat(w, b, |t| {
                        ControlValue::Number(t.interact_target.map_or(0.0, |id| id.to_f64()))
                    })
                },
                writable: has_seat,
            },
        });
    }
    all
}

/// The third-person controls (player.md 5.1), the targeted pulses taking `kinds`.
pub fn controls(kinds: &[String]) -> Vec<ControlDef> {
    let mut seat = seat_controls(kinds);
    let mut walker = walker_controls();
    // Declared in the order a person reads them: the move, the look, the verbs, the buttons.
    let mut all: Vec<ControlDef> = seat.drain(..3).collect();
    all.append(&mut walker.drain(..2).collect());
    all.push(seat.remove(0)); // zoom
    all.append(&mut walker);
    all.append(&mut seat);
    all.extend(button_controls(has_seat));
    all
}

/// The catalog with the given bodies and views, the targeted pulses taking `kinds`.
pub fn catalog(bodies: Bodies, view: ViewFactory, kinds: &[String]) -> ActionCatalog {
    let mut c = ActionCatalog::empty("third_person", bodies, view);
    c.seats = vec![SeatDecl::any("player")];
    c.controls = controls(kinds);
    c
}

/// The catalog as a game runs it: the seat is the body whose observer names it, acting through its
/// observer's perception; the targeted pulses take every kind the perception declares, and the
/// kinds' affordances are those the declarations carry. Validated against `defs`.
pub fn game_catalog(defs: &PerceptionDefs) -> Result<ActionCatalog, Problem> {
    let kinds: Vec<String> = defs.decl.kinds.iter().map(|k| k.kind.clone()).collect();
    let mut c = catalog(
        crate::perception::bridge::bodies,
        crate::perception::bridge::view_factory(),
        &kinds,
    );
    c.affordances = super::catalog::affordances_from(defs)?;
    c.validate(Some(defs))?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controls_are_declared_once_in_reading_order() {
        let names: Vec<String> = controls(&["foe".into()])
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            [
                "forward",
                "strafe",
                "frame",
                "yaw",
                "pitch",
                "zoom",
                "run",
                "jump",
                "primary",
                "secondary",
                "guard",
                "dodge",
                "interact",
                "lock",
                "lock_on",
                "interact_with",
                "held",
                "pressed",
            ]
        );
        let without: Vec<String> = controls(&[]).into_iter().map(|c| c.name).collect();
        assert!(
            !without
                .iter()
                .any(|n| n == "lock_on" || n == "interact_with")
        );
    }
}
