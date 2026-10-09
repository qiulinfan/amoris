//! The helm: the steering law every sailing intent shares, and `come_to_heading`
//! (shared/contract/sailing.md, `come_to_heading`). The law is the contract's PD term on the heading
//! error and the yaw rate, plus an integral term (Slice 2, sailing.md's open choices: the slice 1
//! boat rounds up with the rudder centred, needing about 0.45 of rudder to hold a close-hauled
//! course, so a PD autopilot would sail with a steady error; the integral trims it out, as a real
//! autopilot's rudder offset does). Weather helm mirrors with the tack, so the integral changes
//! sign when the wind comes over the other side, and a tack does not start with the wrong offset.

use pocket_contract::{Problem, detail};
use pocket_physics::boat::NO_GO_HALF_DEG;
use pocket_sim::PlainData;
use serde_json::{Value, json};

use super::{MIN_STEERAGE_MPS, STEERAGE_S, aground_problem, n, put, sail_problem};
use crate::action::affordance::relative;
use crate::action::catalog::{param_f64, param_str};
use crate::action::defs::IntentDef;
use crate::action::executor::{AcceptCx, Accepted, IntentExecutor, Step, TickCx};
use crate::action::view::{self, ActorView};

/// Proportional gain on `error / 45`.
pub const HELM_KP: f64 = 1.6;
/// Derivative gain on `yaw_rate_dps / 30`.
pub const HELM_KD: f64 = 1.2;
/// Integral gain on the error's integral in degree-seconds `/ 45`.
pub const HELM_KI: f64 = 0.8;
/// The integral's bound, as rudder.
pub const HELM_I_MAX: f64 = 0.6;

/// One tick of the steering law toward `error_deg` (positive: turn to starboard): the rudder.
/// `twa_deg` is the true wind angle off the bow, whose sign says the tack. The helm's state (the
/// last heading, the integral and the tack it was built on) lives in the executor's state.
pub fn steer(state: &mut PlainData, heading: f64, twa_deg: f64, error_deg: f64, dt: f64) -> f64 {
    let yaw = match n(state, "helm_last") {
        Some(last) if dt > 0.0 => relative(heading, last) / dt,
        _ => 0.0,
    };
    put(state, "helm_last", PlainData::Number(heading));
    let mut i = n(state, "helm_i").unwrap_or(0.0);
    // The wind came over the other side: the weather helm the integral learned mirrors.
    let tack = if twa_deg < 0.0 { -1.0 } else { 1.0 };
    if n(state, "helm_tack").is_some_and(|t| t != tack) {
        i = -i;
    }
    put(state, "helm_tack", PlainData::Number(tack));
    // The integral works only near the heading, so a long turn does not wind it up.
    if error_deg.abs() < 20.0 {
        i = pocket_sim::math::clamp(i + HELM_KI * error_deg * dt / 45.0, -HELM_I_MAX, HELM_I_MAX);
    }
    put(state, "helm_i", PlainData::Number(i));
    pocket_sim::math::clamp(
        HELM_KP * error_deg / 45.0 - HELM_KD * yaw / 30.0 + i,
        -1.0,
        1.0,
    )
}

/// The signed error from `heading` to `target` (positive: to starboard), honouring a forced turn
/// direction until the remaining error is within 90 degrees (kept in the state as `forced`).
pub fn error_toward(state: &mut PlainData, heading: f64, target: f64, turn: &str) -> f64 {
    let e = relative(target, heading);
    let forced = match n(state, "forced") {
        Some(f) => f,
        None => {
            let f = match turn {
                "starboard" if e < 0.0 => 1.0,
                "port" if e > 0.0 => -1.0,
                _ => 0.0,
            };
            put(state, "forced", PlainData::Number(f));
            f
        }
    };
    if forced == 0.0 {
        return e;
    }
    if e.abs() <= 90.0 && (e * forced >= 0.0) {
        put(state, "forced", PlainData::Number(0.0));
        return e;
    }
    // The long way round: keep turning the forced way.
    if forced > 0.0 {
        if e < 0.0 { e + 360.0 } else { e }
    } else if e > 0.0 {
        e - 360.0
    } else {
        e
    }
}

/// The steering progress readings.
fn progress(error: f64, rudder: f64, on_s: f64) -> Vec<(String, PlainData)> {
    let turning = if rudder > 0.05 {
        "starboard"
    } else if rudder < -0.05 {
        "port"
    } else {
        "steady"
    };
    vec![
        ("heading_error_deg".into(), PlainData::Number(error)),
        ("turning".into(), PlainData::String(turning.into())),
        ("on_heading_s".into(), PlainData::Number(on_s)),
    ]
}

/// `come_to_heading`.
pub struct ComeToHeading {
    pub def: IntentDef,
}

