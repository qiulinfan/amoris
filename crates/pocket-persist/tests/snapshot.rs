//! Snapshots, restore and fork (docs/spec/persistence.md 12): P2 (round trip), P3 (restore
//! continuation), P6 (classification), what must match after a fork (7), restore's refusals and
//! its checks of the target before anything changes, and a cache whose state is a resource of its
//! own.

mod common;

use bevy_ecs::prelude::*;
use common::{Counter, Game, Pos, Score, Tag, game, inputs, registry, run, sim};
use pocket_contract::{Problem, detail};
use pocket_persist::{
    RestoreOptions, SectionData, SectionKey, SectionKind, Snapshot, fork_into, restore,
    sim_registry, snapshot, world_hash,
};
use pocket_sim::{
    EntityAllocator, NoHooks, PersistedCache, RegisterPersisted, SimClock, Staged, Tick,
};
use serde_json::json;

fn snap(g: &Game) -> Snapshot {
    snapshot(g.sim.world(), &g.reg).unwrap()
}

/// P2 at boundaries 0, 1, 60 and a long run: `snapshot(restore(fresh, snapshot(w)))` equals
/// `snapshot(w)` byte for byte, and the bytes parse back to the same snapshot.
#[test]
fn p2_snapshot_round_trip() {
    let mut g = game(7);
    let ins = inputs();
    for at in [0u64, 1, 60, 600] {
        run(&mut g, None, &ins, at);
        let s = snap(&g);
        let bytes = s.to_bytes();
        let parsed = Snapshot::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, s, "from_bytes(to_bytes) at {at}");
        assert_eq!(parsed.to_bytes(), bytes);
        let mut fresh = sim(99);
        let report = restore(
            fresh.world_mut(),
            &parsed,
            &g.reg,
            RestoreOptions::default(),
        )
        .unwrap();
        assert_eq!(report.tick, Tick(at));
        assert_eq!(report.world_hash, s.world_hash());
        let again = snapshot(fresh.world(), &g.reg).unwrap();
        assert_eq!(
            again.to_bytes(),
            bytes,
            "snapshot(restore(snapshot)) at {at}"
        );
    }
}

/// P3: a snapshot restored into a fresh world runs in lockstep with the original, equal hashes at
/// every tick, from several boundaries.
#[test]
fn p3_restore_continuation() {
    let ins = inputs();
    for at in [0u64, 1, 13, 60] {
        let mut a = game(11);
        run(&mut a, None, &ins, at);
        let mut b = game(11);
        let s = snap(&a);
        restore(b.sim.world_mut(), &s, &b.reg, RestoreOptions::default()).unwrap();
        for _ in 0..600 {
            let t = a.sim.clock().tick.0 + 1;
            for i in ins.iter().filter(|i| i.tick == t) {
                let ra = common::apply(&mut a.sim, i.name, &i.params);
                let rb = common::apply(&mut b.sim, i.name, &i.params);
                assert_eq!(ra.is_ok(), rb.is_ok());
            }
            a.sim.step(&mut NoHooks).unwrap();
            b.sim.step(&mut NoHooks).unwrap();
            assert_eq!(
                world_hash(a.sim.world(), &a.reg).unwrap(),
                world_hash(b.sim.world(), &b.reg).unwrap(),
                "tick {t} after a restore at {at}"
            );
        }
    }
}

/// The table of persistence.md 7: after `b = fork(a)` the snapshots are equal byte for byte, the
/// next spawn gets the same id, the clock and seed (hence the RNG) match, the cache matches, and
/// running `b` leaves `a` alone.
#[test]
fn fork_matches_and_is_independent() {
    let mut a = game(3);
    run(&mut a, None, &inputs(), 50);
    let mut b = game(1234);
    fork_into(a.sim.world(), b.sim.world_mut(), &a.reg).unwrap();
    assert_eq!(snap(&a).to_bytes(), snap(&b).to_bytes());
    assert_eq!(
        a.sim.world().resource::<EntityAllocator>(),
        b.sim.world().resource::<EntityAllocator>()
    );
    assert_eq!(
        a.sim.world().resource::<SimClock>(),
        b.sim.world().resource::<SimClock>()
    );
    assert_eq!(
        a.sim.world().resource::<Counter>(),
        b.sim.world().resource::<Counter>()
    );
    let na = a.sim.boundary().spawn(Pos { x: 0.0, y: 0.0 }).unwrap();
    let nb = b.sim.boundary().spawn(Pos { x: 0.0, y: 0.0 }).unwrap();
    assert_eq!(na, nb, "the next spawn gets the same id");
    let before = snap(&a);
    for _ in 0..30 {
        common::apply(
            &mut b.sim,
            "spawn",
            &serde_json::json!({"x": 5.0, "y": 5.0}),
        )
        .unwrap();
        b.sim.step(&mut NoHooks).unwrap();
    }
    assert_eq!(
        snap(&a),
        before,
        "running the fork changed nothing in the source"
    );
    assert_ne!(snap(&a).world_hash(), snap(&b).world_hash());
}

