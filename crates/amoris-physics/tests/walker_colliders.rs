//! A Walker stands on and walks over fixed colliders (terrain, piers), as on voxel volumes.
use amoris_physics::{Collider, RigidBody, Shape, Transform, Walker};
use amoris_sim::math;
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
fn fixed(sim: &mut Sim, at: [f64; 3], rotation: [f64; 4], half: [f64; 3]) {
    sim.boundary()
        .spawn((
            Transform {
                position: at,
                rotation,
            },
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid { half_extents: half }),
        ))
        .unwrap();
}
fn walker(sim: &mut Sim, eye: [f64; 3]) -> EntityId {
    sim.boundary()
        .spawn((
            Transform::at(eye),
            Walker {
                gravity_mps2: 9.81,
                ..Walker::default()
            },
        ))
        .unwrap()
}
fn run(sim: &mut Sim, id: EntityId, ticks: u32, forward: f64) -> (Transform, Walker) {
    for _ in 0..ticks {
        let e = entity::require(sim.world(), id).unwrap();
        sim.world_mut().get_mut::<Walker>(e).unwrap().forward = forward;
        let report = sim.step(&mut NoHooks).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
    }
    let e = entity::require(sim.world(), id).unwrap();
    (
        *sim.world().get::<Transform>(e).unwrap(),
        *sim.world().get::<Walker>(e).unwrap(),
    )
}

#[test]
fn a_walker_stands_on_a_fixed_box() {
    let mut sim = world();
    fixed(
        &mut sim,
        [0.0, -0.5, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        [5.0, 0.5, 5.0],
    );
    let id = walker(&mut sim, [0.0, 3.0, 0.0]);
    let (tf, w) = run(&mut sim, id, 120, 0.0);
    assert!(w.grounded, "{w:?}");
    let feet = tf.position[1] - w.eye_m;
    assert!(feet.abs() < 0.01, "feet at {feet}");
}

#[test]
fn a_walker_climbs_a_gentle_slope_and_is_stopped_by_a_wall() {
    let mut sim = world();
    // Flat ground, then a 15 degree ramp rising toward -z.
    fixed(
        &mut sim,
        [0.0, -0.5, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        [5.0, 0.5, 5.0],
    );
    let a = 15f64.to_radians();
    let (s, c) = math::sin_cos(a * 0.5);
    // A box rotated about +x by +a: its top rises toward -z.
    fixed(
        &mut sim,
        [0.0, 1.0 - 0.5 * c, -10.0],
        [s, 0.0, 0.0, c],
        [5.0, 0.5, 5.0],
    );
    let id = walker(&mut sim, [0.0, 1.62, 2.0]);
    let (tf, w) = run(&mut sim, id, 150, 1.0);
    // Walking forward (-z) for 2.5 s at 4.3 m/s: well up the ramp, never through it.
    assert!(
        tf.position[2] < -6.0 && tf.position[2] > -14.0,
        "{:?}",
        tf.position
    );
    // The box rests on its uphill bottom edge, half its width up the ramp from its centre.
    let top = -10.0 + 0.5 * math::sin(a);
    let ground = 1.0 + (top - tf.position[2]).tan_from_angle(a);
    let reach = w.half_width_m * math::tan(a);
    let feet = tf.position[1] - w.eye_m;
    assert!(
        feet > ground + reach - 0.03 && feet < ground + reach + 0.45,
        "feet {feet}, ramp {ground} at z {}",
        tf.position[2]
    );
    // A wall stops the walk.
    let mut sim = world();
    fixed(
        &mut sim,
        [0.0, -0.5, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        [5.0, 0.5, 5.0],
    );
    fixed(
        &mut sim,
        [0.0, 1.5, -2.0],
        [0.0, 0.0, 0.0, 1.0],
        [5.0, 1.5, 0.25],
    );
    let id = walker(&mut sim, [0.0, 1.62, 2.0]);
    let (tf, w) = run(&mut sim, id, 120, 1.0);
    assert!(
        tf.position[2] > -1.75 + w.half_width_m - 0.01,
        "{:?}",
        tf.position
    );
}

trait Ramp {
    fn tan_from_angle(self, a: f64) -> f64;
}
impl Ramp for f64 {
    /// Height a ramp of angle `a` rises over a run of `self` metres.
    fn tan_from_angle(self, a: f64) -> f64 {
        self * math::tan(a)
    }
}
