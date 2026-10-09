//! A small world for perception's unit tests (shared/contract/perception.md, Checks, visibility
//! unit tests): a watcher on a seat with a narrow field of view, things it may see, walls that
//! hide them, and events of every scope. Positions are exact so each edge falls where the test
//! puts it.

#![allow(dead_code)]

use pocket_interface::perception::state::{ObserverEvents, ObserverMemory, PerceivedEvent};
use pocket_interface::perception::{
    Observer, Occluder, Perceivable, PerceptionDefs, Vec3, load, plugin,
};
use pocket_physics::{Collider, RigidBody, Shape, Transform};
use pocket_sim::{
    EntityId, EventKind, EventSeq, Name, NewEvent, NoHooks, PlainData, Sim, SimConfig, TickRate,
};
use serde_json::{Value, json};

/// The watcher's profile: 100 m of sight in a 90-degree field ahead from its origin, occlusion,
/// 50 m of attention, one second of memory, three entries, eight events.
pub const WATCH: &str = r#"{
  "observers": [{
    "name": "watch", "doc": "A watcher.",
    "sight": {"range_m": 100, "fov_deg": 90, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": true},
    "hearing": {"scale": 1},
    "attention_m": 50, "memory_s": 1, "memory_capacity": 3, "event_capacity": 8,
    "sightings": ["thing"], "chart": true, "positions": true,
    "instruments": ["x_m"], "budget_tokens": 400
  }],
  "instruments": [
    {"name": "x_m", "doc": "Where the watcher stands.", "unit": {"unit": "metres"}, "precision": 1,
     "source": {"from": "field", "component": "Transform", "path": "position.0"}}
  ],
  "kinds": [{
    "kind": "thing", "doc": "A thing.",
    "facts": [
      {"name": "z_m", "doc": "Its z, coarse.", "unit": {"unit": "metres"}, "precision": 1,
       "exposure": "coarse", "source": {"from": "field", "component": "Transform", "path": "position.2"}},
      {"name": "x_m", "doc": "Its x, full.", "unit": {"unit": "metres"}, "precision": 2,
       "exposure": "full", "source": {"from": "field", "component": "Transform", "path": "position.0"}},
      {"name": "secret_m", "doc": "Its height, hidden.", "unit": {"unit": "metres"}, "precision": 1,
       "exposure": "hidden", "source": {"from": "field", "component": "Transform", "path": "position.1"}}
    ]
  }],
  "events": [
    {"kind": "ev.private", "doc": "To its subject.", "scope": {"scope": "private"}, "data": [
      {"name": "n", "doc": "A count.", "unit": {"unit": "count"}, "exposure": "coarse",
       "source": {"from": "data", "path": "n"}},
      {"name": "hush", "doc": "Hidden data.", "unit": {"unit": "count"}, "exposure": "hidden",
       "source": {"from": "data", "path": "hush"}}
    ]},
    {"kind": "ev.sight", "doc": "Seen where it happens.", "scope": {"scope": "sight"}},
    {"kind": "ev.sound", "doc": "Heard within 10 m.", "scope": {"scope": "sound", "radius_m": 10}},
    {"kind": "ev.global", "doc": "Known to all.", "scope": {"scope": "global"}},
    {"kind": "ev.hidden", "doc": "Bookkeeping.", "scope": {"scope": "hidden"}}
  ]
}"#;

pub fn defs() -> PerceptionDefs {
    load(WATCH).expect("the watcher's declarations load")
}

/// A world with physics and perception and nothing in it.
pub fn sim() -> Sim {
    sim_with(defs())
}

pub fn sim_with(defs: PerceptionDefs) -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 3,
    })
    .expect("a valid configuration");
    pocket_physics::plugin(&mut sim).expect("physics installs");
    plugin(&mut sim, defs).expect("perception installs");
    sim
}

/// The watcher on seat `watch` at `at`, facing the bearing `heading` (0: -z).
pub fn watcher(sim: &mut Sim, at: [f64; 3], heading: f64) -> EntityId {
    let mut b = sim.boundary();
    b.spawn((
        Name::new("Watcher").expect("a name"),
        Transform::at_yaw(at, -heading),
        Observer {
            profile: "watch".into(),
            seat: Some("watch".into()),
            omniscient: false,
        },
    ))
    .expect("spawns")
}

