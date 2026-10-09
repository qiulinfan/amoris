//! The sailing game as a session plays it: physics, perception from the sailing declarations (with
//! the perception fixture's hidden fact and events), the sailing action catalog over the seat's
//! perception, and the continuous time rules. The Sloop is the seat `skipper`'s body; a test lays
//! marks, crates and islands, each perceivable as its kind.

use bevy_ecs::prelude::World;
use pocket_interface::action::{Caller, FieldWriter};
use pocket_interface::perception::{Observer, Perceivable, Vec3};
use pocket_interface::time::control::Controller;
use pocket_interface::time::play::{DecisionFilter, PlayPacing};
use pocket_physics::Transform;
use pocket_sim::{EntityId, Name, Sim, SimConfig, TickRate};
use serde_json::Value;

use super::Delivered;

fn write_field(
    w: &mut World,
    _body: EntityId,
    _component: &str,
    _field: &str,
    v: &Value,
) -> Result<(), pocket_contract::Problem> {
    let tick = w.resource::<pocket_sim::SimClock>().tick.0;
    w.resource_mut::<Delivered>().0.push((tick, v.clone()));
    Ok(())
}

/// A world with the systems and no entities.
pub fn fresh(seed: u64) -> Sim {
    fresh_with(pocket_interface::perception::probe::defs(), seed)
}

/// [`fresh`] with the perception declarations given.
pub fn fresh_with(defs: pocket_interface::perception::PerceptionDefs, seed: u64) -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed,
    })
    .unwrap();
    pocket_physics::plugin(&mut sim).unwrap();
    let catalog = pocket_interface::action::sailing::game_catalog(&defs).unwrap();
    pocket_interface::perception::plugin(&mut sim, defs).unwrap();
    pocket_interface::action::plugin(&mut sim, catalog).unwrap();
    pocket_interface::time::turns::plugin(&mut sim, Default::default()).unwrap();
    sim.world_mut().insert_resource(Delivered::default());
    sim.world_mut().insert_resource(FieldWriter(write_field));
    sim
}

fn name(n: &str) -> Name {
    Name::new(n).unwrap()
}

/// The sea, a breeze from `wind_from` at `wind` m/s, and the Sloop heading `heading` with its sail
/// set: the skipper's body.
pub fn world(heading: f64, wind_from: f64, wind: f64, seed: u64) -> (Sim, EntityId) {
    world_in(fresh(seed), heading, wind_from, wind, true)
}

/// [`world`] with the sail set or furled (eased right out either way).
pub fn world_sail(
    heading: f64,
    wind_from: f64,
    wind: f64,
    sail_set: bool,
    seed: u64,
) -> (Sim, EntityId) {
    world_in(fresh(seed), heading, wind_from, wind, sail_set)
}

/// [`world`] laid in `sim` (from [`fresh_with`]).
pub fn world_in(
    mut sim: Sim,
    heading: f64,
    wind_from: f64,
    wind: f64,
    sail_set: bool,
) -> (Sim, EntityId) {
    let mut b = sim.boundary();
    b.spawn((name("Sea"), pocket_physics::sailing::calm_sea()))
        .unwrap();
    b.spawn((
        name("Breeze"),
        pocket_physics::sailing::breeze(wind_from, wind),
    ))
    .unwrap();
    let mut sloop = pocket_physics::sailing::sloop([0.0; 3], heading);
    if sail_set {
        sloop.6 = pocket_physics::Boat::sail_set();
    }
    let id = b.spawn((name("Sloop"), sloop)).unwrap();
    let e = b.entity(id).unwrap();
    b.world_mut().entity_mut(e).insert((
        Observer {
            profile: "skipper".into(),
            seat: Some("skipper".into()),
            omniscient: false,
            team: None,
        },
        boat_kind(),
    ));
    (sim, id)
}

fn boat_kind() -> Perceivable {
    Perceivable {
        kind: "boat".into(),
        detect_m: 1500.0,
        height_m: 6.0,
        priority: 70,
        chart_m: None,
    }
}

/// A second boat, perceivable, observing nothing.
pub fn other_boat(sim: &mut Sim, n: &str, at: [f64; 3], heading: f64) -> EntityId {
    let mut b = sim.boundary();
    let id = b
        .spawn((name(n), pocket_physics::sailing::sloop(at, heading)))
        .unwrap();
    let e = b.entity(id).unwrap();
    b.world_mut().entity_mut(e).insert(boat_kind());
    id
}

/// A mark at `at`, on the chart.
pub fn mark(sim: &mut Sim, n: &str, at: [f64; 3]) -> EntityId {
    let perc = Perceivable {
        kind: "mark".into(),
        detect_m: 400.0,
        height_m: 2.0,
        priority: 80,
        chart_m: Some(Vec3::of(at)),
    };
    sim.boundary()
        .spawn((name(n), Transform::at(at), perc))
        .unwrap()
}

/// A crate adrift at `at`.
pub fn crate_at(sim: &mut Sim, n: &str, at: [f64; 3]) -> EntityId {
    let mut b = sim.boundary();
    let id = b
        .spawn((name(n), pocket_physics::sailing::crate_box(at, 0.0)))
        .unwrap();
    let e = b.entity(id).unwrap();
    b.world_mut().entity_mut(e).insert(Perceivable {
        kind: "crate".into(),
        detect_m: 120.0,
        height_m: 0.5,
        priority: 40,
        chart_m: None,
    });
    id
}

/// An island of `radius` at `at`, on the chart.
pub fn island(sim: &mut Sim, n: &str, at: [f64; 3], radius: f64) -> EntityId {
    sim.boundary()
        .spawn((
            name(n),
            pocket_interface::perception::probe::island(at, radius, true),
        ))
        .unwrap()
}

/// The skipper as its player.
pub fn skipper() -> Caller {
    Caller::Player {
        seat: "skipper".into(),
    }
}

/// The sailing default decision filter (sailing.md, Time).
pub fn sailing_filter() -> DecisionFilter {
    DecisionFilter {
        events: [
            "intent.succeeded",
            "intent.failed",
            "intent.reached",
            "sighted",
            "boat.aground",
            "mark.rounded",
            "course.finished",
            "crate.taken",
        ]
        .map(String::from)
        .to_vec(),
        idle_s: Some(10.0),
        every_s: Some(120.0),
    }
}

/// A stepped session with the sailing filter.
pub fn stepped() -> Controller {
    Controller::new(PlayPacing::Stepped, sailing_filter(), Vec::new())
}
