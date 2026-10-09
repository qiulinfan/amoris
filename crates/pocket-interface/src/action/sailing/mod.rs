//! The sailing game's action declarations (shared/contract/sailing.md, Actions and Codes): the
//! seat `skipper`, the controls `rudder`, `sheet`, `hoist` and `interact`, the intents
//! `come_to_heading`, `trim_sail` and `sail_to` with their Rust executors, the crate's
//! `take_aboard` affordance, and the `sail.*` codes. [`catalog`] assembles them.

pub mod helm;
pub mod instruments;
pub mod sail_to;
pub mod trim;

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_contract::{Detail, Problem, Shape, Use};
use pocket_physics::Boat;
use pocket_sim::{EntityId, PlainData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::catalog::{ActionCatalog, Bodies, IntentEntry, SeatDecl, ViewFactory};
use super::defs::{
    AffordanceDef, CodeDef, ControlBinding, ControlDef, ControlKind, Effect, IntentDef,
    ProgressDef, Requirement, TargetSpec,
};
use super::state::ControlValue;
use super::view::{self, ActorView};
use crate::perception::Unit;

/// Below this speed through the water the rudder has too little way to steer, m/s.
pub const MIN_STEERAGE_MPS: f64 = 0.3;
/// How long a boat may lack steerage off its heading before `come_to_heading` fails, seconds.
pub const STEERAGE_S: f64 = 10.0;
/// How much nearer `sail_to` must come ...
pub const NO_PROGRESS_M: f64 = 5.0;
/// ... within this many seconds.
pub const NO_PROGRESS_S: f64 = 60.0;
/// How near a crate must float to be taken aboard (master's island `REACH`).
pub const REACH_M: f64 = 3.0;

/// A number in an executor's state.
pub fn n(state: &PlainData, key: &str) -> Option<f64> {
    match state.get(key)? {
        PlainData::Number(x) => Some(*x),
        _ => None,
    }
}

/// A string in an executor's state.
pub fn text_of(state: &PlainData, key: &str) -> Option<String> {
    match state.get(key)? {
        PlainData::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Sets `key` in an executor's state (an object, keys kept in byte order).
pub fn put(state: &mut PlainData, key: &str, value: PlainData) {
    let mut entries = match std::mem::take(state) {
        PlainData::Object(e) => e,
        _ => Vec::new(),
    };
    match entries.binary_search_by(|(k, _)| k.as_bytes().cmp(key.as_bytes())) {
        Ok(i) => entries[i].1 = value,
        Err(i) => entries.insert(i, (key.to_owned(), value)),
    }
    *state = PlainData::Object(entries);
}

/// The sailing code `code` over `detail`, its message from the code's template.
pub fn sail_problem(catalog: &ActionCatalog, code: &str, detail: Detail) -> Problem {
    catalog.problem(code, detail)
}

/// `sail.aground`: the nearest island the seat knows, and where the boat is.
pub fn aground_problem(catalog: &ActionCatalog, v: &dyn ActorView) -> Problem {
    let island = v
        .all_known()
        .into_iter()
        .filter(|k| k.kind == "island")
        .min_by(|a, b| a.range_m.total_cmp(&b.range_m).then(a.id.cmp(&b.id)));
    let pos = view::position(v, "pos_m").unwrap_or([0.0; 3]);
    let tenth = |x: f64| (x * 10.0).round() / 10.0;
    let island = island.map_or(json!("land"), |k| json!(k.entity_name()));
    sail_problem(
        catalog,
        "sail.aground",
        pocket_contract::detail([
            ("island", island),
            (
                "pos_m",
                json!({"x": tenth(pos[0]), "y": tenth(pos[1]), "z": tenth(pos[2])}),
            ),
        ]),
    )
}

/// Which way `come_to_heading` turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    #[default]
    Shortest,
    Port,
    Starboard,
}

/// `come_to_heading`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComeToHeadingParams {
    /// The heading to come to, degrees from 0 up to but not including 360.
    #[schemars(range(min = 0.0), extend("exclusiveMaximum" = 360))]
    pub heading_deg: f64,
    /// How close counts as on the heading, degrees (default 5).
    #[serde(default)]
    #[schemars(range(min = 0.5, max = 45.0))]
    pub tolerance_deg: Option<f64>,
    /// Which way to turn (default shortest).
    #[serde(default)]
    pub turn: Option<Turn>,
    /// How long the heading must stay within tolerance, seconds (default 2).
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 30.0))]
    pub settle_s: Option<f64>,
    /// Hold the heading once reached (default false).
    #[serde(default)]
    pub keep: Option<bool>,
    /// Until it must be reached, seconds (default 60).
    #[serde(default)]
    #[schemars(range(min = 1.0, max = 600.0))]
    pub timeout_s: Option<f64>,
}

