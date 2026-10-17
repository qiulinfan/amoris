//! Determinism, forks and restores of the sailing scene through the persistence declarations
//! (charter 3.3; persistence.md 7 and 8; simulation.md 8 and 12): two runs agree tick by tick, a
//! fork at every tick continues identically, a restored snapshot continues in lockstep with its
//! original, storage order changes nothing, and the cache survives its own bytes.

use pocket_physics::probe::{
    self, Ledger, Snap, chain, check_forks, fork, fresh, ledger, level, sailing_world,
};
use pocket_physics::{Physics, PhysicsCache, RebuiltSolver};
use pocket_sim::persisted::PersistedCache;
use pocket_sim::{NoHooks, Sim};

fn step(sim: &mut Sim) {
    let r = sim.step(&mut NoHooks).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn hash(l: &Ledger, sim: &Sim) -> u64 {
    l.snapshot(sim.world()).unwrap().hash()
}

#[test]
fn two_runs_agree_at_every_tick() {
    let a = chain(600).unwrap();
    let b = chain(600).unwrap();
    assert_eq!(a, b);
    // The state moves: no two consecutive hashes are equal.
    assert!(a.windows(2).all(|w| w[0] != w[1]));
}

#[test]
fn a_fork_at_every_tick_continues_identically() {
    check_forks().unwrap();
}

/// persistence.md P2 and P3 over this crate's sections: at ticks 0, 1, 60 and 600 a snapshot
/// restores into a fresh world whose snapshot has the same bytes, and that world runs 600 ticks
/// with the hashes of the original, which is never restored (`chain`).
#[test]
fn a_restore_continues_in_lockstep() {
    let l = ledger();
    let reference = chain(1200).unwrap();
    let mut sim = sailing_world();
    let mut at = 0;
    for mark in [0u64, 1, 60, 600] {
        while at < mark {
            step(&mut sim);
            at += 1;
        }
        let snap = l.snapshot(sim.world()).unwrap();
        let mut copy = fresh();
        l.restore(&mut copy, &snap).unwrap();
        assert_eq!(
            l.snapshot(copy.world()).unwrap(),
            snap,
            "round trip at tick {mark}"
        );
        let start = usize::try_from(mark).unwrap();
        for k in 1..=600 {
            step(&mut copy);
            assert_eq!(hash(&l, &copy), reference[start + k], "tick {mark} + {k}");
        }
    }
}

/// A restore adds no component: with the Sloop's `Velocity` removed (a body then moves as if it
/// were zero and shows none), a fork's snapshot equals its source's and continues with its hashes.
#[test]
fn a_body_without_its_velocity_forks_as_it_is() {
    let l = ledger();
    let mut sim = sailing_world();
    let sloop = pocket_sim::EntityId::new(3).unwrap();
    let e = pocket_sim::entity::require(sim.world(), sloop).unwrap();
    sim.boundary()
        .world_mut()
        .entity_mut(e)
        .remove::<pocket_physics::Velocity>();
    step(&mut sim);
    step(&mut sim);
    let mut f = fork(&l, &sim, fresh).unwrap();
    assert_eq!(
        l.snapshot(f.world()).unwrap(),
        l.snapshot(sim.world()).unwrap()
    );
    let e = pocket_sim::entity::require(f.world(), sloop).unwrap();
    assert!(f.world().get::<pocket_physics::Velocity>(e).is_none());
    for k in 1..=60 {
        step(&mut sim);
        step(&mut f);
        assert_eq!(hash(&l, &f), hash(&l, &sim), "tick 2 + {k}");
    }
}

/// simulation.md 12, item 2: the world's storage permuted before every tick gives the same chain.
#[test]
fn storage_order_changes_nothing() {
    let l = ledger();
    let mut plain = sailing_world();
    let mut shuffled = sailing_world();
    for t in 0..300u64 {
        pocket_sim::order::shuffle_storage(shuffled.world_mut(), t);
        step(&mut plain);
        step(&mut shuffled);
        assert_eq!(hash(&l, &plain), hash(&l, &shuffled), "tick {}", t + 1);
    }
}

/// The declarations cover the physics state: every section the ledger writes is named, the cache
/// among them, and nothing physics keeps is left unclassified.
#[test]
fn the_declarations_name_every_section() {
    let l = ledger();
    let classes: Vec<_> = l.names.iter().map(|(n, c)| format!("{c} {n}")).collect();
    for want in [
        "Cache physics.rapier",
        "Component Transform",
        "Component Velocity",
        "Component RigidBody",
        "Component Collider",
        "Component ExternalForce",
        "Component Floater",
        "Component Boat",
        "Component Sail",
        "Component Hull",
        "Component Sea",
        "Component Wind",
        "Derived pocket_physics::solver::Physics",
        "Derived pocket_physics::solver::BodyIndex",
        "Derived pocket_physics::solver::ContactLog",
        "Ignored pocket_physics::solver::RebuiltSolver",
    ] {
        assert!(
            classes.iter().any(|c| c == want),
            "{want} missing from {classes:?}"
        );
    }
    let mut sim = sailing_world();
    step(&mut sim);
    let snap: Snap = l.snapshot(sim.world()).unwrap();
    let names: Vec<_> = snap.sections.iter().map(|s| s.0).collect();
    for want in [
        "entities",
        "SimClock",
        "physics.rapier",
        "Transform",
        "Boat",
        "Sea",
        "Wind",
    ] {
        assert!(names.contains(&want), "{want} not in {names:?}");
    }
}

/// The cache's bytes decode to a solver that encodes to the same bytes; trailing bytes and a body
/// that names no entity are refused; an empty section means no solver.
#[test]
fn the_cache_round_trips_and_refuses_bad_bytes() {
    let mut sim = sailing_world();
    for _ in 0..30 {
        step(&mut sim);
    }
    let mut bytes = Vec::new();
    PhysicsCache::encode(sim.world(), &mut bytes).unwrap();
    let mut copy = fresh();
    PhysicsCache::decode(&bytes)
        .unwrap()
        .apply(copy.world_mut());
    let mut again = Vec::new();
    PhysicsCache::encode(copy.world(), &mut again).unwrap();
    assert_eq!(bytes, again);

    let mut longer = bytes.clone();
    longer.push(0);
    let e = PhysicsCache::decode(&longer).err().unwrap();
    assert_eq!(e.code, "persist.noncanonical");

    let mut bad = Physics::new(1.0 / 60.0);
    bad.world
        .insert_body(rapier3d::prelude::RigidBodyBuilder::dynamic());
    let mut w = bevy_ecs::prelude::World::new();
    w.insert_resource(bad);
    let mut orphan = Vec::new();
    PhysicsCache::encode(&w, &mut orphan).unwrap();
    assert_eq!(
        PhysicsCache::decode(&orphan).err().unwrap().code,
        "persist.noncanonical"
    );

    let mut empty = Vec::new();
    PhysicsCache::encode(&bevy_ecs::prelude::World::new(), &mut empty).unwrap();
    assert!(empty.is_empty());
    let mut gone = fresh();
    PhysicsCache::decode(&empty)
        .unwrap()
        .apply(gone.world_mut());
    assert!(gone.world().get_resource::<Physics>().is_none());
}

/// versions.md 3.6: the identity names the versions `Cargo.lock` pins.
#[test]
fn the_cache_identity_matches_the_lock_file() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    let version = |name: &str| {
        let at = lock
            .find(&format!("name = \"{name}\"\nversion = \""))
            .unwrap_or_else(|| panic!("{name} not locked"));
        let rest = &lock[at..];
        let v = rest.split('"').nth(3).unwrap();
        v.to_owned()
    };
    let id = PhysicsCache::identity();
    for name in ["rapier3d", "parry3d", "bincode"] {
        assert!(
            id.contains(&format!("{name} {}", version(name))),
            "{id} against {name} {}",
            version(name)
        );
    }
    assert_eq!(id, pocket_physics::IDENTITY);
}

