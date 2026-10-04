//! Determinism, forks and restores of the sailing scene through the persistence declarations
//! (charter 3.3; persistence.md 7 and 8; simulation.md 8 and 12): two runs agree tick by tick, a
//! fork at every tick continues identically, a restored snapshot continues in lockstep with its
//! original, storage order changes nothing, and the cache survives its own bytes.

use pocket_physics::probe::{
    self, Ledger, Snap, chain, check_forks, fork, fresh, ledger, sailing_world,
};
use pocket_physics::{Physics, PhysicsCache};
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
