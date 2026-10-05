//! Bodies built from components (architecture.md 4.4; numeric.md 5 and 7; simulation.md 8.4):
//! the mass rule's refusal at the step, falling and landing with contact events, writes from
//! outside, changed and despawned bodies, non-finite state, the queries, buoyancy and the rudder.

use pocket_physics::query::{cast_ray, overlap_ball, water_at, wind_at};
use pocket_physics::sailing::{self, calm_sea, crate_box, sloop};
use pocket_physics::{
    Boat, Collider, ExternalForce, Floater, MassProps, Physics, RigidBody, Shape, Transform,
    Velocity,
};
use pocket_sim::{
    EntityId, EventInbox, Name, NoHooks, Sim, SimConfig, StepReport, TickRate, entity,
};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    pocket_physics::plugin(&mut sim).unwrap();
    sim
}

fn steps(sim: &mut Sim, n: u32) -> Vec<StepReport> {
    (0..n).map(|_| sim.step(&mut NoHooks).unwrap()).collect()
}

fn get<C: bevy_ecs::component::Component + Copy>(sim: &Sim, id: EntityId) -> C {
    let e = entity::require(sim.world(), id).unwrap();
    *sim.world().get::<C>(e).unwrap()
}

fn hull() -> Shape {
    Shape::ConvexHull {
        points: vec![
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.0, 0.5, -0.5],
            [0.0, 0.0, 0.5],
        ],
    }
}

fn ground(sim: &mut Sim) -> EntityId {
    let c = Collider::new(Shape::Cuboid {
        half_extents: [10.0, 0.5, 10.0],
    });
    sim.boundary()
        .spawn((Transform::at([0.0, -0.5, 0.0]), RigidBody::fixed(), c))
        .unwrap()
}

fn ball(sim: &mut Sim, at: [f64; 3], radius: f64) -> EntityId {
    let c = Collider::new(Shape::Ball { radius }).with_density(1000.0);
    sim.boundary()
        .spawn((
            Transform::at(at),
            Velocity::default(),
            RigidBody::dynamic(),
            c,
            ExternalForce::default(),
        ))
        .unwrap()
}

/// The causes of a report's failures.
fn causes(r: &StepReport) -> Vec<String> {
    r.errors
        .iter()
        .map(|p| p.detail["cause"]["code"].as_str().unwrap_or("").to_owned())
        .collect()
}

#[test]
fn a_dynamic_hull_with_a_density_is_refused() {
    let mut sim = world();
    let refused = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 5.0, 0.0]),
            RigidBody::dynamic(),
            Collider::new(hull()).with_density(500.0),
        ))
        .unwrap();
    let stated = sim
        .boundary()
        .spawn((
            Transform::at([3.0, 5.0, 0.0]),
            RigidBody::dynamic().with_mass(MassProps::solid_box(80.0, [0.5; 3], [0.0; 3])),
            Collider::new(hull()),
        ))
        .unwrap();
    let r = steps(&mut sim, 30);
    for report in &r {
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        let e = &report.errors[0];
        assert_eq!(e.code, "sim.system_failed");
        assert_eq!(e.detail["system"], "physics.step");
        assert_eq!(e.detail["entity"], refused.get());
        assert_eq!(e.detail["cause"]["code"], "sim.collider_density");
    }
    // Never built: it stays where it was put. The stated one falls.
    assert_eq!(get::<Transform>(&sim, refused).position[1], 5.0);
    assert!(get::<Transform>(&sim, stated).position[1] < 4.0);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 1);
}

