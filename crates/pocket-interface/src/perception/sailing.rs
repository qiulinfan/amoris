//! The sailing showcase's derived facts and instruments (shared/contract/sailing.md, The skipper's
//! perception): what the skipper reads that is not a component field as it stands (the true wind
//! at the boat, the point of sail, the tack, the trim's name, the course over the ground, the next
//! mark, the crates left) and the observer-relative facts of islands and crates. The declarations
//! that name them are game data (`samples/sailing/perception.json`); these are the engine-provided
//! functions beside the sailing systems (README, The game definition).

use pocket_physics::geom::{self, V3};
use pocket_physics::query::wind_at;
use pocket_physics::{Boat, Collider, PartShape, Shape, Trim, Velocity};
use pocket_sim::{EntityIndex, math};

use super::defs::{PerceptionDefs, Vec3};
use super::facts::{FactCx, live, name_of, project_field};
use super::geometry::{bearing_deg, heading_deg, relative_deg};
use super::state::{FactValue, Observer, Perceivable};

/// How near a crate must float, across the water, to be taken aboard (master's island `REACH`;
/// the sample's `take_aboard` rule).
pub const REACH_M: f64 = 3.0;
/// How far above or below.
pub const REACH_UP_M: f64 = 3.0;

/// The skipper's seat and profile (sailing.md: one seat, `skipper`, whose body is its boat).
pub const SKIPPER: &str = "skipper";

/// The observer on the skipper's boat.
pub fn skipper() -> Observer {
    Observer {
        profile: SKIPPER.to_owned(),
        seat: Some(SKIPPER.to_owned()),
        omniscient: false,
    }
}

fn perceivable(
    kind: &str,
    detect_m: f64,
    height_m: f64,
    priority: u8,
    chart: Option<V3>,
) -> Perceivable {
    Perceivable {
        kind: kind.to_owned(),
        detect_m,
        height_m,
        priority,
        chart_m: chart.map(Vec3::of),
    }
}

/// An island as sailing.md's kinds table makes it perceivable: seen from 3000 m, its terrain's
/// height, priority 50, on the chart at its centre. Islands also carry `Occluder`.
pub fn island(centre: V3, height_m: f64) -> Perceivable {
    perceivable("island", 3000.0, height_m, 50, Some(centre))
}

/// A course mark: seen from 400 m, 2 m tall, priority 80, on the chart where it is laid.
pub fn mark(laid: V3) -> Perceivable {
    perceivable("mark", 400.0, 2.0, 80, Some(laid))
}

/// Another boat: seen from 1500 m, 6 m tall (its mast), priority 70, not charted.
pub fn boat() -> Perceivable {
    perceivable("boat", 1500.0, 6.0, 70, None)
}

/// A crate adrift: seen from 120 m, 0.5 m tall, priority 40, not charted.
pub fn adrift() -> Perceivable {
    perceivable("crate", 120.0, 0.5, 40, None)
}

/// The sailing functions, by the names the declarations use.
pub fn functions(defs: PerceptionDefs) -> PerceptionDefs {
    defs.derive("sailing.course_deg", course_deg)
        .derive("sailing.wind_from_deg", wind_from_deg)
        .derive("sailing.wind_mps", wind_mps)
        .derive("sailing.twa_deg", twa_deg)
        .derive("sailing.point_of_sail", point_of_sail)
        .derive("sailing.tack", tack)
        .derive("sailing.trim", trim)
        .derive("sailing.sail", sail)
        .derive("sailing.next_mark", next_mark)
        .derive("sailing.left", left)
        .derive("sailing.island_radius", island_radius)
        .derive("sailing.shore_m", shore_m)
        .derive("sailing.shore_brg_deg", shore_brg_deg)
        .derive("sailing.alongside", alongside)
        .derive("sailing.mark_next", mark_next)
}

/// The true wind at the entity: where it blows from and how fast.
fn wind(cx: &FactCx<'_>) -> (f64, f64) {
    let v = wind_at(cx.world, cx.at);
    (
        bearing_deg(-v[0], -v[2]),
        math::sqrt(v[0] * v[0] + v[2] * v[2]),
    )
}

fn heading(cx: &FactCx<'_>) -> Option<f64> {
    cx.get::<Boat>()
        .map(|b| b.heading_deg)
        .or_else(|| cx.get::<pocket_physics::Transform>().map(heading_deg))
}

fn twa(cx: &FactCx<'_>) -> Option<f64> {
    Some(relative_deg(wind(cx).0, heading(cx)?))
}

fn course_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    let v = cx.get::<Velocity>()?.linear;
    Some(FactValue::Number(bearing_deg(v[0], v[2])))
}

fn wind_from_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    Some(FactValue::Number(wind(cx).0))
}

fn wind_mps(cx: &FactCx<'_>) -> Option<FactValue> {
    Some(FactValue::Number(wind(cx).1))
}

fn twa_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    twa(cx).map(FactValue::Number)
}

/// By `abs(twa_deg)`: in irons below 45, close-hauled to 60, close reach to 80, beam reach to
/// 100, broad reach to 150, running from 150.
fn point_of_sail(cx: &FactCx<'_>) -> Option<FactValue> {
    let a = twa(cx)?.abs();
    let name = if a < 45.0 {
        "in_irons"
    } else if a < 60.0 {
        "close_hauled"
    } else if a < 80.0 {
        "close_reach"
    } else if a < 100.0 {
        "beam_reach"
    } else if a < 150.0 {
        "broad_reach"
    } else {
        "running"
    };
    Some(FactValue::Text(name.to_owned()))
}

fn tack(cx: &FactCx<'_>) -> Option<FactValue> {
    let side = if twa(cx)? > 0.0 { "starboard" } else { "port" };
    Some(FactValue::Text(side.to_owned()))
}