/// Equal persisted state, equal bytes, whatever the storage layout: a world whose storage was
/// shuffled snapshots the same.
#[test]
fn same_state_same_bytes_after_a_storage_shuffle() {
    let mut a = game(5);
    run(&mut a, None, &inputs(), 30);
    let before = snap(&a).to_bytes();
    pocket_sim::order::shuffle_storage(a.sim.world_mut(), 77);
    assert_eq!(snap(&a).to_bytes(), before);
}

/// P6: a world with every type snapshots; an unregistered component, or an unregistered resource,
/// fails with `persist.unclassified`; a type registered twice is refused.
#[test]
fn p6_classification() {
    #[derive(Component)]
    struct Stray;
    #[derive(Resource)]
    struct Hidden;
    let mut g = game(1);
    run(&mut g, None, &inputs(), 10);
    snap(&g);
    g.sim.boundary().spawn(Stray).unwrap();
    let e = snapshot(g.sim.world(), &g.reg).unwrap_err();
    assert_eq!(e.code, "persist.unclassified");
    let e = world_hash(g.sim.world(), &g.reg).unwrap_err();
    assert_eq!(e.code, "persist.unclassified");
    let mut h = game(1);
    h.sim.world_mut().insert_resource(Hidden);
    assert_eq!(
        snapshot(h.sim.world(), &h.reg).unwrap_err().code,
        "persist.unclassified"
    );
    let mut twice = sim_registry();
    twice.component::<Pos>().component::<Pos>();
    assert_eq!(twice.build().err().unwrap().code, "persist.duplicate_name");
}

/// Sections appear only with rows: registering a type the world does not use leaves its bytes and
/// hash unchanged; a component whose last row goes takes its section with it.
#[test]
fn sections_appear_with_rows() {
    let mut g = game(2);
    let tag = SectionKey::new(SectionKind::Component, "Tag");
    assert!(snap(&g).section(&tag).is_none());
    common::apply(&mut g.sim, "tag", &serde_json::json!({"id": 1, "n": 3})).unwrap();
    assert!(snap(&g).section(&tag).is_some());
    common::apply(&mut g.sim, "untag", &serde_json::json!({"id": 1})).unwrap();
    assert!(snap(&g).section(&tag).is_none());
    // A registry without the test's Bag type gives the same bytes for a world without bags.
    let mut b = sim_registry();
    b.component::<Pos>()
        .component::<common::Vel>()
        .component::<Tag>()
        .resource::<Score>()
        .cache::<Counter>();
    let narrow = b.build().unwrap();
    assert_eq!(
        snapshot(g.sim.world(), &narrow).unwrap().to_bytes(),
        snap(&g).to_bytes()
    );
}

