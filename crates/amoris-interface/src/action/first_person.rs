//! The `first_person` catalog (docs/spec/voxels.md 6): one seat, `player`, whose body carries a
//! `Walker`; its controls write the walker's inputs, its pulses act for one tick. No intents,
//! affordances or codes: the game's rules give `primary` and `secondary` their meaning.

use amoris_contract::Problem;
use amoris_physics::Walker;
use amoris_sim::EntityId;
use bevy_ecs::prelude::World;

use super::catalog::{ActionCatalog, Bodies, SeatDecl, ViewFactory};
use super::defs::{ControlBinding, ControlDef, ControlKind};
use super::state::ControlValue;
use crate::perception::PerceptionDefs;

/// The hotbar's slots, as the `slot` control names them.
pub const SLOTS: [&str; 9] = ["0", "1", "2", "3", "4", "5", "6", "7", "8"];

fn has_walker(world: &World, body: EntityId) -> bool {
    amoris_sim::entity::entity(world, body).is_some_and(|e| world.get::<Walker>(e).is_some())
}

fn with_walker(
    world: &mut World,
    body: EntityId,
    f: impl FnOnce(&mut Walker),
) -> Result<(), Problem> {
    let e = amoris_sim::entity::require(world, body)?;
    let mut w = world
        .get_mut::<Walker>(e)
        .ok_or_else(|| super::state::internal("first_person", "the seat's body has no Walker"))?;
    f(&mut w);
    Ok(())
}

fn number(v: Option<&ControlValue>) -> Result<f64, Problem> {
    match v {
        Some(ControlValue::Number(x)) => Ok(*x),
        _ => Err(super::state::internal(
            "first_person",
            "an axis without a number",
        )),
    }
}

fn flag(v: Option<&ControlValue>) -> Result<bool, Problem> {
    match v {
        Some(ControlValue::Bool(b)) => Ok(*b),
        _ => Err(super::state::internal(
            "first_person",
            "a toggle without a boolean",
        )),
    }
}

fn read(world: &World, body: EntityId, f: fn(&Walker) -> ControlValue) -> Option<ControlValue> {
    let e = amoris_sim::entity::entity(world, body)?;
    world.get::<Walker>(e).map(f)
}

macro_rules! axis {
    ($name:literal, $doc:literal, $min:expr, $max:expr, $field:ident) => {
        ControlDef {
            name: $name.into(),
            doc: $doc.into(),
            kind: ControlKind::Axis {
                min: $min,
                max: $max,
                default: 0.0,
            },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = number(v)?;
                    with_walker(w, b, |walker| walker.$field = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Number(walker.$field)),
                writable: has_walker,
            },
        }
    };
}

macro_rules! pulse {
    ($name:literal, $doc:literal, $field:ident) => {
        ControlDef {
            name: $name.into(),
            doc: $doc.into(),
            kind: ControlKind::Pulse { target_kinds: None },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, _| with_walker(w, b, |walker| walker.$field = true),
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.$field)),
                writable: has_walker,
            },
        }
    };
}

/// The first-person controls (voxels.md 6).
pub fn controls() -> Vec<ControlDef> {
    vec![
        axis!(
            "forward",
            "Walk forward, -1 (back) to 1.",
            -1.0,
            1.0,
            forward
        ),
        axis!(
            "strafe",
            "Walk sideways, -1 (left) to 1 (right).",
            -1.0,
            1.0,
            strafe
        ),
        axis!(
            "yaw",
            "Turn, radians about +y: 0 looks along -z, positive turns left.",
            -std::f64::consts::PI,
            std::f64::consts::PI,
            yaw
        ),
        axis!(
            "pitch",
            "Look up (positive) or down, radians.",
            -amoris_physics::walker::MAX_PITCH,
            amoris_physics::walker::MAX_PITCH,
            pitch
        ),
        ControlDef {
            name: "run".into(),
            doc: "Run instead of walking.".into(),
            kind: ControlKind::Toggle { default: false },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = flag(v)?;
                    with_walker(w, b, |walker| walker.run = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.run)),
                writable: has_walker,
            },
        },
        ControlDef {
            name: "rise".into(),
            doc: "Held: swim up in a liquid.".into(),
            kind: ControlKind::Toggle { default: false },
            channel: "body".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let x = flag(v)?;
                    with_walker(w, b, |walker| walker.rise = x)
                },
                read: |w, b| read(w, b, |walker| ControlValue::Bool(walker.rise)),
                writable: has_walker,
            },
        },
        ControlDef {
            name: "slot".into(),
            doc: "The selected hotbar slot, \"0\" to \"8\".".into(),
            kind: ControlKind::Choice {
                values: SLOTS.iter().map(|s| (*s).to_owned()).collect(),
                default: "0".into(),
            },
            channel: "hands".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| {
                    let slot = match v {
                        Some(ControlValue::Name(n)) => SLOTS.iter().position(|s| s == n),
                        _ => None,
                    }
                    .and_then(|i| u32::try_from(i).ok())
                    .ok_or_else(|| super::state::internal("first_person", "an unknown slot"))?;
                    with_walker(w, b, |walker| walker.slot = slot)
                },
                read: |w, b| {
                    read(w, b, |walker| {
                        let i = usize::try_from(walker.slot).unwrap_or(0).min(8);
                        ControlValue::Name(SLOTS[i].to_owned())
                    })
                },
                writable: has_walker,
            },
        },
        pulse!("jump", "Jump, if standing on something.", jump),
        pulse!(
            "primary",
            "The primary action for one tick (the block game: break the targeted block).",
            primary
        ),
        pulse!(
            "secondary",
            "The secondary action for one tick (the block game: place a block).",
            secondary
        ),
    ]
}

/// The catalog with the given bodies and views.
pub fn catalog(bodies: Bodies, view: ViewFactory) -> ActionCatalog {
    let mut c = ActionCatalog::empty("first_person", bodies, view);
    c.seats = vec![SeatDecl::any("player")];
    c.controls = controls();
    c
}

/// The catalog as a game runs it: the seat is the body whose observer names it, acting through its
/// observer's perception. Validated against `defs`.
pub fn game_catalog(defs: &PerceptionDefs) -> Result<ActionCatalog, Problem> {
    let c = catalog(
        crate::perception::bridge::bodies,
        crate::perception::bridge::view_factory(),
    );
    c.validate(Some(defs))?;
    Ok(c)
}
