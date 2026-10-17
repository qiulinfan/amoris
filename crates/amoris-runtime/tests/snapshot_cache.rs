//! The game thread's snapshot cache on the sailing game (amoris-persist's `SnapshotCache`,
//! persistence.md 4.3): after every tick and every edit, the cache's snapshot is the game's, byte
//! for byte: physics, scripts, project components, spawns, destroys, components added and removed,
//! a restore and a fork.
mod common;

use amoris_link::Source;
use amoris_persist::{Snapshot, SnapshotCache};
use amoris_runtime::{Command, Game};
use serde_json::{Value, json};

#[track_caller]
fn same(c: &mut SnapshotCache, g: &Game, at: &str) -> Snapshot {
    let a = c
        .snapshot(g.world(), g.registry())
        .unwrap_or_else(|e| panic!("{e:#?}"));
    let b = g.snapshot().unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(
        a.to_bytes(),
        b.to_bytes(),
        "the cache's snapshot differs {at}"
    );
    a
}

fn edit(g: &mut Game, seq: u64, ops: Value) {
    g.apply(&Command::new(
        Source::Editor,
        seq,
        "world.edit",
        json!({"ops": ops, "label": "test"}),
    ))
    .unwrap_or_else(|e| panic!("{e:#?}"));
}

/// The edit before tick `t`, cycling through the kinds of change.
fn ops(t: u64) -> Option<Value> {
    let x = f64::from(u32::try_from(t % 50).unwrap_or(0)) / 10.0;
    match t % 10 {
        1 => Some(
            json!([{"set": {"entity": "Sloop", "component": "Boat", "value": {"rudder": x / 5.0 - 0.5}}}]),
        ),
        3 => Some(
            json!([{"set": {"entity": "Crate1", "component": "Transform", "value": {"position.1": x}}}]),
        ),
        4 => Some(
            json!([{"spawn": {"name": format!("Buoy{t}"), "components": {
            "Transform": {"position": [x, 0.0, -x]},
            "Model": {"mesh": "builtin:sphere"}}}}]),
        ),
        6 => Some(
            json!([{"set": {"entity": "Crate2", "component": "Light", "value": {"intensity": x}}}]),
        ),
        7 => Some(json!([{"remove": {"entity": "Crate2", "component": "Light"}}])),
        8 => Some(json!([{"destroy": {"entity": format!("Buoy{}", t - 4)}}])),
        _ => None,
    }
}

#[test]
fn the_cache_follows_the_sailing_game() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 3).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut c = SnapshotCache::new();
        same(&mut c, &g, "at the start");
        let mut seq = 0;
        let mut kept = None;
        for _ in 0..200 {
            let t = g.tick().0 + 1;
            if let Some(o) = ops(t) {
                seq += 1;
                edit(&mut g, seq, o);
                same(&mut c, &g, &format!("after the edit before tick {t}"));
            }
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let s = same(&mut c, &g, &format!("after tick {t}"));
            if t == 120 {
                kept = Some(s);
            }
        }
        // Restored in place, then a fork: each starts the cache's view over.
        let old = kept.expect("tick 120 kept");
        g.restore(&old).unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(
            same(&mut c, &g, "after the restore").world_hash(),
            old.world_hash()
        );
        let mut fork = g.fork().unwrap_or_else(|e| panic!("{e:#?}"));
        for _ in 0..30 {
            fork.step().unwrap_or_else(|e| panic!("{e:#?}"));
            same(&mut c, &fork, "on the fork");
        }
        g.step().unwrap_or_else(|e| panic!("{e:#?}"));
        same(&mut c, &g, "back on the game");
    });
}