/// Restore is atomic for errors: a snapshot with an unknown section, another version, an orphan row
/// or bytes that are not canonical is refused and the world is unchanged.
#[test]
fn restore_refuses_and_changes_nothing() {
    let mut src = game(4);
    run(&mut src, None, &inputs(), 20);
    let good = snap(&src);
    let mut target = game(4);
    let before = snap(&target);
    // A registry that knows the target's types but not the snapshot's cache section.
    let mut narrow = sim_registry();
    narrow
        .component::<Pos>()
        .component::<common::Vel>()
        .component::<Tag>()
        .resource::<Score>()
        .ignore::<Counter>("this registry does not persist the test cache");
    let narrow = narrow.build().unwrap();
    let e = restore(
        target.sim.world_mut(),
        &good,
        &narrow,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.unknown_section");
    assert_eq!(snap(&target), before);

    let edit = |f: &dyn Fn(&mut pocket_persist::SectionData)| {
        let mut sections = good.sections().to_vec();
        for s in &mut sections {
            f(s);
        }
        Snapshot::from_parts(good.header().clone(), sections)
    };
    let pos = SectionKey::new(SectionKind::Component, "Pos");
    let bumped = edit(&|s| {
        if s.key == pos {
            s.version = 2;
        }
    });
    let e = restore(
        target.sim.world_mut(),
        &bumped,
        &target.reg,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.version");
    // A row for entity 999, which is not live: change the first row's id.
    let orphan = edit(&|s| {
        if s.key == pos {
            let mut b = s.bytes.to_vec();
            b[1..9].copy_from_slice(&999u64.to_le_bytes());
            s.bytes = b.into();
        }
    });
    let e = restore(
        target.sim.world_mut(),
        &orphan,
        &target.reg,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.orphan");
    // Trailing bytes in a resource section.
    let score = SectionKey::new(SectionKind::Resource, "Score");
    let trailing = edit(&|s| {
        if s.key == score {
            let mut b = s.bytes.to_vec();
            b.push(0);
            s.bytes = b.into();
        }
    });
    let e = restore(
        target.sim.world_mut(),
        &trailing,
        &target.reg,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.noncanonical");
    assert_eq!(
        snap(&target),
        before,
        "every refusal left the world as it was"
    );
}

/// `from_bytes` checks magic, format, order and the trailing hash.
#[test]
fn snapshot_bytes_are_checked() {
    let g = game(9);
    let bytes = snap(&g).to_bytes();
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert_eq!(
        Snapshot::from_bytes(&bad).unwrap_err().code,
        "persist.format"
    );
    let mut bad = bytes.clone();
    bad[8] = 2;
    assert_eq!(
        Snapshot::from_bytes(&bad).unwrap_err().code,
        "version.format"
    );
    let mut bad = bytes.clone();
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert_eq!(
        Snapshot::from_bytes(&bad).unwrap_err().code,
        "persist.hash_mismatch"
    );
    let mut bad = bytes.clone();
    bad.push(0);
    assert_eq!(
        Snapshot::from_bytes(&bad).unwrap_err().code,
        "persist.noncanonical"
    );
    assert_eq!(
        Snapshot::from_bytes(&bytes[..bytes.len() - 20])
            .unwrap_err()
            .code,
        "persist.truncated"
    );
}

/// A world restored from a snapshot is not poisoned and its derived state is rebuilt: it steps.
#[test]
fn restored_world_steps() {
    let mut a = game(8);
    run(&mut a, None, &inputs(), 25);
    let s = snap(&a);
    let mut b = sim(8);
    restore(
        b.world_mut(),
        &s,
        &registry(),
        RestoreOptions { verify: true },
    )
    .unwrap();
    b.step(&mut NoHooks).unwrap();
    assert_eq!(b.clock().tick, Tick(26));
}

/// Whole-world operations run at a boundary: inside a tick they fail with
/// `persist.not_at_boundary`.
#[test]
fn not_at_a_boundary() {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut g = game(1);
    g.sim
        .add_exclusive(
            "test.peek",
            pocket_sim::TickPhase::Update,
            pocket_sim::RunCondition::Always,
            |w: &mut World, _: &mut pocket_sim::SystemCtx<'_>| {
                let code = world_hash(w, &registry()).unwrap_err().code;
                SEEN.lock().unwrap().push(code);
            },
        )
        .unwrap();
    g.sim.step(&mut NoHooks).unwrap();
    assert_eq!(*SEEN.lock().unwrap(), ["persist.not_at_boundary"]);
    world_hash(g.sim.world(), &g.reg).unwrap();
}

/// A cache whose state lives in a resource of its own, as `pocket-physics` keeps `Physics` beside
/// `PhysicsCache` (persistence.md 14, choice 15): the section writes `Solver`, which is declared
/// Derived so that restore removes it before applying the sections.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
struct Solver {
    steps: u64,
}

struct SolverCache;

struct StagedSolver(Solver);

impl Staged for StagedSolver {
    fn apply(self: Box<Self>, world: &mut World) {
        world.insert_resource(self.0);
    }
}

impl PersistedCache for SolverCache {
    const NAME: &'static str = "test.solver";
    fn identity() -> &'static str {
        "test solver 1"
    }
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem> {
        if let Some(s) = world.get_resource::<Solver>() {
            out.extend_from_slice(&s.steps.to_le_bytes());
        }
        Ok(())
    }
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem> {
        let steps = <[u8; 8]>::try_from(bytes)
            .map_err(|_| Problem::new("persist.noncanonical", "eight bytes", detail([])))?;
        Ok(Box::new(StagedSolver(Solver {
            steps: u64::from_le_bytes(steps),
        })))
    }
    fn rebuild(world: &mut World) -> Result<(), Problem> {
        world.remove_resource::<Solver>();
        Ok(())
    }
}

/// Restoring a snapshot taken before the solver existed into a world that holds one leaves no
/// solver (a stale one would survive as Ignored and fail the restore's own check), and restoring
/// one taken with it brings it back.
#[test]
fn a_cache_state_resource_is_removed_by_restore() {
    let mut b = sim_registry();
    common::declare(&mut b);
    b.cache::<SolverCache>().derived::<Solver>(
        "the state of cache test.solver: removed by restore, put back by the section's staged apply",
    );
    let reg = b.build().unwrap();
    let key = SectionKey::new(SectionKind::Cache, "test.solver");
    let early = sim(6);
    let before = snapshot(early.world(), &reg).unwrap();
    assert!(before.section(&key).is_none());
    let mut late = sim(6);
    late.world_mut().insert_resource(Solver { steps: 41 });
    let with = snapshot(late.world(), &reg).unwrap();
    assert!(with.section(&key).is_some());
    restore(late.world_mut(), &before, &reg, RestoreOptions::default()).unwrap();
    assert!(late.world().get_resource::<Solver>().is_none());
    assert_eq!(snapshot(late.world(), &reg).unwrap(), before);
    restore(late.world_mut(), &with, &reg, RestoreOptions::default()).unwrap();
    assert_eq!(late.world().resource::<Solver>(), &Solver { steps: 41 });
    assert_eq!(snapshot(late.world(), &reg).unwrap(), with);
}

/// Restore is atomic for errors (6.4) in what it checks of the target too: a snapshot without its
/// entities section (it parses) and a target world holding an unclassified type are refused before
/// the world changes.
#[test]
fn restore_checks_the_snapshot_and_the_target_first() {
    #[derive(Resource)]
    struct Hidden;
    let mut src = game(4);
    run(&mut src, None, &inputs(), 20);
    let good = snap(&src);
    let mut target = game(4);
    run(&mut target, None, &inputs(), 7);
    let before = snap(&target);
    let resources: Vec<SectionData> = good
        .sections()
        .iter()
        .filter(|s| s.key.kind == SectionKind::Resource)
        .cloned()
        .collect();
    let partial = Snapshot::from_parts(good.header().clone(), resources);
    let partial = Snapshot::from_bytes(&partial.to_bytes()).unwrap();
    let e = restore(
        target.sim.world_mut(),
        &partial,
        &target.reg,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.noncanonical");
    assert_eq!(
        snap(&target),
        before,
        "the world is as it was and still hashes"
    );

    // A registry that tolerates `Hidden` shows the world unchanged after the refusal.
    let mut lenient = sim_registry();
    common::declare(&mut lenient);
    lenient.ignore::<Hidden>("this test's stray resource");
    let lenient = lenient.build().unwrap();
    target.sim.world_mut().insert_resource(Hidden);
    let seen = snapshot(target.sim.world(), &lenient).unwrap();
    let e = restore(
        target.sim.world_mut(),
        &good,
        &target.reg,
        RestoreOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "persist.unclassified");
    assert_eq!(snapshot(target.sim.world(), &lenient).unwrap(), seen);
}

/// A persisted component on an entity without an `EntityId` would be in no section and no hash and
/// would survive a restore: snapshot, the world hash and restore refuse it (2: never skipped).
#[test]
fn a_persisted_row_without_an_entity_id_is_refused() {
    let mut g = game(2);
    run(&mut g, None, &inputs(), 5);
    let good = snap(&g);
    let stray = g.sim.world_mut().spawn(Pos { x: 1.0, y: 2.0 }).id();
    for e in [
        snapshot(g.sim.world(), &g.reg).unwrap_err(),
        world_hash(g.sim.world(), &g.reg).unwrap_err(),
        restore(g.sim.world_mut(), &good, &g.reg, RestoreOptions::default()).unwrap_err(),
    ] {
        assert_eq!(
            (e.code.as_str(), e.detail["section"].clone()),
            ("persist.encode", json!("component:Pos"))
        );
    }
    assert!(
        g.sim.world().get_entity(stray).is_ok(),
        "restore changed nothing"
    );
    g.sim.world_mut().despawn(stray);
    assert_eq!(snap(&g), good);
}
