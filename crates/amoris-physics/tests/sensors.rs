//! Sensors and collision groups (simulation.md 8.5): a sensor reports the bodies and walkers that
//! enter and leave it and pushes nothing; groups and masks decide which colliders meet; a filter
//! changed on a built body takes effect at the next step without a rebuild.

use amoris_physics::query::{cast_ray, overlap_ball};
use amoris_physics::{
    Collider, CollisionFilter, ExternalForce, RigidBody, SensorOverlaps, Shape, Transform,
    Velocity, Walker,
};
use amoris_sim::{EntityId, NoHooks, PlainData, Sim, SimConfig, StepReport, TickRate, entity};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}

fn steps(sim: &mut Sim, n: u32) -> Vec<StepReport> {
    (0..n)
        .map(|_| {
            let r = sim.step(&mut NoHooks).unwrap();
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            r
        })
        .collect()
}

fn get<C: bevy_ecs::component::Component + Clone>(sim: &Sim, id: EntityId) -> Option<C> {
    let e = entity::require(sim.world(), id).unwrap();
    sim.world().get::<C>(e).cloned()
}

fn cuboid(
    sim: &mut Sim,
    at: [f64; 3],
    half: [f64; 3],
    filter: Option<CollisionFilter>,
) -> EntityId {
    let c = Collider::new(Shape::Cuboid { half_extents: half });
    let mut b = sim.boundary();
    let id = b.spawn((Transform::at(at), RigidBody::fixed(), c)).unwrap();
    if let Some(f) = filter {
        let e = entity::require(b.world_mut(), id).unwrap();
        b.world_mut().entity_mut(e).insert(f);
    }
    id
}

fn ball(sim: &mut Sim, at: [f64; 3], filter: Option<CollisionFilter>) -> EntityId {
    let c = Collider::new(Shape::Ball { radius: 0.5 }).with_density(1000.0);
    let mut b = sim.boundary();
    let id = b
        .spawn((
            Transform::at(at),
            Velocity::default(),
            RigidBody::dynamic(),
            c,
            ExternalForce::default(),
        ))
        .unwrap();
    if let Some(f) = filter {
        let e = entity::require(b.world_mut(), id).unwrap();
        b.world_mut().entity_mut(e).insert(f);
    }
    id
}

/// Every event of the reports as `(tick, kind, subject, other)`, physics' only.
fn log(reports: &[StepReport]) -> Vec<(u64, String, u64, u64)> {
    reports
        .iter()
        .flat_map(|r| r.events.iter().map(move |e| (r.tick.0, e)))
        .filter(|(_, e)| e.kind.as_str().starts_with("physics."))
        .map(|(t, e)| {
            let other = match e.data.get("other") {
                Some(PlainData::Number(n)) => *n as u64,
                _ => 0,
            };
            (
                t,
                e.kind.as_str().to_owned(),
                e.subject.map_or(0, |s| s.get()),
                other,
            )
        })
        .collect()
}

fn kinds(events: &[(u64, String, u64, u64)]) -> Vec<(&str, u64, u64)> {
    events
        .iter()
        .map(|(_, k, s, o)| (k.as_str(), *s, *o))
        .collect()
}

#[test]
fn a_ball_falls_through_a_sensor_onto_the_ground() {
    let mut sim = world();
    let ground = cuboid(&mut sim, [0.0, -0.5, 0.0], [10.0, 0.5, 10.0], None);
    let zone = cuboid(
        &mut sim,
        [0.0, 3.0, 0.0],
        [1.0, 0.5, 1.0],
        Some(CollisionFilter::sensor()),
    );
    let b = ball(&mut sim, [0.0, 5.0, 0.0], None);
    let reports = steps(&mut sim, 240);
    let y = get::<Transform>(&sim, b).unwrap().position[1];
    assert!(
        (y - 0.5).abs() < 0.02,
        "the ball rests on the ground at {y}"
    );
    let events = log(&reports);
    let (g, z, b) = (ground.get(), zone.get(), b.get());
    assert_eq!(
        kinds(&events),
        [
            ("physics.sensor_entered", z, b),
            ("physics.sensor_left", z, b),
            ("physics.contact_began", g, b),
        ],
        "{events:?}"
    );
    assert!(events[0].0 < events[1].0 && events[1].0 < events[2].0);
    // Queries pass through the sensor: a ray down from above it meets the ground.
    let hit = cast_ray(sim.world(), [3.0, 10.0, 3.0], [0.0, -1.0, 0.0], 20.0, None).unwrap();
    assert_eq!(hit.entity, ground);
    assert!(!overlap_ball(sim.world(), [0.0, 3.0, 0.0], 0.2).contains(&zone));
}

#[test]
fn groups_and_masks_decide_what_meets() {
    // Group 1: the ground; group 2: balls that meet only group 2; group 4: a ball that meets all.
    let mut sim = world();
    let in_group = |groups: u32, mask: u32| CollisionFilter {
        sensor: false,
        groups,
        mask,
    };
    cuboid(
        &mut sim,
        [0.0, -0.5, 0.0],
        [10.0, 0.5, 10.0],
        Some(in_group(1, u32::MAX)),
    );
    let ghost = ball(&mut sim, [-3.0, 2.0, 0.0], Some(in_group(2, 2)));
    let solid = ball(&mut sim, [3.0, 2.0, 0.0], Some(in_group(4, u32::MAX)));
    let plain = ball(&mut sim, [0.0, 2.0, 0.0], None);
    // A second ghost dropped onto the first: they meet each other.
    let stacked = ball(&mut sim, [-3.0, 4.0, 0.0], Some(in_group(2, 2)));
    steps(&mut sim, 60);
    let y = |id| get::<Transform>(&sim, id).unwrap().position[1];
    assert!(
        y(solid) > 0.4 && y(plain) > 0.4,
        "{} {}",
        y(solid),
        y(plain)
    );
    assert!(
        y(ghost) < -1.0,
        "the ghost fell through the ground: {}",
        y(ghost)
    );
    assert!(
        y(stacked) - y(ghost) > 0.9,
        "ghosts meet each other: {} over {}",
        y(stacked),
        y(ghost)
    );
}

