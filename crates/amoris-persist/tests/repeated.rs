//! A snapshot cache gives the same snapshot as `snapshot` after every tick and every write
//! (persistence.md 4.3): sections no row of which changed are taken over, the others encoded again.

mod common;

use std::sync::Arc;

use amoris_persist::{
    RestoreOptions, SectionKey, SectionKind, Snapshot, SnapshotCache, restore, snapshot,
};
use amoris_sim::NoHooks;
use common::{Bag, Game, Mood, Pos, Tag, apply, ent, game};
use serde_json::{Value, json};

fn full(g: &Game) -> Snapshot {
    snapshot(g.sim.world(), &g.reg).expect("snapshot")
}

fn cached(c: &mut SnapshotCache, g: &Game) -> Snapshot {
    c.snapshot(g.sim.world(), &g.reg).expect("cached snapshot")
}

/// The cache's snapshot equals the full one, byte for byte.
#[track_caller]
fn same(c: &mut SnapshotCache, g: &Game, at: &str) -> Snapshot {
    let a = cached(c, g);
    let b = full(g);
    assert_eq!(
        a.to_bytes(),
        b.to_bytes(),
        "the cache's snapshot differs {at}"
    );
    assert_eq!(a, b);
    a
}

fn component(name: &str) -> SectionKey {
    SectionKey::new(SectionKind::Component, name)
}

/// Whether both snapshots hold the same bytes for the section (the cache took it over).
fn shared(a: &Snapshot, b: &Snapshot, name: &str) -> bool {
    match (a.section(&component(name)), b.section(&component(name))) {
        (Some(x), Some(y)) => Arc::ptr_eq(&x.bytes, &y.bytes),
        _ => false,
    }
}

/// A command for each boundary, cycling through every kind of write: values set, components
/// added and removed, entities spawned and despawned, writes of the same value.
fn command(tick: u64, live: &[u64]) -> Option<(&'static str, Value)> {
    let pick = |k: u64| live[(k as usize) % live.len()];
    if live.is_empty() {
        return Some(("spawn", json!({"x": 1.0, "y": 2.0})));
    }
    match tick % 9 {
        0 => Some(("push", json!({"id": pick(tick), "dx": 0.5, "dy": -0.25}))),
        1 => Some(("tag", json!({"id": pick(tick / 9), "n": tick}))),
        2 => Some(("untag", json!({"id": pick(tick / 9 + 1)}))),
        3 => Some((
            "bag",
            json!({"id": pick(tick), "item": "rope", "count": tick % 5}),
        )),
        4 => Some(("spawn", json!({"x": 0.5, "y": -0.5}))),
        5 if live.len() > 3 => Some(("despawn", json!({"id": pick(tick)}))),
        6 => Some(("push", json!({"id": pick(tick), "dx": 0.0, "dy": 0.0}))),
        _ => None,
    }
}

fn live(g: &Game) -> Vec<u64> {
    g.sim
        .world()
        .resource::<amoris_sim::EntityIndex>()
        .ids()
        .map(|i| i.get())
        .collect()
}

#[test]
fn the_cache_matches_the_snapshot_after_every_tick_and_write() {
    let mut g = game(5);
    let mut c = SnapshotCache::new();
    let mut last = same(&mut c, &g, "at the start");
    let mut taken_over = 0;
    for _ in 0..240 {
        let t = g.sim.clock().tick.0 + 1;
        if let Some((name, params)) = command(t, &live(&g)) {
            // A refused write changes nothing; both must still agree.
            let _ = apply(&mut g.sim, name, &params);
            last = same(&mut c, &g, &format!("after {name} before tick {t}"));
        }
        g.sim.step(&mut NoHooks).expect("step");
        let now = same(&mut c, &g, &format!("after tick {t}"));
        // Taken twice with nothing between: every section is taken over.
        let again = same(&mut c, &g, &format!("again after tick {t}"));
        for s in now.sections() {
            let k = again.section(&s.key).expect("the same sections");
            if s.key.kind == SectionKind::Component {
                assert!(Arc::ptr_eq(&s.bytes, &k.bytes), "{} encoded again", s.key);
            }
        }
        taken_over += usize::from(shared(&last, &now, "Tag"));
        last = now;
    }
    // Most ticks write no tag: its section was taken over from the snapshot before.
    assert!(taken_over > 100, "Tag taken over after {taken_over} ticks");
}

#[test]
fn a_row_moved_to_another_entity_is_seen() {
    let mut g = game(6);
    let mut c = SnapshotCache::new();
    assert!(apply(&mut g.sim, "tag", &json!({"id": 1, "n": 4})).is_ok());
    let before = same(&mut c, &g, "with the tag on 1");
    // The same count of rows, on another entity, in one boundary.
    assert!(apply(&mut g.sim, "untag", &json!({"id": 1})).is_ok());
    assert!(apply(&mut g.sim, "tag", &json!({"id": 2, "n": 4})).is_ok());
    let after = same(&mut c, &g, "with the tag moved to 2");
    assert_ne!(
        before.section(&component("Tag")),
        after.section(&component("Tag"))
    );
    // A tagged entity despawned and a new one tagged.
    assert!(apply(&mut g.sim, "despawn", &json!({"id": 2})).is_ok());
    assert!(apply(&mut g.sim, "spawn", &json!({"x": 9.0, "y": 9.0})).is_ok());
    let newest = *live(&g).last().expect("an entity");
    assert!(apply(&mut g.sim, "tag", &json!({"id": newest, "n": 4})).is_ok());
    same(&mut c, &g, "with the tag on a new entity");
    // The last row removed: the section is gone.
    assert!(apply(&mut g.sim, "untag", &json!({"id": newest})).is_ok());
    let gone = same(&mut c, &g, "without tags");
    assert!(gone.section(&component("Tag")).is_none());
}

