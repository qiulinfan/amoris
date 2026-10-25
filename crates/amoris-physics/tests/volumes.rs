//! Rapier bodies and ray casts against block volumes (docs/spec/voxels.md 5.1): a body lands on a
//! volume and sleeps, a dig under it wakes it and it falls through, the solver's chunk shapes are
//! written by reference except while a chunk has changed since the last step, forks and cold
//! restores continue with their source's hashes, and `query::cast_ray` meets the volume.

use amoris_physics::probe::blocks::{
    DIG_TICK, blocks_world, cache_bytes, check_volume_forks, chunk_keys, fresh_blocks, middle_body,
    volume_id,
};
use amoris_physics::probe::{Ledger, fork, ledger};
use amoris_physics::query::cast_ray;
use amoris_physics::{BodyIndex, Physics, PhysicsCache, RebuiltSolver, Transform};
use amoris_sim::persisted::PersistedCache;
use amoris_sim::voxels::Voxels;
use amoris_sim::{EntityId, NoHooks, Sim};

fn step(sim: &mut Sim) {
    let r = sim.step(&mut NoHooks).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn steps(sim: &mut Sim, n: u64) {
    for _ in 0..n {
        step(sim);
    }
}

fn hash(l: &Ledger, sim: &Sim) -> u64 {
    l.snapshot(sim.world()).unwrap().hash()
}

fn position(sim: &Sim, id: EntityId) -> [f64; 3] {
    let e = amoris_sim::entity::require(sim.world(), id).unwrap();
    sim.world().get::<Transform>(e).unwrap().position
}

fn sleeping(sim: &Sim, id: EntityId) -> bool {
    let h = sim.world().resource::<BodyIndex>().get(id).unwrap();
    sim.world().resource::<Physics>().world.bodies[h].is_sleeping()
}

/// The bodies come to rest on the floor (a box's centre and a ball's 0.5 m up, also the one across
/// the chunk border) and sleep; the dig under the middle box wakes it, it falls through the hole,
/// and the others stay where they are.
#[test]
fn a_body_lands_on_a_volume_and_falls_through_a_dig() {
    let mut sim = blocks_world(1.0, 1);
    steps(&mut sim, DIG_TICK - 1);
    let middle = middle_body(1);
    for k in 2..=10 {
        let id = EntityId::new(k).unwrap();
        let p = position(&sim, id);
        assert!((p[1] - 0.5).abs() < 0.02, "body {k} rests at {p:?}");
        assert!(sleeping(&sim, id), "body {k} sleeps");
    }
    let chunks = sim.world().resource::<Physics>().volumes.len();
    assert_eq!(chunks, 9, "one collider per chunk of the 40 x 40 floor");
    steps(&mut sim, 60);
    let p = position(&sim, middle);
    assert!(p[1] < -3.0, "the middle box fell through the hole to {p:?}");
    for k in (2..=10).filter(|&k| k != middle.get()) {
        let p = position(&sim, EntityId::new(k).unwrap());
        assert!((p[1] - 0.5).abs() < 0.02, "body {k} stayed at {p:?}");
    }
}

/// A ray down onto the floor meets the volume at its top, facing up, and leaves out an excluded
/// body; through the dug hole, after the box has fallen far, it meets nothing within its reach.
#[test]
fn cast_ray_meets_a_volume() {
    let mut sim = blocks_world(1.0, 1);
    step(&mut sim);
    let hit = cast_ray(
        sim.world(),
        [10.0, 10.0, 10.0],
        [0.0, -1.0, 0.0],
        20.0,
        None,
    )
    .unwrap();
    assert_eq!(hit.entity, volume_id());
    assert!((hit.distance - 10.0).abs() < 1e-4, "{hit:?}");
    assert!((hit.normal[1] - 1.0).abs() < 1e-4, "{hit:?}");
    let side = cast_ray(
        sim.world(),
        [30.0, -1.5, 10.0],
        [-1.0, 0.0, 0.0],
        20.0,
        None,
    )
    .unwrap();
    assert_eq!(side.entity, volume_id());
    assert!((side.distance - 10.0).abs() < 1e-4, "{side:?}");
    steps(&mut sim, DIG_TICK - 2);
    let down = cast_ray(
        sim.world(),
        [0.0, 10.0, 0.0],
        [0.0, -1.0, 0.0],
        20.0,
        Some(middle_body(1)),
    );
    assert_eq!(down.map(|h| h.entity), Some(volume_id()));
    steps(&mut sim, 120);
    assert_eq!(
        cast_ray(sim.world(), [0.0, 10.0, 0.0], [0.0, -1.0, 0.0], 20.0, None),
        None
    );
}

/// `physics.volume_fork`, the web test: forks at every tick and a cold restore before the dig.
#[test]
fn forks_and_cold_restores_keep_the_hashes() {
    check_volume_forks().unwrap();
}

/// A boundary write to the volume leaves the solver's chunk as it was until the next step, so a
/// snapshot then writes that chunk's shape inline, and a fork continues with its source's hashes;
/// after the step, which rebuilds the chunk, it is by reference again.
#[test]
fn a_chunk_changed_since_the_step_is_written_inline() {
    let l = ledger();
    let mut sim = blocks_world(0.75, 1);
    steps(&mut sim, 5);
    let colliders = chunk_keys(&sim).len();
    let fresh = cache_bytes(&sim).unwrap();
    assert!(fresh.by_reference(colliders), "{fresh:?}");
    let by_ref = fresh.cache;
    let e = amoris_sim::entity::require(sim.world(), volume_id()).unwrap();
    let tick = sim.clock().tick.0;
    sim.boundary()
        .world_mut()
        .get_mut::<Voxels>(e)
        .unwrap()
        .set([3, 2, 3], 0, tick)
        .unwrap();
    let stale = cache_bytes(&sim).unwrap().cache;
    assert!(
        stale > by_ref + 1000,
        "{stale} bytes, by reference {by_ref}"
    );
    let mut f = fork(&l, &sim, fresh_blocks).unwrap();
    assert_eq!(
        l.snapshot(f.world()).unwrap(),
        l.snapshot(sim.world()).unwrap()
    );
    for k in 1..=30 {
        step(&mut sim);
        step(&mut f);
        assert_eq!(hash(&l, &f), hash(&l, &sim), "tick 5 + {k}");
    }
    let again = cache_bytes(&sim).unwrap();
    assert!(again.by_reference(colliders), "{again:?}");
}

/// A cache decoded where the volume is missing cannot build its chunks' shapes: the solver is
/// rebuilt from the components at the next step, which says so.
#[test]
fn a_chunk_shape_without_its_volume_rebuilds_the_solver() {
    let l = ledger();
    let mut sim = blocks_world(0.625, 0);
    steps(&mut sim, 3);
    let snap = l.snapshot(sim.world()).unwrap();
    drop(sim);
    let mut bare = fresh_blocks();
    PhysicsCache::decode(snap.section("physics.rapier").unwrap())
        .unwrap()
        .apply(bare.world_mut());
    assert_eq!(bare.world().resource::<Physics>().world.bodies.len(), 0);
    let rebuilt = bare.world().get_resource::<RebuiltSolver>().is_some();
    assert!(rebuilt, "the solver was rebuilt");
}