#[test]
fn a_ball_falls_lands_and_reports_the_contact() {
    let mut sim = world();
    let g = ground(&mut sim);
    let b = ball(&mut sim, [0.0, 2.0, 0.0], 0.5);
    let reports = steps(&mut sim, 180);
    let y = get::<Transform>(&sim, b).position[1];
    assert!((y - 0.5).abs() < 0.02, "the ball rests at {y}");
    assert!(get::<Velocity>(&sim, b).linear[1].abs() < 0.05);
    let began: Vec<_> = reports
        .iter()
        .flat_map(|r| r.events.iter())
        .filter(|e| e.kind.as_str() == "physics.contact_began")
        .collect();
    assert_eq!(began.len(), 1, "{began:?}");
    assert_eq!(began[0].subject, Some(g));
    assert_eq!(
        began[0].data.get("other"),
        Some(&pocket_sim::PlainData::Number(b.to_f64()))
    );
}

#[test]
fn writes_from_outside_move_the_body() {
    let mut sim = world();
    ground(&mut sim);
    let b = ball(&mut sim, [0.0, 0.5, 0.0], 0.5);
    steps(&mut sim, 30);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<Transform>(e)
        .unwrap()
        .position = [4.0, 3.0, 1.0];
    steps(&mut sim, 1);
    let p = get::<Transform>(&sim, b).position;
    assert!(
        (p[0] - 4.0).abs() < 1e-6 && p[1] < 3.0 && p[1] > 2.9,
        "{p:?}"
    );
    sim.boundary()
        .world_mut()
        .get_mut::<Velocity>(e)
        .unwrap()
        .linear = [0.0, 0.0, 6.0];
    steps(&mut sim, 30);
    assert!(get::<Transform>(&sim, b).position[2] > 3.0);
}

#[test]
fn a_changed_or_removed_collider_changes_the_solver() {
    let mut sim = world();
    ground(&mut sim);
    let b = ball(&mut sim, [0.0, 0.5, 0.0], 0.5);
    steps(&mut sim, 60);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<Collider>(e)
        .unwrap()
        .shape = Shape::Ball { radius: 1.0 };
    steps(&mut sim, 120);
    let y = get::<Transform>(&sim, b).position[1];
    assert!((y - 1.0).abs() < 0.05, "the bigger ball rests at {y}");
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 2);
    sim.boundary().despawn(b).unwrap();
    steps(&mut sim, 1);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 1);
    // Taking the collider away leaves the entity without a body too.
    let c = ball(&mut sim, [5.0, 1.0, 0.0], 0.5);
    steps(&mut sim, 1);
    let e = entity::require(sim.world(), c).unwrap();
    sim.boundary()
        .world_mut()
        .entity_mut(e)
        .remove::<Collider>();
    steps(&mut sim, 1);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 1);
}

#[test]
fn a_non_finite_velocity_is_reported_and_reset() {
    let mut sim = world();
    let b = ball(&mut sim, [0.0, 5.0, 0.0], 0.5);
    steps(&mut sim, 5);
    let before = get::<Transform>(&sim, b);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<Velocity>(e)
        .unwrap()
        .linear = [f64::NAN, 0.0, 0.0];
    let r = steps(&mut sim, 1);
    let codes: Vec<_> = r[0]
        .errors
        .iter()
        .map(|p| p.detail["cause"]["code"].clone())
        .collect();
    assert_eq!(codes, ["number.not_finite"], "{:?}", r[0].errors);
    assert_eq!(get::<Transform>(&sim, b), before);
    assert_eq!(get::<Velocity>(&sim, b), Velocity::default());
    // It falls again from there.
    let r = steps(&mut sim, 10);
    assert!(r.iter().all(|r| r.errors.is_empty()));
    assert!(get::<Transform>(&sim, b).position[1] < before.position[1]);
}

