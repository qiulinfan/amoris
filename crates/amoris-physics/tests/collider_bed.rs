use amoris_physics::{
    Collider, RigidBody, Shape, Transform,
    water::{BedSource, WaterColliderBed, WaterRegion},
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, TickRate, entity};
fn sim() -> Sim {
    let mut s = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap();
    amoris_physics::plugin(&mut s).unwrap();
    s
}
fn region(wet: bool) -> WaterRegion {
    let mut r = amoris_physics::water::dam_scene(1);
    r.cell_m = 0.25;
    r.size = [16; 2];
    r.bed = BedSource::Colliders {
        top: 2.0,
        bottom: -1.0,
    };
    r.dry_below = 0.0;
    r.edges = Default::default();
    r.friction = 0.0;
    if wet {
        for t in &mut r.tiles {
            t.bed.fill(-1.0);
            t.depth.fill(2.0);
            t.u.fill(0.0);
            t.w.fill(0.0);
        }
    } else {
        r.tiles.clear();
    }
    r
}
fn slope(s: &mut Sim) -> (EntityId, Transform) {
    let a = amoris_sim::math::atan2(0.1, 1.0);
    let (sn, cs) = amoris_sim::math::sin_cos(a * 0.5);
    let tf = Transform {
        position: [2.0, 0.2 - 0.25 / amoris_sim::math::cos(a), 2.0],
        rotation: [0.0, 0.0, sn, cs],
    };
    let id = s
        .boundary()
        .spawn((
            tf,
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [3.0, 0.25, 3.0],
            }),
        ))
        .unwrap();
    (id, tf)
}
fn step(s: &mut Sim) {
    let r = s.step(&mut NoHooks).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}
fn read(s: &Sim, id: EntityId) -> WaterRegion {
    s.world()
        .get::<WaterRegion>(entity::require(s.world(), id).unwrap())
        .unwrap()
        .clone()
}
#[test]
fn first_tick_uses_current_fixed_geometry_and_a_continuous_plane() {
    let mut s = sim();
    slope(&mut s);
    let id = s.boundary().spawn((region(false),)).unwrap();
    step(&mut s);
    let r = read(&s, id);
    let mut error = 0.0f64;
    for z in 0..16 {
        for x in 0..16 {
            let (t, n) = r.locate(x, z);
            let expected = (x as f64 + 0.5) * 0.025;
            error = amoris_sim::math::max(error, (r.tiles[t].bed[n] - expected).abs());
            assert_eq!(r.tiles[t].depth[n], 0.0);
        }
    }
    assert!(error < 2e-6, "analytic plane {error}");
    assert!(
        s.world()
            .get::<WaterColliderBed>(entity::require(s.world(), id).unwrap())
            .is_some()
    );
    eprintln!("collider plane max error={error}");
}
#[test]
fn fixed_edits_rebed_raise_displacement_and_lowering_keeps_water() {
    let mut s = sim();
    let (floor, mut tf) = slope(&mut s);
    let id = s.boundary().spawn((region(true),)).unwrap();
    step(&mut s);
    let first = read(&s, id);
    let volume = first.volume();
    tf.position[1] += 0.2;
    let e = entity::require(s.world(), floor).unwrap();
    s.world_mut().get_mut::<Transform>(e).unwrap().position = tf.position;
    step(&mut s);
    let raised = read(&s, id);
    assert!((volume - raised.volume() - 3.2).abs() < 2e-5);
    let v = raised.volume();
    tf.position[1] -= 0.3;
    let e = entity::require(s.world(), floor).unwrap();
    s.world_mut().get_mut::<Transform>(e).unwrap().position = tf.position;
    step(&mut s);
    let lower = read(&s, id);
    assert!((lower.volume() - v).abs() < 1e-9);
    let (t, n) = lower.locate(8, 8);
    assert!((lower.tiles[t].bed[n] - (-0.1 + 8.5 * 0.025)).abs() < 2e-6);
}
#[test]
fn fixed_roofs_win_but_dynamic_bodies_are_not_bed_and_missing_geometry_uses_bottom() {
    let mut s = sim();
    slope(&mut s);
    s.boundary()
        .spawn((
            Transform::at([2.0, 1.0, 2.0]),
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [0.5, 0.1, 0.5],
            }),
        ))
        .unwrap();
    let mut c = Collider::new(Shape::Cuboid {
        half_extents: [0.4; 3],
    });
    c.density = Some(1000.0);
    s.boundary()
        .spawn((Transform::at([2.0, 1.7, 2.0]), RigidBody::dynamic(), c))
        .unwrap();
    let id = s.boundary().spawn((region(false),)).unwrap();
    let other = s
        .boundary()
        .spawn((Transform::at([20.0, 0.0, 20.0]), region(false)))
        .unwrap();
    step(&mut s);
    let r = read(&s, id);
    let (t, n) = r.locate(8, 8);
    assert!((r.tiles[t].bed[n] - 1.1).abs() < 2e-6);
    assert!(
        read(&s, other)
            .tiles
            .iter()
            .flat_map(|t| &t.bed)
            .all(|b| *b == -1.0)
    );
}

#[test]
fn a_pose_sign_bit_edit_has_the_same_geometry_identity_with_a_fresh_cache() {
    let state = |s: &Sim, id| {
        s.world()
            .get::<WaterColliderBed>(entity::require(s.world(), id).unwrap())
            .unwrap()
            .source
    };
    let mut s = sim();
    let (floor, mut tf) = slope(&mut s);
    let id = s.boundary().spawn((region(false),)).unwrap();
    step(&mut s);
    let old = state(&s, id);
    tf.rotation[0] = -0.0;
    let e = entity::require(s.world(), floor).unwrap();
    s.world_mut().get_mut::<Transform>(e).unwrap().rotation = tf.rotation;
    step(&mut s);
    let edited = state(&s, id);
    assert_ne!(old, edited);
    let mut fresh = sim();
    fresh
        .boundary()
        .spawn((
            tf,
            RigidBody::fixed(),
            Collider::new(Shape::Cuboid {
                half_extents: [3.0, 0.25, 3.0],
            }),
        ))
        .unwrap();
    let fresh_id = fresh.boundary().spawn((region(false),)).unwrap();
    step(&mut fresh);
    assert_eq!(edited, state(&fresh, fresh_id));
}