/// The cache rebuilt from the components (versions.md 6.3): a new solver whose bodies come back at
/// the next step, close to where they were.
#[test]
fn the_cache_rebuilds_from_the_components() {
    let mut sim = sailing_world();
    for _ in 0..60 {
        step(&mut sim);
    }
    let before = probe::boat_position(&sim);
    PhysicsCache::rebuild(sim.world_mut()).unwrap();
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 0);
    step(&mut sim);
    assert_eq!(sim.world().resource::<Physics>().world.bodies.len(), 7);
    let after = probe::boat_position(&sim);
    let d = pocket_physics::geom::length(pocket_physics::geom::sub(after, before));
    assert!(d < 0.2, "the rebuilt boat moved {d}");
}

/// persistence.md 8, layout 2: a fixed collider's shape is in the cache only while its components
/// no longer build it. A boundary write to the terrain's `Collider` leaves the solver's collider as
/// it was until the next step, so a snapshot then writes the shape inline and a fork continues with
/// its source's hashes; after the step, which rebuilds the terrain, it is by reference again.
#[test]
fn a_stale_fixed_collider_is_written_inline() {
    let l = ledger();
    let mut sim = level::level_world(19, 0);
    step(&mut sim);
    step(&mut sim);
    let (by_ref, terrain) = level::cache_and_terrain_bytes(&sim).unwrap();
    assert!(
        by_ref < terrain,
        "{by_ref} bytes with a {terrain}-byte terrain"
    );
    let e = pocket_sim::entity::require(sim.world(), level::terrain_id()).unwrap();
    sim.boundary()
        .world_mut()
        .get_mut::<pocket_physics::Collider>(e)
        .unwrap()
        .friction = 0.8;
    let (stale, _) = level::cache_and_terrain_bytes(&sim).unwrap();
    assert!(
        stale > terrain,
        "{stale} bytes with a {terrain}-byte terrain"
    );
    let mut f = fork(&l, &sim, fresh).unwrap();
    assert_eq!(
        l.snapshot(f.world()).unwrap(),
        l.snapshot(sim.world()).unwrap()
    );
    for k in 1..=30 {
        step(&mut sim);
        step(&mut f);
        assert_eq!(hash(&l, &f), hash(&l, &sim), "tick 2 + {k}");
    }
    let (again, _) = level::cache_and_terrain_bytes(&sim).unwrap();
    assert!(
        again < terrain,
        "{again} bytes with a {terrain}-byte terrain"
    );
}