#[test]
fn a_filter_changed_on_a_built_body_takes_effect_at_the_next_step() {
    let mut sim = world();
    let floor = cuboid(&mut sim, [0.0, -0.5, 0.0], [10.0, 0.5, 10.0], None);
    let b = ball(&mut sim, [0.0, 0.5, 0.0], None);
    let mut reports = steps(&mut sim, 30);
    assert!((get::<Transform>(&sim, b).unwrap().position[1] - 0.5).abs() < 0.02);
    // The floor becomes a sensor: the ball resting on it falls through and is reported in it.
    {
        let mut bd = sim.boundary();
        let e = entity::require(bd.world_mut(), floor).unwrap();
        bd.world_mut()
            .entity_mut(e)
            .insert(CollisionFilter::sensor());
    }
    reports.extend(steps(&mut sim, 60));
    assert!(get::<Transform>(&sim, b).unwrap().position[1] < -2.0);
    let events = log(&reports);
    let (f, b) = (floor.get(), b.get());
    assert_eq!(
        kinds(&events),
        [
            ("physics.contact_began", f, b),
            ("physics.contact_ended", f, b),
            ("physics.sensor_entered", f, b),
            ("physics.sensor_left", f, b),
        ],
        "{events:?}"
    );
}

fn walker(sim: &mut Sim, eye: [f64; 3], filter: Option<CollisionFilter>) -> EntityId {
    let mut b = sim.boundary();
    let id = b
        .spawn((
            Transform::at(eye),
            Walker {
                gravity_mps2: 9.81,
                ..Walker::default()
            },
        ))
        .unwrap();
    if let Some(f) = filter {
        let e = entity::require(b.world_mut(), id).unwrap();
        b.world_mut().entity_mut(e).insert(f);
    }
    id
}

/// Steps with the walkers walking forward (along -z).
fn walk(sim: &mut Sim, walkers: &[EntityId], ticks: u32) -> Vec<StepReport> {
    (0..ticks)
        .map(|_| {
            for id in walkers {
                let e = entity::require(sim.world(), *id).unwrap();
                sim.world_mut().get_mut::<Walker>(e).unwrap().forward = 1.0;
            }
            let r = sim.step(&mut NoHooks).unwrap();
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            r
        })
        .collect()
}

#[test]
fn a_walker_walks_through_a_sensor_and_is_reported() {
    let mut sim = world();
    cuboid(&mut sim, [0.0, -0.5, 0.0], [20.0, 0.5, 20.0], None);
    // A gate 4 m ahead, 1 m deep, that only group 2 notices.
    let gate = cuboid(
        &mut sim,
        [0.0, 1.0, -4.0],
        [2.0, 1.0, 0.5],
        Some(CollisionFilter {
            sensor: true,
            groups: 1,
            mask: 2,
        }),
    );
    let w = walker(&mut sim, [0.0, 1.62, 0.0], None);
    let unseen = walker(
        &mut sim,
        [1.0, 1.62, 0.0],
        Some(CollisionFilter {
            sensor: false,
            groups: 4,
            mask: u32::MAX,
        }),
    );
    let mut reports = walk(&mut sim, &[w, unseen], 150);
    // Both walked through the gate, which stops neither.
    for id in [w, unseen] {
        let z = get::<Transform>(&sim, id).unwrap().position[2];
        assert!(z < -6.0, "{id:?} at z {z}");
    }
    let events = log(&reports);
    let (g, wi) = (gate.get(), w.get());
    assert_eq!(
        kinds(&events),
        [
            ("physics.sensor_entered", g, wi),
            ("physics.sensor_left", g, wi)
        ],
        "{events:?}"
    );
    assert!(get::<SensorOverlaps>(&sim, w).is_none());
    // Turned back into the gate, it stops inside and the overlap is kept.
    {
        let e = entity::require(sim.world(), w).unwrap();
        let mut wk = sim.world_mut().get_mut::<Walker>(e).unwrap();
        wk.yaw = std::f64::consts::PI;
    }
    let mut inside = false;
    for _ in 0..200 {
        reports = walk(&mut sim, &[w], 1);
        if !log(&reports).is_empty() {
            inside = true;
            break;
        }
    }
    assert!(inside);
    assert_eq!(kinds(&log(&reports)), [("physics.sensor_entered", g, wi)]);
    assert_eq!(
        get::<SensorOverlaps>(&sim, w),
        Some(SensorOverlaps {
            sensors: vec![gate]
        })
    );
    assert!(
        log(&steps(&mut sim, 5)).is_empty(),
        "standing inside says nothing"
    );
    // Despawning the gate tells the walker it left.
    sim.boundary().despawn(gate).unwrap();
    let reports = steps(&mut sim, 1);
    assert_eq!(kinds(&log(&reports)), [("physics.sensor_left", g, wi)]);
    assert!(get::<SensorOverlaps>(&sim, w).is_none());
}