/// `"best"`: the best trim for the apparent wind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Best {
    Best,
}

/// A sheet target: a number from 0 to 1, or `"best"`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SheetTarget {
    Value(#[schemars(range(min = 0.0, max = 1.0))] f64),
    Best(Best),
}

/// `trim_sail`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrimSailParams {
    /// The sheet to trim to, 0 to 1, or "best" (default).
    #[serde(default)]
    pub sheet: Option<SheetTarget>,
    /// How much sail to set first, 0 to 1 (default: unchanged).
    #[serde(default)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub hoist: Option<f64>,
    /// How close the sheet must come (default 0.02).
    #[serde(default)]
    #[schemars(range(min = 0.005, max = 0.2))]
    pub tolerance: Option<f64>,
    /// Keep trimming once reached (default false).
    #[serde(default)]
    pub keep: Option<bool>,
    /// Until it must be reached, seconds (default 20).
    #[serde(default)]
    #[schemars(range(min = 1.0, max = 120.0))]
    pub timeout_s: Option<f64>,
}

/// Whether `sail_to` trims the sail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrimMode {
    #[default]
    Auto,
    Manual,
}

/// `sail_to`'s parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SailToParams {
    /// Within this distance of the target counts as arrived, metres (default 10).
    #[serde(default)]
    #[schemars(range(min = 1.0, max = 500.0))]
    pub arrive_m: Option<f64>,
    /// auto (default) also sets and trims the sail; manual leaves it to the caller.
    #[serde(default)]
    pub trim: Option<TrimMode>,
    /// Until it must arrive, seconds (default 900).
    #[serde(default)]
    #[schemars(range(min = 10.0, max = 3600.0))]
    pub timeout_s: Option<f64>,
}

fn boat_write(
    world: &mut World,
    body: EntityId,
    v: Option<&ControlValue>,
    f: fn(&mut Boat, f64),
) -> Result<(), Problem> {
    let e = pocket_sim::entity::require(world, body)?;
    let Some(ControlValue::Number(x)) = v else {
        return Err(super::state::internal(
            "sailing",
            "a boat control without a number",
        ));
    };
    let mut boat = world
        .get_mut::<Boat>(e)
        .ok_or_else(|| super::state::internal("sailing", "the seat's body has no Boat to steer"))?;
    f(&mut boat, *x);
    Ok(())
}

fn boat_read(world: &World, body: EntityId, f: fn(&Boat) -> f64) -> Option<ControlValue> {
    let e = pocket_sim::entity::entity(world, body)?;
    world.get::<Boat>(e).map(|b| ControlValue::Number(f(b)))
}

/// Whether `body` is a boat: the sailing controls are a `Boat`'s.
fn has_boat(world: &World, body: EntityId) -> bool {
    pocket_sim::entity::entity(world, body).is_some_and(|e| world.get::<Boat>(e).is_some())
}

fn axis(name: &str, doc: &str, min: f64, max: f64, default: f64, channel: &str) -> ControlKind {
    let _ = (name, doc, channel);
    ControlKind::Axis { min, max, default }
}