fn trim(cx: &FactCx<'_>) -> Option<FactValue> {
    let name = match cx.get::<Boat>()?.trim {
        Trim::Furled => "furled",
        Trim::Luffing => "luffing",
        Trim::Good => "good",
        Trim::Overtrimmed => "overtrimmed",
    };
    Some(FactValue::Text(name.to_owned()))
}

/// A boat's sail as another boat sees it: set or furled.
fn sail(cx: &FactCx<'_>) -> Option<FactValue> {
    let set = cx.get::<Boat>()?.hoist_now >= 0.5;
    Some(FactValue::Text(
        if set { "set" } else { "furled" }.to_owned(),
    ))
}

/// The course's `next`: the lowest-id entity with a project `Course` component, its `next` field,
/// the `order` of the mark to round next (as `mark.rounded`'s `order`; never an entity id,
/// docs/spec/perception-slice2.md 8).
fn course_next(cx: &FactCx<'_>) -> Option<f64> {
    let index = cx.world.get_resource::<EntityIndex>()?;
    index.iter().find_map(
        |(_, e)| match project_field(cx.world, e, "Course", "next") {
            Some(FactValue::Number(n)) => Some(n),
            _ => None,
        },
    )
}

/// A mark's `order`: its project `Mark` component's field; `None` for anything that is not a
/// mark.
fn mark_order(cx: &FactCx<'_>, e: bevy_ecs::prelude::Entity) -> Option<f64> {
    match project_field(cx.world, e, "Mark", "order") {
        Some(FactValue::Number(n)) => Some(n),
        _ => None,
    }
}

/// The course's next mark by name (the lowest-id mark whose `order` is the course's `next`), or
/// `none`.
fn next_mark(cx: &FactCx<'_>) -> Option<FactValue> {
    let none = || Some(FactValue::Text("none".to_owned()));
    let Some(next) = course_next(cx) else {
        return none();
    };
    let Some(index) = cx.world.get_resource::<EntityIndex>() else {
        return none();
    };
    for (id, e) in index.iter() {
        if mark_order(cx, e) == Some(next) {
            return Some(FactValue::Text(
                name_of(cx.world, e).unwrap_or_else(|| format!("#{}", id.get())),
            ));
        }
    }
    none()
}

/// A mark's chart fact `next`: whether its `order` is the course's next.
fn mark_next(cx: &FactCx<'_>) -> Option<FactValue> {
    let e = cx.entity.or_else(|| live(cx.world, cx.id))?;
    let next = course_next(cx)?;
    Some(FactValue::Bool(mark_order(cx, e) == Some(next)))
}

/// Crates aboard subtracted from the crates there were (the project's `Tally`).
fn left(cx: &FactCx<'_>) -> Option<FactValue> {
    let e = cx.entity?;
    let n = |f: &str| match project_field(cx.world, e, "Tally", f) {
        Some(FactValue::Number(n)) => Some(n),
        _ => None,
    };
    Some(FactValue::Number(math::max(n("total")? - n("taken")?, 0.0)))
}

fn horizontal(p: V3) -> f64 {
    math::sqrt(p[0] * p[0] + p[2] * p[2])
}

/// How far a part reaches from the body's vertical axis.
fn part_reach(shape: &PartShape) -> f64 {
    match shape {
        PartShape::Ball { radius } | PartShape::Capsule { radius, .. } => *radius,
        PartShape::Cuboid { half_extents: h } => horizontal(*h),
        PartShape::ConvexHull { points } => {
            points.iter().map(|p| horizontal(*p)).fold(0.0, math::max)
        }
    }
}

/// An island's radius: how far its collider reaches from its centre on the ground.
fn island_radius(cx: &FactCx<'_>) -> Option<FactValue> {
    let r = match &cx.get::<Collider>()?.shape {
        Shape::Ball { radius } | Shape::Capsule { radius, .. } => *radius,
        Shape::Cuboid { half_extents: h } => horizontal(*h),
        Shape::ConvexHull { points } => points.iter().map(|p| horizontal(*p)).fold(0.0, math::max),
        Shape::TriMesh { vertices, .. } => {
            vertices.iter().map(|p| horizontal(*p)).fold(0.0, math::max)
        }
        Shape::Compound { parts } => parts
            .iter()
            .map(|p| horizontal(p.position) + part_reach(&p.shape))
            .fold(0.0, math::max),
    };
    Some(FactValue::Number(r))
}

fn known_number(cx: &FactCx<'_>, name: &str) -> Option<f64> {
    match cx.known.get(name) {
        Some(FactValue::Number(x)) => Some(*x),
        _ => None,
    }
}

/// The distance from the observer's boat to an island's nearest shore: its centre's distance
/// less its known radius, at least 0.
fn shore_m(cx: &FactCx<'_>) -> Option<FactValue> {
    let from = cx.observer?.position;
    let d = horizontal(geom::sub(cx.at, from));
    Some(FactValue::Number(math::max(
        d - known_number(cx, "radius_m")?,
        0.0,
    )))
}

/// The bearing of that shore point: the bearing to the island's centre.
fn shore_brg_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    let d = geom::sub(cx.at, cx.observer?.position);
    Some(FactValue::Number(bearing_deg(d[0], d[2])))
}

/// Whether a crate floats within reach of the observer's boat to be taken aboard.
fn alongside(cx: &FactCx<'_>) -> Option<FactValue> {
    let d = geom::sub(cx.at, cx.observer?.position);
    Some(FactValue::Bool(
        horizontal(d) <= REACH_M && d[1].abs() <= REACH_UP_M,
    ))
}
