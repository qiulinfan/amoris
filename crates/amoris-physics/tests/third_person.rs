//! A third-person walker (docs/spec/player.md 5.1): `physics.third_person` turns its moves from
//! their frame into the look's, picks and keeps a lock that turns the look and the body, the rig's
//! arm follows the zoom and is pulled in at a wall, and the pulses act for one tick.
use amoris_physics::{
    Avatar, CameraRig, Collider, Lockable, MoveFrame, RigidBody, Shape, ThirdPerson, Transform,
    Walker,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 3,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}

fn fixed(sim: &mut Sim, at: [f64; 3], half: [f64; 3]) {
    sim.boundary()
        .spawn((
            Transform::at(at),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid { half_extents: half }),
        ))
        .unwrap();
}

/// A floor, the hero (walker, body, rig) at the origin looking north, and its ids.
struct Rig {
    sim: Sim,
    hero: EntityId,
    body: EntityId,
    camera: EntityId,
}

fn rig() -> Rig {
    let mut sim = world();
    fixed(&mut sim, [0.0, -0.5, 0.0], [40.0, 0.5, 40.0]);
    let hero = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 1.62, 0.0]),
            Walker {
                jump_mps: 0.0,
                ..Walker::default()
            },
            ThirdPerson::default(),
        ))
        .unwrap();
    let body = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 0.0, 0.0]),
            Avatar {
                walker: Some(hero),
                turn_rate: 100.0,
                ..Avatar::default()
            },
        ))
        .unwrap();
    let camera = sim
        .boundary()
        .spawn((
            Transform::at([0.0, 2.0, 4.0]),
            CameraRig {
                target: Some(hero),
                ..CameraRig::default()
            },
        ))
        .unwrap();
    Rig {
        sim,
        hero,
        body,
        camera,
    }
}

fn get<C: bevy_ecs::component::Component + Clone>(sim: &Sim, id: EntityId) -> C {
    let e = entity::require(sim.world(), id).unwrap();
    sim.world().get::<C>(e).unwrap().clone()
}

fn set<C: bevy_ecs::component::Component<Mutability = bevy_ecs::component::Mutable>>(
    sim: &mut Sim,
    id: EntityId,
    f: impl FnOnce(&mut C),
) {
    let e = entity::require(sim.world(), id).unwrap();
    f(&mut sim.world_mut().get_mut::<C>(e).unwrap());
}

fn step(sim: &mut Sim, ticks: u32) {
    for _ in 0..ticks {
        let report = sim.step(&mut NoHooks).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
}

fn at(sim: &Sim, id: EntityId) -> [f64; 3] {
    get::<Transform>(sim, id).position
}

#[test]
fn moves_follow_their_frame() {
    // Camera: with the look turned to the west, forward walks west.
    let mut r = rig();
    set::<Walker>(&mut r.sim, r.hero, |w| w.yaw = std::f64::consts::FRAC_PI_2);
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.forward = 1.0);
    step(&mut r.sim, 60);
    let p = at(&r.sim, r.hero);
    assert!(p[0] < -3.5 && p[2].abs() < 0.01, "{p:?}");
    // World: forward walks north whatever the look.
    let mut r = rig();
    set::<Walker>(&mut r.sim, r.hero, |w| w.yaw = 2.0);
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| {
        t.frame = MoveFrame::World;
        t.strafe = 1.0;
    });
    step(&mut r.sim, 60);
    let p = at(&r.sim, r.hero);
    assert!(p[0] > 3.5 && p[2].abs() < 0.01, "east: {p:?}");
    let w: Walker = get(&r.sim, r.hero);
    assert_eq!(w.yaw, 2.0, "the move never turns the look");
    // Body: forward walks the way the body faces, which the scene set to the south.
    let mut r = rig();
    set::<Avatar>(&mut r.sim, r.body, |a| {
        a.yaw = std::f64::consts::PI;
        a.facing = amoris_physics::Facing::Fixed;
    });
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| {
        t.frame = MoveFrame::Body;
        t.forward = 1.0;
    });
    step(&mut r.sim, 60);
    let p = at(&r.sim, r.hero);
    assert!(p[2] > 3.5 && p[0].abs() < 0.01, "south: {p:?}");
    let tp: ThirdPerson = get(&r.sim, r.hero);
    assert!((tp.facing - std::f64::consts::PI).abs() < 1e-9, "{tp:?}");
}

