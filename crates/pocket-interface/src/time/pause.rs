//! `pause` and `resume` (shared/contract/time.md, Real time, Halts and Requests): the developer's
//! and the allowed seats' sources of pause in real-time pacing, and the end of a halt. Pauses and
//! halts are session state: they decide when ticks run, never what they compute, and are never
//! recorded.
//!
//! - **`pause`** (real time): a developer's pause, or the caller's seat's when the session allows it
//!   to pause (`Controller::may_pause`); any other seat is refused with `time.cannot_pause`. In
//!   another pacing nothing runs on its own, so `pause` is `time.wrong_mode`.
//! - **`resume`**: a developer's ends its pause and, in any pacing, a halt after a script failure;
//!   a seat's ends its own pause (real time only). A hot update that lands ends a halt too
//!   ([`Controller::hot_update_landed`]).
//!
//! Both answer the caller's `time.status`.

use bevy_ecs::prelude::World;
use pocket_contract::{CheckOptions, Problem, codes, decode};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::control::Controller;
use super::play::PlayPacing;
use super::session::seat_of;
use super::{Pacing, TimeModel};
use crate::action::request::Caller;

/// `pause` and `resume` take only the seat (a developer pausing for a seat names none).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PauseRequest {
    /// Player role: omitted or its own seat.
    pub seat: Option<String>,
}

fn structure(world: &World) -> String {
    world
        .get_resource::<super::turns::TimeRules>()
        .and_then(|r| serde_json::to_value(&r.structure).ok())
        .and_then(|v| {
            v.get("structure")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "continuous".into())
}

fn wrong_mode(world: &World, ctl: &Controller, request: &str) -> Problem {
    let takes: &[&str] = match ctl.pacing {
        PlayPacing::Stepped => &["step", "continue"],
        PlayPacing::Lockstep { .. } => &["commit", "continue"],
        PlayPacing::RealTime { .. } => &["pause", "resume", "continue"],
    };
    codes::time_wrong_mode(request, ctl.pacing.name(), &structure(world), takes)
}

/// The caller's `time.status` after the change.
fn status(world: &World, ctl: &Controller, caller: &Caller, now_ms: f64) -> Value {
    let rate = world.resource::<pocket_sim::SimClock>().rate;
    let model = TimeModel::new(rate, Pacing::Stepped);
    let seat = match caller {
        Caller::Player { seat } => Some(seat.as_str()),
        _ => None,
    };
    ctl.status(world, &model, seat, now_ms)
}

/// The seat a player pauses or resumes for: its own (a named other seat is `seat.not_yours`).
fn players_seat(
    world: &World,
    caller: &Caller,
    req: &PauseRequest,
    request: &str,
) -> Result<String, Problem> {
    let seat = seat_of(world, caller, req.seat.as_deref(), request)?;
    seat.ok_or_else(|| crate::action::state::internal(request, "a player without a seat"))
}

/// `pause` (time.md, Real time).
pub fn pause(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now_ms: f64,
) -> Result<Value, Problem> {
    let req: PauseRequest = decode(raw, &CheckOptions::new("the pause request"))?.value;
    if !matches!(ctl.pacing, PlayPacing::RealTime { .. }) {
        return Err(wrong_mode(world, ctl, "pause"));
    }
    match caller {
        Caller::Developer => ctl.paused_by_developer = true,
        Caller::Checker => return Err(codes::permission_denied("pause", "checker", "developer")),
        Caller::Player { .. } => {
            let seat = players_seat(world, caller, &req, "pause")?;
            if !ctl.may_pause.contains(&seat) {
                return Err(codes::time_cannot_pause(&seat));
            }
            ctl.paused_by_seats.insert(seat);
        }
    }
    Ok(status(world, ctl, caller, now_ms))
}

/// `resume` (time.md, Real time and Halts).
pub fn resume(
    world: &World,
    ctl: &mut Controller,
    caller: &Caller,
    raw: &Value,
    now_ms: f64,
) -> Result<Value, Problem> {
    let req: PauseRequest = decode(raw, &CheckOptions::new("the resume request"))?.value;
    let real_time = matches!(ctl.pacing, PlayPacing::RealTime { .. });
    match caller {
        Caller::Developer => ctl.developer_resumed(),
        Caller::Checker => {
            return Err(codes::permission_denied("resume", "checker", "developer"));
        }
        Caller::Player { .. } if !real_time => return Err(wrong_mode(world, ctl, "resume")),
        Caller::Player { .. } => {
            let seat = players_seat(world, caller, &req, "resume")?;
            if !ctl.may_pause.contains(&seat) {
                return Err(codes::time_cannot_pause(&seat));
            }
            ctl.paused_by_seats.remove(&seat);
        }
    }
    Ok(status(world, ctl, caller, now_ms))
}