/// The sailing controls (sailing.md, Controls). `interact` delivers its target into `interact_to`
/// (`Crew.take` in the sample), which the game's rule reads.
pub fn controls(interact_to: (&'static str, &'static str)) -> Vec<ControlDef> {
    vec![
        ControlDef {
            name: "rudder".into(),
            doc: "The rudder's target, -1 to 1; positive turns the bow to starboard.".into(),
            kind: axis("rudder", "", -1.0, 1.0, 0.0, "helm"),
            channel: "helm".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| boat_write(w, b, v, |boat, x| boat.rudder = x),
                read: |w, b| boat_read(w, b, |boat| boat.rudder),
                writable: has_boat,
            },
        },
        ControlDef {
            name: "sheet".into(),
            doc: "The sheet's target, 0 hard in to 1 eased right out.".into(),
            kind: axis("sheet", "", 0.0, 1.0, 1.0, "sail"),
            channel: "sail".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| boat_write(w, b, v, |boat, x| boat.sheet = x),
                read: |w, b| boat_read(w, b, |boat| boat.sheet),
                writable: has_boat,
            },
        },
        ControlDef {
            name: "hoist".into(),
            doc: "How much sail to set, 0 furled to 1.".into(),
            kind: axis("hoist", "", 0.0, 1.0, 0.0, "sail"),
            channel: "sail".into(),
            binding: ControlBinding::Engine {
                write: |w, b, v| boat_write(w, b, v, |boat, x| boat.hoist = x),
                read: |w, b| boat_read(w, b, |boat| boat.hoist),
                writable: has_boat,
            },
        },
        ControlDef {
            name: "interact".into(),
            doc: "Take the targeted crate aboard (the crate's take_aboard).".into(),
            kind: ControlKind::Pulse {
                target_kinds: Some(vec!["crate".into()]),
            },
            channel: "hands".into(),
            binding: ControlBinding::Field {
                component: interact_to.0,
                field: interact_to.1,
                // Any body: the game's field writer gives a body the component it lacks.
                writable: |_, _| true,
            },
        },
    ]
}

fn progress(names: &[(&str, &str, Unit, u8)]) -> Vec<ProgressDef> {
    names
        .iter()
        .map(|(n, d, u, p)| ProgressDef {
            name: (*n).into(),
            doc: (*d).into(),
            unit: u.clone(),
            precision: *p,
        })
        .collect()
}

fn names(values: &[&str]) -> Unit {
    Unit::Enum {
        values: values.iter().map(|v| (*v).to_owned()).collect(),
    }
}

fn fraction() -> Unit {
    Unit::Fraction { min: 0.0, max: 1.0 }
}