#[test]
fn a_write_right_after_a_snapshot_is_seen() {
    let mut g = game(7);
    let mut c = SnapshotCache::new();
    let start = same(&mut c, &g, "at the start");
    // No tick and no other write between: the write's change tick must still be newer.
    let e = ent(&g.sim, 1).expect("entity 1");
    g.sim.world_mut().get_mut::<Pos>(e).expect("a position").x = 42.0;
    let a = same(&mut c, &g, "after a direct write");
    assert!(!shared(&start, &a, "Pos"), "Pos was taken over");
    assert!(shared(&start, &a, "Vel"), "Vel was encoded again");
    g.sim
        .world_mut()
        .entity_mut(e)
        .insert(Bag {
            items: Default::default(),
            owner: None,
            mood: Mood::Calm,
        })
        .insert(Tag(3));
    let b = same(&mut c, &g, "after inserts");
    assert!(shared(&a, &b, "Pos"), "Pos was encoded again");
    // Written with the value it had: encoded again, the same bytes.
    *g.sim.world_mut().get_mut::<Tag>(e).expect("a tag") = Tag(3);
    let d = same(&mut c, &g, "after writing the same value");
    assert_eq!(d.section(&component("Tag")), b.section(&component("Tag")));
}

#[test]
fn a_restore_and_another_world_start_the_cache_over() {
    let mut g = game(8);
    let mut c = SnapshotCache::new();
    for _ in 0..20 {
        g.sim.step(&mut NoHooks).expect("step");
        same(&mut c, &g, "while running");
    }
    let old = full(&g);
    for _ in 0..15 {
        g.sim.step(&mut NoHooks).expect("step");
        same(&mut c, &g, "while running on");
    }
    // Restored in place: every row is written again.
    restore(g.sim.world_mut(), &old, &g.reg, RestoreOptions::default()).expect("restore");
    let back = same(&mut c, &g, "after the restore");
    assert_eq!(back.world_hash(), old.world_hash());
    // Another world through the same cache.
    let mut other = game(9);
    other.sim.step(&mut NoHooks).expect("step");
    same(&mut c, &other, "on another world");
    same(&mut c, &g, "back on the first world");
}

#[test]
fn resources_are_shared_until_written_removed_reinserted_or_restored() {
    use common::Score;
    let mut g = game(11);
    let mut cache = SnapshotCache::new();
    let key = SectionKey::new(SectionKind::Resource, "Score");
    let first = same(&mut cache, &g, "resource start");
    let again = same(&mut cache, &g, "resource unchanged");
    let ptr = |s: &Snapshot| s.section(&key).unwrap().bytes.clone();
    assert!(Arc::ptr_eq(&ptr(&first), &ptr(&again)));
    g.sim.world_mut().resource_mut::<Score>().0 = 42;
    let changed = same(&mut cache, &g, "resource written after snapshot");
    assert!(!Arc::ptr_eq(&ptr(&again), &ptr(&changed)));
    *g.sim.world_mut().resource_mut::<Score>() = Score(42);
    let same_value = same(&mut cache, &g, "resource written with equal value");
    assert_eq!(changed, same_value);
    assert!(!Arc::ptr_eq(&ptr(&changed), &ptr(&same_value)));
    g.sim.world_mut().remove_resource::<Score>();
    assert!(
        same(&mut cache, &g, "resource removed")
            .section(&key)
            .is_none()
    );
    g.sim.world_mut().insert_resource(Score(42));
    let added = same(&mut cache, &g, "resource reinserted");
    g.sim.world_mut().remove_resource::<Score>();
    g.sim.world_mut().insert_resource(Score(42));
    let replaced = same(
        &mut cache,
        &g,
        "resource removed and replaced between snapshots",
    );
    assert_eq!(added, replaced);
    assert!(!Arc::ptr_eq(&ptr(&added), &ptr(&replaced)));
    let saved = full(&g);
    g.sim.world_mut().resource_mut::<Score>().0 = 43;
    same(&mut cache, &g, "before resource restore");
    restore(g.sim.world_mut(), &saved, &g.reg, RestoreOptions::default()).unwrap();
    let restored = same(&mut cache, &g, "resource restored in place");
    assert_eq!(restored.world_hash(), saved.world_hash());
    // A failed take must reset resource caches as well as kept component queries.
    let e = ent(&g.sim, 1).unwrap();
    g.sim.world_mut().get_mut::<Pos>(e).unwrap().x = f64::NAN;
    assert!(cache.snapshot(g.sim.world(), &g.reg).is_err());
    g.sim.world_mut().get_mut::<Pos>(e).unwrap().x = 0.5;
    same(&mut cache, &g, "resource cache after a failed take");
}