/// A thing at `at`, seen from `detect_m`, `height_m` tall.
pub fn thing(sim: &mut Sim, name: &str, at: [f64; 3], detect_m: f64, height_m: f64) -> EntityId {
    let mut b = sim.boundary();
    b.spawn((
        Name::new(name).expect("a name"),
        Transform::at(at),
        Perceivable {
            kind: "thing".into(),
            detect_m,
            height_m,
            priority: 10,
            chart_m: None,
        },
    ))
    .expect("spawns")
}

/// A charted thing, its chart position where it stands at the start.
pub fn charted(sim: &mut Sim, name: &str, at: [f64; 3]) -> EntityId {
    let id = thing(sim, name, at, 100.0, 1.0);
    let e = sim.boundary().entity(id).expect("live");
    sim.world_mut()
        .get_mut::<Perceivable>(e)
        .expect("perceivable")
        .chart_m = Some(Vec3::of(at));
    id
}

/// A fixed wall: a box of half extents `h` centred at `at`, which occludes.
pub fn wall(sim: &mut Sim, at: [f64; 3], h: [f64; 3]) -> EntityId {
    let mut b = sim.boundary();
    b.spawn((
        Name::new("Wall").expect("a name"),
        Transform::at(at),
        RigidBody::fixed(),
        Collider::new(Shape::Cuboid { half_extents: h }),
        Occluder {},
    ))
    .expect("spawns")
}

/// Moves an entity at the boundary.
pub fn place(sim: &mut Sim, id: EntityId, at: [f64; 3]) {
    let e = sim.boundary().entity(id).expect("live");
    sim.world_mut()
        .get_mut::<Transform>(e)
        .expect("a transform")
        .position = at;
}

/// Turns the watcher to face `heading`.
pub fn turn(sim: &mut Sim, id: EntityId, heading: f64) {
    let e = sim.boundary().entity(id).expect("live");
    sim.world_mut()
        .get_mut::<Transform>(e)
        .expect("a transform")
        .rotation = pocket_physics::geom::yaw(-heading);
}

/// Emits an event at the boundary.
pub fn emit(
    sim: &mut Sim,
    kind: &str,
    subject: Option<EntityId>,
    cause: Option<EventSeq>,
    data: Value,
) -> EventSeq {
    let mut e = NewEvent::new(EventKind::new(kind).expect("a kind"));
    if let Some(s) = subject {
        e = e.subject(s);
    }
    if let Some(c) = cause {
        e = e.cause(c);
    }
    e = e.data(PlainData::from_json(&data));
    sim.boundary().emit(e)
}

/// `{"at_m": {x, y, z}}`.
pub fn at(p: [f64; 3]) -> Value {
    json!({"at_m": {"x": p[0], "y": p[1], "z": p[2]}})
}

pub fn step(sim: &mut Sim) {
    sim.step(&mut NoHooks).expect("a tick");
}

pub fn steps(sim: &mut Sim, n: u64) {
    for _ in 0..n {
        step(sim);
    }
}

/// The observer's memory.
pub fn memory(sim: &Sim, observer: EntityId) -> ObserverMemory {
    let e = sim
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .get(observer)
        .expect("live");
    sim.world()
        .get::<ObserverMemory>(e)
        .cloned()
        .unwrap_or_default()
}

/// The ids the observer remembers.
pub fn remembered(sim: &Sim, observer: EntityId) -> Vec<EntityId> {
    memory(sim, observer).entries.keys().copied().collect()
}

/// The observer's perceived events.
pub fn ring(sim: &Sim, observer: EntityId) -> Vec<PerceivedEvent> {
    let e = sim
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .get(observer)
        .expect("live");
    sim.world()
        .get::<ObserverEvents>(e)
        .map(|r| r.ring.iter().map(|x| x.event.clone()).collect())
        .unwrap_or_default()
}

/// The kinds of the perceived events, in order.
pub fn kinds(sim: &Sim, observer: EntityId) -> Vec<String> {
    ring(sim, observer).into_iter().map(|e| e.kind).collect()
}
