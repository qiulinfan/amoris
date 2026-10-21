//! Actual force/torque from field columns, including finite depth and sea/region priority.
use amoris_physics::{
    BuoyPoint, Collider, ExternalForce, Floater, MassProps, RigidBody, Sea, Shape, Transform,
    Velocity,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

fn world() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    sim
}
fn field(sim: &mut Sim, surface: f64, depth: f64, speed: f64) {
    let mut region = amoris_physics::water::dam_scene(1);
    region.cell_m = 1.0;
    region.dry_below = 0.0;
    for tile in &mut region.tiles {
        tile.bed.fill(surface - depth);
        tile.depth.fill(depth);
        tile.u.fill(speed);
        tile.w.fill(0.0);
        for n in (0..256).step_by(16) {
            tile.u[n] = 0.0;
        }
    }
    sim.boundary().spawn((region,)).unwrap();
}
fn body(sim: &mut Sim, at: [f64; 3], size: f64, offset: f64) -> EntityId {
    sim.boundary()
        .spawn((
            Transform::at(at),
            Velocity::default(),
            RigidBody::dynamic().with_mass(MassProps::solid_box(100.0, [0.2; 3], [0.0; 3])),
            Collider::new(Shape::Cuboid {
                half_extents: [0.2; 3],
            }),
            ExternalForce::default(),
            Floater {
                points: vec![BuoyPoint {
                    at: [offset, 0.0, 0.0],
                    volume: 0.2,
                    size,
                }],
                drag: 1.0,
                heave: 0.0,
                submerged: 0.0,
            },
        ))
        .unwrap()
}
fn step(sim: &mut Sim, id: EntityId) -> (Velocity, f64) {
    let report = sim.step(&mut NoHooks).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let e = entity::require(sim.world(), id).unwrap();
    (
        *sim.world().get::<Velocity>(e).unwrap(),
        sim.world().get::<Floater>(e).unwrap().submerged,
    )
}
#[test]
fn field_lifts_advects_and_turns_a_rigid_body_without_a_sea() {
    let mut sim = world();
    field(&mut sim, 2.0, 2.0, 2.0);
    let id = body(&mut sim, [8.0, 1.0, 8.0], 0.2, 0.5);
    let (v, submerged) = step(&mut sim, id);
    assert_eq!(submerged, 1.0);
    assert!(v.linear[1] > 0.15 && v.linear[1] < 0.18, "{v:?}");
    assert!(v.linear[0] > 0.05 && v.linear[0] < 0.08, "{v:?}");
    assert!(
        v.angular[2] > 0.1,
        "off-centre Archimedes force must exert torque: {v:?}"
    );
}
#[test]
fn bed_clips_displaced_volume_and_a_dry_field_masks_the_sea() {
    let mut sim = world();
    field(&mut sim, 2.0, 0.2, 0.0);
    let id = body(&mut sim, [8.0, 1.7, 8.0], 0.4, 0.0);
    let (v, submerged) = step(&mut sim, id);
    assert!((submerged - 0.25).abs() < 1e-9, "{submerged}");
    assert!(v.linear[1] < -0.07 && v.linear[1] > -0.09, "{v:?}");
    let mut sim = world();
    sim.boundary()
        .spawn((Sea {
            level: 10.0,
            ..Sea::default()
        },))
        .unwrap();
    field(&mut sim, 0.0, 0.0, 0.0);
    let id = body(&mut sim, [8.0, 1.0, 8.0], 0.2, 0.0);
    let (v, submerged) = step(&mut sim, id);
    assert_eq!(submerged, 0.0);
    assert!(v.linear[1] < -0.15, "{v:?}");
}
#[test]
fn points_outside_the_field_use_the_sea_and_first_region_owns_overlaps() {
    let mut sim = world();
    field(&mut sim, 0.0, 0.0, 0.0);
    field(&mut sim, 2.0, 2.0, 0.0);
    sim.boundary()
        .spawn((Sea {
            level: 2.0,
            ..Sea::default()
        },))
        .unwrap();
    let outside = body(&mut sim, [20.0, 1.0, 8.0], 0.2, 0.0);
    let inside = body(&mut sim, [8.0, 1.0, 8.0], 0.2, 0.0);
    let (v, wet) = step(&mut sim, outside);
    assert_eq!(wet, 1.0);
    assert!(v.linear[1] > 0.15);
    let e = entity::require(sim.world(), inside).unwrap();
    assert_eq!(sim.world().get::<Floater>(e).unwrap().submerged, 0.0);
    assert!(sim.world().get::<Velocity>(e).unwrap().linear[1] < -0.15);
}
