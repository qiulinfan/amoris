//! Joints (simulation.md 8.5): a pendulum on a spherical joint, a hinged door that turns within
//! its limits and stays on its hinge, a slider held to its axis and its travel, two boxes welded
//! by a fixed joint, a joint removed or changed between ticks, and the refusals.

use amoris_physics::{
    Collider, ExternalForce, Joint, JointKind, RigidBody, Shape, Transform, Velocity,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, StepReport, TickRate, entity, math};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 3,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}

fn steps(sim: &mut Sim, n: u32) -> Vec<StepReport> {
    (0..n).map(|_| sim.step(&mut NoHooks).unwrap()).collect()
}

fn quiet(reports: &[StepReport]) {
    for r in reports {
        assert!(r.errors.is_empty(), "tick {}: {:?}", r.tick.0, r.errors);
    }
}

fn tf(sim: &Sim, id: EntityId) -> Transform {
    let e = entity::require(sim.world(), id).unwrap();
    *sim.world().get::<Transform>(e).unwrap()
}

fn post(sim: &mut Sim, at: [f64; 3]) -> EntityId {
    let c = Collider::new(Shape::Ball { radius: 0.1 });
    sim.boundary()
        .spawn((Transform::at(at), RigidBody::fixed(), c))
        .unwrap()
}

fn body(sim: &mut Sim, at: [f64; 3], shape: Shape, joint: Option<Joint>) -> EntityId {
    let c = Collider::new(shape).with_density(500.0);
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
    if let Some(j) = joint {
        let e = entity::require(b.world_mut(), id).unwrap();
        b.world_mut().entity_mut(e).insert(j);
    }
    id
}

