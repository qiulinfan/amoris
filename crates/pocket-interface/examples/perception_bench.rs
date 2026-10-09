//! Times perception natively (shared/contract/perception.md, Performance): the
//! `interface.perception` system per tick on the sailing probe (`perception.tick`), the same with
//! 100 NPC observers over 1,000 perceivable entities among occluding walls (`perception.npc`), and
//! `observe` at the skipper's default budget in the text projection (`observe.sail`). Medians of
//! five rounds. For reading; the budgets are the check's `perf` step's (budgets.md).
//!
//! `cargo run -p pocket-interface --example perception_bench --release`

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick

use std::time::Instant;

use pocket_interface::perception::probe::{self, PLAYER};
use pocket_interface::perception::{
    NoAffordances, ObserveRequest, Observer, Occluder, Perceivable, SeatParts, load, observe,
    plugin,
};
use pocket_interface::projection::Projection;
use pocket_physics::{Collider, RigidBody, Shape, Transform};
use pocket_sim::{Pcg32, Sim, SimConfig, StepHooks, SystemKey, TickPhase, TickRate};

/// Accumulates the perception system's time.
#[derive(Default)]
struct Timer {
    started: Option<Instant>,
    us: f64,
}

impl StepHooks for Timer {
    fn phase(&mut self, _phase: TickPhase, _begin: bool) {}

    fn system(&mut self, key: &SystemKey, begin: bool) {
        if key.as_str() != pocket_interface::perception::SYSTEM {
            return;
        }
        if begin {
            self.started = Some(Instant::now());
        } else if let Some(t) = self.started.take() {
            self.us += t.elapsed().as_secs_f64() * 1e6;
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

const GUARDS: &str = r#"{
  "observers": [{
    "name": "guard", "doc": "An NPC on watch.",
    "sight": {"range_m": 60, "fov_deg": 120, "eye_m": {"x": 0, "y": 1.6, "z": 0}, "occlusion": true},
    "hearing": {"scale": 1}, "attention_m": 30, "memory_s": 10, "memory_capacity": 32,
    "event_capacity": 64, "sightings": ["thing"], "budget_tokens": 400
  }],
  "kinds": [{"kind": "thing", "doc": "A thing.", "facts": [
    {"name": "x_m", "doc": "Its x.", "unit": {"unit": "metres"}, "precision": 1, "exposure": "coarse",
     "source": {"from": "field", "component": "Transform", "path": "position.0"}}
  ]}]
}"#;

/// 100 guards and 1,000 things over 600 m square, among 20 walls.
fn npc_world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 5,
    })
    .expect("a valid configuration");
    pocket_physics::plugin(&mut sim).expect("physics installs");
    plugin(&mut sim, load(GUARDS).expect("declarations")).expect("perception installs");
    let mut rng = Pcg32::new(11, 3);
    let at = |rng: &mut Pcg32| {
        [
            rng.range(-300.0, 300.0).expect("a draw"),
            0.0,
            rng.range(-300.0, 300.0).expect("a draw"),
        ]
    };
    let mut b = sim.boundary();
    for _ in 0..20 {
        let p = at(&mut rng);
        b.spawn((
            Transform::at(p),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [8.0, 3.0, 1.0],
            }),
            Occluder {},
        ))
        .expect("spawns");
    }
    for _ in 0..1000 {
        let p = at(&mut rng);
        let perc = Perceivable {
            kind: "thing".into(),
            detect_m: 80.0,
            height_m: 1.5,
            priority: 1,
            chart_m: None,
        };
        b.spawn((Transform::at(p), perc)).expect("spawns");
    }
    for _ in 0..100 {
        let p = at(&mut rng);
        let yaw = rng.range(0.0, 360.0).expect("a draw");
        let o = Observer {
            profile: "guard".into(),
            seat: None,
            omniscient: false,
        };
        b.spawn((Transform::at_yaw(p, yaw), o)).expect("spawns");
    }
    sim
}

fn per_tick(make: fn() -> Sim, ticks: u32) -> f64 {
    let mut rounds = Vec::new();
    for _ in 0..5 {
        let mut sim = make();
        let mut t = Timer::default();
        for _ in 0..ticks {
            sim.step(&mut t).expect("a tick");
        }
        rounds.push(t.us / f64::from(ticks));
    }
    median(rounds)
}

fn main() {
    let sail = per_tick(|| probe::world(0).0, 600);
    println!("perception.tick  {sail:8.1} us a tick (sailing probe, one skipper)");
    let npc = per_tick(npc_world, 120);
    println!("perception.npc   {npc:8.1} us a tick (100 observers, 1,000 entities, 20 walls)");
    let (sim, _) = probe::world(600);
    let req = ObserveRequest {
        projection: Some(Projection::Text),
        since: Some(0),
        ..ObserveRequest::default()
    };
    let mut calls = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..200 {
            let a = observe(
                sim.world(),
                &PLAYER,
                &req,
                &SeatParts::default(),
                &NoAffordances,
            );
            assert!(a.is_ok());
        }
        calls.push(t.elapsed().as_secs_f64() * 1e6 / 200.0);
    }
    println!(
        "observe.sail     {:8.1} us a call (text, default budget)",
        median(calls)
    );
}
