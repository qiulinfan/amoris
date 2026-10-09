//! `sail_to` (shared/contract/sailing.md, `sail_to`): sail to a point or an entity the seat knows,
//! straight when the wind allows and beating in tacks inside a corridor when it does not, trimming
//! the sail unless told not to. It steers as `come_to_heading` does, toward the heading its leg
//! asks for, and never plans round land (actions.md, open choice 3): it refuses a target on a known
//! island and warns of one on the course.

use pocket_contract::{Problem, detail};
use pocket_physics::best_sheet;
use pocket_sim::PlainData;
use serde_json::{Value, json};

use super::helm::{aground, steer};
use super::instruments::range_bearing;
use super::{NO_PROGRESS_M, NO_PROGRESS_S, aground_problem, n, put, sail_problem, text_of};
use crate::action::affordance::relative;
use crate::action::catalog::{param_f64, param_str};
use crate::action::defs::IntentDef;
use crate::action::executor::{AcceptCx, Accepted, IntentExecutor, Step, TickCx};
use crate::action::state::ResolvedTarget;
use crate::action::view::{self, ActorView, Known};

/// The angle off the true wind `sail_to` beats at, and below which it beats rather than steering
/// straight for the target (Slice 2, sailing.md's open choices): the slice 1 boat's best velocity
/// made good to windward, measured at 6 m/s (`tests/explore.rs`, `explore_polar`: 0.53 m/s at 50
/// degrees, where it stalls and falls off, 0.58 to 0.62 from 60 to 65). The simulation's own
/// `CLOSE_HAULED_DEG` (50) is the angle it can point, not the one it sails best at.
pub const BEAT_DEG: f64 = 60.0;

/// `sail_to`.
pub struct SailTo {
    pub def: IntentDef,
}

/// The target's position as the seat knows it now, or `None` when it is no longer known.
fn target_pos(v: &dyn ActorView, t: Option<&ResolvedTarget>) -> Option<[f64; 3]> {
    match t? {
        ResolvedTarget::Point(p) => Some(*p),
        ResolvedTarget::Entity(e) => v.known(*e).map(|k| k.pos_m),
    }
}

/// The known islands: their chart centre and radius.
fn islands(v: &dyn ActorView) -> Vec<(Known, f64)> {
    v.all_known()
        .into_iter()
        .filter(|k| k.kind == "island")
        .filter_map(|k| {
            let r = k.fact("radius_m").and_then(Value::as_f64)?;
            Some((k, r))
        })
        .collect()
}