/// A snapshot whose shapes by reference no world holds any more restores by building them from the
/// restored components (no other test uses this scene, so nothing else holds its terrain), and
/// continues in lockstep with an unbroken run; decoded where those components are missing, the
/// cache falls back to a solver rebuilt from the components at the next step, and says so.
#[test]
fn a_cold_restore_builds_the_shapes_from_the_components() {
    let l = ledger();
    let reference: Vec<u64> = {
        let mut sim = level::level_world(18, 0);
        (0..40)
            .map(|_| {
                step(&mut sim);
                hash(&l, &sim)
            })
            .collect()
    };
    let mut sim = level::level_world(18, 0);
    for _ in 0..10 {
        step(&mut sim);
    }
    let snap = l.snapshot(sim.world()).unwrap();
    drop(sim);
    let mut cold = fresh();
    l.restore(&mut cold, &snap).unwrap();
    assert_eq!(l.snapshot(cold.world()).unwrap(), snap);
    for (t, want) in reference.iter().enumerate().skip(10) {
        step(&mut cold);
        assert_eq!(hash(&l, &cold), *want, "tick {}", t + 1);
    }
    drop(cold);

    let mut bare = fresh();
    PhysicsCache::decode(snap.section("physics.rapier").unwrap())
        .unwrap()
        .apply(bare.world_mut());
    assert_eq!(bare.world().resource::<Physics>().world.bodies.len(), 0);
    assert!(bare.world().get_resource::<RebuiltSolver>().is_some());
}