/// The sailing intents' declarations.
pub fn intents() -> Vec<IntentDef> {
    vec![
        IntentDef {
            name: "come_to_heading".into(),
            doc: "Turn the boat onto a heading and, with keep, hold it there (an autopilot). Succeeds \
                  when within tolerance_deg for settle_s; fails aground, without steerage (in irons) \
                  for 10 s, or on timeout."
                .into(),
            params: Shape::of::<ComeToHeadingParams>(),
            defaults: helm::defaults(),
            target: TargetSpec::None,
            channels: vec!["helm".into()],
            can_hold: true,
            default_timeout_s: 60.0,
            failures: vec!["sail.aground".into(), "sail.lost_steerage".into(), "intent.timeout".into()],
            progress: progress(&[
                ("heading_error_deg", "Signed degrees from the heading to the target.", Unit::Angle, 0),
                ("turning", "port, starboard or steady.", names(&["port", "starboard", "steady"]), 0),
                ("on_heading_s", "Seconds within tolerance so far.", Unit::Seconds, 1),
            ]),
        },
        IntentDef {
            name: "trim_sail".into(),
            doc: "Set the sail and trim the sheet to a value or to the best for the wind; with keep, \
                  go on trimming as the wind changes. Refused with sail.not_set when furled without \
                  hoist."
                .into(),
            params: Shape::of::<TrimSailParams>(),
            defaults: trim::defaults(),
            target: TargetSpec::None,
            channels: vec!["sail".into()],
            can_hold: true,
            default_timeout_s: 20.0,
            failures: vec!["intent.timeout".into()],
            progress: progress(&[
                ("sheet", "The sheet now, 0 to 1.", fraction(), 2),
                ("target_sheet", "The sheet aimed at, 0 to 1.", fraction(), 2),
                ("trim", "furled, luffing, good or overtrimmed.", names(&["furled", "luffing", "good", "overtrimmed"]), 0),
                ("drive", "The sail's drive as a share of the best.", fraction(), 2),
            ]),
        },
        IntentDef {
            name: "sail_to".into(),
            doc: "Sail to a point or a known mark, boat or crate: straight when the wind allows, \
                  beating in tacks when it does not, trimming the sail unless trim is manual. Does \
                  not steer round land. Fails aground, without progress (5 m in 60 s), when the \
                  target is lost, or on timeout."
                .into(),
            params: Shape::of::<SailToParams>(),
            defaults: sail_to::defaults(),
            target: TargetSpec::EntityOrPoint {
                kinds: vec!["mark".into(), "boat".into(), "crate".into()],
            },
            channels: vec!["helm".into(), "sail".into()],
            can_hold: false,
            default_timeout_s: 900.0,
            failures: vec![
                "sail.aground".into(),
                "sail.no_progress".into(),
                "perception.target_lost".into(),
                "intent.timeout".into(),
            ],
            progress: progress(&[
                ("distance_m", "Metres to the target.", Unit::Metres, 0),
                ("bearing_deg", "The target's bearing.", Unit::Bearing, 0),
                ("vmg_mps", "Velocity made good toward the target.", Unit::MetresPerSecond, 1),
                ("eta_s", "Seconds to arrive at this vmg.", Unit::Seconds, 0),
                ("leg", "direct, port_tack or starboard_tack.", names(&["direct", "port_tack", "starboard_tack"]), 0),
                ("tacks", "Tacks so far.", Unit::Count, 0),
            ]),
        },
    ]
}

/// The crate's `take_aboard` (sailing.md, Taking a crate aboard).
pub fn take_aboard() -> AffordanceDef {
    AffordanceDef {
        verb: "take_aboard".into(),
        doc: "Take the crate aboard; it must be seen and within 3 m.".into(),
        effect: Effect::Pulse {
            control: "interact".into(),
        },
        requires: vec![Requirement::Seen, Requirement::Within { max_m: REACH_M }],
    }
}

fn code(code: &str, uses: &[Use], when: &str, detail: &[&str], template: &str) -> CodeDef {
    CodeDef {
        code: code.into(),
        uses: uses.to_vec(),
        when: when.into(),
        detail: detail.iter().map(|s| (*s).to_owned()).collect(),
        template: template.into(),
    }
}