#[test]
fn the_queries() {
    let mut sim = world();
    let g = ground(&mut sim);
    let b = ball(&mut sim, [3.0, 0.5, 0.0], 0.5);
    sim.boundary()
        .spawn((Name::new("Sea").unwrap(), sailing::island_sea()))
        .unwrap();
    sim.boundary().spawn(sailing::breeze(270.0, 6.0)).unwrap();
    steps(&mut sim, 2);
    let hit = cast_ray(sim.world(), [0.0, 5.0, 0.0], [0.0, -1.0, 0.0], 100.0, None).unwrap();
    assert_eq!(hit.entity, g);
    assert!((hit.distance - 5.0).abs() < 1e-3 && (hit.normal[1] - 1.0).abs() < 1e-3);
    let hit = cast_ray(sim.world(), [3.0, 5.0, 0.0], [0.0, -1.0, 0.0], 100.0, None).unwrap();
    assert_eq!(hit.entity, b);
    let past = cast_ray(
        sim.world(),
        [3.0, 5.0, 0.0],
        [0.0, -1.0, 0.0],
        100.0,
        Some(b),
    )
    .unwrap();
    assert_eq!(past.entity, g);
    assert!(cast_ray(sim.world(), [0.0, 5.0, 0.0], [0.0, 1.0, 0.0], 100.0, None).is_none());
    assert_eq!(overlap_ball(sim.world(), [3.0, 0.5, 0.0], 0.2), vec![b]);
    assert_eq!(overlap_ball(sim.world(), [3.0, 0.0, 0.0], 0.2), vec![g, b]);
    let w = water_at(sim.world(), 1.0, 2.0).unwrap();
    assert!(w.height.abs() < 0.35);
    let wind = wind_at(sim.world(), [0.0; 3]);
    assert!((wind[0] - 6.0).abs() < 1e-12);
}

/// On a calm sea the Sloop floats about half under and upright; a crate floats too.
#[test]
fn buoyancy_holds_up_the_sloop_and_a_crate() {
    let mut sim = world();
    sim.boundary().spawn(calm_sea()).unwrap();
    let s = sim.boundary().spawn(sloop([0.0; 3], 0.0)).unwrap();
    let c = sim
        .boundary()
        .spawn(crate_box([5.0, 0.0, 0.0], 0.0))
        .unwrap();
    steps(&mut sim, 600);
    let (tf, boat) = (get::<Transform>(&sim, s), get::<Boat>(&sim, s));
    assert!(boat.afloat && boat.heel_deg.abs() < 2.0, "{boat:?}");
    assert!(tf.position[1] > -0.4 && tf.position[1] < 0.1, "{tf:?}");
    let e = entity::require(sim.world(), c).unwrap();
    let f = sim.world().get::<Floater>(e).unwrap();
    // A crate of 350 kg/m^3 floats with about a third under.
    assert!(f.submerged > 0.25 && f.submerged < 0.45, "{}", f.submerged);
}

/// Positive rudder turns the bow to starboard: the heading grows.
#[test]
fn the_rudder_turns_the_bow_to_starboard() {
    for (rudder, sign) in [(1.0, 1.0), (-1.0, -1.0)] {
        let mut sim = world();
        sim.boundary().spawn(calm_sea()).unwrap();
        let mut b = sloop([0.0; 3], 0.0);
        b.1.linear = [0.0, 0.0, -2.5];
        b.6.rudder = rudder;
        let s = sim.boundary().spawn(b).unwrap();
        steps(&mut sim, 120);
        let h = get::<Boat>(&sim, s).heading_deg;
        let turned = if h > 180.0 { h - 360.0 } else { h };
        assert!(turned * sign > 10.0, "rudder {rudder}: heading {h}");
    }
}

/// In irons (the bow into the wind) the sail draws nothing and the boat makes no way ahead.
#[test]
fn no_drive_head_to_wind() {
    let mut sim = world();
    sim.boundary().spawn(calm_sea()).unwrap();
    sim.boundary().spawn(sailing::breeze(270.0, 6.0)).unwrap();
    let mut b = sloop([0.0; 3], 270.0);
    b.6 = Boat::sail_set();
    let s = sim.boundary().spawn(b).unwrap();
    steps(&mut sim, 300);
    let boat = get::<Boat>(&sim, s);
    assert!(boat.speed < 0.05, "{boat:?}");
    assert!(boat.awa_deg.abs() < 10.0, "{boat:?}");
    assert_eq!(boat.trim, pocket_physics::Trim::Luffing);
    let p = get::<Transform>(&sim, s).position;
    assert!(p[0] > -0.5, "it sailed into the wind to {p:?}");
}