/// A cold restore whose components do not build the shape its cache refers to (a snapshot this
/// engine did not write: here the terrain's `Collider` section of a world whose terrain has
/// another friction) rebuilds the solver from the components, and the next step reports the
/// terrain once, as a failed invocation of `physics.step` caused by `persist.noncanonical`.
#[test]
fn a_restore_whose_components_do_not_build_its_shapes_is_reported() {
    let l = ledger();
    let snapshot_at_tick_5 = |friction: f64| {
        let mut sim = level::level_world(17, 0);
        let e = pocket_sim::entity::require(sim.world(), level::terrain_id()).unwrap();
        sim.boundary()
            .world_mut()
            .get_mut::<pocket_physics::Collider>(e)
            .unwrap()
            .friction = friction;
        for _ in 0..5 {
            step(&mut sim);
        }
        l.snapshot(sim.world()).unwrap()
    };
    let mut snap = snapshot_at_tick_5(0.5);
    let other = snapshot_at_tick_5(0.8);
    let i = snap
        .sections
        .iter()
        .position(|s| s.0 == "Collider")
        .unwrap();
    snap.sections[i].1 = other.section("Collider").unwrap().to_vec();

    let mut cold = fresh();
    l.restore(&mut cold, &snap).unwrap();
    assert_eq!(cold.world().resource::<Physics>().world.bodies.len(), 0);
    let r = cold.step(&mut NoHooks).unwrap();
    let errors: Vec<_> = r
        .errors
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    assert_eq!(r.errors.len(), 1, "{errors:?}");
    let e = &r.errors[0];
    assert_eq!(e.code, "sim.system_failed");
    assert!(
        errors[0].contains("persist.noncanonical") && errors[0].contains("physics.step"),
        "{errors:?}"
    );
    assert_eq!(cold.world().resource::<Physics>().world.bodies.len(), 5);
    step(&mut cold);
}

/// A fixed collider's shape that its components do not build (here put into the solver directly,
/// as a snapshot this engine did not write could) is written inline, and so restores as it was,
/// although the components still hash to the body's spec; it never reaches another world, whose
/// terrain of the same components is by reference and built from them.
#[test]
fn a_shape_its_components_do_not_build_stays_in_its_world() {
    let l = ledger();
    let terrain_shape = |sim: &Sim| {
        let p = &sim.world().resource::<Physics>().world;
        let (_, co) = p
            .colliders
            .iter()
            .find(|(_, c)| p.bodies.get(c.parent().unwrap()).unwrap().is_fixed())
            .unwrap();
        co.shape().shape_type()
    };
    let mut odd = level::level_world(16, 0);
    step(&mut odd);
    let (by_ref, terrain) = level::cache_and_terrain_bytes(&odd).unwrap();
    assert!(by_ref < terrain);
    {
        let mut p = odd.world_mut().resource_mut::<Physics>();
        let w = &mut p.world;
        let h = w
            .colliders
            .iter()
            .find(|(_, c)| w.bodies.get(c.parent().unwrap()).unwrap().is_fixed())
            .map(|(h, _)| h)
            .unwrap();
        w.colliders
            .get_mut(h)
            .unwrap()
            .set_shape(rapier3d::prelude::SharedShape::ball(2.0));
    }
    assert_eq!(terrain_shape(&odd), rapier3d::prelude::ShapeType::Ball);
    // No world holds the terrain's built mesh any more, so a slot by reference would build it
    // again: the fork's ball shows the shape was written inline.
    let f = fork(&l, &odd, fresh).unwrap();
    assert_eq!(terrain_shape(&f), rapier3d::prelude::ShapeType::Ball);
    assert_eq!(
        l.snapshot(f.world()).unwrap(),
        l.snapshot(odd.world()).unwrap()
    );

    let mut plain = level::level_world(16, 0);
    step(&mut plain);
    let (by_ref, _) = level::cache_and_terrain_bytes(&plain).unwrap();
    assert!(by_ref < terrain);
    let f = fork(&l, &plain, fresh).unwrap();
    assert_eq!(terrain_shape(&f), rapier3d::prelude::ShapeType::TriMesh);
}

/// docs/bench/physics.md, "Static geometry and the cache": layout 2 changed the cache's bytes and
/// nothing else. The per-tick hashes of every other section, folded, are layout 1's for the
/// sailing scene, forked or not, and for a level scene whose terrain is by reference, forked or
/// not (`probe::layout`). About 50 s in a debug build, so `#[ignore]`d there; the local check's
/// test step runs it (`cargo test --release --workspace -- --include-ignored`), as does `cargo test
/// --release -p pocket-physics --test determinism -- --include-ignored`.
#[test]
#[ignore = "about 50 s in a debug build; the check's test step runs it in release"]
fn the_trajectories_are_layout_1s() {
    let got = probe::layout::trajectories().unwrap();
    for ((name, want), got) in probe::layout::LAYOUT1_TRAJECTORIES.iter().zip(got) {
        assert_eq!(got, *want, "{name}: {got:016x}, layout 1 {want:016x}");
    }
}