/// The warnings a start toward `heading` gets: `sail.no_go_zone` within the no-go angle of the
/// wind, `sail.not_set` with the sail furled and too little way to steer.
pub fn heading_warnings(cx: &AcceptCx<'_>, heading: f64) -> Vec<Problem> {
    let mut out = Vec::new();
    if let Some(from) = view::number(cx.view, "wind_from_deg") {
        let off = relative(heading, from).abs();
        if off < NO_GO_HALF_DEG {
            out.push(sail_problem(
                cx.catalog,
                "sail.no_go_zone",
                detail([
                    ("heading_deg", json!(heading)),
                    ("wind_from_deg", json!(from.round())),
                    ("off_wind_deg", json!(off.round())),
                    ("no_go_half_deg", json!(NO_GO_HALF_DEG)),
                ]),
            ));
        }
    }
    let hoist = view::number(cx.view, "hoist").unwrap_or(0.0);
    let speed = view::number(cx.view, "speed_mps").unwrap_or(0.0);
    if hoist <= 0.0 && speed.abs() < MIN_STEERAGE_MPS {
        out.push(sail_problem(
            cx.catalog,
            "sail.not_set",
            detail([("hoist", json!(0)), ("suggest", json!({"hoist": 1}))]),
        ));
    }
    out
}

/// Whether the boat has run aground since the intent started (the `afloat` instrument false after
/// the first physics step it saw).
pub fn aground(view: &dyn ActorView, started: pocket_sim::Tick) -> bool {
    view.tick() > started
        && view
            .instrument("afloat")
            .and_then(|v| v.as_bool())
            .is_some_and(|a| !a)
}

impl IntentExecutor for ComeToHeading {
    fn def(&self) -> &IntentDef {
        &self.def
    }

    fn accept(&self, cx: &AcceptCx<'_>) -> Result<Accepted, Vec<Problem>> {
        let heading = param_f64(cx.params, "heading_deg").unwrap_or(0.0);
        let current = view::number(cx.view, "heading_deg").unwrap_or(heading);
        let error = relative(heading, current);
        Ok(Accepted {
            state: PlainData::Object(Vec::new()),
            progress: progress(error, 0.0, 0.0),
            warnings: heading_warnings(cx, heading),
        })
    }

    fn tick(&self, cx: &mut TickCx<'_, '_>) -> Step {
        let p = &cx.instance.params;
        let target = param_f64(p, "heading_deg").unwrap_or(0.0);
        let tolerance = param_f64(p, "tolerance_deg").unwrap_or(5.0);
        let settle = param_f64(p, "settle_s").unwrap_or(2.0);
        let turn = param_str(p, "turn").unwrap_or("shortest");
        let heading = view::number(cx.view, "heading_deg").unwrap_or(target);
        let speed = view::number(cx.view, "speed_mps").unwrap_or(0.0);
        let error = error_toward(cx.state, heading, target, turn);
        let twa = view::number(cx.view, "twa_deg").unwrap_or(0.0);
        let rudder = steer(cx.state, heading, twa, error, cx.dt);
        cx.controls.axis("rudder", rudder);
        let within = error.abs() <= tolerance;
        let on_s = if within {
            n(cx.state, "on_s").unwrap_or(0.0) + cx.dt
        } else {
            0.0
        };
        put(cx.state, "on_s", PlainData::Number(on_s));
        let prog = progress(error, rudder, on_s);
        if aground(cx.view, cx.instance.started_tick) {
            return Step::Failed {
                problem: aground_problem(cx.catalog, cx.view),
                progress: prog,
            };
        }
        let slow_s = if speed.abs() < MIN_STEERAGE_MPS && !within {
            n(cx.state, "slow_s").unwrap_or(0.0) + cx.dt
        } else {
            0.0
        };
        put(cx.state, "slow_s", PlainData::Number(slow_s));
        if slow_s >= STEERAGE_S {
            return Step::Failed {
                problem: sail_problem(
                    cx.catalog,
                    "sail.lost_steerage",
                    detail([
                        ("speed_mps", json!((speed * 10.0).round() / 10.0)),
                        ("heading_error_deg", json!(error.round())),
                        ("for_s", json!(STEERAGE_S)),
                    ]),
                ),
                progress: prog,
            };
        }
        let held = cx.instance.status == crate::action::state::IntentStatus::Holding;
        if !held && on_s >= settle {
            return Step::Reached { progress: prog };
        }
        Step::Continue { progress: prog }
    }
}

/// `come_to_heading`'s parameters' defaults (sailing.md).
pub fn defaults() -> serde_json::Map<String, Value> {
    let v = json!({"tolerance_deg": 5, "turn": "shortest", "settle_s": 2, "keep": false,
                   "timeout_s": 60});
    v.as_object().cloned().unwrap_or_default()
}