#[test]
fn the_events_reach_the_next_tick() {
    let mut sim = world();
    ground(&mut sim);
    ball(&mut sim, [0.0, 0.52, 0.0], 0.5);
    let r = steps(&mut sim, 5);
    let inbox = sim.world().resource::<EventInbox>();
    assert!(r.iter().any(|r| !r.events.is_empty()));
    assert!(
        inbox
            .events()
            .iter()
            .all(|e| e.kind.as_str().starts_with("physics."))
    );
}

/// `ExternalForce` is zero at every boundary: applied to a dynamic body, dropped on a fixed one or
/// on an entity without a collider, never carried into the next tick.
#[test]
fn gathered_forces_never_carry_over() {
    let mut sim = world();
    sim.boundary().spawn(calm_sea()).unwrap();
    let h = [0.3; 3];
    let fixed = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 0.0, 0.0]),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid { half_extents: h }),
            Floater::solid_box(h),
            ExternalForce::default(),
        ))
        .unwrap();
    let loose = sim
        .boundary()
        .spawn((
            Transform::at([3.0, 0.0, 0.0]),
            RigidBody::dynamic(),
            Floater::solid_box(h),
            ExternalForce::default(),
        ))
        .unwrap();
    for _ in 0..5 {
        steps(&mut sim, 1);
        for id in [fixed, loose] {
            assert_eq!(get::<ExternalForce>(&sim, id), Default::default());
        }
    }
    let e = entity::require(sim.world(), fixed).unwrap();
    assert!(sim.world().get::<Floater>(e).unwrap().submerged > 0.4);
}

/// The ball's contact events with the ground, as `(tick, began)`.
fn contact_log(reports: &[StepReport], ball: EntityId) -> Vec<(u64, bool)> {
    let other = pocket_sim::PlainData::Number(ball.to_f64());
    reports
        .iter()
        .flat_map(|r| r.events.iter().map(move |e| (r.tick.0, e)))
        .filter(|(_, e)| e.data.get("other") == Some(&other))
        .filter_map(|(t, e)| match e.kind.as_str() {
            "physics.contact_began" => Some((t, true)),
            "physics.contact_ended" => Some((t, false)),
            _ => None,
        })
        .collect()
}

/// Contact events stay paired when a touching body goes: a rebuilt body (its `Collider` changed)
/// ends its contacts and begins them again in the same tick, end first, and a despawned one ends
/// them.
#[test]
fn a_rebuilt_or_despawned_body_ends_its_contacts() {
    let mut sim = world();
    let g = ground(&mut sim);
    let b = ball(&mut sim, [0.0, 0.5, 0.0], 0.5);
    let mut reports = steps(&mut sim, 30);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<Collider>(e)
        .unwrap()
        .friction = 0.9;
    reports.extend(steps(&mut sim, 30));
    sim.boundary().despawn(b).unwrap();
    reports.extend(steps(&mut sim, 5));
    assert_eq!(
        contact_log(&reports, b),
        [(1, true), (31, false), (31, true), (61, false)]
    );
    // The ground is the subject of every one: the lower id.
    assert!(
        reports
            .iter()
            .flat_map(|r| r.events.iter())
            .all(|e| e.subject == Some(g))
    );
}