#[test]
fn a_lock_turns_the_look_and_the_body_and_ends_with_its_target() {
    let mut r = rig();
    // A foe to the north-east, one behind the hero, and a nearer one straight ahead that a wall
    // hides: it would score best in sight.
    let foe = r
        .sim
        .boundary()
        .spawn((Transform::at([6.0, 1.62, -6.0]), Lockable {}))
        .unwrap();
    r.sim
        .boundary()
        .spawn((Transform::at([0.0, 1.62, 10.0]), Lockable {}))
        .unwrap();
    fixed(&mut r.sim, [0.0, 1.5, -3.0], [1.0, 1.5, 0.25]);
    let hidden = r
        .sim
        .boundary()
        .spawn((Transform::at([0.0, 1.62, -5.0]), Lockable {}))
        .unwrap();
    step(&mut r.sim, 2);
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.lock_toggle = true);
    step(&mut r.sim, 1);
    let tp: ThirdPerson = get(&r.sim, r.hero);
    assert_eq!(tp.lock, Some(foe), "{tp:?}");
    assert!(!tp.lock_toggle, "a pulse acts for one tick");
    // The look turns at most 8 rad/s toward it, then holds it; the body faces it.
    let w: Walker = get(&r.sim, r.hero);
    assert!((w.yaw + 8.0 / 60.0).abs() < 1e-9, "{}", w.yaw);
    step(&mut r.sim, 30);
    let w: Walker = get(&r.sim, r.hero);
    let goal = -std::f64::consts::FRAC_PI_4;
    assert!((w.yaw - goal).abs() < 1e-9, "{}", w.yaw);
    let a: Avatar = get(&r.sim, r.body);
    assert!((a.yaw - goal).abs() < 1e-6, "{a:?}");
    // Circling: strafing keeps the look on it; forward closes the distance.
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.strafe = 1.0);
    step(&mut r.sim, 90);
    let p = at(&r.sim, r.hero);
    let d0 = (72.0f64).sqrt();
    let (dx, dz) = (6.0 - p[0], -6.0 - p[2]);
    let d = (dx * dx + dz * dz).sqrt();
    assert!((d - d0).abs() < 1.0, "circled at {d} of {d0}");
    // Asking for the hidden one by name locks on to it.
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| {
        t.strafe = 0.0;
        t.lock_request = Some(hidden);
    });
    step(&mut r.sim, 1);
    assert_eq!(get::<ThirdPerson>(&r.sim, r.hero).lock, Some(hidden));
    // Once it is no longer lockable, the lock ends.
    let e = entity::require(r.sim.world(), hidden).unwrap();
    r.sim.world_mut().entity_mut(e).remove::<Lockable>();
    step(&mut r.sim, 1);
    assert_eq!(get::<ThirdPerson>(&r.sim, r.hero).lock, None);
    // A toggle with a lock lets go.
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.lock_request = Some(foe));
    step(&mut r.sim, 1);
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.lock_toggle = true);
    step(&mut r.sim, 1);
    assert_eq!(get::<ThirdPerson>(&r.sim, r.hero).lock, None);
}

#[test]
fn the_arm_follows_the_zoom_and_is_pulled_in_at_a_wall() {
    let mut r = rig();
    step(&mut r.sim, 120);
    let arm = |sim: &Sim| get::<CameraRig>(sim, r.camera).length_m;
    assert!((arm(&r.sim) - 4.2).abs() < 1e-9, "{}", arm(&r.sim));
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.zoom = 0.5);
    step(&mut r.sim, 1);
    assert!((arm(&r.sim) - 2.1).abs() < 1e-9, "shortens at once");
    set::<ThirdPerson>(&mut r.sim, r.hero, |t| t.zoom = 1.5);
    step(&mut r.sim, 120);
    assert!((arm(&r.sim) - 6.3).abs() < 1e-9, "{}", arm(&r.sim));
    // A wall 3 m behind the hero (south): the arm stops short of it.
    fixed(&mut r.sim, [0.0, 2.0, 3.0], [5.0, 2.0, 0.2]);
    step(&mut r.sim, 2);
    let length = arm(&r.sim);
    assert!(length < 2.8 && length > 0.6, "{length}");
    let cam = at(&r.sim, r.camera);
    assert!(cam[2] < 2.8, "{cam:?}");
}

#[test]
fn the_same_inputs_give_the_same_world() {
    let run = || {
        let mut r = rig();
        r.sim
            .boundary()
            .spawn((Transform::at([3.0, 1.62, -5.0]), Lockable {}))
            .unwrap();
        set::<ThirdPerson>(&mut r.sim, r.hero, |t| {
            t.forward = 0.7;
            t.strafe = -0.4;
            t.lock_toggle = true;
        });
        step(&mut r.sim, 90);
        (
            at(&r.sim, r.hero),
            get::<Walker>(&r.sim, r.hero),
            get::<ThirdPerson>(&r.sim, r.hero),
            get::<Avatar>(&r.sim, r.body),
            get::<Transform>(&r.sim, r.camera),
        )
    };
    assert_eq!(run(), run());
}