/// The distance along the segment `a`-`b` to the point nearest `c`, and that nearest distance.
fn along(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> (f64, f64) {
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 0.0 {
        pocket_sim::math::clamp(((c[0] - a[0]) * dx + (c[2] - a[2]) * dz) / len2, 0.0, 1.0)
    } else {
        0.0
    };
    let (px, pz) = (a[0] + t * dx, a[2] + t * dz);
    let (ex, ez) = (c[0] - px, c[2] - pz);
    (
        t * pocket_sim::math::sqrt(len2),
        pocket_sim::math::sqrt(ex * ex + ez * ez),
    )
}

fn round(x: f64) -> f64 {
    x.round()
}

fn progress(dist: f64, brg: f64, vmg: f64, leg: &str, tacks: f64) -> Vec<(String, PlainData)> {
    let mut p = vec![
        ("distance_m".into(), PlainData::Number(dist)),
        ("bearing_deg".into(), PlainData::Number(brg)),
        ("vmg_mps".into(), PlainData::Number(vmg)),
    ];
    if vmg > 0.1 {
        p.push(("eta_s".into(), PlainData::Number(dist / vmg)));
    }
    p.push(("leg".into(), PlainData::String(leg.into())));
    p.push(("tacks".into(), PlainData::Number(tacks)));
    p
}

/// How long the leeway estimate takes to follow the boat, seconds.
const LEEWAY_S: f64 = 5.0;
/// The hysteresis of the change from beating to sailing straight for the target, degrees.
const FETCH_HYSTERESIS_DEG: f64 = 5.0;

/// The leeway, the signed angle from the heading to the course over the ground, smoothed in the
/// state: the slice 1 boat makes about 14 degrees of it close-hauled. Below steerage way the
/// course says nothing and the estimate is kept.
fn leeway(state: &mut PlainData, v: &dyn ActorView, heading: f64, speed: f64, dt: f64) -> f64 {
    let kept = n(state, "lee").unwrap_or(0.0);
    let lee = match view::number(v, "course_deg") {
        Some(course) if speed > 0.5 => {
            let now = relative(course, heading);
            kept + (now - kept) * pocket_sim::math::min(1.0, dt / LEEWAY_S)
        }
        _ => kept,
    };
    put(state, "lee", PlainData::Number(lee));
    lee
}

/// The heading the leg asks for, and the leg's name. Straight for the target, the heading set so
/// the course over the ground (heading plus leeway) points at it, when the target can be fetched:
/// its bearing is at least [`BEAT_DEG`] plus the leeway off the wind (the layline). Otherwise
/// close-hauled on the tack kept in the state, tacking at the corridor's edge.
fn leg(
    state: &mut PlainData,
    wind_from: f64,
    brg: f64,
    dist: f64,
    arrive: f64,
    lee: f64,
) -> (f64, String) {
    let off = relative(brg, wind_from);
    let direct = text_of(state, "beat").is_none() && n(state, "tacks").is_some();
    let fetch = BEAT_DEG + lee.abs() - if direct { FETCH_HYSTERESIS_DEG } else { 0.0 };
    if off.abs() >= fetch {
        put(state, "beat", PlainData::Null);
        return (super::instruments::bearing(brg - lee), "direct".into());
    }
    // Starboard tack: the wind over the starboard bow, heading `wind - BEAT_DEG`; port: `+`.
    let starboard = wind_from - BEAT_DEG;
    let port = wind_from + BEAT_DEG;
    let mut tack = match text_of(state, "beat") {
        Some(t) => t,
        None => {
            if relative(brg, starboard).abs() <= relative(brg, port).abs() {
                "starboard_tack".to_owned()
            } else {
                "port_tack".to_owned()
            }
        }
    };
    // The corridor: the distance from the line through the target along the wind. And a tack
    // whose course over the ground no longer closes on the target (near it, the corridor is
    // wider than the distance) gives way to the other.
    let lateral = dist * pocket_sim::math::sin(off.to_radians());
    let limit = pocket_sim::math::max(4.0 * arrive, 0.25 * dist);
    let closing = |course: f64| pocket_sim::math::cos(relative(brg, course).to_radians()) > 0.05;
    if tack == "starboard_tack" && (lateral > limit || !closing(starboard + lee)) {
        tack = "port_tack".into();
    } else if tack == "port_tack" && (-lateral > limit || !closing(port + lee)) {
        tack = "starboard_tack".into();
    }
    put(state, "beat", PlainData::String(tack.clone()));
    let heading = if tack == "starboard_tack" {
        starboard
    } else {
        port
    };
    (super::instruments::bearing(heading), tack)
}

impl IntentExecutor for SailTo {
    fn def(&self) -> &IntentDef {
        &self.def
    }

    fn channels(&self, params: &PlainData) -> Vec<String> {
        let mut c = vec!["helm".to_owned()];
        if param_str(params, "trim") != Some("manual") {
            c.push("sail".into());
        }
        c
    }

    fn accept(&self, cx: &AcceptCx<'_>) -> Result<Accepted, Vec<Problem>> {
        let pos = view::position(cx.view, "pos_m").unwrap_or([0.0; 3]);
        let Some(at) = target_pos(cx.view, cx.target) else {
            return Err(vec![crate::action::state::internal(
                "sail_to",
                "a validated target is not known",
            )]);
        };
        let mut warnings = Vec::new();
        for (isle, r) in islands(cx.view) {
            let (dist, _) = range_bearing(isle.pos_m, at);
            if matches!(cx.target, Some(ResolvedTarget::Point(_))) && dist <= r {
                return Err(vec![sail_problem(
                    cx.catalog,
                    "sail.target_on_land",
                    detail([
                        ("island", json!(isle.entity_name())),
                        ("radius_m", json!(round(r))),
                        ("distance_m", json!(round(dist))),
                    ]),
                )]);
            }
            let (along_m, off_m) = along(pos, at, isle.pos_m);
            if off_m <= r {
                warnings.push(sail_problem(
                    cx.catalog,
                    "sail.land_on_course",
                    detail([
                        ("island", json!(isle.entity_name())),
                        ("along_m", json!(round(along_m))),
                    ]),
                ));
            }
        }
        if param_str(cx.params, "trim") == Some("manual")
            && view::number(cx.view, "hoist").unwrap_or(0.0) <= 0.0
        {
            warnings.push(super::trim::not_set(cx.catalog));
        }
        let (dist, brg) = range_bearing(pos, at);
        Ok(Accepted {
            state: PlainData::Object(Vec::new()),
            progress: progress(dist, brg, 0.0, "direct", 0.0),
            warnings,
        })
    }

    fn tick(&self, cx: &mut TickCx<'_, '_>) -> Step {
        let p = &cx.instance.params;
        let arrive = param_f64(p, "arrive_m").unwrap_or(10.0);
        let pos = view::position(cx.view, "pos_m").unwrap_or([0.0; 3]);
        let Some(at) = target_pos(cx.view, cx.instance.target.as_ref()) else {
            let target = cx
                .instance
                .target
                .and_then(|t| t.entity())
                .map_or(0, pocket_sim::EntityId::get);
            return Step::Failed {
                problem: pocket_contract::codes::target_lost(
                    &pocket_contract::codes::EntityName::new(target, None),
                ),
                progress: cx.instance.progress.clone(),
            };
        };
        let (dist, brg) = range_bearing(pos, at);
        let heading = view::number(cx.view, "heading_deg").unwrap_or(brg);
        let speed = view::number(cx.view, "speed_mps").unwrap_or(0.0);
        let course = view::number(cx.view, "course_deg").unwrap_or(heading);
        let vmg = speed * pocket_sim::math::cos(relative(brg, course).to_radians());
        let wind = view::number(cx.view, "wind_from_deg").unwrap_or(heading + 180.0);
        let lee = leeway(cx.state, cx.view, heading, speed, cx.dt);
        let (want, leg_name) = leg(cx.state, wind, brg, dist, arrive, lee);
        // A tack: the commanded heading crossed the wind's direction while close to it.
        let side = relative(want, wind).signum();
        let tacks = n(cx.state, "tacks").unwrap_or(0.0);
        let tacks = match n(cx.state, "side") {
            Some(s) if s != side && relative(want, wind).abs() < 90.0 => tacks + 1.0,
            _ => tacks,
        };
        put(cx.state, "side", PlainData::Number(side));
        put(cx.state, "tacks", PlainData::Number(tacks));
        let error = relative(want, heading);
        let twa = view::number(cx.view, "twa_deg").unwrap_or(0.0);
        let rudder = steer(cx.state, heading, twa, error, cx.dt);
        cx.controls.axis("rudder", rudder);
        if param_str(p, "trim") != Some("manual") {
            if view::number(cx.view, "hoist").unwrap_or(0.0) < 1.0 {
                cx.controls.axis("hoist", 1.0);
            }
            let awa = view::number(cx.view, "awa_deg").unwrap_or(180.0);
            cx.controls.axis("sheet", best_sheet(awa));
        }
        let prog = progress(dist, brg, vmg, &leg_name, tacks);
        if dist <= arrive {
            return Step::Succeeded { progress: prog };
        }
        if aground(cx.view, cx.instance.started_tick) {
            return Step::Failed {
                problem: aground_problem(cx.catalog, cx.view),
                progress: prog,
            };
        }
        // No progress: the distance must fall by NO_PROGRESS_M within each NO_PROGRESS_S.
        let now = cx.view.tick().to_f64() * cx.dt;
        let (mark_d, mark_t) = match (n(cx.state, "mark_d"), n(cx.state, "mark_t")) {
            (Some(d), Some(t)) => (d, t),
            _ => (dist, now),
        };
        let (mark_d, mark_t) = if dist <= mark_d - NO_PROGRESS_M {
            (dist, now)
        } else {
            (mark_d, mark_t)
        };
        put(cx.state, "mark_d", PlainData::Number(mark_d));
        put(cx.state, "mark_t", PlainData::Number(mark_t));
        if now - mark_t >= NO_PROGRESS_S {
            return Step::Failed {
                problem: sail_problem(
                    cx.catalog,
                    "sail.no_progress",
                    detail([
                        ("distance_m", json!(round(dist))),
                        ("needed_m", json!(NO_PROGRESS_M)),
                        ("window_s", json!(NO_PROGRESS_S)),
                    ]),
                ),
                progress: prog,
            };
        }
        Step::Continue { progress: prog }
    }
}

/// `sail_to`'s parameters' defaults.
pub fn defaults() -> serde_json::Map<String, Value> {
    let v = json!({"arrive_m": 10, "trim": "auto", "timeout_s": 900});
    v.as_object().cloned().unwrap_or_default()
}