/// A value finite and positive in `f64` but 0 or infinite in the solver's `f32` is refused when
/// the step would build the body, and the body is not built: a ball of radius 1e39, a ground of
/// half extents 1e39, a ball of radius 1e-30 whose mass from its density rounds to 0, stated mass
/// properties of 1e-300. A position of 1e39 is `sim.body_invalid`, not a non-finite solver state.
#[test]
fn values_the_solver_cannot_hold_are_refused() {
    let mut sim = world();
    let huge = ball(&mut sim, [0.0, 5.0, 0.0], 1e39);
    let tiny = ball(&mut sim, [3.0, 5.0, 0.0], 1e-30);
    let ground = sim
        .boundary()
        .spawn((
            Transform::at([0.0, -0.5, 0.0]),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [1e39, 0.5, 1e39],
            }),
        ))
        .unwrap();
    let weightless = sim
        .boundary()
        .spawn((
            Transform::at([6.0, 5.0, 0.0]),
            Velocity::default(),
            RigidBody::dynamic().with_mass(MassProps {
                mass: 1e-300,
                center: [0.0; 3],
                inertia: [1e-300; 3],
            }),
            Collider::new(Shape::Ball { radius: 0.5 }),
        ))
        .unwrap();
    let far = ball(&mut sim, [1e39, 5.0, 0.0], 0.5);
    let r = steps(&mut sim, 3);
    for report in &r {
        let by: Vec<_> = report
            .errors
            .iter()
            .map(|p| {
                (
                    p.detail["entity"].as_u64().unwrap(),
                    p.detail["cause"]["code"].clone(),
                )
            })
            .collect();
        assert_eq!(
            by,
            [
                (huge.get(), "sim.collider_invalid".into()),
                (tiny.get(), "sim.collider_invalid".into()),
                (ground.get(), "sim.collider_invalid".into()),
                (weightless.get(), "sim.body_invalid".into()),
                (far.get(), "sim.body_invalid".into()),
            ],
            "{:?}",
            report.errors
        );
    }
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 0);
    // A body already built and then moved to 1e39 is refused the move once: it stays where it was
    // and its Transform is written back from there.
    let b = ball(&mut sim, [0.0, 5.0, 0.0], 0.5);
    steps(&mut sim, 2);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<Transform>(e)
        .unwrap()
        .position = [1e39, 0.0, 0.0];
    let r = steps(&mut sim, 2);
    let mine = |r: &StepReport| {
        r.errors
            .iter()
            .filter(|p| p.detail["entity"] == b.get())
            .map(|p| p.detail["cause"]["code"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(mine(&r[0]), ["sim.body_invalid"]);
    assert!(mine(&r[1]).is_empty());
    assert!(get::<Transform>(&sim, b).position[0].abs() < 1e-6);
}

/// A `Collider` without a `Transform` gets no body and is reported; taking a body's `Transform`
/// away removes its body. Nothing inserts a missing component.
#[test]
fn a_collider_needs_a_transform() {
    let mut sim = world();
    let bare = sim
        .boundary()
        .spawn(Collider::new(Shape::Ball { radius: 0.5 }))
        .unwrap();
    let r = steps(&mut sim, 1);
    assert_eq!(causes(&r[0]), ["sim.body_invalid"]);
    assert_eq!(
        r[0].errors[0].detail["cause"]["detail"]["reason"],
        "no Transform"
    );
    let e = entity::require(sim.world(), bare).unwrap();
    assert!(sim.world().get::<Transform>(e).is_none());
    sim.boundary().despawn(bare).unwrap();
    let b = ball(&mut sim, [0.0, 5.0, 0.0], 0.5);
    steps(&mut sim, 1);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 1);
    let e = entity::require(sim.world(), b).unwrap();
    sim.boundary()
        .world_mut()
        .entity_mut(e)
        .remove::<Transform>();
    let r = steps(&mut sim, 1);
    assert_eq!(causes(&r[0]), ["sim.body_invalid"]);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 0);
    sim.boundary().despawn(b).unwrap();
    // A body without a Velocity or an ExternalForce moves as if they were zero.
    let plain = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 5.0, 0.0]),
            RigidBody::dynamic(),
            Collider::new(Shape::Ball { radius: 0.5 }).with_density(1000.0),
        ))
        .unwrap();
    let r = steps(&mut sim, 10);
    assert!(r.iter().all(|r| r.errors.is_empty()));
    let e = entity::require(sim.world(), plain).unwrap();
    assert!(sim.world().get::<Velocity>(e).is_none());
    assert!(get::<Transform>(&sim, plain).position[1] < 5.0);
}