fn set<C: bevy_ecs::component::Component<Mutability = bevy_ecs::component::Mutable>>(
    sim: &mut Sim,
    id: EntityId,
    f: impl FnOnce(&mut C),
) {
    let mut b = sim.boundary();
    let e = entity::require(b.world_mut(), id).unwrap();
    f(&mut b.world_mut().get_mut::<C>(e).unwrap());
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

fn ball() -> Shape {
    Shape::Ball { radius: 0.2 }
}

#[test]
fn a_pendulum_swings_on_its_spherical_joint_and_falls_when_it_is_removed() {
    let mut sim = world();
    let pivot = post(&mut sim, [0.0, 5.0, 0.0]);
    let bob = body(
        &mut sim,
        [2.0, 5.0, 0.0],
        ball(),
        Some(Joint {
            anchor: [-2.0, 0.0, 0.0],
            ..Joint::new(JointKind::Spherical, pivot)
        }),
    );
    let mut lowest: f64 = 5.0;
    for _ in 0..90 {
        quiet(&steps(&mut sim, 1));
        let p = tf(&sim, bob).position;
        assert!(
            (dist(p, [0.0, 5.0, 0.0]) - 2.0).abs() < 0.05,
            "off its rope: {p:?}"
        );
        lowest = math::min(lowest, p[1]);
    }
    assert!(lowest < 3.2, "it swung down to {lowest}");
    // Without the joint it falls away.
    {
        let mut b = sim.boundary();
        let e = entity::require(b.world_mut(), bob).unwrap();
        b.world_mut().entity_mut(e).remove::<Joint>();
    }
    quiet(&steps(&mut sim, 60));
    assert!(dist(tf(&sim, bob).position, [0.0, 5.0, 0.0]) > 3.0);
}

/// A door hinged about +y at its edge, pushed at its far end: it turns, stops at its limit, and
/// its hinge stays where it was.
#[test]
fn a_hinged_door_turns_within_its_limits_and_stays_on_its_hinge() {
    let mut sim = world();
    let frame = post(&mut sim, [0.0, 1.0, 0.0]);
    let door = body(
        &mut sim,
        [0.5, 1.0, 0.0],
        Shape::Cuboid {
            half_extents: [0.5, 1.0, 0.05],
        },
        Some(Joint {
            anchor: [-0.5, 0.0, 0.0],
            axis: [0.0, 1.0, 0.0],
            limits: Some([-0.5, 0.5]),
            ..Joint::new(JointKind::Revolute, frame)
        }),
    );
    for _ in 0..120 {
        set::<ExternalForce>(&mut sim, door, |f| {
            f.add_at([0.0, 0.0, 30.0], [1.0, 1.0, 0.0], [0.5, 1.0, 0.0])
        });
        quiet(&steps(&mut sim, 1));
    }
    let t = tf(&sim, door);
    // The door's edge (its local -0.5 on x) is still at the hinge.
    let q = t.rotation;
    let edge = amoris_physics::geom::add(
        t.position,
        amoris_physics::geom::rotate(q, [-0.5, 0.0, 0.0]),
    );
    assert!(
        dist(edge, [0.0, 1.0, 0.0]) < 0.05,
        "hinge moved to {edge:?}"
    );
    // Turned about y by at most the limit (a yaw of 0.5 rad), and not about other axes much.
    let yaw = 2.0 * math::atan2(q[1], q[3]);
    assert!(yaw.abs() > 0.3 && yaw.abs() < 0.55, "yaw {yaw}");
    assert!(q[0].abs() < 0.05 && q[2].abs() < 0.05, "{q:?}");
}

#[test]
fn a_slider_keeps_to_its_axis_and_its_travel() {
    let mut sim = world();
    let rail = post(&mut sim, [0.0, 2.0, 0.0]);
    let slider = body(
        &mut sim,
        [0.0, 2.0, 0.0],
        ball(),
        Some(Joint {
            axis: [1.0, 0.0, 0.0],
            limits: Some([-1.0, 1.0]),
            ..Joint::new(JointKind::Prismatic, rail)
        }),
    );
    for _ in 0..120 {
        set::<ExternalForce>(&mut sim, slider, |f| f.force = [20.0, 0.0, 5.0]);
        quiet(&steps(&mut sim, 1));
    }
    let p = tf(&sim, slider).position;
    assert!(
        (p[1] - 2.0).abs() < 0.02 && p[2].abs() < 0.02,
        "off the rail: {p:?}"
    );
    assert!(p[0] > 0.9 && p[0] < 1.05, "at the end of its travel: {p:?}");
}

#[test]
fn welded_boxes_fall_as_one_and_a_changed_joint_is_rebuilt() {
    let mut sim = world();
    let cube = Shape::Cuboid {
        half_extents: [0.25; 3],
    };
    let a = body(&mut sim, [0.0, 4.0, 0.0], cube.clone(), None);
    let b = body(
        &mut sim,
        [0.6, 4.0, 0.0],
        cube,
        Some(Joint {
            anchor: [-0.3, 0.0, 0.0],
            other_anchor: [0.3, 0.0, 0.0],
            ..Joint::new(JointKind::Fixed, a)
        }),
    );
    quiet(&steps(&mut sim, 30));
    let (pa, pb) = (tf(&sim, a).position, tf(&sim, b).position);
    assert!(pa[1] < 3.0, "they fell: {pa:?}");
    assert!((dist(pa, pb) - 0.6).abs() < 0.01, "{pa:?} {pb:?}");
    // The weld made longer: the solver's joint is rebuilt with it.
    set::<Joint>(&mut sim, b, |j| j.anchor = [-0.6, 0.0, 0.0]);
    quiet(&steps(&mut sim, 30));
    let (pa, pb) = (tf(&sim, a).position, tf(&sim, b).position);
    assert!((dist(pa, pb) - 0.9).abs() < 0.02, "{pa:?} {pb:?}");
}

/// The refusals, every tick until mended: a joint to the entity itself, to an entity with no
/// body, with a zero axis, with limits upside down.
#[test]
fn a_joint_that_cannot_be_built_is_reported() {
    let mut sim = world();
    let nothing = sim.boundary().spawn(()).unwrap();
    let id = body(&mut sim, [0.0, 3.0, 0.0], ball(), None);
    let cases = [
        Joint::new(JointKind::Fixed, id),
        Joint::new(JointKind::Spherical, nothing),
        Joint {
            axis: [0.0; 3],
            ..Joint::new(JointKind::Revolute, nothing)
        },
        Joint {
            limits: Some([1.0, -1.0]),
            ..Joint::new(JointKind::Prismatic, nothing)
        },
    ];
    for j in cases {
        {
            let mut b = sim.boundary();
            let e = entity::require(b.world_mut(), id).unwrap();
            b.world_mut().entity_mut(e).insert(j);
        }
        for r in steps(&mut sim, 2) {
            let codes: Vec<&str> = r
                .errors
                .iter()
                .map(|p| p.detail["cause"]["code"].as_str().unwrap_or(""))
                .collect();
            assert_eq!(codes, ["sim.joint_invalid"], "{j:?}");
        }
    }
}