/// The sailing game's codes (sailing.md, Codes).
pub fn codes() -> Vec<CodeDef> {
    vec![
        code(
            "sail.no_go_zone",
            &[Use::Warn],
            "come_to_heading toward within NO_GO_HALF_DEG of the wind.",
            &[
                "heading_deg",
                "wind_from_deg",
                "off_wind_deg",
                "no_go_half_deg",
            ],
            "Heading {heading_deg} is {off_wind_deg} degrees off the wind (from {wind_from_deg}); \
             within {no_go_half_deg} the sail cannot drive and the boat will stop.",
        ),
        code(
            "sail.not_set",
            &[Use::Refuse, Use::Warn],
            "The sail is furled where it is needed.",
            &["hoist", "suggest"],
            "The sail is furled; hoist it first, for example with \"hoist\": 1.",
        ),
        code(
            "sail.target_on_land",
            &[Use::Refuse],
            "sail_to a point within a known island's chart radius.",
            &["island", "radius_m", "distance_m"],
            "That point is on {island}, {distance_m} m from its centre within its {radius_m} m \
             radius.",
        ),
        code(
            "sail.land_on_course",
            &[Use::Warn],
            "The straight course passes within a known island's radius.",
            &["island", "along_m"],
            "{island} lies on the direct course {along_m} m ahead; sail_to does not steer round \
             land.",
        ),
        code(
            "sail.aground",
            &[Use::Fail],
            "The boat touched land.",
            &["island", "pos_m"],
            "The boat ran aground on {island}.",
        ),
        code(
            "sail.lost_steerage",
            &[Use::Fail],
            "Too little way to steer, off the heading, for STEERAGE_S.",
            &["speed_mps", "heading_error_deg", "for_s"],
            "The boat lost steerage: {speed_mps} m/s, {heading_error_deg} degrees off for {for_s} \
             s; bear away from the wind to gather way.",
        ),
        code(
            "sail.no_progress",
            &[Use::Fail],
            "sail_to came no nearer by NO_PROGRESS_M in NO_PROGRESS_S.",
            &["distance_m", "needed_m", "window_s"],
            "No progress toward the target: still {distance_m} m away after {window_s} s.",
        ),
        code(
            "sail.crate_gone",
            &[Use::Warn],
            "An interact pulse found its crate gone at the boundary it applied.",
            &["crate"],
            "{crate} was no longer there to take aboard.",
        ),
    ]
}

/// The sailing game's catalog: the seat `skipper`, its controls (with `interact` bound to
/// `interact_to`), intents with their executors, the crate's affordance and the codes.
pub fn catalog(
    bodies: Bodies,
    view: ViewFactory,
    interact_to: (&'static str, &'static str),
) -> ActionCatalog {
    let mut c = ActionCatalog::empty("sailing", bodies, view);
    c.seats = vec![SeatDecl::any("skipper")];
    c.controls = controls(interact_to);
    let mut defs = intents().into_iter();
    let mut next = || defs.next().unwrap_or_else(unreachable_def);
    let helm: Arc<dyn super::executor::IntentExecutor> =
        Arc::new(helm::ComeToHeading { def: next() });
    let trim: Arc<dyn super::executor::IntentExecutor> = Arc::new(trim::TrimSail { def: next() });
    let to: Arc<dyn super::executor::IntentExecutor> = Arc::new(sail_to::SailTo { def: next() });
    c.intents = [helm, trim, to]
        .into_iter()
        .map(|x| IntentEntry {
            def: x.def().clone(),
            executor: Some(x),
        })
        .collect();
    c.affordances = BTreeMap::from([("crate".to_owned(), vec![take_aboard()])]);
    c.codes = codes();
    c
}

/// The sailing game as a game runs it: seats are the bodies whose observer names them, each seat
/// acts through its observer's perception (`perception::bridge`), `interact` is delivered into
/// the project's `Crew.take`, and the crate's affordances are those the perception declarations
/// carry on its kind (`samples/sailing/perception.json`). Validated against `defs`.
pub fn game_catalog(defs: &crate::perception::PerceptionDefs) -> Result<ActionCatalog, Problem> {
    let mut c = catalog(
        crate::perception::bridge::bodies,
        crate::perception::bridge::view_factory(),
        ("Crew", "take"),
    );
    c.affordances = super::catalog::affordances_from(defs)?;
    c.validate(Some(defs))?;
    Ok(c)
}

fn unreachable_def() -> IntentDef {
    IntentDef {
        name: "none".into(),
        doc: String::new(),
        params: Shape::from_schema(json!(true)),
        defaults: serde_json::Map::new(),
        target: TargetSpec::None,
        channels: Vec::new(),
        can_hold: false,
        default_timeout_s: 1.0,
        failures: Vec::new(),
        progress: Vec::new(),
    }
}
