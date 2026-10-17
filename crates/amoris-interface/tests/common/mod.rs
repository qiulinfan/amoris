//! A sailing world for the action layer's tests: physics, the Sloop as the seat `skipper`, a breeze,
//! a calm sea, and whatever marks, islands and crates a test lays, known to the skipper through a
//! [`BoatView`] listing them (marks and crates seen, islands charted with their radius).

#![allow(dead_code)]

pub mod fixture;
pub mod play;

use std::sync::Arc;

use amoris_interface::action::sailing::{self, instruments::BoatView, instruments::Listed};
use amoris_interface::action::{ActionCatalog, Caller, FieldWriter};
use amoris_physics::{Boat, Collider, RigidBody, Shape, Transform};
use amoris_sim::{EntityId, Name, NoHooks, PlainData, Sim, SimConfig, TickRate};
use bevy_ecs::prelude::{Resource, World};
use serde_json::{Value, json};

/// What the skipper knows besides its own boat.
#[derive(Resource, Clone, Default)]
pub struct Known(pub Vec<Listed>);

/// The pulses `interact` delivers, by tick: what a game rule would read.
#[derive(Resource, Clone, Default)]
pub struct Delivered(pub Vec<(u64, Value)>);

fn bodies(w: &World) -> Vec<(String, EntityId)> {
    let index = w.resource::<amoris_sim::EntityIndex>();
    index
        .iter()
        .filter(|(_, e)| w.get::<Name>(*e).is_some_and(|n| n.as_str() == "Sloop"))
        .map(|(id, _)| ("skipper".to_owned(), id))
        .collect()
}

fn write_field(
    w: &mut World,
    _body: EntityId,
    _component: &str,
    _field: &str,
    v: &Value,
) -> Result<(), amoris_contract::Problem> {
    let tick = w.resource::<amoris_sim::SimClock>().tick.0;
    w.resource_mut::<Delivered>().0.push((tick, v.clone()));
    Ok(())
}

/// The sailing catalog with a [`BoatView`] over the world's [`Known`] list.
pub fn catalog() -> ActionCatalog {
    sailing::catalog(
        bodies,
        Arc::new(|w: &World, row| {
            Box::new(BoatView {
                world: w,
                body: row.body,
                listed: w
                    .get_resource::<Known>()
                    .map(|k| k.0.clone())
                    .unwrap_or_default(),
            })
        }),
        ("Crew", "take"),
    )
}

/// A sailing world: wind from `wind_from` at `wind` m/s, the Sloop at the origin heading `heading`
/// with its sail set (or furled), sheet eased right out.
pub fn world(
    heading: f64,
    wind_from: f64,
    wind: f64,
    sail_set: bool,
    seed: u64,
) -> (Sim, EntityId) {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    amoris_interface::action::plugin(&mut sim, catalog()).unwrap();
    amoris_interface::time::turns::plugin(&mut sim, Default::default()).unwrap();
    sim.world_mut().insert_resource(Known::default());
    sim.world_mut().insert_resource(Delivered::default());
    sim.world_mut().insert_resource(FieldWriter(write_field));
    let mut b = sim.boundary();
    let name = |n: &str| Name::new(n).unwrap();
    b.spawn((name("Sea"), amoris_physics::sailing::calm_sea()))
        .unwrap();
    b.spawn((
        name("Breeze"),
        amoris_physics::sailing::breeze(wind_from, wind),
    ))
    .unwrap();
    let mut sloop = amoris_physics::sailing::sloop([0.0; 3], heading);
    if sail_set {
        sloop.6 = Boat::sail_set();
    }
    let id = b.spawn((name("Sloop"), sloop)).unwrap();
    (sim, id)
}

/// Lays a mark at `at`, seen by the skipper.
pub fn mark(sim: &mut Sim, name: &str, at: [f64; 3]) -> EntityId {
    let id = sim
        .boundary()
        .spawn((Name::new(name).unwrap(), Transform::at(at)))
        .unwrap();
    sim.world_mut().resource_mut::<Known>().0.push(Listed {
        id,
        kind: "mark".into(),
        chart_m: None,
        facts: Vec::new(),
    });
    id
}

/// An island of radius `r` at `at`: a fixed block of land, on the skipper's chart.
pub fn island(sim: &mut Sim, name: &str, at: [f64; 3], r: f64) -> EntityId {
    let id = sim
        .boundary()
        .spawn((
            Name::new(name).unwrap(),
            Transform::at([at[0], 0.0, at[2]]),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [r * 0.7, 4.0, r * 0.7],
            }),
        ))
        .unwrap();
    sim.world_mut().resource_mut::<Known>().0.push(Listed {
        id,
        kind: "island".into(),
        chart_m: Some([at[0], 0.0, at[2]]),
        facts: vec![("radius_m".into(), json!(r))],
    });
    id
}

/// A crate floating at `at`, seen by the skipper.
pub fn crate_at(sim: &mut Sim, name: &str, at: [f64; 3]) -> EntityId {
    let id = sim
        .boundary()
        .spawn((
            Name::new(name).unwrap(),
            amoris_physics::sailing::crate_box(at, 0.0),
        ))
        .unwrap();
    sim.world_mut().resource_mut::<Known>().0.push(Listed {
        id,
        kind: "crate".into(),
        chart_m: None,
        facts: Vec::new(),
    });
    id
}

/// An act for the skipper as its player.
pub fn act(
    sim: &mut Sim,
    req: Value,
) -> Result<amoris_interface::action::ActDone, amoris_contract::Problem> {
    let caller = Caller::Player {
        seat: "skipper".into(),
    };
    amoris_interface::action::act(sim.world_mut(), &caller, &req)
}

/// Steps one tick.
pub fn step(sim: &mut Sim) {
    sim.step(&mut NoHooks).unwrap();
}

/// The boat.
pub fn boat(sim: &Sim, id: EntityId) -> Boat {
    let e = amoris_sim::entity::require(sim.world(), id).unwrap();
    *sim.world().get::<Boat>(e).unwrap()
}

/// The boat's position.
pub fn pos(sim: &Sim, id: EntityId) -> [f64; 3] {
    let e = amoris_sim::entity::require(sim.world(), id).unwrap();
    sim.world().get::<Transform>(e).unwrap().position
}

/// An intent's instance.
pub fn intent(sim: &Sim, id: u64) -> amoris_interface::action::IntentInstance {
    sim.world()
        .resource::<amoris_interface::action::IntentTable>()
        .by_id
        .get(&id)
        .cloned()
        .unwrap_or_else(|| panic!("no intent {id}"))
}

/// A progress reading as a number.
pub fn reading(i: &amoris_interface::action::IntentInstance, name: &str) -> Option<f64> {
    i.progress
        .iter()
        .find(|(n, _)| n == name)
        .and_then(|(_, v)| match v {
            PlainData::Number(x) => Some(*x),
            _ => None,
        })
}

/// The id an act's first outcome started.
pub fn started(done: &amoris_interface::action::ActDone) -> u64 {
    done.outcomes[0]["intent_id"]
        .as_u64()
        .or_else(|| done.outcomes[0]["then"]["intent_id"].as_u64())
        .expect("an intent started")
}

/// The persistence declarations of the simulation, physics, actions and time: what a world hash
/// covers (`amoris-persist` aside, which this crate cannot link; amoris_physics::probe's ledger).
pub fn ledger() -> amoris_physics::probe::Ledger {
    let mut l = amoris_physics::probe::ledger();
    amoris_interface::action::declare(&mut l);
    amoris_interface::time::turns::declare(&mut l);
    amoris_interface::perception::declare(&mut l);
    l
}

/// The world's hash over [`ledger`]'s sections.
pub fn ledger_hash(sim: &Sim) -> u64 {
    ledger().snapshot(sim.world()).unwrap().hash()
}
